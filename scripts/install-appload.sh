#!/bin/sh
# Register Tom's Diary with an existing xovi + AppLoad installation and
# enable the proven xovi boot wrapper. This does not install xovi itself.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
HOST="${1:-root@10.11.99.1}"
XOVI=/home/root/xovi
APP_DIR="$XOVI/exthome/appload/horcrux"
REMOTE_UNIT=/tmp/horcrux-xovi.service

ssh "$HOST" "
    test -x '$XOVI/start'
    test -f '$XOVI/extensions.d/appload.so'
    test -f /etc/systemd/system/horcrux.service
" || {
    echo "compatible xovi, AppLoad, and horcrux.service must be installed first" >&2
    exit 1
}

ssh "$HOST" "install -d -m 0755 '$APP_DIR'"
scp \
    "$ROOT/packaging/appload/external.manifest.json" \
    "$ROOT/packaging/appload/icon.png" \
    "$HOST:$APP_DIR/"
scp "$ROOT/packaging/systemd/xovi.service" "$HOST:$REMOTE_UNIT"

ssh "$HOST" "
    chmod 0644 '$APP_DIR/external.manifest.json' '$APP_DIR/icon.png'
    install -m 0644 '$REMOTE_UNIT' /etc/systemd/system/xovi.service
    rm -f '$REMOTE_UNIT'
    systemctl daemon-reload
    systemctl enable xovi.service
"

cat <<EOF
Installed the Tom's Diary AppLoad entry on $HOST.
xovi.service is enabled and will restore AppLoad at every boot.

Reboot the tablet once, then open AppLoad and tap Tom's Diary.
Recovery if an extension causes a boot loop:
  systemctl disable xovi.service
  reboot
EOF
