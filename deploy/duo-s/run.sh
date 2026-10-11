#!/bin/sh
while IFS= read -r setting; do
    export "${setting?}"
done < /etc/default/c2-node

waited=0
while [ -z "${C2_GNSS_PORT:-}" ] && [ "$(date +%Y)" -lt 2025 ] && [ "$waited" -lt 120 ]; do
    sleep 1
    waited=$((waited + 1))
done

if [ -n "${C2_GNSS_PORT:-}" ] && [ -x /etc/init.d/S49ntp ]; then
    /etc/init.d/S49ntp stop >/dev/null 2>&1
    pidof ntpd >/dev/null || ntpd -gq >/dev/null 2>&1 &
fi

echo "${C2_PINMUX:-}" | tr ',' '\n' | while IFS= read -r pin; do
    [ -n "$pin" ] && duo-pinmux -w "$pin" >/dev/null
done

sleep 2
mkdir -p "$C2_INBOX"
/opt/c2/c2-node 2>&1 | logger -t c2-node
