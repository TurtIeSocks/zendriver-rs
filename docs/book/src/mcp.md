# MCP server (`zendriver-mcp`)

`zendriver-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io/)
server that exposes zendriver-rs through 65 MCP tools, so any
MCP-compatible client (Claude Desktop, Claude Code, custom agents) can
drive a real, stealth-by-default Chrome browser.

## Install

```bash
cargo install zendriver-mcp
```

The default build enables all gated features (`interception`, `expect`,
`cloudflare`, `imperva`, `fetcher`). For a lean build:

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

65 tools across these categories:

| Category               | Tools                                                                                                                       | Count |
| ---------------------- | --------------------------------------------------------------------------------------------------------------------------- | ----: |
| Lifecycle              | `browser_open / _close / _status`                                                                                           |     3 |
| Navigation             | `browser_goto / _back / _forward / _reload / _wait_for_idle / _wait_for_load / _bypass_insecure_warning`                    |     7 |
| Scroll / Window        | `browser_scroll / _get_window / _set_window`                                                                                |     3 |
| Tabs                   | `browser_tab_list / _new / _switch / _close / _activate`                                                                    |     5 |
| Find                   | `browser_find / _find_all`                                                                                                  |     2 |
| Actions                | `browser_click / _hover / _type / _press / _key_sequence / _mouse / _set_value / _clear / _focus / _scroll_into_view / _upload` |    11 |
| Reads                  | `browser_element_state / _get_links / _search_resources`                                                                    |     3 |
| Snapshots / Export     | `browser_html / _screenshot / _pdf / _save_mhtml`                                                                           |     4 |
| Eval                   | `browser_evaluate / _evaluate_main`                                                                                         |     2 |
| Cookies                | `browser_cookies_get / _set / _delete / _clear / _persist`                                                                  |     5 |
| Storage                | `browser_storage_get / _set / _delete / _clear`                                                                             |     4 |
| Downloads              | `browser_download / _set_download_path`                                                                                     |     2 |
| Frames                 | `browser_frame_list / _frame_goto`                                                                                          |     2 |
| Stealth                | `browser_set_stealth_profile / _set_user_agent`                                                                             |     2 |
| Interception (gated)   | `browser_intercept_add_rule / _remove_rule / _list_rules / _clear_rules`                                                    |     4 |
| Expect (gated)         | `browser_expect_register / _await / _cancel`                                                                                |     3 |
| Cloudflare (gated)     | `browser_solve_turnstile`                                                                                                   |     1 |
| Imperva (gated)        | `browser_solve_imperva`                                                                                                     |     1 |
| Fetcher (gated)        | `browser_install_chrome`                                                                                                    |     1 |

All find / action tools share a `Selector` arg — one-of `css | xpath |
text | text_exact | text_regex | role`, with modifiers `nth /
visible_only / timeout_ms / frame_id`. State-changing tools accept
`return_snapshot: bool` for one-call action + observe. `browser_pdf` /
`_save_mhtml` / `_download` return a binary-output shape (`{ byte_len,
saved_path? , base64? }`): bytes go to `save_path` on the MCP host when
given, else base64-inline (capped at 5 MiB). The default build adds
`imperva` to the gated feature set alongside `interception` / `expect` /
`cloudflare` / `fetcher`.

Full JSON Schema for every tool's input + output is captured in
`crates/zendriver-mcp/tests/snapshots/` and changes there require an
explicit `cargo insta accept` — the wire shape is reviewed.

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
