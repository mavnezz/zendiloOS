#!/usr/bin/env bash
# Builds the installation ISO (docs/releases.md, "Installation ISO"): Debian
# 13's netinst image, checked against its pinned SHA512, with an entry
# "Install zendiloOS" first in both boot menus (BIOS and UEFI) and "Rescue a
# zendiloOS system" beside it (Debian's rescue mode: a shell in the
# installed system, to save its configuration or repair it), the answers
# in packaging/iso/preseed.cfg, the package to install at the end and
# packaging/iso/network.sh, which hands the network to systemd-networkd. The
# image is not rebuilt: xorriso replaces those few files and keeps Debian's
# boot records as they are.
#
# Usage: scripts/build-iso.sh PACKAGE [OUT_DIR]
#   PACKAGE   dist/zendiloos_<version>_amd64.deb
#   OUT_DIR   where zendiloos-<version>-amd64-netinst.iso goes (dist/)
#
# Debian's image is downloaded once into ZENDILOOS_ISO_CACHE (dev/iso-cache).
# The packages themselves come from Debian's mirror during the
# installation; the machine needs the network for it.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
package=${1:?usage: $0 PACKAGE [OUT_DIR]}
out=${2:-$root/dist}
cache=${ZENDILOOS_ISO_CACHE:-$root/dev/iso-cache}

# shellcheck source=../packaging/iso/netinst.env
source "$root/packaging/iso/netinst.env"
name=$(basename "$package")
[[ $name =~ ^zendiloos_([0-9][0-9A-Za-z.+~]*)_amd64\.deb$ ]] || { echo "not an amd64 package: $name" >&2; exit 1; }
version=${BASH_REMATCH[1]}
debian="debian-$DEBIAN_VERSION-amd64-netinst.iso"
iso="$out/zendiloos-$version-amd64-netinst.iso"

say() { printf '\033[1m%s\033[0m\n' "$*"; }

mkdir -p "$cache" "$out"
if ! echo "$DEBIAN_SHA512  $cache/$debian" | sha512sum --check --status 2>/dev/null; then
    say "Downloading $debian"
    # The current point release lives under debian-cd/current, older ones
    # in the archive.
    for base in "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd" \
                "https://cdimage.debian.org/cdimage/archive/$DEBIAN_VERSION/amd64/iso-cd"; do
        curl -sfL --retry 3 -o "$cache/$debian.part" "$base/$debian" && break
    done
    echo "$DEBIAN_SHA512  $cache/$debian.part" | sha512sum --check --status \
        || { echo "$debian does not match its checksum" >&2; rm -f "$cache/$debian.part"; exit 1; }
    mv "$cache/$debian.part" "$cache/$debian"
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
xorriso -osirrox on -indev "$cache/$debian" \
    -extract /isolinux/menu.cfg "$work/menu.cfg" \
    -extract /isolinux/gtk.cfg "$work/gtk.cfg" \
    -extract /boot/grub/grub.cfg "$work/grub.cfg" \
    -extract /md5sum.txt "$work/md5sum.txt" 2>/dev/null
chmod u+w "$work"/*

boot="preseed/file=/cdrom/zendiloos/preseed.cfg"
# Debian's rescue mode, without zendiloOS's answers: it installs nothing.
rescue="rescue/enable=true"

# BIOS: the entry comes first and is the default; Debian's stay below.
cat >"$work/zendiloos.cfg" <<EOF
label zendiloos
	menu label ^Install zendiloOS $version
	menu default
	kernel /install.amd/vmlinuz
	append vga=788 initrd=/install.amd/gtk/initrd.gz $boot --- quiet
label zendiloos-rescue
	menu label ^Rescue a zendiloOS system
	kernel /install.amd/vmlinuz
	append vga=788 initrd=/install.amd/gtk/initrd.gz $rescue --- quiet
EOF
sed -i -e 's/^menu title .*/menu title zendiloOS installer (Debian '"$DEBIAN_VERSION"', BIOS mode)/' \
    -e 's/^include gtk.cfg$/include zendiloos.cfg\ninclude gtk.cfg/' "$work/menu.cfg"
sed -i '/menu default/d' "$work/gtk.cfg"
grep -q '^include zendiloos.cfg$' "$work/menu.cfg" || { echo "the BIOS menu changed shape" >&2; exit 1; }

# UEFI: the same entry first, which GRUB starts by default.
entry=$(cat <<EOF
menuentry --hotkey=z 'Install zendiloOS $version' {
    set background_color=black
    linux    /install.amd/vmlinuz vga=788 $boot --- quiet
    initrd   /install.amd/gtk/initrd.gz
}
menuentry --hotkey=r 'Rescue a zendiloOS system' {
    set background_color=black
    linux    /install.amd/vmlinuz vga=788 $rescue --- quiet
    initrd   /install.amd/gtk/initrd.gz
}
EOF
)
awk -v entry="$entry" '!done && /^menuentry /{print entry; done=1} {print}' "$work/grub.cfg" >"$work/grub.new"
mv "$work/grub.new" "$work/grub.cfg"
grep -q "Install zendiloOS" "$work/grub.cfg" || { echo "the UEFI menu changed shape" >&2; exit 1; }
grep -q "Rescue a zendiloOS system" "$work/grub.cfg" || { echo "the UEFI menu has no rescue entry" >&2; exit 1; }

# "Check the CD" compares md5sum.txt: the files changed and added count.
declare -A files=(
    [./isolinux/zendiloos.cfg]="$work/zendiloos.cfg"
    [./isolinux/menu.cfg]="$work/menu.cfg"
    [./isolinux/gtk.cfg]="$work/gtk.cfg"
    [./boot/grub/grub.cfg]="$work/grub.cfg"
    [./zendiloos/preseed.cfg]="$root/packaging/iso/preseed.cfg"
    [./zendiloos/network.sh]="$root/packaging/iso/network.sh"
    [./zendiloos/$name]="$package"
)
for path in "${!files[@]}"; do
    grep -v "  ${path//./\\.}\$" "$work/md5sum.txt" >"$work/md5sum.new" || true
    mv "$work/md5sum.new" "$work/md5sum.txt"
    printf '%s  %s\n' "$(md5sum <"${files[$path]}" | cut -d' ' -f1)" "$path" >>"$work/md5sum.txt"
done

say "Building $(basename "$iso")"
rm -f "$iso"
maps=()
for path in "${!files[@]}"; do maps+=(-map "${files[$path]}" "${path#.}"); done
xorriso -indev "$cache/$debian" -outdev "$iso" \
    -volid "ZENDILOOS $(printf '%.22s' "$version")" \
    "${maps[@]}" -map "$work/md5sum.txt" /md5sum.txt \
    -boot_image any replay 2>"$work/xorriso.log" \
    || { cat "$work/xorriso.log" >&2; exit 1; }
say "Built $iso ($(du -h "$iso" | cut -f1))"
