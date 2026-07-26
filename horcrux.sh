#!/bin/sh
# horcrux launcher for the reMarkable 2.
#
# Stops xochitl (the stock UI), starts the rm2fb display server (rM2-stuff
# rm2display), runs horcrux under the rm2fb client preload, and always
# restores the stock state afterwards — even on crashes or SIGTERM.
#
# rm2fb only runs while the diary is open: the stock UI drives the panel
# natively, and two panel drivers never coexist.

HORCRUX_DIR="$(cd "$(dirname "$0")" && pwd)"

restore() {
    systemctl stop rm2fb.service rm2fb.socket 2>/dev/null
    systemctl reset-failed xochitl 2>/dev/null
    systemctl start xochitl
}
trap restore EXIT

if [ -f "$HORCRUX_DIR/horcrux.env" ]; then
    set -a
    # shellcheck disable=SC1091
    . "$HORCRUX_DIR/horcrux.env"
    set +a
fi

systemctl stop xochitl
i=0
while [ "$i" -lt 50 ]; do
    case "$(systemctl is-active xochitl 2>/dev/null)" in
        inactive|failed) break ;;
    esac
    i=$((i + 1))
    sleep 0.2
done
sleep 1
systemctl reset-failed rm2fb.service 2>/dev/null
systemctl start rm2fb.socket rm2fb.service
sleep 1
LD_PRELOAD=/opt/lib/librm2fb_client.so.1 "$HORCRUX_DIR/horcrux" "$@"
