#!/bin/bash
# Sync version across all project manifests.
# Source of truth: tauri.conf.json
# Usage: ./scripts/sync-version.sh [new_version]
#   If new_version is provided, updates all files to that version.
#   If omitted, reads from tauri.conf.json and syncs the others.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$SCRIPT_DIR/.."
TAURI_CONF="$ROOT/app/src-tauri/tauri.conf.json"
CARGO_TOML="$ROOT/app/src-tauri/Cargo.toml"
PACKAGE_JSON="$ROOT/app/package.json"

if [ -n "${1:-}" ]; then
    VERSION="$1"
    # Update source of truth first
    tmp=$(mktemp)
    jq --arg v "$VERSION" '.version = $v' "$TAURI_CONF" > "$tmp" && mv "$tmp" "$TAURI_CONF"
else
    VERSION=$(jq -r '.version' "$TAURI_CONF")
fi

# Sync Cargo.toml (first version = line in [package])
sed -i "0,/^version = \".*\"/s//version = \"$VERSION\"/" "$CARGO_TOML"

# Sync package.json
tmp=$(mktemp)
jq --arg v "$VERSION" '.version = $v' "$PACKAGE_JSON" > "$tmp" && mv "$tmp" "$PACKAGE_JSON"

echo "Synced version to $VERSION across:"
echo "  $TAURI_CONF"
echo "  $CARGO_TOML"
echo "  $PACKAGE_JSON"
