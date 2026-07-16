# Final review fixes — per-context proxy auth feature

Fixes for 5 findings from the final review, clustered in
`crates/zendriver/src/proxy.rs`, `crates/zendriver/src/browser_context.rs`,
and `crates/zendriver/src/browser.rs`.

## Per-finding resolution

**Finding 1 (Important) — percent-encoded userinfo sent un-decoded.**
Fixed in `crates/zendriver/src/proxy.rs`. `split_proxy_url` now
percent-decodes both `u.username()` and `u.password()` via
`percent_encoding::percent_decode_str(..).decode_utf8_lossy().into_owned()`
before storing them as `ParsedProxy::credentials`. Added
`percent-encoding` as a new workspace dependency (`Cargo.toml`
`[workspace.dependencies]`, referenced via `percent-encoding.workspace = true`
in `crates/zendriver/Cargo.toml`) — it was already in `Cargo.lock`
transitively via `url`, so `cargo build -p zendriver` only added one line
back to the lockfile (`zendriver`'s own dependency list gained
`percent-encoding`). Test: `splits_percent_encoded_userinfo` — asserts
`http://bob:p%40ss%3Aword@proxy.example:8080` yields password `p@ss:word`.

**Finding 2 (Minor) — credentials leak into error messages.**
Fixed in `crates/zendriver/src/proxy.rs`. Added a private `redact_userinfo`
helper that does string surgery (not `url::Url`-based, since it must also
work on URLs that failed to parse) to replace `user[:pass]@` with `***@` in
the authority component, leaving scheme/host/port/path/query untouched. All
three error sites (`invalid proxy URL`, `missing host`, `missing port`) now
format the redacted string instead of the raw `url`. Tests:
`error_messages_never_contain_the_password` (missing-host and missing-port
cases with a password in the userinfo) and
`redact_userinfo_masks_credentials_but_keeps_host` (unit-tests the helper
directly, including the no-userinfo and unparseable-string no-op cases).

**Finding 3 (Minor) — socks proxies rejected for missing port.**
Fixed in `crates/zendriver/src/proxy.rs`. When `port_or_known_default()`
returns `None` and the scheme is `socks4`/`socks5`, the port now defaults to
`1080` instead of erroring. Tests: `socks5_defaults_to_1080`,
`socks4_defaults_to_1080`, and `explicit_port_overrides_socks_default`
(`socks5://host:9999` keeps the explicit port).

**Finding 4 (Minor) — the #208 one-actor invariant has no test.**
Fixed in `crates/zendriver/src/browser.rs`. Added
`tab_registrar_chains_tracker_and_auth_into_one_actor`
(`#[cfg(all(test, feature = "tracker-blocking"))]`), placed right after
`tab_registrar_installs_context_proxy_auth`. It builds a `BrowserInner` with
both a real `Arc<crate::HostMatcher>` (`tracker_matcher: Some(...)`, blocking
`"evil.example"`) and a `context_proxy_auth` entry for `CTX1` seeded, then
emits `Target.attachedToTarget` for a page target in `CTX1`. It asserts
exactly one `Fetch.enable` is sent (`mock.expect_cmd`), replies, then polls
`mock.try_recv_cmd()` after a short sleep to confirm no second command (and
in particular no second `Fetch.enable`) arrives — proving the registrar
chains tracker-blocking + auth into the single `InterceptBuilder` per
session rather than spawning two competing actors.

**Finding 5 (Minor) — weak assertion in an existing test.**
Fixed in `crates/zendriver/src/browser_context.rs`,
`explicit_proxy_auth_overrides_userinfo`. Replaced
`let _ = tokio::time::timeout(...).await;` with
`.await.expect("build timed out").unwrap().unwrap();`, bound to `_ctx` (not
discarded outright) — binding matters here because dropping the built
`BrowserContext` immediately would fire the background task that
unregisters its `context_proxy_auth` entry, racing the very assertion the
test makes afterward. This now matches the sibling tests
(`build_strips_userinfo_from_proxy_server`, `build_registers_embedded_credentials`).

## Command output (real, captured this session)

```
$ cargo test -p zendriver --lib --features interception proxy:: 2>&1 | tail -15
running 12 tests
test proxy::tests::explicit_port_overrides_socks_default ... ok
test proxy::tests::socks4_defaults_to_1080 ... ok
test proxy::tests::redact_userinfo_masks_credentials_but_keeps_host ... ok
test proxy::tests::no_userinfo_yields_no_credentials ... ok
test proxy::tests::fills_default_port_from_scheme ... ok
test proxy::tests::socks5_defaults_to_1080 ... ok
test proxy::tests::unparseable_is_an_error ... ok
test proxy::tests::missing_host_is_an_error ... ok
test proxy::tests::splits_percent_encoded_userinfo ... ok
test proxy::tests::username_without_password_yields_empty_pass ... ok
test proxy::tests::splits_userinfo_from_server ... ok
test proxy::tests::error_messages_never_contain_the_password ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 302 filtered out; finished in 0.00s
```

```
$ cargo test -p zendriver --lib --features tracker-blocking 2>&1 | tail -8
test tab::tests::wait_for_idle_opts_evicts_stuck_request_past_max_inflight_age ... ok
test tab::tests::wait_for_idle_resolves_after_quiet_window_post_response ... ok
test browser::tests::close_sends_browser_close_before_any_process_kill ... ok

test result: ok. 320 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.02s
```

(`tab_registrar_chains_tracker_and_auth_into_one_actor` verified individually too:
`test browser::tests::tab_registrar_chains_tracker_and_auth_into_one_actor ... ok`.)

```
$ cargo test -p zendriver --lib --features interception builder_tests 2>&1 | tail -8
test query::tests::predicate_builder_tests::describe_predicates_renders_css_and_filter ... ok
test browser_context::builder_tests::build_strips_userinfo_from_proxy_server ... ok
test browser_context::builder_tests::build_registers_embedded_credentials ... ok
test browser_context::builder_tests::explicit_proxy_auth_overrides_userinfo ... ok

test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 303 filtered out; finished in 0.01s
```

```
$ cargo fmt --all && cargo fmt --all --check
(no output — clean)
```

```
$ cargo clippy -p zendriver --all-targets --features interception -- -D warnings 2>&1 | tail -4
    Checking zendriver v0.2.20 (.../crates/zendriver)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.73s
```

```
$ cargo clippy -p zendriver --all-targets -- -D warnings 2>&1 | tail -4
    Checking zendriver v0.2.20 (.../crates/zendriver)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.79s
```

Additionally ran (not in the contract, but per project CLAUDE.md since a
feature-gated test was touched):

```
$ cargo clippy -p zendriver --all-targets --features tracker-blocking -- -D warnings 2>&1 | tail -4
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.38s

$ cargo clippy --workspace --all-targets --locked -- -D warnings 2>&1 | tail -4
    Checking zendriver-mcp v0.6.8 (.../crates/zendriver-mcp)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.12s
```

## Lockfile

`cargo build -p zendriver` confirmed the lockfile is consistent after adding
`percent-encoding` — the only diff is `zendriver` gaining `percent-encoding`
in its dependency list (the crate itself was already present transitively
via `url`, so no new crate was pulled in, no version bump elsewhere).

## Findings NOT fixed

None — all 5 findings fixed and verified.
