#!/usr/bin/env bash
# Builds the installation ISO (docs/releases.md, "Installation ISO"): Debian
# 13's netinst image of the package's architecture, checked against its
# pinned SHA512, with an entry "Install zendiloOS" first in its boot menus
# (on amd64 both, BIOS and UEFI; arm64 starts through UEFI only) and "Rescue
# a zendiloOS system" beside it (Debian's rescue mode: a shell in the
# installed system, to save its configuration or repair it), the answers in
# packaging/iso/preseed.cfg, the package to install at the end and
# packaging/iso/network.sh, which hands the network to systemd-networkd. The
# image is not rebuilt: xorriso replaces those few files and keeps Debian's
# boot records as they are.
#
# Usage: scripts/build-iso.sh PACKAGE [OUT_DIR]
#   PACKAGE   dist/zendiloos_<version>_<arch>.deb, amd64 or arm64
#   OUT_DIR   where zendiloos-<version>-<arch>-netinst.iso goes (dist/)
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
[[ $name =~ ^zendiloos_([0-9][0-9A-Za-z.+~]*)_(amd64|arm64)\.deb$ ]] \
    || { echo "not a package for amd64 or arm64: $name" >&2; exit 1; }
version=${BASH_REMATCH[1]}
arch=${BASH_REMATCH[2]}
checksum=DEBIAN_SHA512_${arch^^}
sha512=${!checksum}
debian="debian-$DEBIAN_VERSION-$arch-netinst.iso"
iso="$out/zendiloos-$version-$arch-netinst.iso"
# The installer's kernel and initrd. amd64 starts the graphical installer,
# as Debian's first entry there does, arm64 the text one, as Debian's does
# there: many boards have a serial console and no screen. Only amd64's
# kernel takes a VGA mode.
case $arch in
    amd64) installer=/install.amd initrd=/install.amd/gtk/initrd.gz vga="vga=788 " ;;
    arm64) installer=/install.a64 initrd=/install.a64/initrd.gz vga="" ;;
esac

say() { printf '\033[1m%s\033[0m\n' "$*"; }

mkdir -p "$cache" "$out"
if ! echo "$sha512  $cache/$debian" | sha512sum --check --status 2>/dev/null; then
    say "Downloading $debian"
    # The current point release lives under debian-cd/current, older ones
    # in the archive.
    for base in "https://cdimage.debian.org/debian-cd/current/$arch/iso-cd" \
                "https://cdimage.debian.org/cdimage/archive/$DEBIAN_VERSION/$arch/iso-cd"; do
        curl -sfL --retry 3 -o "$cache/$debian.part" "$base/$debian" && break
    done
    echo "$sha512  $cache/$debian.part" | sha512sum --check --status \
        || { echo "$debian does not match its checksum" >&2; rm -f "$cache/$debian.part"; exit 1; }
    mv "$cache/$debian.part" "$cache/$debian"
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
extract=(-extract /boot/grub/grub.cfg "$work/grub.cfg" -extract /md5sum.txt "$work/md5sum.txt")
if [[ $arch == amd64 ]]; then
    extract+=(-extract /isolinux/menu.cfg "$work/menu.cfg" -extract /isolinux/gtk.cfg "$work/gtk.cfg")
fi
xorriso -osirrox on -indev "$cache/$debian" "${extract[@]}" 2>/dev/null
chmod u+w "$work"/*

boot="preseed/file=/cdrom/zendiloos/preseed.cfg"
# Debian's rescue mode, without zendiloOS's answers: it installs nothing.
rescue="rescue/enable=true"
# The files changed and added, by their path on the ISO.
declare -A files=()

# BIOS, amd64's only: the entry comes first and is the default; Debian's
# stay below.
if [[ $arch == amd64 ]]; then
    cat >"$work/zendiloos.cfg" <<EOF
label zendiloos
	menu label ^Install zendiloOS $version
	menu default
	kernel $installer/vmlinuz
	append ${vga}initrd=$initrd $boot --- quiet
label zendiloos-rescue
	menu label ^Rescue a zendiloOS system
	kernel $installer/vmlinuz
	append ${vga}initrd=$initrd $rescue --- quiet
EOF
    sed -i -e 's/^menu title .*/menu title zendiloOS installer (Debian '"$DEBIAN_VERSION"', BIOS mode)/' \
        -e 's/^include gtk.cfg$/include zendiloos.cfg\ninclude gtk.cfg/' "$work/menu.cfg"
    sed -i '/menu default/d' "$work/gtk.cfg"
    grep -q '^include zendiloos.cfg$' "$work/menu.cfg" || { echo "the BIOS menu changed shape" >&2; exit 1; }
    files+=(
        [./isolinux/zendiloos.cfg]="$work/zendiloos.cfg"
        [./isolinux/menu.cfg]="$work/menu.cfg"
        [./isolinux/gtk.cfg]="$work/gtk.cfg"
    )
fi

# UEFI: the same entry first, which GRUB starts by default.
entry=$(cat <<EOF
menuentry --hotkey=z 'Install zendiloOS $version' {
    set background_color=black
    linux    $installer/vmlinuz ${vga}$boot --- quiet
    initrd   $initrd
}
menuentry --hotkey=r 'Rescue a zendiloOS system' {
    set background_color=black
    linux    $installer/vmlinuz ${vga}$rescue --- quiet
    initrd   $initrd
}
EOF
)
awk -v entry="$entry" '!done && /^menuentry /{print entry; done=1} {print}' "$work/grub.cfg" >"$work/grub.new"
mv "$work/grub.new" "$work/grub.cfg"
grep -q "Install zendiloOS" "$work/grub.cfg" || { echo "the UEFI menu changed shape" >&2; exit 1; }
grep -q "Rescue a zendiloOS system" "$work/grub.cfg" || { echo "the UEFI menu has no rescue entry" >&2; exit 1; }

# The answers, with the kernel's headers of the architecture, which DKMS
# builds ZFS against. On arm64 GRUB goes to the removable media path too
# (\EFI\BOOT\BOOTAA64.EFI): the firmware of many boards (U-Boot's UEFI)
# keeps no boot entries and finds a system only there.
sed "s|^d-i pkgsel/include string linux-headers-amd64\$|d-i pkgsel/include string linux-headers-$arch|" \
    "$root/packaging/iso/preseed.cfg" >"$work/preseed.cfg"
grep -q "^d-i pkgsel/include string linux-headers-$arch\$" "$work/preseed.cfg" \
    || { echo "packaging/iso/preseed.cfg no longer names the kernel's headers" >&2; exit 1; }
if [[ $arch == arm64 ]]; then
    printf '%s\n' '' '### arm64 (scripts/build-iso.sh): GRUB on the removable media path too.' \
        'd-i grub-installer/force-efi-extra-removable boolean true' >>"$work/preseed.cfg"
fi

# "Check the CD" compares md5sum.txt: the files changed and added count.
files+=(
    [./boot/grub/grub.cfg]="$work/grub.cfg"
    [./zendiloos/preseed.cfg]="$work/preseed.cfg"
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
