#!/bin/sh
# Reproducible cross-build of horcrux for the reMarkable 2
# (armv7-unknown-linux-gnueabihf, dynamically linked against glibc 2.35).
#
# Toolchain: rustup target + cargo-zigbuild + zig 0.16.0 (downloaded into
# toolchain/ on first run). No sudo required.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
ZIG_VERSION=0.16.0
ZIG_DIR="$ROOT/toolchain/zig-$ZIG_VERSION"

if [ ! -x "$ZIG_DIR/zig" ]; then
    echo ">> downloading zig $ZIG_VERSION into toolchain/"
    mkdir -p "$ROOT/toolchain"
    curl -fL "https://ziglang.org/download/$ZIG_VERSION/zig-x86_64-linux-$ZIG_VERSION.tar.xz" \
        -o "$ROOT/toolchain/zig.tar.xz"
    tar -C "$ROOT/toolchain" -xf "$ROOT/toolchain/zig.tar.xz"
    mv "$ROOT/toolchain/zig-x86_64-linux-$ZIG_VERSION" "$ZIG_DIR"
    rm "$ROOT/toolchain/zig.tar.xz"
fi
export PATH="$ZIG_DIR:$PATH"

command -v cargo >/dev/null 2>&1 || export PATH="$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null 2>&1 || {
    echo "cargo not found; install rustup: https://rustup.rs" >&2
    exit 1
}
command -v cargo-zigbuild >/dev/null 2>&1 || {
    echo "cargo-zigbuild not found; run: cargo install cargo-zigbuild" >&2
    exit 1
}

rustup target add armv7-unknown-linux-gnueabihf

# The .2.35 suffix pins the glibc symbol versions to reMarkable OS 3.x
# (kirkstone, glibc 2.35), so the binary loads on the device.
cargo zigbuild --release --target armv7-unknown-linux-gnueabihf.2.35 "$@"

BIN="$ROOT/target/armv7-unknown-linux-gnueabihf/release/horcrux"
file "$BIN"
