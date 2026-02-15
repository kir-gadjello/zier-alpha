#!/usr/bin/env bash
# Build a UPX‑packed ultra‑small release binary.
#
# This script creates a special "release‑packed" build that:
#  1. Builds with profile.release-packed (inherits release + panic=abort)
#  2. Strips all symbols (strip command)
#  3. Compresses with UPX using LZMA for best compression
#
# The final binary is written to target/release-packed/zier-alpha
#
# Usage: ./scripts/build_release_upx_packed.sh [--no-upx] [--force]
#   --no-upx: skip UPX compression (just strip)
#   --force: rebuild even if binary appears up‑to‑date

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*"; exit 1; }

# Config
CARGO_BIN="${CARGO_BIN:-cargo}"
PROFILE="release-packed"
BUILD_DIR="target/${PROFILE}"
BINARY_SRC="${BUILD_DIR}/zier-alpha"
BINARY_DST="${BUILD_DIR}/zier-alpha-upx"
STRIP_CMD=""  # Determine per‑platform
UPX_CMD="${UPX_CMD:-upx}"  # allow override
UPX_LEVEL="${UPX_LEVEL:---best}"  # or --ultra-brute for even smaller but slower

# Parse flags
SKIP_UPX=false
FORCE_BUILD=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-upx) SKIP_UPX=true; shift ;;
    --force) FORCE_BUILD=true; shift ;;
    *) error "Unknown flag: $1" ;;
  esac
done

# Ensure we're in repo root
cd "$(dirname "$0")/.."

# Check if we need to rebuild
if [[ ! -f "$BINARY_SRC" || "$FORCE_BUILD" == "true" ]]; then
  info "Building with profile: ${PROFILE} (panic=abort, LTO, no debug)"
  ${CARGO_BIN} build --profile "${PROFILE}"
else
  info "Binary already exists at ${BINARY_SRC}; use --force to rebuild"
fi

# If binary doesn't exist after build, abort
[[ -f "$BINARY_SRC" ]] || error "Build failed: ${BINARY_SRC} not found"

# Copy to working path (we'll compress this copy)
cp -f "$BINARY_SRC" "$BINARY_DST"

# Show original size
ORIG_SIZE=$(stat -c%s "$BINARY_DST" 2>/dev/null || stat -f%z "$BINARY_DST")
ORIG_HUMAN=$(numfmt --to=iec-i --suffix=B "$ORIG_SIZE" 2>/dev/null || echo "${ORIG_SIZE} bytes")
info "Original size: ${ORIG_HUMAN}"

# Step 1: Strip symbols
info "Stripping symbols..."
case "$(uname -s)" in
  Linux*)
    strip --strip-all "$BINARY_DST" 2>/dev/null || warn "strip failed (maybe already stripped)"
    ;;
  Darwin*)
    strip -x "$BINARY_DST" 2>/dev/null || warn "strip failed"
    ;;
  *)
    warn "Unknown OS; skipping strip"
    ;;
esac

STRIPPED_SIZE=$(stat -c%s "$BINARY_DST" 2>/dev/null || stat -f%z "$BINARY_DST")
STRIPPED_HUMAN=$(numfmt --to=iec-i --suffix=B "$STRIPPED_SIZE" 2>/dev/null || echo "${STRIPPED_SIZE} bytes")
info "After stripping: ${STRIPPED_HUMAN}"

# Step 2: UPX compression (unless skipped)
if [[ "$SKIP_UPX" == "false" ]]; then
  if command -v "$UPX_CMD" &>/dev/null; then
    info "Compressing with UPX (this may take a few seconds)..."
    # Use --lzma for better compression; --best implies --lzma for UPX ≥ 3.96
    "$UPX_CMD" ${UPX_LEVEL} --lzma -o "$BINARY_DST".tmp "$BINARY_DST" 2>/dev/null
    mv "$BINARY_DST".tmp "$BINARY_DST"
    COMPRESSED_SIZE=$(stat -c%s "$BINARY_DST" 2>/dev/null || stat -f%z "$BINARY_DST")
    COMPRESSED_HUMAN=$(numfmt --to=iec-i --suffix=B "$COMPRESSED_SIZE" 2>/dev/null || echo "${COMPRESSED_SIZE} bytes")
    info "After UPX: ${COMPRESSED_HUMAN}"

    # Compute ratios
    ratio_strip=$(awk "BEGIN {printf \"%.1f%%\", ($STRIPPED_SIZE - $COMPRESSED_SIZE)/$STRIPPED_SIZE * 100}")
    ratio_orig=$(awk "BEGIN {printf \"%.1f%%\", ($ORIG_SIZE - $COMPRESSED_SIZE)/$ORIG_SIZE * 100}")
    info "Compression ratios:"
    echo "  vs original: ${ratio_orig} reduction"
    echo "  vs stripped: ${ratio_strip} reduction"

    # Show UPX details if possible
    if "$UPX_CMD" --version &>/dev/null; then
      UPX_VER=$("$UPX_CMD" --version | head -n1)
      info "UPX: ${UPX_VER}"
    fi
  else
    warn "UPX not found in PATH. Skipping compression."
    echo "To install UPX: brew install upx   (macOS)   or   apt install upx   (Ubuntu/Debian)"
    echo "Alternatively, set UPX_CMD to point to your upx executable."
    echo "Final binary (stripped only): $BINARY_DST"
  fi
else
  info "UPX compression skipped (--no-upx)."
  echo "Final binary (stripped only): $BINARY_DST"
fi

info "Build complete!"
echo "Binary: $BINARY_DST"
