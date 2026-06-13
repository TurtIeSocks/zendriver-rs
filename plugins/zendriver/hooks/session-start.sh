#!/usr/bin/env bash
# If the zendriver-mcp binary isn't provisioned, nudge the user to run /zendriver:setup.
set -euo pipefail

DEST="${CLAUDE_PLUGIN_DATA:-$HOME/.claude/plugins/data/zendriver-zendriver-rs}/bin/zendriver-mcp"

if [ ! -x "$DEST" ]; then
  cat <<'JSON'
{
  "hookSpecificOutput": {
    "hookEventName": "SessionStart",
    "additionalContext": "The zendriver MCP binary is not provisioned yet, so the zendriver browser tools will be unavailable until it is. If the user wants to use zendriver, tell them to run the /zendriver:setup command once (it offers a prebuilt download, a from-source cargo build, or linking an existing binary), then restart the session."
  }
}
JSON
fi
exit 0
