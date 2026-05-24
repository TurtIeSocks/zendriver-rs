#!/usr/bin/env bash
# scripts/publish.sh
# Topological publish order. Run from repo root.
# Usage: ./scripts/publish.sh [--dry-run]
set -euo pipefail

DRY_RUN=""
if [[ "${1:-}" == "--dry-run" ]]; then DRY_RUN="--dry-run"; fi

CRATES=(
    zendriver-transport
    zendriver-stealth
    zendriver-interception
    zendriver-fetcher
    zendriver-cloudflare
    zendriver
)

for crate in "${CRATES[@]}"; do
    echo "==> Publishing $crate $DRY_RUN"
    (cd "crates/$crate" && cargo publish $DRY_RUN --locked)
    if [[ -z "$DRY_RUN" ]]; then
        echo "Waiting 30s for crates.io index propagation..."
        sleep 30
    fi
done
