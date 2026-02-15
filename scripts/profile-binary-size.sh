#!/usr/bin/env bash
# Binary size profiling script for Zier Alpha
# Usage: ./scripts/profile-binary-size.sh [--crates|--sections|--symbols|--bloaty]
# Default: shows total size, crate breakdown (if cargo-bloat available), and fallback to symbols

set -euo pipefail

# Configuration
BINARY="target/release/zier-alpha"
CARGO_BLOAT_BIN="${CARGO_BLOAT_BIN:-cargo bloat}"  # command to run
TOP_N="${TOP_N:-30}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() { echo -e "${GREEN}[INFO]${NC} $*"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }

# Ensure binary exists
if [[ ! -f "$BINARY" ]]; then
    log_info "Release binary not found. Building..."
    cargo build --release
fi

# Total size
log_info "Binary total size:"
ls -lh "$BINARY" | awk '{print "  " $5 " (" $6 ")"}'
echo ""

# Try to run cargo-bloat (crate-level breakdown)
log_info "Crate size breakdown (top $TOP_N):"

# Function to execute cargo-bloat and capture output
run_cargo_bloat() {
    local CMD
    # Build command from env or defaults
    # Default to "cargo bloat" if not set
    local cmd_str="${CARGO_BLOAT_BIN:-cargo bloat}"
    read -ra CMD <<< "$cmd_str"
    # Run and capture stdout; ignore stderr, but preserve exit status for debugging
    # We capture output to check if non-empty later
    "${CMD[@]}" --release --bin zier-alpha --crates 2>/dev/null | head -"$TOP_N"
}

output=$(run_cargo_bloat) || output=""

if [[ -n "$output" ]]; then
    echo "$output"
    echo ""
else
    log_warn "cargo-bloat not available or produced no output. To install: cargo install cargo-bloat"
    log_info "Falling back to section breakdown (size) ..."
    if command -v size &>/dev/null; then
        echo "  Section sizes (bytes):"
        size -B "$BINARY" 2>/dev/null || size "$BINARY"
        echo ""
    fi
    log_info "For per-symbol details, install bloaty or cargo-bloat and re‑run."
    echo ""
fi

# Check for bloaty (section / symbol granularity)
if [[ "${1:-}" == "--sections" ]] || command -v bloaty &>/dev/null; then
    log_info "Section breakdown (top $TOP_N):"
    if command -v bloaty &>/dev/null; then
        bloaty -d sections -n "$TOP_N" "$BINARY" 2>/dev/null || log_error "bloaty failed"
    else
        log_warn "bloaty not installed. To install: see https://github.com/google/bloaty"
    fi
    echo ""
fi

# Symbol-level analysis (optional, very verbose)
if [[ "${1:-}" == "--symbols" ]]; then
    log_info "Largest symbols (top $TOP_N):"
    if command -v nm &>/dev/null; then
        # nm --size-sort prints symbols with size; filter for text/data/bss
        nm --size-sort --radix=d "$BINARY" 2>/dev/null | tail -"$TOP_N" | awk '{printf "  %8s %s\n", $1, $3}'
    else
        log_warn "nm not found (install binutils)"
    fi
    echo ""
fi

# Summary with demangling suggestions
log_info "Tips for reducing binary size:"
cat <<'EOF'
  - Inspect large crates: look for regex, tokio-full, sqlite with loadable extensions.
  - Consider feature flags to disable unused components (e.g., desktop, gguf, fastembed).
  - Use `cargo bloat --release --bin zier-alpha` for full per-crate report.
  - Switch to `tokio` with only necessary features (rt, macros, sync, time, net?).
  - Replace heavy regex with simpler string operations if possible.
  - Evaluate embedded assets (embed, rust-embed) and compress them.
EOF
