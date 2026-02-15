# Building Zier Alpha for Release & Distribution

This document covers advanced build configurations, binary size optimization, and packaging for distribution.

## Table of Contents

- [Profiles](#profiles)
- [Binary Size Optimization](#binary-size-optimization)
- [Static Builds](#static-builds)
- [UPX Compression](#upx-compression)
- [One‑Step Packed Build](#one‑step-packed-build)
- [Verification](#verification)
- [Troubleshooting](#troubleshooting)

---

## Profiles

Zier Alpha defines several Cargo profiles in `.cargo/config.toml`:

| Profile | Purpose | Key Settings |
|---------|---------|--------------|
| `dev` (default) | Fast iteration | `debug = true`, `opt-level = 0` |
| `test` | Fast testing with optimizations | `opt-level = 3`, `debug = false`, `lto = false` |
| `release` | Optimized release build | `opt-level = 3`, `lto = true`, `codegen-units = 1`, `debug = false`, `panic = "unwind"` (backtraces enabled) |
| `release-packed` | Ultra‑small distribution build | inherits `release` + `panic = "abort"` (no unwinding) |

### When to use which?

- **Development**: `cargo build` (fast, debuggable)
- **Testing**: `cargo test` (already uses optimized `test` profile)
- **Local release**: `cargo build --release` (balanced size + debuggability)
- **Distribution**: `./scripts/build_release_upx_packed.sh --profile release-packed` (smallest, but panic=abort)

---

## Binary Size Optimization

Rust binaries can be large due to:

- Monolithic standard library (but we use `panic = "unwind"` for backtraces; `panic = "abort"` saves ~200–500 KB)
- Lack of dead code elimination across crates (solved by LTO)
- Debug symbols (stripped in `release` and `test`)

Our `release` profile already applies:

- **Full LTO** (`lto = true`) – removes dead code across crate boundaries.
- **Single codegen unit** (`codegen-units = 1`) – maximizes optimization opportunities.
- **No debug info** (`debug = false`) – strips DWARF symbols from the binary.

For even smaller binaries, see **UPX Compression** below.

---

## Static Builds

A **statically linked** binary contains all required libraries and runs on any system with a compatible kernel. This is ideal for distributing to machines without Rust or specific library versions.

### Linux (MUSL)

We support static builds via the `x86_64-unknown-linux-musl` target. The `.cargo/config.toml` includes:

```toml
[target.x86_64-unknown-linux-musl]
rustflags = ["-C", "target-feature=+crt-static"]
```

To build:

```bash
# Install MUSL target (once)
rustup target add x86_64-unknown-linux-musl

# Build static binary
cargo build --release --target x86_64-unknown-linux-musl
# Output: target/x86_64-unknown-linux-musl/release/zier-alpha
```

Verify with:

```bash
file target/x86_64-unknown-linux-musl/release/zier-alpha
# Should contain "statically linked"

ldd target/x86_64-unknown-linux-musl/release/zier-alpha
# Should say "not a dynamic executable"
```

**Note:** Some crates may not support MUSL out of the box (e.g., those relying on `glibc`). Zier Alpha's dependencies are compatible.

### macOS

macOS does not support fully static binaries in the traditional sense due to system libraries. Use dynamic linking with the OS.

---

## UPX Compression

UPX (Ultimate Packer for eXecutables) can further compress release binaries by 40–60%. It works by compressing the executable and adding a small decompression stub that runs on startup.

### Trade‑offs

| Aspect | Impact |
|--------|--------|
| **Binary size** | ~20–25 MB from ~50 MB (with LZMA) |
| **Startup latency** | Decompression takes ~50–200 ms (first run; OS caches pages) |
| **Debugging** | Core dumps, `gdb`, `perf` see decompressed code; stack traces show UPX symbols. Use uncompressed binary for debugging. |
| **Antivirus/EDR** | Packed binaries often trigger false positives. Consider code signing for distribution. |
| **Panic backtraces** | If `release-packed` uses `panic = "abort"`, no backtrace anyway. If you keep unwind, UPX may interfere with backtrace symbol resolution. |

### Installing UPX

- **macOS (Homebrew)**: `brew install upx`
- **Ubuntu/Debian**: `sudo apt install upx`
- **Windows (Chocolatey)**: `choco install upx`

### Using UPX Manually

```bash
# Build unstripped release
cargo build --release

# Strip symbols (speeds up compression slightly)
strip --strip-all target/release/zier-alpha 2>/dev/null || true

# Compress with LZMA (best)
upx --best --lzma -o target/release/zier-alpha-upx target/release/zier-alpha

# Check sizes
ls -lh target/release/zier-alpha*
```

---

## One‑Step Packed Build

We provide `scripts/build_release_upx_packed.sh` to automate:

1. Build with `--profile release-packed` (full LTO + `panic = "abort"`).
2. Strip all symbols (platform‑specific `strip`).
3. Compress with UPX using LZMA.
4. Print size metrics and ratios.
5. Support cross‑compilation via `--target`.

### Usage

```bash
# Build for host (strips + UPX)
./scripts/build_release_upx_packed.sh

# Build for static Linux (requires MUSL target)
rustup target add x86_64-unknown-linux-musl
./scripts/build_release_upx_packed.sh --target x86_64-unknown-linux-musl

# Skip UPX (only strip)
./scripts/build_release_upx_packed.sh --no-upx

# Force rebuild
./scripts/build_release_upx_packed.sh --force
```

**Output:**

- `target/release-packed/zier-alpha-upx` (host build)
- `target/<target>/release-packed/zier-alpha-upx` (cross‑compile)

The script prints:

```
[INFO] Original size: 52.3MB
[INFO] After stripping: 51.8MB
[INFO] After UPX: 21.1MB
[INFO] Compression ratios:
  vs original: 59.7% reduction
  vs stripped: 59.3% reduction
```

---

## Verification

After building (especially static builds), verify:

```bash
# 1. Check architecture and static linking
file target/release-packed/zier-alpha-upx
# Expected: "statically linked" for MUSL builds; for native macOS/Linux, "dynamically linked" is fine.

# 2. Check dynamic dependencies
ldd target/release-packed/zier-alpha-upx 2>/dev/null || true
# Expected: "not a dynamic executable" for static MUSL; otherwise list of .dylib/.so files.

# 3. Run the binary
./target/release-packed/zier-alpha-upx --version

# 4. Check UPX compression (if applied)
upx -t target/release-packed/zier-alpha-upx
```

---

## Troubleshooting

### UPX fails with "unknown format"

- Ensure you have a recent UPX version (≥ 3.96). Older versions may not support your binary format (e.g., macOS ARM64).
- Try `upx --lzma` instead of `--best` explicitly.

### Linker errors when cross‑compiling to MUSL

- Make sure the MUSL target is installed: `rustup target add x86_64-unknown-linux-musl`
- If you get "linking with `cc` failed", install MUSL toolchain: `apt install musl-tools` (Ubuntu) or `brew install mattgodbolt/musl-cross/musl-cross` (macOS).

###panic = "abort" no backtraces

If you need backtraces in the distribution build, edit `.cargo/config.toml` and change `panic = "abort"` to `panic = "unwind"` for `[profile.release-packed]`. This will increase binary size by ~200–500 KB.

### Stripping removes needed symbols

Some crates (e.g., `backtrace`) rely on symbol tables. If you encounter runtime errors after stripping, try without stripping (`--no-upx` skips strip as well? Actually script always strips; modify script to skip or use `strip --strip-unneeded` only). For most Zier Alpha builds, full strip is safe.

---

## Advanced: Customizing the Build Script

You can override environment variables used by `build_release_upx_packed.sh`:

```bash
CARGO_BIN=cargo          # alternative cargo (e.g., cross)
BINARY_NAME=my-zier      # output binary name
UPX_CMD=upx              # path to upx
UPX_LEVEL="--ultra-brute" # more compression (slower)
```

For very large binaries, `--ultra-brute` may take several minutes but can squeeze out a few extra percent.

---

## Summary

| Build command | Binary location | Size | Static? | UPX? | Panic |
|---------------|-----------------|------|---------|------|-------|
| `cargo build` | `target/debug/zier-alpha` | ~120 MB | no | no | unwind |
| `cargo build --release` | `target/release/zier-alpha` | ~50 MB | no (glibc) | no | unwind |
| `cargo build --release --target x86_64-unknown-linux-musl` | `target/x86_64-unknown-linux-musl/release/zier-alpha` | ~45–50 MB | **yes** | no | unwind |
| `./scripts/build_release_upx_packed.sh` | `target/release-packed/zier-alpha-upx` | ~20–25 MB | no | **yes** | abort |
| `./scripts/build_release_upx_packed.sh --target x86_64-unknown-linux-musl` | `target/x86_64-unknown-linux-musl/release-packed/zier-alpha-upx` | ~18–23 MB | **yes** | **yes** | abort |

Choose the appropriate build based on your distribution needs.
