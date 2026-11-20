#!/usr/bin/env bash
# Tests removing and purging the package in a test VM (docs/testing.md),
# with nft, systemctl and the files inside it:
#
#   1. what a NAS in use has: the firewall's table, the NAS's SSH key for
#      backups, an ACME account with a DNS token acme.sh kept, an issued
#      certificate's key, NUT's files (one with Debian's sample kept beside
#      it, one zendiloOS made), a unit pinning a share's folders for an app
#   2. `apt-get remove`: the firewall's rules stay in force (the services
#      they guard run on), and so does everything else until a purge
#   3. `apt-get purge`: the rules go, and so do the secrets and the pin
#      unit; NUT's sample comes back; the database stays
#
# Usage: scripts/vm-remove-test.sh
#
# Needs the VM running with the package installed; it is removed, so the
# VM is of no use for other tests afterwards. Runs in the second VM by
# default (the first holds a LAN test).
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)

say() { printf '\033[1m%s\033[0m\n' "$*"; }
check() { printf '   ✓ %s\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

# Whether PATH is there in the VM.
there() { vm "sudo test -e $1"; }
table() { vm 'sudo nft list table inet zendiloos_firewall >/dev/null 2>&1'; }

vm 'dpkg -s zendiloos >/dev/null 2>&1' || fail "zendiloos is not installed"

say "1. What a NAS in use has"
pin=/etc/systemd/system/zendiloos-app-test-shares.service
vm "sudo bash -s" <<EOF || fail "setting the VM up"
set -e
table() { nft list table inet zendiloos_firewall >/dev/null 2>&1; }
table || nft add table inet zendiloos_firewall
install -d -m 0700 /var/lib/zendiloos-backup /var/lib/zendiloos-acme/accounts/1 /etc/zendiloos/web/certificates/1
echo key >/var/lib/zendiloos-backup/id_ed25519
echo "SAVED_CF_Token='a token'" >/var/lib/zendiloos-acme/accounts/1/account.conf
echo key >/etc/zendiloos/web/certificates/1/key.pem
install -d /etc/nut
printf '# Debian sample\nMODE=none\n' >/etc/nut/nut.conf.before-zendiloos
printf '# Written by zendiloOS; changes made here are replaced.\nMODE=netserver\n' >/etc/nut/nut.conf
printf '# Written by zendiloOS; changes made here are replaced.\n[upsmon_local]\n    password = secret\n' >/etc/nut/upsd.users
printf '# Made by hand\n' >/etc/nut/ups.conf
printf '# Created by zendiloOS for the app test; removed with the app.\n[Service]\nExecStart=/usr/libexec/zendiloos/zendiloos-agent pin-app-shares test\n[Install]\nWantedBy=multi-user.target\n' >$pin
systemctl daemon-reload
systemctl enable zendiloos-app-test-shares.service 2>/dev/null
EOF
there /etc/systemd/system/multi-user.target.wants/zendiloos-app-test-shares.service || fail "the pin unit is not enabled"
check "the firewall's table, a backup key, acme.sh's account with a token, a certificate's key, NUT's files, a pin unit"

say "2. Removed"
vm 'sudo DEBIAN_FRONTEND=noninteractive apt-get remove -y -q zendiloos >/dev/null' || fail "apt-get remove"
table || fail "the firewall's rules went with a remove, the services they guard still running"
for path in /var/lib/zendiloos-backup/id_ed25519 /etc/zendiloos/web/certificates/1/key.pem /etc/nut/upsd.users; do
    there "$path" || fail "$path went with a remove"
done
check "the firewall's rules in force; the configuration kept until a purge"

say "3. Purged"
vm 'sudo DEBIAN_FRONTEND=noninteractive apt-get purge -y -q zendiloos >/dev/null' || fail "apt-get purge"
! table || fail "the firewall's rules are still in force after a purge"
for path in /var/lib/zendiloos-backup /var/lib/zendiloos-acme /etc/zendiloos/web /etc/nut/upsd.users \
    /etc/nut/nut.conf.before-zendiloos "$pin" /etc/systemd/system/multi-user.target.wants/zendiloos-app-test-shares.service; do
    ! there "$path" || fail "$path is left after a purge"
done
[[ $(vm 'sudo cat /etc/nut/nut.conf') == $'# Debian sample\nMODE=none' ]] || fail "NUT's sample is not back: $(vm 'sudo cat /etc/nut/nut.conf')"
[[ $(vm 'sudo cat /etc/nut/ups.conf') == '# Made by hand' ]] || fail "a NUT file zendiloOS did not write changed"
there /var/lib/zendiloos || fail "the database went with a purge"
check "the rules, the secrets and the pin unit gone; NUT's sample back, the file made by hand kept; the database kept"

say "Passed: a remove leaves the firewall in force, a purge takes zendiloOS's secrets and units"
