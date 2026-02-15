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
# Usage: ./scripts/build_release_upx_packed.sh [--no-upx] [--force] [--target <TRIPLE>]
#   --no-upx: skip UPX compression (just strip)
#   --force: rebuild even if binary appears up‑to‑date
#   --target <TRIPLE>: cross‑compile target (e.g., x86_64-unknown-linux-musl for static Linux)
#
# Environment overrides:
#   CARGO_BIN=cargo
#   BINARY_NAME=zier-alpha
#   UPX_CMD=upx
#   UPX_LEVEL="--best"  (or "--ultra-brute")
#
# Examples:
#   $0                          # host build, compress
#   $0 --no-upx                # host build, strip only
#   $0 --target x86_64-unknown-linux-musl   # static Linux build + UPX
#   $0 --target aarch64-unknown-linux-musl  # ARM64 static Linux

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
BINARY_NAME="${BINARY_NAME:-zier-alpha}"
UPX_CMD="${UPX_CMD:-upx}"
UPX_LEVEL="${UPX_LEVEL:---best}"

# Parse flags
SKIP_UPX=false
FORCE_BUILD=false
TARGET=""

show_help() {
  cat <<EOF
Usage: $0 [options]

Build a UPX‑packed ultra‑small release binary.

Options:
  --no-upx        Skip UPX compression (only strip)
  --force         Rebuild even if binary appears up‑to‑date
  --target <TRIPLE> Cross‑compile target (e.g., x86_64-unknown-linux-musl)
  -h, --help      Show this help message

Environment:
  CARGO_BIN       Cargo command (default: cargo)
  BINARY_NAME     Binary name (default: zier-alpha)
  UPX_CMD         UPX executable (default: upx)
  UPX_LEVEL       UPX compression level (default: --best)

Output:
  The final binary is written to target/release-packed/${BINARY_NAME}-upx
  (or target/<target>/release-packed/ when using --target, then copied).

Examples:
  # Build for host, compress with UPX
  $0

  # Build fully static Linux binary (requires musl target)
  $0 --target x86_64-unknown-linux-musl

  # Only strip (no UPX)
  $0 --no-upx
EOF
}

cd "$(dirname "$0")/.."  # ensure repo root

# Help?
if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  show_help
  exit 0
fi

# Parse arguments
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-upx) SKIP_UPX=true; shift ;;
    --force) FORCE_BUILD=true; shift ;;
    --target) TARGET="$2"; shift 2 ;;
    -h|--help) show_help; exit 0 ;;
    *) error "Unknown flag: $1" ;;
  esac
done

# Warn about static builds
if [[ -z "$TARGET" ]] && [[ "$(uname -s)" == "Linux" ]]; then
  warn "No --target specified on Linux: building for native glibc (dynamically linked)."
  warn "For a fully static, self-contained binary, install and use:"
  warn "  rustup target add x86_64-unknown-linux-musl"
  warn "  $0 --target x86_64-unknown-linux-musl"
  echo
fi

# Determine build directory (cargo uses target/<target-triple>/<profile> when cross‑compiling)
if [[ -z "$TARGET" ]]; then
  BUILD_DIR="target/${PROFILE}"
else
  BUILD_DIR="target/${TARGET}/${PROFILE}"
fi

BINARY_SRC="${BUILD_DIR}/${BINARY_NAME}"
BINARY_DST="${BUILD_DIR}/${BINARY_NAME}-upx"

# Build if needed
if [[ ! -f "$BINARY_SRC" || "$FORCE_BUILD" == "true" ]]; then
  info "Building profile: ${PROFILE}"
  if [[ -n "$TARGET" ]]; then
    info "Target: ${TARGET}"
    ${CARGO_BIN} build --profile "${PROFILE}" --target "${TARGET}"
  else
    ${CARGO_BIN} build --profile "${PROFILE}"
  fi
else
  info "Binary already exists at ${BINARY_SRC}; use --force to rebuild"
fi

[[ -f "$BINARY_SRC" ]] || error "Build failed: ${BINARY_SRC} not found"

# Work on a copy
cp -f "$BINARY_SRC" "$BINARY_DST"

# Show original size
ORIG_SIZE=$(stat -c%s "$BINARY_DST" 2>/dev/null || stat -f%z "$BINARY_DST")
ORIG_HUMAN=$(numfmt --to=iec-i --suffix=B "$ORIG_SIZE" 2>/dev/null || echo "${ORIG_SIZE} bytes")
info "Original size: ${ORIG_HUMAN}"

# Strip symbols
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

# Optional UPX compression
if [[ "$SKIP_UPX" == "false" ]]; then
  if command -v "$UPX_CMD" &>/dev/null; then
    info "Compressing with UPX (level=${UPX_LEVEL})..."

    # Show file type for diagnosis
    if command -v file &>/dev/null; then
      info "File type: $(file "$BINARY_DST" | head -n1)"
    fi

    # Try UPX compression; capture output
    UPX_OUTPUT=$("$UPX_CMD" ${UPX_LEVEL} --lzma -o "$BINARY_DST".tmp "$BINARY_DST" 2>&1)
    UPX_EXIT=$?

    if [[ $UPX_EXIT -ne 0 ]] || echo "$UPX_OUTPUT" | grep -q "Packed 0 files"; then
      warn "UPX failed to compress or reported no files packed."
      echo "$UPX_OUTPUT" | sed 's/^/  /'
      echo "Possible reasons:"
      echo "  - Binary already packed (run with --force to rebuild clean)"
      echo "  - UPX does not support this binary format (e.g., macOS universal, exotic arch)"
      echo "  - Binary is already using LZMA and cannot be compressed further"
      echo "Final binary (stripped only): $BINARY_DST"
    else
      # Success – replace original
      mv "$BINARY_DST".tmp "$BINARY_DST"
      COMPRESSED_SIZE=$(stat -c%s "$BINARY_DST" 2>/dev/null || stat -f%z "$BINARY_DST")
      COMPRESSED_HUMAN=$(numfmt --to=iec-i --suffix=B "$COMPRESSED_SIZE" 2>/dev/null || echo "${COMPRESSED_SIZE} bytes")
      info "After UPX: ${COMPRESSED_HUMAN}"

      # Ratios
      ratio_strip=$(awk "BEGIN {printf \"%.1f%%\", ($STRIPPED_SIZE - $COMPRESSED_SIZE)/$STRIPPED_SIZE * 100}")
      ratio_orig=$(awk "BEGIN {printf \"%.1f%%\", ($ORIG_SIZE - $COMPRESSED_SIZE)/$ORIG_SIZE * 100}")
      info "Compression ratios:"
      echo "  vs original: ${ratio_orig} reduction"
      echo "  vs stripped: ${ratio_strip} reduction"

      # Show UPX version if available
      if "$UPX_CMD" --version &>/dev/null; then
        UPX_VER=$("$UPX_CMD" --version | head -n1)
        info "UPX: ${UPX_VER}"
      fi
    fi
  else
    warn "UPX not found in PATH. Skipping compression."
    echo "To install UPX: brew install upx   (macOS)   or   apt install upx   (Ubuntu/Debian)"
    echo "Final binary (stripped only): $BINARY_DST"
  fi
else
  info "UPX compression skipped (--no-upx)."
  echo "Final binary (stripped only): $BINARY_DST"
fi

info "Build complete!"
echo "Binary: $BINARY_DST"

# Print detailed statistics if tools available
echo
info "Binary statistics:"
if command -v size &>/dev/null; then
  echo "  Section sizes (text/data/bss):"
  size -B "$BINARY_DST" 2>/dev/null || size "$BINARY_DST" | awk '{print "    " $0}'
fi
if command -v nm &>/dev/null; then
  # Count symbols by type
  SYM_COUNT=$(nm -g "$BINARY_DST" 2>/dev/null | wc -l)
  echo "  Global symbols: $SYM_COUNT"
fi
if command -v file &>/dev/null; then
  echo "  Type: $(file "$BINARY_DST" | head -n1)"
fi
if [[ -n "$TARGET" ]]; then
  echo
  info "Verification (optional):"
  echo "  file \"$BINARY_DST\"   # should say 'statically linked'"
  echo "  ldd \"$BINARY_DST\"    # should say 'not a dynamic executable'"
fi
