---
description: Provision the zendriver-mcp binary for this plugin. Offers a prebuilt download (no Rust), a from-source cargo build, or linking an existing binary. Run once, then restart the session.
argument-hint: "[prebuilt|source|link]"
---

# Set up zendriver

Provision the `zendriver-mcp` binary so the bundled MCP server can start.

## Steps

1. **Resolve paths.** The provisioner script is at `${CLAUDE_PLUGIN_ROOT}/scripts/setup.sh`
   and the binary must land at `${CLAUDE_PLUGIN_DATA}/bin/zendriver-mcp`. First run
   `echo "ROOT=$CLAUDE_PLUGIN_ROOT DATA=$CLAUDE_PLUGIN_DATA"` to confirm both resolve to real
   paths. If `CLAUDE_PLUGIN_DATA` is empty, fall back to
   `$HOME/.claude/plugins/data/zendriver-zendriver-rs`.

2. **Probe the environment** so you can recommend a mode:
   - `uname -sm` (platform — is a prebuilt likely available?).
   - `command -v cargo` (can we build from source?).
   - `command -v zendriver-mcp` (is one already on PATH to link?).

3. **Choose a mode.** If the user passed one in `$ARGUMENTS` (`prebuilt`, `source`, or
   `link`), use it. Otherwise ask them with AskUserQuestion, presenting:
   - **Download prebuilt** *(recommended — fast, no Rust)*: fetches the matching binary from
     the latest GitHub release and verifies its checksum.
   - **Build from source** *(needs Rust; a few minutes)*: compiles the public source yourself
     with `cargo install` — choose this if you'd rather not run a prebuilt binary.
   - **Link existing**: symlink a `zendriver-mcp` already on your PATH.

4. **Run the provisioner** with the chosen mode:
   ```bash
   bash "$CLAUDE_PLUGIN_ROOT/scripts/setup.sh" --mode <prebuilt|source|link> \
     --dest "${CLAUDE_PLUGIN_DATA:-$HOME/.claude/plugins/data/zendriver-zendriver-rs}/bin/zendriver-mcp"
   ```
   Surface the script's output. If it fails (e.g. prebuilt unavailable for the platform),
   suggest another mode.

5. **Tell the user to restart the session** (or run `/reload-plugins` if available) so the
   `zendriver` MCP server picks up the new binary. On first `browser_open`, Chrome is fetched
   automatically — no system Chrome needed.
