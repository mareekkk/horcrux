# Building horcrux

Target: `armv7-unknown-linux-gnueabihf`, **dynamically linked against
glibc 2.35** (reMarkable OS 3.x is kirkstone/glibc 2.35). Dynamic linking is
required — the rm2fb client shim works via `LD_PRELOAD` interposition.

Everything below runs **without sudo**. The one-command path is:

```sh
scripts/build.sh
```

which does exactly the following.

## Toolchain (as set up on the reference build machine)

1. **rustup** (minimal profile), then:

   ```sh
   rustup target add armv7-unknown-linux-gnueabihf
   ```

2. **zig 0.16.0**, used purely as a cross C toolchain/linker:

   ```sh
   mkdir -p toolchain
   curl -fL https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz \
       -o toolchain/zig.tar.xz
   tar -C toolchain -xf toolchain/zig.tar.xz
   mv toolchain/zig-x86_64-linux-0.16.0 toolchain/zig-0.16.0
   rm toolchain/zig.tar.xz
   ```

3. **cargo-zigbuild**:

   ```sh
   cargo install cargo-zigbuild
   ```

## Build

```sh
export PATH="$PWD/toolchain/zig-0.16.0:$PATH"
cargo zigbuild --release --target armv7-unknown-linux-gnueabihf.2.35
```

The `.2.35` suffix makes zig cc link against glibc 2.35 symbol versions, so
the binary loads on the device (newer symbols would fail with
`version 'GLIBC_2.xx' not found`). In practice the highest version referenced
is 2.34.

## Verify

```sh
file target/armv7-unknown-linux-gnueabihf/release/horcrux
# ELF 32-bit LSB pie executable, ARM, EABI5, dynamically linked,
# interpreter /lib/ld-linux-armhf.so.3, stripped
```

The interpreter `/lib/ld-linux-armhf.so.3` is the hard-float ARM dynamic
loader present on the device — this is what lets the rm2fb
`LD_PRELOAD` shim interpose `open`/`ioctl`.

## Host sanity

```sh
cargo check
cargo test                          # unit tests (parser, planner, memory, …)
cargo run --example render_sample   # renders handwriting strokes to a PNG
```

## Notes

- No `.cargo/config.toml` is needed: cargo-zigbuild supplies the linker and
  `CC`/`AR` wrappers itself. Plain `cargo build --target armv7-…` will not
  work without them — use `scripts/build.sh`.
- `ring` (rustls' crypto) cross-compiles fine with zig cc for this target.
