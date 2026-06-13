---
description: Scrape a URL with the zendriver stealth browser and return its content. Dispatches the zendriver-scraper subagent. Usage: /zendriver:scrape <url> [goal].
argument-hint: "<url> [what to extract]"
---

# Scrape a URL with zendriver

Arguments (`$ARGUMENTS`): the first token is the URL; everything after it is the extraction
goal. If no goal is given, default to "full readable main content as Markdown".

1. Parse `$ARGUMENTS` into `<url>` and `<goal>`.
2. Dispatch the **`zendriver-scraper`** subagent (Task/Agent tool, `subagent_type:
   zendriver-scraper`) with a prompt like:
   > Scrape `<url>`. Goal: `<goal>`. Follow the scraping skill; bypass any wall; extract
   > surgically; close the browser when done.
   For a large or slow job, run it in the background.
3. Return the subagent's result to the user. Don't re-scrape in the main thread.

If the subagent reports the binary/tools are unavailable, tell the user to run
`/zendriver:setup` first.
