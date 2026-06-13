#!/usr/bin/env bash
# Provision the zendriver-mcp binary for the Claude Code plugin.
# Usage: setup.sh --mode <prebuilt|source|link> [--dest <path>]
set -euo pipefail

REPO="TurtIeSocks/zendriver-rs"
MODE=""
DEST="${CLAUDE_PLUGIN_DATA:-$HOME/.claude/plugins/data/zendriver-zendriver-rs}/bin/zendriver-mcp"

while [ $# -gt 0 ]; do
  case "$1" in
    --mode) MODE="$2"; shift 2 ;;
    --dest) DEST="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

[ -n "$MODE" ] || { echo "missing --mode <prebuilt|source|link>" >&2; exit 2; }
mkdir -p "$(dirname "$DEST")"

# Map uname to a Rust target triple + asset extension.
detect_target() {
  local os arch
  os="$(uname -s)"; arch="$(uname -m)"
  case "$os" in
    Darwin) case "$arch" in
        arm64) echo "aarch64-apple-darwin tar.gz" ;;
        x86_64) echo "x86_64-apple-darwin tar.gz" ;;
        *) return 1 ;; esac ;;
    Linux) case "$arch" in
        x86_64) echo "x86_64-unknown-linux-gnu tar.gz" ;;
        aarch64|arm64) echo "aarch64-unknown-linux-gnu tar.gz" ;;
        *) return 1 ;; esac ;;
    MINGW*|MSYS*|CYGWIN*) echo "x86_64-pc-windows-msvc zip" ;;
    *) return 1 ;;
  esac
}

latest_tag() {
  # Newest release tag matching zendriver-mcp-v*
  if command -v gh >/dev/null 2>&1; then
    gh release list --repo "$REPO" --limit 30 \
      | awk '{print $1}' | grep '^zendriver-mcp-v' | head -n1
  else
    curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=30" \
      | grep -o '"tag_name": *"zendriver-mcp-v[^"]*"' \
      | head -n1 | sed 's/.*"\(zendriver-mcp-v[^"]*\)"/\1/'
  fi
}

install_prebuilt() {
  local triple ext tag tmp asset url
  read -r triple ext < <(detect_target) || { echo "unsupported platform: $(uname -sm)" >&2; exit 1; }
  tag="$(latest_tag)"; [ -n "$tag" ] || { echo "no zendriver-mcp-v* release found" >&2; exit 1; }
  asset="zendriver-mcp-${triple}.${ext}"
  tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' RETURN
  url="https://github.com/$REPO/releases/download/$tag"
  echo "Downloading $asset from $tag ..." >&2
  curl -fsSL "$url/$asset" -o "$tmp/$asset"
  curl -fsSL "$url/SHA256SUMS" -o "$tmp/SHA256SUMS"
  ( cd "$tmp" && grep -F "$asset" SHA256SUMS | shasum -a 256 -c - ) \
    || { echo "checksum verification FAILED" >&2; exit 1; }
  local binname="zendriver-mcp"; [ "$ext" = "zip" ] && binname="zendriver-mcp.exe"
  if [ "$ext" = "zip" ]; then unzip -o "$tmp/$asset" -d "$tmp" >/dev/null
  else tar -xzf "$tmp/$asset" -C "$tmp"; fi
  [ -f "$tmp/$binname" ] || { echo "archive did not contain $binname" >&2; exit 1; }
  install -m 0755 "$tmp/$binname" "$DEST" 2>/dev/null \
    || { mv "$tmp/$binname" "$DEST"; chmod +x "$DEST"; }
  echo "Installed prebuilt binary -> $DEST" >&2
}

install_source() {
  command -v cargo >/dev/null 2>&1 || { echo "cargo not found; install Rust or use --mode prebuilt" >&2; exit 1; }
  local root; root="$(dirname "$(dirname "$DEST")")"   # .../zendriver-zendriver-rs
  echo "Building from source via cargo (this can take several minutes) ..." >&2
  cargo install zendriver-mcp --root "$root"
  echo "Installed source build -> $DEST" >&2
}

install_link() {
  local found; found="$(command -v zendriver-mcp || true)"
  [ -n "$found" ] || { echo "no zendriver-mcp on PATH to link" >&2; exit 1; }
  ln -sf "$found" "$DEST"
  echo "Linked $found -> $DEST" >&2
}

case "$MODE" in
  prebuilt) install_prebuilt ;;
  source)   install_source ;;
  link)     install_link ;;
  *) echo "invalid --mode: $MODE" >&2; exit 2 ;;
esac

"$DEST" --version >/dev/null 2>&1 && echo "OK: zendriver-mcp runs." >&2 || echo "WARN: installed but --version check failed." >&2
