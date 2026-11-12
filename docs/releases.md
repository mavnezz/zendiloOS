# Releases and update channels

The NAS updates zendiloOS from this repository's GitHub Releases
(architecture 4.10). CI publishes them; nothing is uploaded by hand.

## Channels

| Channel | What | Made by |
| --- | --- | --- |
| Stable | releases | a tag `vX.Y.Z` |
| Test | pre-releases: release candidates and betas, and stable ones | a tag `vX.Y.Z-rc.N` or `vX.Y.Z-beta.N` |
| Development | the build of the latest commit on `main`, and all others | every push to `main` (the rolling release `dev`) |

Versions follow Debian's order (`scripts/release-version.sh`):

| Git | Package version |
| --- | --- |
| tag `v1.2.0` | `1.2.0` |
| tag `v1.2.0-rc.1` | `1.2.0~rc.1` (before `1.2.0`) |
| commit on `main` after `v1.2.0` | `1.2.0+dev.<the commit's UTC time>.g<commit>` |

A NAS takes only a version newer than its own, so an administrator moving
from Test or Development back to Stable stays on the version installed until
a stable release is newer.

## What a release holds

Everything for two architectures, `amd64` (x86-64) and `arm64`, of one
version:

- `zendiloos_<version>_amd64.deb` and `zendiloos_<version>_arm64.deb`
- `zendiloos_<version>_<arch>.cdx.json`, one per package: its software
  bill of materials (SBOM), CycloneDX 1.5 in JSON: the Rust crates of
  zendiloosd and zendiloos-agent as built for that architecture
  (cargo-cyclonedx; a few crates are for one of them only), the npm
  packages the web interface's bundle holds (as the build found them),
  and the package itself with its SHA-256 and the Debian packages it
  depends on, recommends and suggests (by name and the versions it asks
  for: Debian's mirror gives the rest at installation). `scripts/sbom.sh`
  makes them, `just check` checks the licences of both against
  `backend/deny.toml`, and `docs/licences.md` is the overview made from
  them
- `SHA256SUMS`: the checksums of both packages, both SBOMs and, if there
  are, the ISOs
- `SHA256SUMS.minisig`: its signature, made with the release key, whose
  trusted comment (signed as well) names the build: `zendiloos <version>`
- `zendiloos-amd64-netinst.iso` and `zendiloos-arm64-netinst.iso`, for
  stable releases: the installation ISOs

A NAS takes the package of its own architecture: zendiloosd picks it by
its name (the architecture zendiloosd was built for), and the runner
installs it only if both its name and the `Architecture` the package
itself says are what `dpkg --print-architecture` says; a package of the
other architecture is refused, signed or not. An amd64 NAS of an earlier
version takes its package from a release with both as before, as it
always looked for `_amd64.deb`.

The NAS checks the signature with the public key its own package brought
along (`/usr/share/zendiloos/release-signing.pub`, from
`packaging/release-signing.pub`), and installs nothing older than what runs.
Every build is signed with the same key, the development build of each
push to `main` too, so the signature alone does not say which build it is
for: the NAS takes it only if its trusted comment is exactly
`zendiloos <the version of the package>`, and a build only if its channel
takes its kind, told by the version: a development build (`+dev.`) only on
Development, a test release (`~rc.N`, `~beta.N`) on Test and Development, a
release everywhere. A version of another form counts as a development
build. CI has signed every release so since the first (`-t
"zendiloos $version"` in the step Sign, as `just sign-release` does); the
runner of the installed version checks, so an update from 0.3.0 or older
is still taken the way that version takes it, and every update after with
these checks.

## The release key

Set up once: the public key is `packaging/release-signing.pub` (key ID
`B97DD0234085B950`), the secret one is `MINISIGN_SECRET_KEY`, with no
password (`MINISIGN_PASSWORD` stays unset).

It belongs in the environment `release`, which CI's release job deploys
to, not among the repository's secrets: a workflow of a pull request or of
another branch then never sees it. Settings → Environments → `release`
(made by the first run that names it): deployment branches and tags
"Selected", with the branch `main` and the tag pattern `v*`; then the
environment secret `MINISIGN_SECRET_KEY` with the key, and the repository
secret of that name deleted. Until then the job reads the repository
secret as before.
A copy of the secret key lives on the maintainer's machine in
`~/.minisign/zendiloos-release.key`; keep a backup of it offline and nowhere
else: whoever has it can ship code to every NAS.

Should the key ever have to change, make a new pair (`just release-key`,
which also takes a password, then set `MINISIGN_PASSWORD` too) and ship the
new public key in a release still signed with the old one; installations
then take the new key with that update. Packages built before a public key
was in the repository take no updates of zendiloOS; they are updated once by
hand (`apt install ./zendiloos_….deb`).

## Making a release

1. A pull request adds the release's entry to `debian/changelog` and sets
   the same version in `backend/Cargo.toml` (`[workspace.package]`, with
   `-` where Debian writes `~`); `just check` fails while the two differ.
   A tag whose version is not the newest entry's fails the package build,
   so a release candidate needs an entry of its own too.
2. On its merge commit in `main`:

   ```sh
   git tag -a v1.2.0-rc.1 -m "zendiloOS 1.2.0, first release candidate"
   git push origin v1.2.0-rc.1
   ```

   Tags `v*` cannot be moved or deleted, so check the version first. A tag
   on a commit that `main` does not contain is not released.

CI checks, builds both packages with the tag's version (arm64's
cross-compiled in the same container, see below), runs the backend's tests
on an arm64 machine too, signs `SHA256SUMS` and creates the GitHub release (a pre-release for a tag with a `-`), with
notes generated from the pull requests. They are what the NAS shows as the
release notes, so replace them with notes written for people:
`gh release edit v1.2.0-rc.1 --notes-file notes.md`. Pushes to `main` replace the
release `dev`; its notes list the changes since the last release tag (the
newest 50 commit subjects). A run of `main` is never cancelled halfway, so
`dev` is not left deleted; one that `main` has moved past publishes
nothing, and the version counts from the commit's time, so a re-run of an
older commit never looks newer.

## What is new in the installed version

Each package says what changed in its version: `debian/rules` writes
`/usr/share/zendiloos/changes.md` with `scripts/release-notes.sh`, and
Control panel → Updates shows it below the installed version, without
asking GitHub. A release or test release takes the newest entry of
`debian/changelog`, as bullets; a development build "Changes since
v1.2.0:" and the subjects of the commits since that tag, newest first, at
most 50, the same list as the notes of the release `dev`. The package
build reads the commits from the repository it was given (`/src` in
`just deb`); where git cannot tell (no git, a worktree, no release tag),
a development build takes the changelog's entry as well.
`just release-notes-test` checks the script.

The page shows these notes and those of an available release as
Markdown (headings, lists, emphasis, code and links, which open in a new
tab); HTML in them shows as text, and images are not loaded.

## Installation ISO

Only the way in: afterwards a NAS updates itself, so an ISO is made for
stable releases only. The repository variable `ZENDILOOS_ISO` (Settings →
Secrets and variables → Actions → Variables) changes that: `stable` (the
default), `test` (test releases too), `all` (every development build too)
or `off`.

There is one per architecture. Each is Debian 13's netinst image of that
architecture (`packaging/iso/netinst.env`, each pinned by its SHA512) with
an entry "Install zendiloOS <version>" first in its boot menus, and the
answers of `packaging/iso/preseed.cfg`. amd64's has both menus, BIOS and
UEFI, and starts the graphical installer; arm64's starts through UEFI
only, as Debian's does, with GRUB's menu, and the text installer, which a
board's serial console shows as well as a screen. The arm64 image names
the kernel headers of its architecture (`linux-headers-arm64`) and puts
GRUB on the removable media path too (`\EFI\BOOT\BOOTAA64.EFI`): the UEFI
of many boards (U-Boot's) keeps no boot entries and starts a disk only
from there.
The installer asks only for the language, location and keyboard, the
machine's name, root's password and the disk for the system, never chosen
for you: on a NAS the wrong disk would cost data. Packages come from
Debian's mirror, with `contrib` for ZFS, which DKMS builds for the
installed kernel (no ZFS module is shipped, see architecture 4.10); at the
end the release's package is installed from the ISO, and the setup is left
open (architecture 4.5): no setup code, the console shows the address of
the web interface, and whoever opens it first from the own network creates
the administrator. Last, `packaging/iso/network.sh` hands the network
Debian's installer set up with ifupdown to systemd-networkd and
systemd-resolved, so zendiloOS can change it from the first boot
(architecture 6, "Network"); what it does not understand stays with
ifupdown, which zendiloOS then offers to take over.

Beside it, "Rescue a zendiloOS system" boots Debian's rescue mode
(`rescue/enable=true`, without zendiloOS's answers: it installs nothing):
a shell in the installed system, to copy a configuration backup off it,
put a database from `/var/lib/zendiloos/backup` back or repair what keeps
it from starting. What a recovery mode is to become is in architecture
4.15.

Each release carries them as `zendiloos-amd64-netinst.iso` and
`zendiloos-arm64-netinst.iso`, listed in the signed `SHA256SUMS`, so the
newest stable ones are always
`https://github.com/mavnezz/zendiloOS/releases/latest/download/zendiloos-amd64-netinst.iso`
and
`https://github.com/mavnezz/zendiloOS/releases/latest/download/zendiloos-arm64-netinst.iso`.

Locally: `just iso` builds one for the newest package in `dist/` of this
machine's architecture, `just iso arm64` (or `ZENDILOOS_ARCH=arm64`) the
other's; `just iso-test` installs from it into a fresh QEMU machine,
unattended, and checks that both boot menus offer the rescue mode, that
zendiloOS answers, takes its setup, makes a ZFS pool, that
systemd-networkd manages the network and that names resolve (apt reaches
Debian's mirror), and that a configuration backup downloads and restores;
`just iso-test arm64` does it in QEMU's `virt` machine with UEFI (AAVMF),
emulated on an x86 machine and so for hours (`docs/testing.md`). The
workflow *ISO test* does the same on GitHub, by hand. `just iso-network-test` (part of `just check`) runs the
network's switch against what Debian's installer writes. A newer Debian
point release is taken by changing `packaging/iso/netinst.env`: its
version and both checksums.

## Two architectures

Each package is built in the same `debian:trixie` container
(`packaging/build.Dockerfile`, pinned by the digest of its index, which
names the image of each architecture): `just deb` builds for this
machine's architecture, `just deb arm64` (or `ZENDILOOS_ARCH=arm64 just
deb`) for arm64 on an x86 machine, cross-compiled
(`dpkg-buildpackage --host-arch arm64`): Rust's standard library for
`aarch64-unknown-linux-gnu`, Debian's cross compiler
`gcc-aarch64-linux-gnu` for linking and for the C the crates bring, and
the C library for arm64 through multiarch, from which `dpkg-shlibdeps`
tells the package's dependencies as for a native build. On an arm64
machine it is the other way round. The web interface is the same for
both. Building on an arm64 machine instead would have needed one in CI
(GitHub has them) and an emulated one on every x86 workstation, which
takes hours for what takes minutes cross-compiled; so one build
environment for both, and the arm64 runner only for what must run there:
CI's job *Rust on arm64* runs the backend's tests natively, beside the
check, and a release waits for it. `just check` runs on amd64 only.

`debian/control` suggests libvirt, QEMU for x86 and OVMF on amd64 only
(`[amd64]`): virtual machines are not offered on arm64 yet
(architecture 4.13).

## The repository must stay public

The NAS asks GitHub's API without an account. A private repository answers
404, and the NAS shows that zendiloOS's releases could not be fetched;
Debian's updates work all the same.
