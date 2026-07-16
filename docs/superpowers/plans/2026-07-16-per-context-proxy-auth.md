# Per-Context Proxy Authentication Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bind a proxy + credentials to a `BrowserContext` via a builder so every tab it opens is transparently authenticated, with no per-tab boilerplate and no handle to hold.

**Architecture:** A `BrowserContextBuilder` (`browser.browser_context()…build()`) parses the proxy URL, sends `Target.createBrowserContext` with a userinfo-free `proxyServer`, and registers the credentials in a `BrowserInner` map keyed by `browserContextId`. The existing `TabRegistrar` — which already installs a per-tab tracker-blocking interception actor — is extended to chain a `handle_auth` into that **same** actor when the new tab's context has registered credentials (one actor per session, per zendriver#208). `BrowserContext::Drop` unregisters the credentials.

**Tech Stack:** Rust (edition 2024), Tokio, CDP over `zendriver-transport`, `zendriver-interception` (`Fetch.*`), `url` crate (already a dependency), `MockConnection` test harness.

## Global Constraints

- **MSRV 1.85, edition 2024.** — copied from workspace root.
- **One actor per session (zendriver#208).** Auth (`handle_auth`) and tracker-blocking (`block_hosts`) MUST chain into a single `InterceptBuilder` per session; never two actors on one session.
- **`default = []`; `interception` is opt-in; `tracker-blocking` implies `interception`.** The builder's `proxy`/`proxy_bypass`/`build` path MUST compile and pass on default features. All auth behavior is gated `#[cfg(feature = "interception")]`.
- **Before every push:** `cargo fmt --all` then `cargo clippy --workspace --all-targets --locked -- -D warnings`. Because this touches feature-gated code, also `cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings`.
- **Reuse `ZendriverError::Navigation`** for proxy-parse / missing-`browserContextId` errors (no new error variant).
- **`url` crate** is already `url.workspace = true` in `crates/zendriver/Cargo.toml` — do NOT add a dependency.

---

### Task 1: Proxy-URL parsing helper

Pure, Chrome-free string parsing. Splits `scheme://[user[:pass]@]host:port` into a userinfo-free `proxyServer` plus optional credentials. Compiles on default features (no interception needed).

**Files:**
- Create: `crates/zendriver/src/proxy.rs`
- Modify: `crates/zendriver/src/lib.rs` (add `mod proxy;`)
- Test: inline `#[cfg(test)] mod tests` in `crates/zendriver/src/proxy.rs`

**Interfaces:**
- Consumes: `crate::error::ZendriverError`, `url::Url`.
- Produces:
  - `pub(crate) struct ParsedProxy { pub server: String, pub credentials: Option<(String, String)> }`
  - `pub(crate) fn split_proxy_url(url: &str) -> Result<ParsedProxy, ZendriverError>`

- [ ] **Step 1: Write the failing tests**

Create `crates/zendriver/src/proxy.rs`:

```rust
//! Proxy-URL parsing shared by per-context proxy configuration.
//!
//! Chrome's `Target.createBrowserContext` `proxyServer` field ignores any
//! userinfo in the URL, so credentials are split out here for the
//! interception auth handler (`Fetch.authRequired`) and the returned
//! `server` string is userinfo-free.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_userinfo_from_server() {
        let p = split_proxy_url("http://bob:s3cret@proxy.example:8080").unwrap();
        assert_eq!(p.server, "http://proxy.example:8080");
        assert_eq!(p.credentials, Some(("bob".into(), "s3cret".into())));
    }

    #[test]
    fn no_userinfo_yields_no_credentials() {
        let p = split_proxy_url("http://proxy.example:8080").unwrap();
        assert_eq!(p.server, "http://proxy.example:8080");
        assert_eq!(p.credentials, None);
    }

    #[test]
    fn fills_default_port_from_scheme() {
        let p = split_proxy_url("http://proxy.example").unwrap();
        assert_eq!(p.server, "http://proxy.example:80");
    }

    #[test]
    fn username_without_password_yields_empty_pass() {
        let p = split_proxy_url("http://bob@proxy.example:8080").unwrap();
        assert_eq!(p.credentials, Some(("bob".into(), String::new())));
    }

    #[test]
    fn missing_host_is_an_error() {
        assert!(split_proxy_url("http://").is_err());
    }

    #[test]
    fn unparseable_is_an_error() {
        assert!(split_proxy_url("not a url").is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zendriver --lib proxy:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function split_proxy_url` / `cannot find type ParsedProxy`.

- [ ] **Step 3: Write the implementation**

Add above the `#[cfg(test)]` block in `crates/zendriver/src/proxy.rs`:

```rust
use crate::error::ZendriverError;

/// A proxy split into the CDP `proxyServer` string (no credentials) and the
/// optional auth credentials answered separately via `Fetch.authRequired`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedProxy {
    /// `scheme://host:port` with any userinfo stripped.
    pub server: String,
    /// `(user, pass)` when the URL carried userinfo; else `None`.
    pub credentials: Option<(String, String)>,
}

/// Parse `scheme://[user[:pass]@]host:port[/...]` into a [`ParsedProxy`].
///
/// # Errors
///
/// Returns [`ZendriverError::Navigation`] if the URL is unparseable or is
/// missing a host or port.
pub(crate) fn split_proxy_url(url: &str) -> Result<ParsedProxy, ZendriverError> {
    let u = url::Url::parse(url)
        .map_err(|e| ZendriverError::Navigation(format!("invalid proxy URL {url:?}: {e}")))?;
    let host = u
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| ZendriverError::Navigation(format!("proxy URL {url:?} missing host")))?;
    let port = u
        .port_or_known_default()
        .ok_or_else(|| ZendriverError::Navigation(format!("proxy URL {url:?} missing port")))?;
    let credentials = if u.username().is_empty() {
        None
    } else {
        Some((
            u.username().to_string(),
            u.password().unwrap_or_default().to_string(),
        ))
    };
    Ok(ParsedProxy {
        server: format!("{}://{}:{}", u.scheme(), host, port),
        credentials,
    })
}
```

Add to `crates/zendriver/src/lib.rs` next to the other `mod` declarations:

```rust
mod proxy;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zendriver --lib proxy:: 2>&1 | tail -20`
Expected: PASS (6 tests). Note: if `split_proxy_url` triggers `dead_code` because no non-test caller exists yet, add `#[allow(dead_code)]` on `ParsedProxy` + `split_proxy_url` **temporarily**; Task 2 adds the real caller and you remove the allow then. Prefer to check: `cargo clippy -p zendriver --lib 2>&1 | grep -i "never used"`.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver/src/proxy.rs crates/zendriver/src/lib.rs
git commit -m "feat(proxy): add split_proxy_url helper for per-context proxy config"
```

---

### Task 2: `BrowserContextBuilder` + `Browser::browser_context()` + credential registry

The builder and its `build()`; the `BrowserInner.context_proxy_auth` registry the builder writes to; a shared raw-send helper so the existing `create_browser_context_with` keeps sending its `proxyServer` verbatim (unchanged behavior) while the builder sends the userinfo-stripped server.

**Files:**
- Modify: `crates/zendriver/src/browser.rs` — add `context_proxy_auth` field to `BrowserInner` (every construction site); add `BrowserInner::create_browser_context_raw`; add `Browser::browser_context`; refactor `create_browser_context_with` (`browser.rs:3156`).
- Modify: `crates/zendriver/src/browser_context.rs` — add `BrowserContextBuilder`.
- Test: `crates/zendriver/src/browser_context.rs` (`#[cfg(test)]` modules), `crates/zendriver/src/browser.rs` (existing context tests must stay green).

**Interfaces:**
- Consumes: `crate::proxy::{split_proxy_url, ParsedProxy}` (Task 1); `crate::BrowserContext`, `BrowserInner`, `ZendriverError`.
- Produces:
  - `BrowserInner.context_proxy_auth: tokio::sync::Mutex<HashMap<String, (String, String)>>` (gated `#[cfg(feature = "interception")]`) — read by Task 3, cleared by Task 4.
  - `async fn BrowserInner::create_browser_context_raw(&self, proxy_server: Option<&str>, bypass: Option<&str>) -> Result<String, ZendriverError>` — returns the new `browserContextId`.
  - `pub fn Browser::browser_context(&self) -> BrowserContextBuilder`
  - `BrowserContextBuilder` with `pub fn proxy(self, impl Into<String>) -> Self`, `pub fn proxy_bypass(self, impl Into<String>) -> Self`, `#[cfg(feature="interception")] pub fn proxy_auth(self, impl Into<String>, impl Into<String>) -> Self`, `pub async fn build(self) -> Result<BrowserContext, ZendriverError>`.

- [ ] **Step 1: Add the `context_proxy_auth` field to `BrowserInner` and every constructor**

In `crates/zendriver/src/browser.rs`, add the field to the `BrowserInner` struct definition, immediately after the `proxy_auth_handle` field (around `browser.rs:1322`):

```rust
    /// Per-`browserContextId` proxy credentials, registered by
    /// [`crate::BrowserContextBuilder::build`] and read by the `TabRegistrar`
    /// to install a `Fetch.authRequired` handler on each tab opened in that
    /// context. Removed on `BrowserContext` drop.
    #[cfg(feature = "interception")]
    pub(crate) context_proxy_auth:
        tokio::sync::Mutex<HashMap<String, (String, String)>>,
```

Then add the initializer to **every** `BrowserInner { … }` struct literal. There are ~10; find them all with:

```bash
grep -n "proxy_auth_handle: std::sync::OnceLock::new()" crates/zendriver/src/browser.rs
```

Beside each `proxy_auth_handle: std::sync::OnceLock::new(),` line, add:

```rust
                #[cfg(feature = "interception")]
                context_proxy_auth: tokio::sync::Mutex::new(HashMap::new()),
```

(match the surrounding indentation at each site).

- [ ] **Step 2: Verify it still compiles on both feature sets**

Run: `cargo build -p zendriver 2>&1 | tail -5 && cargo build -p zendriver --features interception 2>&1 | tail -5`
Expected: both succeed. (If a construction site was missed, the error names the site — add the field there.)

- [ ] **Step 3: Extract the raw `createBrowserContext` send**

In `crates/zendriver/src/browser.rs`, add this method to `impl BrowserInner` (near `dispose_browser_context`, around `browser.rs:1348`):

```rust
    /// Send `Target.createBrowserContext` with an optional (verbatim)
    /// `proxyServer` + `proxyBypassList` and return the new
    /// `browserContextId`. Callers that need userinfo stripping do it before
    /// calling; this method sends whatever `proxy_server` it is given.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] if the response lacks
    /// `browserContextId`; bubbles transport errors from `call_raw`.
    pub(crate) async fn create_browser_context_raw(
        &self,
        proxy_server: Option<&str>,
        bypass: Option<&str>,
    ) -> Result<String, ZendriverError> {
        let mut params = serde_json::Map::new();
        if let Some(p) = proxy_server {
            params.insert("proxyServer".into(), serde_json::Value::String(p.to_string()));
        }
        if let Some(b) = bypass {
            params.insert("proxyBypassList".into(), serde_json::Value::String(b.to_string()));
        }
        let res = self
            .conn
            .call_raw("Target.createBrowserContext", serde_json::Value::Object(params), None)
            .await?;
        res.get("browserContextId")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                ZendriverError::Navigation(
                    "Target.createBrowserContext returned no browserContextId".into(),
                )
            })
    }
```

Then rewrite the body of `Browser::create_browser_context_with` (`browser.rs:3156-3199`) to delegate — preserving its verbatim-`proxyServer` behavior:

```rust
    pub async fn create_browser_context_with(
        &self,
        proxy_server: Option<&str>,
        proxy_bypass_list: Option<&str>,
    ) -> Result<crate::BrowserContext, ZendriverError> {
        let id = self
            .inner
            .create_browser_context_raw(proxy_server, proxy_bypass_list)
            .await?;
        Ok(crate::BrowserContext {
            browser: self.inner.clone(),
            id,
        })
    }
```

- [ ] **Step 4: Run the existing context tests to confirm no behavior change**

Run: `cargo test -p zendriver --lib create_browser_context 2>&1 | tail -20`
Expected: PASS — `create_browser_context_with_sends_correct_cdp` and `create_browser_context_without_proxy_omits_fields` still pass (verbatim `proxyServer`, omitted-when-`None` fields unchanged).

- [ ] **Step 5: Write the failing builder tests**

In `crates/zendriver/src/browser_context.rs`, add a new test module:

```rust
#[cfg(test)]
mod builder_tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    /// `.proxy("http://user:pass@host:port")` sends a **userinfo-free**
    /// `proxyServer` to `Target.createBrowserContext`.
    #[tokio::test]
    async fn build_strips_userinfo_from_proxy_server() {
        let (mut mock, conn) = MockConnection::pair();
        let inner = crate::browser::test_only_inner_from_conn(conn.clone());
        let browser = crate::Browser::test_only_from_inner(inner);

        let fut = tokio::spawn(async move {
            browser
                .browser_context()
                .proxy("http://bob:s3cret@proxy.example:8080")
                .proxy_bypass("<-loopback>")
                .build()
                .await
        });

        let id = mock.expect_cmd("Target.createBrowserContext").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["proxyServer"], "http://proxy.example:8080");
        assert_eq!(sent["params"]["proxyBypassList"], "<-loopback>");
        mock.reply(id, serde_json::json!({ "browserContextId": "CTX1" })).await;

        let ctx = tokio::time::timeout(std::time::Duration::from_secs(2), fut)
            .await
            .expect("build timed out")
            .unwrap()
            .unwrap();
        assert_eq!(ctx.id(), "CTX1");
        conn.shutdown();
    }

    /// Under `interception`, embedded userinfo is registered as the context's
    /// proxy credentials.
    #[cfg(feature = "interception")]
    #[tokio::test]
    async fn build_registers_embedded_credentials() {
        let (mut mock, conn) = MockConnection::pair();
        let inner = crate::browser::test_only_inner_from_conn(conn.clone());
        let browser = crate::Browser::test_only_from_inner(inner.clone());

        let fut = tokio::spawn(async move {
            browser
                .browser_context()
                .proxy("http://bob:s3cret@proxy.example:8080")
                .build()
                .await
        });

        let id = mock.expect_cmd("Target.createBrowserContext").await;
        mock.reply(id, serde_json::json!({ "browserContextId": "CTX1" })).await;
        let _ctx = tokio::time::timeout(std::time::Duration::from_secs(2), fut)
            .await
            .expect("build timed out")
            .unwrap()
            .unwrap();

        let creds = inner.context_proxy_auth.lock().await.get("CTX1").cloned();
        assert_eq!(creds, Some(("bob".into(), "s3cret".into())));
        conn.shutdown();
    }

    /// Explicit `.proxy_auth()` overrides embedded userinfo.
    #[cfg(feature = "interception")]
    #[tokio::test]
    async fn explicit_proxy_auth_overrides_userinfo() {
        let (mut mock, conn) = MockConnection::pair();
        let inner = crate::browser::test_only_inner_from_conn(conn.clone());
        let browser = crate::Browser::test_only_from_inner(inner.clone());

        let fut = tokio::spawn(async move {
            browser
                .browser_context()
                .proxy("http://bob:s3cret@proxy.example:8080")
                .proxy_auth("alice", "hunter2")
                .build()
                .await
        });

        let id = mock.expect_cmd("Target.createBrowserContext").await;
        mock.reply(id, serde_json::json!({ "browserContextId": "CTX1" })).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), fut).await;

        let creds = inner.context_proxy_auth.lock().await.get("CTX1").cloned();
        assert_eq!(creds, Some(("alice".into(), "hunter2".into())));
        conn.shutdown();
    }
}
```

> **Note on test helpers:** these tests need `crate::browser::test_only_inner_from_conn` (already exists — used by `browser_context.rs` drop/new_tab tests) and a `crate::Browser::test_only_from_inner(Arc<BrowserInner>) -> Browser`. Check whether the latter exists (`grep -n "test_only_from_inner\|fn test_only" crates/zendriver/src/browser.rs`). If it does not, add it under `#[cfg(test)]` in `browser.rs`:
> ```rust
>     #[cfg(test)]
>     pub(crate) fn test_only_from_inner(inner: std::sync::Arc<BrowserInner>) -> Self {
>         Browser { inner }
>     }
> ```

- [ ] **Step 6: Run tests to verify they fail**

Run: `cargo test -p zendriver --lib builder_tests 2>&1 | tail -20`
Expected: FAIL — `no method named browser_context`.

- [ ] **Step 7: Implement `browser_context()` + `BrowserContextBuilder`**

In `crates/zendriver/src/browser.rs`, add to `impl Browser` (near the other context methods, around `browser.rs:3211`):

```rust
    /// Start building an isolated [`crate::BrowserContext`] with an optional
    /// proxy and per-context credentials.
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// let browser = zendriver::Browser::builder().launch().await?;
    /// let ctx = browser
    ///     .browser_context()
    ///     .proxy("http://user:pass@host:3128")
    ///     .proxy_bypass("<-loopback>")
    ///     .build()
    ///     .await?;
    /// let tab = ctx.new_tab().await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn browser_context(&self) -> crate::BrowserContextBuilder {
        crate::BrowserContextBuilder::new(self.inner.clone())
    }
```

In `crates/zendriver/src/browser_context.rs`, add the builder (and re-export it from `lib.rs` — see Step 8):

```rust
/// Builder for an isolated [`BrowserContext`] with an optional proxy and
/// per-context proxy credentials. Created via [`crate::Browser::browser_context`].
pub struct BrowserContextBuilder {
    browser: Arc<BrowserInner>,
    proxy: Option<String>,
    bypass: Option<String>,
    #[cfg(feature = "interception")]
    explicit_auth: Option<(String, String)>,
}

impl BrowserContextBuilder {
    pub(crate) fn new(browser: Arc<BrowserInner>) -> Self {
        Self {
            browser,
            proxy: None,
            bypass: None,
            #[cfg(feature = "interception")]
            explicit_auth: None,
        }
    }

    /// Upstream proxy `scheme://[user[:pass]@]host:port`. Embedded userinfo is
    /// used as the auth credentials (and stripped from the `proxyServer` sent
    /// to Chrome, which ignores it there) unless overridden by
    /// [`Self::proxy_auth`].
    #[must_use]
    pub fn proxy(mut self, proxy: impl Into<String>) -> Self {
        self.proxy = Some(proxy.into());
        self
    }

    /// Hosts matching this pattern bypass the proxy (e.g. `"<-loopback>"`).
    #[must_use]
    pub fn proxy_bypass(mut self, bypass: impl Into<String>) -> Self {
        self.bypass = Some(bypass.into());
        self
    }

    /// Explicit proxy credentials; overrides any userinfo embedded in
    /// [`Self::proxy`].
    #[cfg(feature = "interception")]
    #[must_use]
    pub fn proxy_auth(mut self, user: impl Into<String>, pass: impl Into<String>) -> Self {
        self.explicit_auth = Some((user.into(), pass.into()));
        self
    }

    /// Create the context: sends `Target.createBrowserContext` with a
    /// userinfo-free `proxyServer`, registers credentials (if any), and
    /// returns the [`BrowserContext`].
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] on an invalid proxy URL or a
    /// response missing `browserContextId`.
    pub async fn build(self) -> Result<BrowserContext, ZendriverError> {
        let (proxy_server, embedded_creds) = match self.proxy.as_deref() {
            Some(p) => {
                let parsed = crate::proxy::split_proxy_url(p)?;
                (Some(parsed.server), parsed.credentials)
            }
            None => (None, None),
        };

        let id = self
            .browser
            .create_browser_context_raw(proxy_server.as_deref(), self.bypass.as_deref())
            .await?;

        #[cfg(feature = "interception")]
        {
            if let Some(creds) = self.explicit_auth.or(embedded_creds) {
                self.browser
                    .context_proxy_auth
                    .lock()
                    .await
                    .insert(id.clone(), creds);
            }
        }
        #[cfg(not(feature = "interception"))]
        {
            if embedded_creds.is_some() {
                tracing::warn!(
                    context_id = %id,
                    "proxy credentials supplied but the `interception` feature is off; \
                     per-context proxy auth is inactive"
                );
            }
        }

        Ok(BrowserContext {
            browser: self.browser,
            id,
        })
    }
}
```

- [ ] **Step 8: Re-export the builder**

In `crates/zendriver/src/lib.rs`, wherever `BrowserContext` is re-exported (`grep -n "BrowserContext" crates/zendriver/src/lib.rs`), add `BrowserContextBuilder` to the same `pub use`:

```rust
pub use browser_context::{BrowserContext, BrowserContextBuilder};
```

- [ ] **Step 9: Run tests (default + interception) to verify they pass**

Run:
```bash
cargo test -p zendriver --lib builder_tests 2>&1 | tail -20
cargo test -p zendriver --lib --features interception builder_tests 2>&1 | tail -20
```
Expected: default run passes `build_strips_userinfo_from_proxy_server` (the `#[cfg(feature="interception")]` tests are compiled out); interception run passes all three.

- [ ] **Step 10: Commit**

```bash
git add crates/zendriver/src/browser.rs crates/zendriver/src/browser_context.rs crates/zendriver/src/lib.rs
git commit -m "feat(context): add BrowserContextBuilder with per-context proxy credentials"
```

---

### Task 3: Auto-install auth on each context tab (the core mechanism)

Re-gate the per-session interception handle map from `tracker-blocking` to `interception` (rename `tracker_handles` → `session_intercept_handles`), and extend the `TabRegistrar` page-attach arm to chain `handle_auth` into the one per-session actor when the new tab's `browserContextId` has registered credentials.

**Files:**
- Modify: `crates/zendriver/src/browser.rs` — field decl (`~1334`), all initializers, the page-attach install block (`1560-1574`), the detach removal (`1607-1610`), the clone at `1537-1538`.
- Test: `crates/zendriver/src/browser.rs` (`#[cfg(test)]`, following the `tab_registrar_inserts_page_target_into_tabs_map` pattern at `browser.rs:4324`).

**Interfaces:**
- Consumes: `BrowserInner.context_proxy_auth` (Task 2); `TargetInfo.browser_context_id` (`crates/zendriver-transport/src/observer.rs:120`); `zendriver_interception::InterceptBuilder::{new, handle_auth, block_hosts, start}`.
- Produces: `BrowserInner.session_intercept_handles` (renamed from `tracker_handles`, gated `#[cfg(feature = "interception")]`) — read by Task 4's neighbors / removed on detach here.

- [ ] **Step 1: Rename + re-gate the handle-map field**

In `crates/zendriver/src/browser.rs`, replace the `tracker_handles` field decl (`~1334`) with:

```rust
    /// Live per-session interception handles keyed by `sessionId` (main tab +
    /// each page tab). Holds the single chained actor per session — tracker
    /// blocking and/or per-context proxy auth. Inserted on attach, removed on
    /// detach; dropping a handle stops that tab's actor. One actor per session
    /// so they never double-resolve `Fetch.requestPaused` (cdpdriver/zendriver#208).
    #[cfg(feature = "interception")]
    pub(crate) session_intercept_handles:
        tokio::sync::Mutex<HashMap<String, zendriver_interception::InterceptHandle>>,
```

Update every initializer. Find them:

```bash
grep -n "tracker_handles: tokio::sync::Mutex::new" crates/zendriver/src/browser.rs
```

Replace each `#[cfg(feature = "tracker-blocking")] tracker_handles: tokio::sync::Mutex::new(HashMap::new()),` with:

```rust
                #[cfg(feature = "interception")]
                session_intercept_handles: tokio::sync::Mutex::new(HashMap::new()),
```

(Leave the `tracker_matcher` field and its `#[cfg(feature = "tracker-blocking")]` gate unchanged.)

- [ ] **Step 2: Re-gate the session clone**

At `browser.rs:1537-1538`, change:

```rust
                #[cfg(feature = "tracker-blocking")]
                let new_session_for_tracker = new_session.clone();
```

to:

```rust
                #[cfg(feature = "interception")]
                let new_session_for_intercept = new_session.clone();
```

- [ ] **Step 3: Replace the install block with the chained builder**

Replace the tracker-only install block at `browser.rs:1560-1574` with:

```rust
                // Per-session interception: chain tracker-blocking (if a
                // matcher is configured) and per-context proxy auth (if this
                // tab's browser context registered credentials) into ONE
                // actor, then park the handle keyed by sessionId so it lives
                // with the browser. One actor per session — never two — so
                // they don't double-resolve the same `Fetch.requestPaused`
                // (cdpdriver/zendriver#208).
                #[cfg(feature = "interception")]
                {
                    let mut builder = zendriver_interception::InterceptBuilder::new(
                        &new_session_for_intercept,
                    );
                    let mut needs_actor = false;

                    #[cfg(feature = "tracker-blocking")]
                    if let Some(matcher) = browser.tracker_matcher.clone() {
                        builder = builder.block_hosts(matcher);
                        needs_actor = true;
                    }

                    if let Some(ctx_id) = session.target_info.browser_context_id.as_deref() {
                        let creds =
                            browser.context_proxy_auth.lock().await.get(ctx_id).cloned();
                        if let Some((user, pass)) = creds {
                            builder = builder.handle_auth(user, pass);
                            needs_actor = true;
                        }
                    }

                    if needs_actor {
                        let handle = builder.start();
                        browser.session_intercept_handles.lock().await.insert(
                            new_session_for_intercept.session_id().to_string(),
                            handle,
                        );
                    }
                }
```

- [ ] **Step 4: Re-gate the detach removal**

At `browser.rs:1606-1610`, change:

```rust
        // Drop any tracker-blocking handle for this session (stops its actor).
        #[cfg(feature = "tracker-blocking")]
        {
            browser.tracker_handles.lock().await.remove(session_id);
        }
```

to:

```rust
        // Drop any per-session interception handle (stops its actor).
        #[cfg(feature = "interception")]
        {
            browser.session_intercept_handles.lock().await.remove(session_id);
        }
```

- [ ] **Step 5: Write the failing auto-install test**

Add to the `#[cfg(test)]` tests in `crates/zendriver/src/browser.rs` (model it on `tab_registrar_inserts_page_target_into_tabs_map`, `browser.rs:4324`; it must be `#[cfg(feature = "interception")]`). The test seeds a `BrowserInner` whose `context_proxy_auth` maps `"CTX1" -> ("bob","s3cret")`, emits an attach event whose `targetInfo.browserContextId == "CTX1"`, then drives the `Fetch.authRequired` handshake and asserts `Fetch.continueWithAuth` carries the creds:

```rust
    #[cfg(feature = "interception")]
    #[tokio::test]
    async fn tab_registrar_installs_context_proxy_auth() {
        use zendriver_transport::testing::MockConnection;

        let input_profile = zendriver_stealth::InputProfile::native();
        let registrar = Arc::new(TabRegistrar::new(input_profile.clone()));
        let (mut mock, conn) =
            MockConnection::pair_with_observers(vec![registrar.clone() as Arc<dyn TargetObserver>]);

        let inner = Arc::new_cyclic(|weak: &Weak<BrowserInner>| {
            let main_session = SessionHandle::new(conn.clone(), "S1");
            let main_input = InputController::new(input_profile.clone());
            let main_tab = Tab::new(main_session, weak.clone(), main_input, "T1".to_string());
            let mut map = HashMap::new();
            map.insert("S1".to_string(), main_tab.clone());
            // Seed credentials for context CTX1.
            let mut auth = HashMap::new();
            auth.insert("CTX1".to_string(), ("bob".to_string(), "s3cret".to_string()));
            BrowserInner {
                conn: conn.clone(),
                main_tab,
                child: tokio::sync::Mutex::new(None),
                job: ProcessJob::none(),
                _user_data: None,
                _extension_dirs: Vec::new(),
                owns_process: false,
                stealth_input_profile: input_profile.clone(),
                tabs: tokio::sync::RwLock::new(map),
                debug_host_port: None,
                ws_url: None,
                tabs_changed: tokio::sync::Notify::new(),
                #[cfg(feature = "interception")]
                proxy_auth_handle: std::sync::OnceLock::new(),
                #[cfg(feature = "interception")]
                context_proxy_auth: tokio::sync::Mutex::new(auth),
                #[cfg(feature = "tracker-blocking")]
                tracker_matcher: None,
                #[cfg(feature = "interception")]
                session_intercept_handles: tokio::sync::Mutex::new(HashMap::new()),
            }
        });
        registrar.set_browser(Arc::downgrade(&inner));

        // Attach a page target that belongs to CTX1.
        mock.emit_event(
            "Target.attachedToTarget",
            json!({
                "sessionId": "S2",
                "targetInfo": {
                    "targetId": "T2",
                    "type": "page",
                    "url": "about:blank",
                    "attached": true,
                    "browserContextId": "CTX1",
                },
                "waitingForDebugger": true,
            }),
        )
        .await;

        // The install sends `Fetch.enable { handleAuthRequests: true }`.
        let enable_id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Fetch.enable"),
        )
        .await
        .expect("Fetch.enable not sent");
        let enable = mock.last_sent();
        assert_eq!(enable["params"]["handleAuthRequests"], true);
        mock.reply(enable_id, json!({})).await;

        // Simulate an auth challenge; the actor must answer with the creds.
        mock.emit_event(
            "Fetch.authRequired",
            json!({
                "requestId": "R1",
                "authChallenge": { "source": "Proxy", "origin": "http://proxy", "scheme": "basic", "realm": "" },
            }),
        )
        .await;

        let auth_id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Fetch.continueWithAuth"),
        )
        .await
        .expect("Fetch.continueWithAuth not sent");
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["authChallengeResponse"]["response"], "ProvideCredentials");
        assert_eq!(sent["params"]["authChallengeResponse"]["username"], "bob");
        assert_eq!(sent["params"]["authChallengeResponse"]["password"], "s3cret");
        mock.reply(auth_id, json!({})).await;

        conn.shutdown();
    }
```

> **Verify the wire shape first:** before finalizing the asserts, confirm the exact `Fetch.continueWithAuth` field names the actor emits by reading `crates/zendriver-interception/src/actor.rs:253-283`. Match the assert keys/values to what the actor actually sends (the mock echoes the real actor). Adjust the `authChallengeResponse` keys if they differ.

- [ ] **Step 6: Run the test to verify it fails, then passes**

Run: `cargo test -p zendriver --lib --features interception tab_registrar_installs_context_proxy_auth 2>&1 | tail -30`
Expected: after Steps 1-4 are implemented, PASS. If it fails on the wire-shape asserts, reconcile with `actor.rs` (see the note) — the mechanism (enable + continueWithAuth firing at all) is the real signal.

- [ ] **Step 7: Full gate check across feature sets**

Run in parallel:
```bash
cargo build -p zendriver 2>&1 | tail -3
cargo build -p zendriver --features interception 2>&1 | tail -3
cargo build -p zendriver --features tracker-blocking 2>&1 | tail -3
cargo clippy -p zendriver --all-targets --features interception -- -D warnings 2>&1 | tail -5
```
Expected: all clean. (Guards the `needs_actor` / cfg interplay under interception-without-tracker-blocking.)

- [ ] **Step 8: Commit**

```bash
git add crates/zendriver/src/browser.rs
git commit -m "feat(context): auto-install per-context proxy auth on each context tab

Chains handle_auth into the existing one-actor-per-session interception
install (zendriver#208); renames tracker_handles -> session_intercept_handles
and re-gates it to the interception feature so auth works without tracker-blocking."
```

---

### Task 4: Unregister credentials on `BrowserContext` drop

**Files:**
- Modify: `crates/zendriver/src/browser_context.rs` — `Drop` impl (`browser_context.rs:94-107`).
- Test: `crates/zendriver/src/browser_context.rs` (`#[cfg(test)]`, `#[cfg(feature = "interception")]`).

**Interfaces:**
- Consumes: `BrowserInner.context_proxy_auth` (Task 2).
- Produces: nothing new.

- [ ] **Step 1: Write the failing test**

Add to `crates/zendriver/src/browser_context.rs`:

```rust
#[cfg(all(test, feature = "interception"))]
mod auth_cleanup_tests {
    use super::*;
    use zendriver_transport::testing::MockConnection;

    /// Dropping a `BrowserContext` removes its entry from the browser's
    /// `context_proxy_auth` registry.
    #[tokio::test]
    async fn drop_unregisters_context_credentials() {
        let (mut mock, conn) = MockConnection::pair();
        let inner = crate::browser::test_only_inner_from_conn(conn.clone());
        inner
            .context_proxy_auth
            .lock()
            .await
            .insert("CTX1".to_string(), ("bob".into(), "s3cret".into()));

        {
            let _ctx = BrowserContext {
                browser: inner.clone(),
                id: "CTX1".into(),
            };
        } // drop here

        // Drop spawns a task; wait for the dispose call to confirm it ran.
        let id = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mock.expect_cmd("Target.disposeBrowserContext"),
        )
        .await
        .expect("dispose did not fire");
        mock.reply(id, serde_json::json!({})).await;

        // Give the spawned task a moment to complete the removal.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(inner.context_proxy_auth.lock().await.get("CTX1").is_none());
        conn.shutdown();
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zendriver --lib --features interception drop_unregisters_context_credentials 2>&1 | tail -20`
Expected: FAIL — the entry is still present (drop doesn't remove it yet).

- [ ] **Step 3: Add the removal to `Drop`**

In `crates/zendriver/src/browser_context.rs`, update the `Drop` impl body so the spawned task removes the entry before disposing:

```rust
impl Drop for BrowserContext {
    fn drop(&mut self) {
        let browser = self.browser.clone();
        let id = std::mem::take(&mut self.id);
        if id.is_empty() {
            return; // already disposed
        }
        tokio::spawn(async move {
            #[cfg(feature = "interception")]
            browser.context_proxy_auth.lock().await.remove(&id);
            if let Err(e) = browser.dispose_browser_context(&id).await {
                tracing::warn!(context_id = %id, error = %e, "BrowserContext dispose failed");
            }
        });
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p zendriver --lib --features interception drop_unregisters_context_credentials 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zendriver/src/browser_context.rs
git commit -m "feat(context): unregister proxy credentials on BrowserContext drop"
```

---

### Task 5: Docs, example, MCP ledger, backlog

No new logic — wire the new API into the docs surfaces (`CLAUDE.md` requires all three), modernize the example, and record the MCP decision.

**Files:**
- Modify: `crates/zendriver/examples/browser_context_isolation.rs`
- Modify: `docs/book/src/browser-context.md`
- Modify: `README.md` (+ `crates/zendriver-mcp/README.md` only if it references per-context auth)
- Modify: `crates/zendriver-mcp/mcp-coverage-ledger.toml`
- Modify: `docs/superpowers/deferred-backlog.md`

- [ ] **Step 1: Rewrite the example onto the builder**

In `crates/zendriver/examples/browser_context_isolation.rs`, delete the `split_proxy` helper and the per-tab `handle_auth` juggling; replace each context setup with:

```rust
    let ctx1 = browser.browser_context().proxy(&proxy).build().await?;
    let tab1 = ctx1.new_tab().await?;
    tab1.goto("https://ipv4.webshare.io/").await?;
```

(The builder now handles userinfo splitting and per-tab auth install; no `InterceptHandle` to hold.) Update the module doc comment to describe the first-class API.

Run: `cargo build --example browser_context_isolation -p zendriver --features interception 2>&1 | tail -5`
Expected: compiles.

- [ ] **Step 2: Update the mdBook chapter**

In `docs/book/src/browser-context.md`, replace the roadmap notes at `:73` ("per-context auth is on the roadmap") and `:173` ("No per-context auth yet") with the builder API + a `.proxy("http://user:pass@host:port")` example. Confirm the book builds:

Run: `mdbook build docs/book 2>&1 | tail -5`
Expected: build succeeds.

- [ ] **Step 3: Update the README(s)**

In `README.md`, add per-context proxy auth to the browser-context feature description. Grep first to see if `create_browser_context` is mentioned; keep the MCP tool count unchanged (nothing new on the wire). Only touch `crates/zendriver-mcp/README.md` if it references per-context auth.

Run: `grep -rn "per-context\|browser_context\|proxy_auth" README.md crates/zendriver-mcp/README.md`

- [ ] **Step 4: Record the MCP ledger decision**

In `crates/zendriver-mcp/mcp-coverage-ledger.toml`, add `excluded` entries for the new public items (`Browser::browser_context`, `BrowserContextBuilder` and its methods), following the file's existing entry shape:

```toml
[items."zendriver::Browser::browser_context"]
excluded = "BrowserContext is a handle-returning lifecycle API; per-context proxy+auth is configured at Rust construction time and its tabs are driven through existing tab tools — no request/response MCP surface. Agent-facing per-context proxies would be a separate browser_context_open tool (out of scope)."
```

(Add matching entries for `BrowserContextBuilder::{proxy, proxy_bypass, proxy_auth, build}` if the public-api check flags them individually — see Step 5.)

- [ ] **Step 5: Run the public-api + schema checks**

Run:
```bash
cargo +nightly test -p zendriver-mcp --features public-api-check --test public_api --locked 2>&1 | tail -30
```
If it fails listing the new items as uncovered, either add ledger entries (Step 4) or, if you intentionally changed the public API, regenerate the baseline:
```bash
cargo +nightly public-api -p zendriver --all-features > crates/zendriver-mcp/public-api-baseline.txt
```
Then the schema snapshots (no MCP I/O type changed here, but run to be safe):
```bash
cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked 2>&1 | tail -10
cargo insta accept --all
```

- [ ] **Step 6: Flip the backlog entry**

In `docs/superpowers/deferred-backlog.md`, move the "Per-context proxy auth — first-class API" item from §1 to the `✅ closed since snapshot` section with a one-line note (shipped via `BrowserContextBuilder`).

- [ ] **Step 7: Final gates + commit**

Run (parallel):
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p zendriver-mcp --all-features --all-targets -- -D warnings
cargo test -p zendriver --lib --features interception 2>&1 | tail -10
```
Expected: all clean/green.

```bash
git add crates/zendriver/examples/browser_context_isolation.rs docs/book/src/browser-context.md README.md crates/zendriver-mcp/README.md crates/zendriver-mcp/mcp-coverage-ledger.toml crates/zendriver-mcp/public-api-baseline.txt docs/superpowers/deferred-backlog.md
git commit -m "docs(context): document first-class per-context proxy auth across all surfaces"
```

---

## Self-Review

**Spec coverage:**
- Builder API (`proxy`/`proxy_auth`/`proxy_bypass`/`build`) → Task 2. ✓
- Userinfo auto-split + stripped `proxyServer` → Task 1 + Task 2. ✓
- Explicit `.proxy_auth()` override → Task 2 (test `explicit_proxy_auth_overrides_userinfo`). ✓
- Credential registry + auto-install chained into one actor (#208) → Task 2 (field) + Task 3 (install). ✓
- `create_browser_context_with` kept, routed through shared raw-send → Task 2. ✓
- Feature gating (proxy default-features; auth interception-gated; warn-without-interception) → Task 2 build(). ✓
- Drop cleanup → Task 4. ✓
- MCP ledger excluded → Task 5. ✓
- Docs (rustdoc inline in Tasks 2/3; mdBook + README + example + backlog) → Task 5. ✓
- Integration example on real Chrome (nightly `#[ignore]`) → Task 5 rewrites the example (already `#[ignore]`-class, run manually). ✓

**Placeholder scan:** No TBD/TODO; every code step carries complete code. The two "verify the exact wire shape / test-helper existence" notes point at specific files to read, not vague instructions.

**Type consistency:** `split_proxy_url` / `ParsedProxy{server,credentials}` (Task 1) used verbatim in Task 2 build(). `context_proxy_auth: Mutex<HashMap<String,(String,String)>>` defined Task 2, read Task 3, cleared Task 4 — same type throughout. `session_intercept_handles` renamed consistently across decl/init/install/detach in Task 3. `create_browser_context_raw(Option<&str>, Option<&str>) -> Result<String>` defined + used in Task 2.

**Open risk flagged for the implementer:** Task 3 Step 5's `Fetch.continueWithAuth` assert keys must match `zendriver-interception/src/actor.rs:253-283` exactly — the note says read it first. This is the one place the plan can't fully pin without the actor's current wire shape in front of it.
