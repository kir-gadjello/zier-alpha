#!/usr/bin/env bash
# Strip debug symbols and sections from the release binary
# Usage: ./scripts/strip-binary.sh [--binary path]

set -euo pipefail

BINARY="${1:-target/release/zier-alpha}"

if [[ ! -f "$BINARY" ]]; then
    echo "Error: binary not found at $BINARY"
    echo "Build first: cargo build --release"
    exit 1
fi

# Show original size
echo "Original size:"
du -h "$BINARY"

# Determine platform
OS="$(uname -s)"
case "$OS" in
    Linux*)
        STRIP_CMD="strip --strip-all"
        ;;
    Darwin*)
        STRIP_CMD="strip -x"  # remove local symbols but keep external ones for dynamic linking
        ;;
    *)
        echo "Unsupported OS: $OS"
        exit 1
        ;;
esac

echo "Stripping with: $STRIP_CMD"
$STRIP_CMD "$BINARY"

# Show new size
echo "Stripped size:"
du -h "$BINARY"
