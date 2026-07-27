#!/bin/sh
# Deploy horcrux to a reMarkable 2 over SSH.
# The USB address root@10.11.99.1 is the default; a Wi-Fi host works too.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
HOST="${1:-root@10.11.99.1}"
DEST=/home/root/horcrux
BIN="$ROOT/target/armv7-unknown-linux-gnueabihf/release/horcrux"

if [ ! -f "$BIN" ]; then
    echo "binary missing; run scripts/build.sh first" >&2
    exit 1
fi

ssh "$HOST" "
    mkdir -p '$DEST' '$DEST/fonts'
    chmod 0755 '$DEST' '$DEST/fonts'
"
scp \
    "$ROOT/horcrux.sh" \
    "$ROOT/horcrux-run.sh" \
    "$ROOT/horcrux.env.example" \
    "$ROOT/horcrux.service" \
    "$HOST:$DEST/"
scp "$BIN" "$HOST:$DEST/horcrux.new"
scp "$ROOT"/fonts/* "$HOST:$DEST/fonts/"

ssh "$HOST" "
    chmod 0755 '$DEST/horcrux.new' '$DEST/horcrux.sh' '$DEST/horcrux-run.sh'
    mv '$DEST/horcrux.new' '$DEST/horcrux'
    chmod 0644 '$DEST/horcrux.env.example' '$DEST/horcrux.service'
    chmod 0644 '$DEST'/fonts/*
    cp '$DEST/horcrux.service' /etc/systemd/system/horcrux.service
    chmod 0644 /etc/systemd/system/horcrux.service
    systemctl daemon-reload
    if [ -f '$DEST/horcrux.env' ]; then
        chmod 0600 '$DEST/horcrux.env'
    fi
"

cat <<EOF
Deployed to $HOST:$DEST

If this is the first install:
  1. install a compatible rm2display build
  2. copy $DEST/horcrux.env.example to $DEST/horcrux.env
  3. chmod 600 $DEST/horcrux.env and set HORCRUX_OPENAI_KEY

Start: systemctl start horcrux.service
Quit:  five-finger tap
Logs:  journalctl -u horcrux.service

For a cable-free AppLoad entry, run scripts/install-appload.sh after
installing compatible xovi and AppLoad builds.
EOF
