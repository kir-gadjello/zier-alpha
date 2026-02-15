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
# Usage: ./scripts/build_release_upx_packed.sh [options] [upx-args...]
#   --no-upx        Skip UPX compression (just strip)
#   --force         Rebuild even if binary appears up‑to‑date
#   --target <TRIPLE> Cross‑compile target (e.g., x86_64-unknown-linux-musl)
#   -h, --help      Show this help message
#
# Any additional arguments (e.g., --no-lzma, --ultra-brute) are passed directly to UPX.
#
# Environment overrides:
#   CARGO_BIN=cargo
#   BINARY_NAME=zier-alpha
#   UPX_CMD=upx
#
# Examples:
#   $0                          # host build, compress with defaults
#   $0 --no-upx                # host build, strip only
#   $0 --target x86_64-unknown-linux-musl   # static Linux build + UPX
#   $0 --no-lzma               # Use NRV compression (macOS ARM64 workaround)
#   $0 --ultra-brute --lzma    # Extra time for max compression

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
# Collect extra UPX arguments (passed through)
PASSTHROUGH_ARGS=()

# Parse script flags first
SKIP_UPX=false
FORCE_BUILD=false
TARGET=""
PASSTHROUGH_ARGS=()

cd "$(dirname "$0")/.."  # ensure repo root

# Help?
if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  cat <<EOF
Usage: $0 [script-options] [upx-args...]

Build a UPX‑packed ultra‑small release binary.

Script options:
  --no-upx        Skip UPX compression (only strip)
  --force         Rebuild even if binary appears up‑to‑date
  --target <TRIPLE> Cross‑compile target (e.g., x86_64-unknown-linux-musl)
  -h, --help      Show this help message

UPX arguments (passed through):
  --no-lzma       Disable LZMA (use NRV); useful on macOS ARM64
  --ultra-brute   More exhaustive compression (slower)
  --best          Maximum compression (default)
  See 'upx --help' for all options.

Examples:
  $0                              # default build + UPX (LZMA if supported)
  $0 --no-upx                     # only strip
  $0 --target x86_64-unknown-linux-musl
  $0 --no-lzma                    # force NRV (macOS ARM64)
  $0 --ultra-brute --lzma         # max effort LZMA

Environment:
  CARGO_BIN   cargo command (default: cargo)
  BINARY_NAME  binary name (default: zier-alpha)
  UPX_CMD     upx executable (default: upx)
EOF
  exit 0
fi

# Parse arguments: script flags vs passthrough to UPX
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-upx) SKIP_UPX=true; shift ;;
    --force) FORCE_BUILD=true; shift ;;
    --target) TARGET="$2"; shift 2 ;;
    # End of script flags; everything else goes to UPX
    *) PASSTHROUGH_ARGS+=("$1"); shift ;;
  esac
done

# Warn about static builds on Linux
if [[ -z "$TARGET" ]] && [[ "$(uname -s)" == "Linux" ]]; then
  warn "No --target specified on Linux: building for native glibc (dynamically linked)."
  warn "For a fully static, self-contained binary, install and use:"
  warn "  rustup target add x86_64-unknown-linux-musl"
  warn "  $0 --target x86_64-unknown-linux-musl"
  echo
fi

# Determine build directory
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
    info "Compressing with UPX..."

    # Show file type for diagnosis
    if command -v file &>/dev/null; then
      info "File type: $(file "$BINARY_DST" | head -n1)"
    fi

    # Build UPX command line
    # Base options: ${PASSTHROUGH_ARGS[@]}
    # If no passthrough args provided, use a sensible default strategy based on platform
    UPX_ARGS=("${PASSTHROUGH_ARGS[@]:---best --lzma}")

    # Auto-detect macOS ARM64: LZMA often fails; advise user to use --no-lzma if they haven't
    if [[ "$(uname -s)" == "Darwin" ]] && [[ "$(uname -m)" == "arm64" ]]; then
      if [[ "${PASSTHROUGH_ARGS[@]}" != *"--no-lzma"* ]]; then
        warn "Detected macOS on ARM64. LZMA compression may fail on this platform."
        warn "If UPX reports 'Packed 0 files', retry with: $0 --no-lzma"
        # We still try with LZMA first; if it fails we'll fallback to NRV below.
      fi
    fi

    # We'll try a series of fallback strategies if the primary fails
    # Order: primary (from args or default), then alternatives
    declare -a strategies
    primary="${UPX_ARGS[*]}"
    strategies=("$primary")

    # If primary contains --lzma, add NRV fallback
    if [[ "$primary" == *"--lzma"* ]]; then
      strategies+=("--best --force")  # sometimes helps
      strategies+=("--best")           # may drop to NRV automatically
      strategies+=("--nrv -9")         # pure NRV max
    else
      # If no --lzma, we can still try force and plain best
      strategies+=("--best --force")
    fi

    success=false
    for i in "${!strategies[@]}"; do
      strat="${strategies[$i]}"
      info "Attempt $((i+1)): upx $strat"
      UPX_OUTPUT=$("$UPX_CMD" $strat -o "$BINARY_DST".tmp "$BINARY_DST" 2>&1)
      UPX_EXIT=$?
      echo "$UPX_OUTPUT" | sed 's/^/    /'
      if [[ $UPX_EXIT -eq 0 ]] && ! echo "$UPX_OUTPUT" | grep -q "Packed 0 files"; then
        success=true
        break
      else
        warn "Strategy $((i+1)) failed or packed 0 files. Trying next..."
      fi
    done

    if [[ "$success" == "true" ]]; then
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
    else
      warn "All UPX compression attempts failed or produced no compression."
      echo "Final binary (stripped only): $BINARY_DST"
      echo "Possible reasons:"
      echo "  - Binary already packed or maximally compressed by LTO"
      echo "  - UPX does not support this binary format/architecture"
      echo "  - Binary uses features UPX cannot handle (e.g., full PIE, certain segments)"
      echo "Consider using --no-lzma or a different UPX version."
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

# Stats
echo
info "Binary statistics:"
if command -v size &>/dev/null; then
  echo "  Section sizes (text/data/bss):"
  size -B "$BINARY_DST" 2>/dev/null || size "$BINARY_DST" | awk '{print "    " $0}'
fi
if command -v nm &>/dev/null; then
  SYM_COUNT=$(nm -g "$BINARY_DST" 2>/dev/null | wc -l)
  echo "  Global symbols: $SYM_COUNT"
fi
if command -v file &>/dev/null; then
  echo "  Type: $(file "$BINARY_DST" | head -n1)"
fi
