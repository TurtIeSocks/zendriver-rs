# MCP server (`zendriver-mcp`)

`zendriver-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io/)
server that exposes zendriver-rs through 71 MCP tools (72 with the
optional `fingerprints` feature), so any
MCP-compatible client (Claude Desktop, Claude Code, custom agents) can
drive a real, stealth-by-default Chrome browser.

> **Using Claude Code?** The [`zendriver` plugin](./plugin.md) installs this
> server plus scraping skills, commands, and a subagent in two commands — see
> the [plugin chapter](./plugin.md).

## Install

```bash
cargo install zendriver-mcp
```

The default build enables `interception`, `expect`, `cloudflare`,
`imperva`, `datadome`, `monitor`, `fetcher`, and `tracker-blocking`. The
`fingerprints` and `geo` features are opt-in (add
`--features fingerprints,geo`). For a lean build:

```bash
cargo install zendriver-mcp --no-default-features
```

## Claude Desktop

```json
{
  "mcpServers": {
    "zendriver": {
      "command": "zendriver-mcp"
    }
  }
}
```

## HTTP mode

```bash
zendriver-mcp --http 127.0.0.1:8765
```

Bind localhost-only by default. It is the operator's responsibility to
expose the endpoint via a reverse proxy + mTLS / network policy for
remote access.

## CLI flags

```text
zendriver-mcp [OPTIONS]

OPTIONS:
    --http <ADDR>                  Run streamable HTTP transport on ADDR
                                   (e.g. 127.0.0.1:8765). Default: stdio.
    --stealth-profile <KIND>       Default stealth profile.
                                   [auto|native|spoof_macos|spoof_linux|spoof_windows]
                                   Default: auto
    --log <FILTER>                 Tracing log filter (EnvFilter syntax).
                                   Default: info
    -h, --help
    -V, --version
```

## Tool surface

71 tools across these categories (72 with the optional `fingerprints`
feature):

| Category               | Tools                                                                                                                       | Count |
| ---------------------- | --------------------------------------------------------------------------------------------------------------------------- | ----: |
| Lifecycle              | `browser_open / _close / _status`                                                                                           |     3 |
| Navigation             | `browser_goto / _back / _forward / _reload / _wait_for_idle / _wait_for_load / _bypass_insecure_warning`                    |     7 |
| Scroll / Window        | `browser_scroll / _get_window / _set_window`                                                                                |     3 |
| Tabs                   | `browser_tab_list / _new / _switch / _close / _activate`                                                                    |     5 |
| Find                   | `browser_find / _find_all`                                                                                                  |     2 |
| Actions                | `browser_click / _hover / _tap / _type / _press / _key_sequence / _mouse / _set_value / _clear / _focus / _scroll_into_view / _upload` |    12 |
| Reads                  | `browser_element_state / _get_links / _search_resources`                                                                    |     3 |
| Snapshots / Export     | `browser_html / _screenshot / _pdf / _save_mhtml`                                                                           |     4 |
| Eval                   | `browser_evaluate / _evaluate_main`                                                                                         |     2 |
| Network                | `browser_request`                                                                                                           |     1 |
| Cookies                | `browser_cookies_get / _set / _delete / _clear / _persist`                                                                  |     5 |
| Storage                | `browser_storage_get / _set / _delete / _clear`                                                                             |     4 |
| Downloads              | `browser_download / _set_download_path`                                                                                     |     2 |
| Frames                 | `browser_frame_list / _frame_goto`                                                                                          |     2 |
| Stealth                | `browser_set_stealth_profile / _set_user_agent`                                                                             |     2 |
| Interception (gated)   | `browser_intercept_add_rule / _remove_rule / _list_rules / _clear_rules`                                                    |     4 |
| Expect (gated)         | `browser_expect_register / _await / _cancel`                                                                                |     3 |
| Cloudflare (gated)     | `browser_solve_turnstile`                                                                                                   |     1 |
| Imperva (gated)        | `browser_solve_imperva`                                                                                                     |     1 |
| DataDome (gated)       | `browser_solve_datadome`                                                                                                   |     1 |
| Fetcher (gated)        | `browser_install_chrome`                                                                                                    |     1 |
| Monitor (gated)        | `browser_monitor_start / _read / _stop`                                                                                    |     3 |
| Fingerprint (gated)    | `browser_fingerprint_generate`                                                                                             |     1 |

All find / action tools share a `Selector` arg — one-of `css | xpath |
text | text_exact | text_regex | role`, or bs4-like predicate mode (`tag`
+ `attrs: [{ name, op, value, case_insensitive }]`, with `op` one of
`eq | contains | starts_with | ends_with | has | regex`), plus modifiers
`nth / visible_only / timeout_ms / frame_id`. In predicate mode, `text` /
`text_exact` double as `containing_text` / `text_equals` post-filters;
`text_case_insensitive: bool` makes either comparison case-insensitive.
`AttrPredicate.case_insensitive` does the same for `eq | contains |
starts_with | ends_with` (rejected for `has`/`regex`, which have no value
to fold case on / are already case-insensitive via an inline `(?i)`
pattern flag). State-changing tools accept
`return_snapshot: bool` for one-call action + observe. `browser_pdf` /
`_save_mhtml` / `_download` return a binary-output shape (`{ byte_len,
saved_path? , base64? }`): bytes go to `save_path` on the MCP host when
given, else base64-inline (capped at 5 MiB). The default build enables
`interception` / `expect` / `cloudflare` / `imperva` / `datadome` /
`monitor` / `fetcher` / `tracker-blocking`; `fingerprints` and `geo` are
opt-in.

Full JSON Schema for every tool's input + output is captured in
`crates/zendriver-mcp/tests/snapshots/` and changes there require an
explicit `cargo insta accept` — the wire shape is reviewed.

## `browser_open` options

`browser_open` accepts opt-in third-party tracker / fingerprinter blocking:

- `block_trackers: bool` — enable blocking with the curated bundled list.
- `tracker_blocklist` — one of `{ "url": "..." }`, `{ "path": "..." }`, or
  `{ "domains": ["..."] }` to add custom hosts (implicitly enables blocking).
  Requires the default `tracker-blocking` feature.

The stealth-profile override accepts `geo_country` (ISO 3166-1 alpha-2, e.g.
`"DE"`) to derive a coherent `locale` + `Accept-Language`. The field is always
present in the schema but only takes effect when the `geo` feature is enabled.

`browser_open` also accepts:

- `proxy: string` — route the browser through an upstream proxy
  (`scheme://[user:pass@]host:port`); userinfo is auto-split into proxy-auth
  credentials (requires the default `interception` feature to actually answer
  the `Fetch.authRequired` challenge). Always present in the schema.
- `geo_auto: bool` — auto-derive `locale`/`languages` from the exit IP's
  country via a proxied probe to `ip-api.com` (mirrors `proxy` above), instead
  of naming a country explicitly via `geo_country`. Makes at most one outbound
  request, only at launch, only when `true`. An explicit `persona` locale wins
  and skips the probe. Requires the opt-in `geo` feature; always present in
  the schema (ignored with a logged warning on a non-`geo` build).
- `geo_endpoint: string` — override the probe endpoint (default
  `http://ip-api.com/json`); only meaningful with `geo_auto: true`. Note this
  bypasses proxy mirroring — only the bundled default endpoint routes through
  `proxy`.
- `input_profile: "native" | "coherent"` — select input-timing realism
  (keyboard/mouse), **independent** of `stealth_profile`. When unset, the
  default follows the resolved stealth profile: a spoofed stealth profile
  (`spoof_macos`/`spoof_linux`/`spoof_windows`) implies `"coherent"`
  (human-paced typing, jittery mouse motion), while `auto`/`native` stealth
  implies `"native"` (zero-overhead, deterministic timing) — mirroring
  `BrowserBuilder::resolved_input_profile()`. Pass `"native"` or
  `"coherent"` explicitly to pin the input timing regardless of the stealth
  setting. Wraps `zendriver::stealth::InputProfile` via
  `BrowserBuilder::input_profile`; the output `input_profile` field always
  echoes the *resolved* value, not the raw request.

## `browser_monitor_*` options

`browser_monitor_start` accepts `capture_body_max_bytes: integer` (default
`1048576`, i.e. 1 MiB; `0` means unbounded) alongside `capture_bodies: bool` —
it bounds how much of each HTTP response body is captured per event. A body
over the cap is truncated to a prefix; `browser_monitor_read`'s `http` events
report the truncation via `body_truncated: bool` and `body_full_bytes:
integer` (the full pre-truncation length, regardless of how much was kept).
A body-fetch failure (e.g. Chrome already evicted the response) sets
`body_capture_error: string` instead of silently omitting `body` /
`body_base64` with no explanation.

`browser_monitor_read` can also return a `delivery_boundary` event: a
lagged/reconnected/disconnected transport, a correlation-map eviction, or an
undecodable payload on the underlying event stream, surfaced explicitly
instead of silently dropped. Its `boundary` field is one of `"lagged"` |
`"reconnected"` | `"disconnected"` | `"correlation_evicted"` |
`"decode_failed"` | `"unknown"`, with `generation` / `missed` / `previous` /
`url` populated depending on which. A `"disconnected"` boundary means the
underlying monitor's correlator task has ended — no further events will ever
be buffered for that handle; call `browser_monitor_start` again for a fresh
one. See [Network monitor](./network-monitor.md#delivery-loss-boundaries) for
the full semantics.

## Stealth

Stealth is on by default (matching the `zendriver` library). Configure
the default fingerprint via `--stealth-profile` at server start; switch
live via `browser_set_stealth_profile` (takes effect on the next
`browser_open`).

## Troubleshooting

- **Logs go to stderr** in stdio mode — stdout is reserved for MCP
  JSON-RPC. Use `--log debug` for verbose CDP-call logging.
- **Errors include `_meta.suggested_next` hints** when applicable
  (e.g. `ElementNotFound` suggests reconnaissance via `browser_html` or
  a fresh `browser_find_all` snapshot).
- **HTTP smoke test** binds `127.0.0.1:18765` by convention — if your
  environment has that port taken, set a different port via `--http`.
- **Real-Chrome integration tests** are gated behind cargo feature
  `integration-tests` and `#[ignore]` markers: run via
  `cargo test -p zendriver-mcp --features integration-tests -- --ignored`.
