#!/usr/bin/env bash
# Exercise the shipped rule with real systemd-tmpfiles in an isolated root,
# and the package upgrade hook with stand-in tools. With root (for example
# in the build container), files start as root's and must become tss's;
# otherwise tss is this test's own account. No host paths are changed.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tree=$work/root
ca=$tree/var/lib/swtpm-localca
rule=$root/packaging/tmpfiles/swtpm-localca.conf
uid=$(id -u)
gid=$(id -g)
if [[ $uid == 0 ]]; then uid=1001; gid=1001; fi

fail() { echo "FAILED: $*" >&2; exit 1; }
expect() {
    local path=$1 mode=$2
    [[ $(stat -c '%u:%g:%a' "$path") == "$uid:$gid:$mode" ]] || fail "permissions of $path"
}
apply() { systemd-tmpfiles --root="$tree" --create "$rule"; }

mkdir -p "$tree/etc"
printf 'root:x:0:0::/root:/bin/sh\ntss:x:%s:%s::/:/bin/false\n' "$uid" "$gid" >"$tree/etc/passwd"
printf 'root:x:0:\ntss:x:%s:\n' "$gid" >"$tree/etc/group"

# Fresh installation: the account can enter and write the CA directory.
apply
expect "$ca" 750
[[ -z $(ls -A "$ca") ]] || fail 'created CA files instead of leaving manufacturing to swtpm'

# Upgrade: existing keys, certificates, serial number and lock survive;
# ownership includes files made by root, without widening their modes.
chmod 0700 "$ca"
mkdir "$ca/nested"
printf 'private key\n' >"$ca/signkey.pem"
printf 'certificate\n' >"$ca/issuercert.pem"
printf '7\n' >"$ca/certserial"
printf 'lock\n' >"$ca/.lock.swtpm-localca"
printf 'nested private key\n' >"$ca/nested/key"
chmod 0600 "$ca/signkey.pem" "$ca/certserial" "$ca/.lock.swtpm-localca" "$ca/nested/key"
chmod 0644 "$ca/issuercert.pem"
chmod 0700 "$ca/nested"
if [[ $(id -u) == 0 ]]; then chown -R 0:0 "$ca"; fi
before=$(stat -c '%i:%a' "$ca/signkey.pem")
contents=$(sha256sum "$ca/signkey.pem" "$ca/issuercert.pem" "$ca/certserial" "$ca/.lock.swtpm-localca" "$ca/nested/key")

# An unexpected symlink must not transfer ownership of something outside
# the CA to tss. Give the fake root a file outside it, reached by the link.
printf 'outside\n' >"$tree/outside"
chmod 0600 "$tree/outside"
outside=$(stat -c '%u:%g:%a' "$tree/outside")
ln -s /outside "$ca/link"
apply
expect "$ca" 750
expect "$ca/signkey.pem" 600
expect "$ca/issuercert.pem" 644
expect "$ca/certserial" 600
expect "$ca/.lock.swtpm-localca" 600
expect "$ca/nested" 700
expect "$ca/nested/key" 600
[[ $(stat -c '%u:%g:%a' "$tree/outside") == "$outside" ]] || fail 'followed a symlink outside the CA'
[[ $(stat -c '%i:%a' "$ca/signkey.pem") == "$before" ]] || fail 'replaced or changed the key'
[[ $(sha256sum "$ca/signkey.pem" "$ca/issuercert.pem" "$ca/certserial" "$ca/.lock.swtpm-localca" "$ca/nested/key") == "$contents" ]] \
    || fail 'changed CA data'
apply
[[ $(stat -c '%i:%a' "$ca/signkey.pem") == "$before" ]] || fail 'replaced the key on a repeated run'

# Source just the postinst's definitions; none of its real host actions
# run. Exercise the real upgrade function with only these tools in PATH.
sed '/^case "\$1" in/,$d' "$root/debian/zendiloos.postinst" >"$work/functions"
mkdir "$work/bin"
printf '#!/bin/sh\nexit 0\n' >"$work/bin/swtpm_localca"
cat >"$work/bin/getent" <<'EOF'
#!/bin/sh
[ "${NO_TSS:-0}" != 1 ]
EOF
cat >"$work/bin/systemd-tmpfiles" <<'EOF'
#!/bin/sh
echo "$*" >>"$CALLS"
[ "${PREPARE_FAILS:-0}" != 1 ]
EOF
chmod +x "$work/bin/"*
upgrade() {
    PATH="$work/bin" CALLS="$work/calls" /bin/sh -c '. "$1"; prepare_swtpm_localca' sh "$work/functions"
}
upgrade
[[ $(cat "$work/calls") == '--create /usr/share/zendiloos/swtpm-localca.conf' ]] || fail 'upgrade did not apply the rule'
rm "$work/calls"
NO_TSS=1 upgrade
[[ ! -e $work/calls ]] || fail 'upgrade tried to repair a CA without its account'
if PREPARE_FAILS=1 upgrade; then fail 'upgrade hid a permission repair failure'; fi
rm "$work/calls" "$work/bin/swtpm_localca"
upgrade
[[ ! -e $work/calls ]] || fail 'upgrade changed a NAS without swtpm'

echo 'swtpm local CA: fresh installation, upgrade, existing keys and symlinks passed'
