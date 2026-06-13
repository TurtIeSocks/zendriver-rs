---
description: Extract structured JSON from a URL with the zendriver stealth browser. Dispatches the zendriver-scraper subagent and returns only JSON matching your schema. Usage: /zendriver:extract <url> <schema-or-fields>.
argument-hint: "<url> <json-schema or field list>"
---

# Extract structured data with zendriver

Arguments (`$ARGUMENTS`): the first token is the URL; the remainder is a JSON schema or a
comma/space-separated field list describing the shape you want.

1. Parse `$ARGUMENTS` into `<url>` and `<schema>`.
2. Dispatch the **`zendriver-scraper`** subagent (`subagent_type: zendriver-scraper`) with:
   > Extract from `<url>` into this exact shape: `<schema>`. Follow the scraping skill; bypass
   > any wall. Return ONLY a single valid JSON value matching the shape — no prose, no code
   > fence. Use null for fields you can't find; never invent values.
3. Validate the returned text parses as JSON (`jq empty`). If not, ask the subagent to
   re-emit valid JSON.
4. Return the JSON to the user.

If tools are unavailable, tell the user to run `/zendriver:setup` first.
