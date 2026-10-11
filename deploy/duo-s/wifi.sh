#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: wifi.sh <board-host> <ssid> < passphrase-file" >&2
    exit 1
fi

target="root@$1"
ssid=$2
known_hosts="UserKnownHostsFile=${C2_KNOWN_HOSTS:-$HOME/.ssh/known_hosts}"
here=$(cd "$(dirname "$0")" && pwd)

case "$ssid" in
    *[!A-Za-z0-9_-]* | "")
        echo "ssid must contain only letters, digits, '-' and '_'" >&2
        exit 1
        ;;
esac

psk=$(python3 -c '
import hashlib, sys
passphrase = sys.stdin.read().rstrip("\n").encode()
if not 8 <= len(passphrase) <= 63:
    sys.exit("passphrase must be 8 to 63 characters")
print(hashlib.pbkdf2_hmac("sha1", passphrase, sys.argv[1].encode(), 4096, 32).hex())
' "$ssid")

ssh -o "$known_hosts" "$target" 'cat > /etc/init.d/S99wifi && chmod 755 /etc/init.d/S99wifi' < "$here/S99wifi"
ssh -o "$known_hosts" "$target" '[ -f /etc/wpa_supplicant.conf.orig ] || cp /etc/wpa_supplicant.conf /etc/wpa_supplicant.conf.orig'
ssh -o "$known_hosts" "$target" "awk -v wanted='ssid=\"$ssid\"' '
    /^network=[{]/ { block = \$0; inside = 1; keep = 1; next }
    inside { block = block \"\\n\" \$0; if (index(\$0, wanted)) keep = 0; if (\$0 ~ /^[}]/) { if (keep) print block; inside = 0 }; next }
    { print }
' /etc/wpa_supplicant.conf > /etc/wpa_supplicant.conf.new && chmod 600 /etc/wpa_supplicant.conf.new && mv /etc/wpa_supplicant.conf.new /etc/wpa_supplicant.conf"
printf 'network={\n\tssid="%s"\n\tpsk=%s\n\tkey_mgmt=WPA-PSK\n}\n' "$ssid" "$psk" |
    ssh -o "$known_hosts" "$target" 'cat >> /etc/wpa_supplicant.conf'
echo "network $ssid written on $1"
ssh -o "$known_hosts" "$target" 'chmod 600 /etc/wpa_supplicant.conf && /etc/init.d/S99wifi restart'

echo "wifi configured on $1 for $ssid"
