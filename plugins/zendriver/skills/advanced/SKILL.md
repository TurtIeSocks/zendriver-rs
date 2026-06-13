---
name: advanced
description: Use for zendriver power features beyond a basic scrape — network interception and tracker-blocking, live network monitoring, event-waiting (expect), multi-tab and iframe work, cookie/storage persistence across sessions, file upload/download, and capturing pages as PDF, MHTML, or screenshots.
---

# zendriver advanced toolkit

Reference for capabilities the always-on `scraping` skill leaves out. Pull the section you
need; skip the rest.

## Capture / archive

- `browser_screenshot` — viewport or full-page PNG.
- `browser_pdf` — print-to-PDF of the current page.
- `browser_save_mhtml` — single-file MHTML archive (DOM + resources).
- `browser_get_links` — every link on the page.

## Network interception & tracker-blocking

- `browser_intercept_add_rule` — block, redirect, or modify matching requests.
- `browser_intercept_list_rules` / `browser_intercept_remove_rule` / `browser_intercept_clear_rules`.
- Use to block trackers/ads (faster, cleaner scrapes), stub endpoints, or fault-inject.
- Chain multiple interception needs into ONE rule set on the same tab — don't stand up
  competing interceptors.

## Live network monitoring

- `browser_monitor_start` → `browser_monitor_read` → `browser_monitor_stop` — observe
  requests/responses as they happen (find the JSON API behind a page, watch XHR/fetch).

## Event-waiting (expect)

- `browser_expect_register` → `browser_expect_await` (`browser_expect_cancel` to drop) —
  wait for a specific navigation/request/response/download instead of a blind sleep. Ideal
  for "click submit, then wait for the POST to complete".

## Tabs & frames

- `browser_tab_new` / `browser_tab_list` / `browser_tab_switch` / `browser_tab_activate` /
  `browser_tab_close` — multi-tab flows.
- `browser_frame_list` / `browser_frame_goto` — drive content inside iframes.

## Sessions: cookies & storage

- `browser_cookies_get/set/delete/clear` and `browser_storage_get/set/delete/clear`.
- `browser_cookies_persist` — keep a logged-in session across runs (set a download/profile
  path as needed). Treat saved cookies as secrets.

## Files

- `browser_upload` — attach a file to an `<input type=file>`.
- `browser_download` / `browser_set_download_path` — capture downloads to a known dir.

## Misc

- `browser_set_user_agent`, `browser_set_stealth_profile` — only when a site needs a specific
  identity; the defaults are already stealthy.
- `browser_request` — issue a raw HTTP request through the browser context (reuses cookies).
- `browser_install_chrome` — fetch Chrome explicitly (normally automatic on first open).
