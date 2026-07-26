#!/bin/sh
# horcrux service wrapper — runs inside horcrux.service's own cgroup,
# independent of xochitl. AppLoad starts it with `systemctl start horcrux`;
# systemd stops xochitl first (Conflicts=), so the diary never dies with
# the stock UI. The EXIT trap always restores the stock state.

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

# wait for xochitl to fully release the panel: Conflicts= only queues the
# stop job, and SWTCON init fails if xochitl hasn't finished dying yet
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
