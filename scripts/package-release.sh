#!/bin/sh
# Build a reproducible binary release archive from an existing ARMv7 build.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)
BIN="$ROOT/target/armv7-unknown-linux-gnueabihf/release/horcrux"
OUT_INPUT="${1:-$ROOT/dist}"

if [ ! -f "$BIN" ]; then
    echo "ARMv7 binary missing; run scripts/build.sh first" >&2
    exit 1
fi

mkdir -p "$OUT_INPUT"
OUT=$(cd "$OUT_INPUT" && pwd)
PACKAGE="horcrux-v$VERSION-remarkable2-armv7"
ARCHIVE="$OUT/$PACKAGE.tar.gz"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT HUP INT TERM
STAGE="$WORK/$PACKAGE"

install -d \
    "$STAGE/target/armv7-unknown-linux-gnueabihf/release" \
    "$STAGE/scripts"
install -m 0755 "$BIN" \
    "$STAGE/target/armv7-unknown-linux-gnueabihf/release/horcrux"
install -m 0755 \
    "$ROOT/horcrux.sh" \
    "$ROOT/horcrux-run.sh" \
    "$STAGE/"
install -m 0755 \
    "$ROOT/scripts/deploy.sh" \
    "$ROOT/scripts/install-appload.sh" \
    "$STAGE/scripts/"
install -m 0644 \
    "$ROOT/horcrux.env.example" \
    "$ROOT/horcrux.service" \
    "$ROOT/README.md" \
    "$ROOT/CHANGELOG.md" \
    "$ROOT/LICENSE" \
    "$ROOT/THIRD_PARTY_NOTICES.md" \
    "$STAGE/"
cp -R "$ROOT/fonts" "$ROOT/packaging" "$STAGE/"
find "$STAGE/fonts" "$STAGE/packaging" -type d -exec chmod 0755 {} \;
find "$STAGE/fonts" "$STAGE/packaging" -type f -exec chmod 0644 {} \;

tar \
    --sort=name \
    --mtime="@${SOURCE_DATE_EPOCH:-0}" \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    -C "$WORK" \
    -cf - \
    "$PACKAGE" |
    gzip -n >"$ARCHIVE"

(
    cd "$OUT"
    sha256sum "$PACKAGE.tar.gz" >"$PACKAGE.tar.gz.sha256"
)

printf '%s\n%s\n' "$ARCHIVE" "$ARCHIVE.sha256"
