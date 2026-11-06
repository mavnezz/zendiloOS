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
| commit on `main` after `v1.2.0` | `1.2.0+dev.<UTC time>.g<commit>` |

A NAS takes only a version newer than its own, so an administrator moving
from Test or Development back to Stable stays on the version installed until
a stable release is newer.

## What a release holds

- `zendiloos_<version>_<arch>.deb`
- `SHA256SUMS`: the package's checksum
- `SHA256SUMS.minisig`: its signature, made with the release key
- `zendiloos-amd64-netinst.iso`, for stable releases: the installation ISO

The NAS checks the signature with the public key its own package brought
along (`/usr/share/zendiloos/release-signing.pub`, from
`packaging/release-signing.pub`), and installs nothing older than what runs.

## The release key

Set up once: the public key is `packaging/release-signing.pub` (key ID
`B97DD0234085B950`), the secret one is the repository secret
`MINISIGN_SECRET_KEY`, with no password (`MINISIGN_PASSWORD` stays unset).
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

```sh
git tag -s v1.2.0-rc.1 -m "zendiloOS 1.2.0, first release candidate"
git push origin v1.2.0-rc.1
```

CI checks, builds the package with the tag's version, signs `SHA256SUMS`
and creates the GitHub release (a pre-release for a tag with a `-`), with
notes generated from the commits; edit them on GitHub as needed, they are
what the NAS shows as the release notes. Pushes to `main` replace the
release `dev`.

## Installation ISO

Only the way in: afterwards a NAS updates itself, so an ISO is made for
stable releases only. The repository variable `ZENDILOOS_ISO` (Settings →
Secrets and variables → Actions → Variables) changes that: `stable` (the
default), `test` (test releases too), `all` (every development build too)
or `off`.

The ISO is Debian 13's netinst image (`packaging/iso/netinst.env`, pinned
by its SHA512) with an entry "Install zendiloOS <version>" first in both
boot menus, BIOS and UEFI, and the answers of `packaging/iso/preseed.cfg`.
The installer asks only for the language, location and keyboard, the
machine's name, root's password and the disk for the system, never chosen
for you: on a NAS the wrong disk would cost data. Packages come from
Debian's mirror, with `contrib` for ZFS, which DKMS builds for the
installed kernel (no ZFS module is shipped, see architecture 4.10); at the
end the release's package is installed from the ISO. The console then
shows the address of the web interface and the setup code.

Each release carries it as `zendiloos-amd64-netinst.iso`, listed in the
signed `SHA256SUMS`, so the newest stable one is always
`https://github.com/mavnezz/zendiloOS/releases/latest/download/zendiloos-amd64-netinst.iso`.

Locally: `just iso` builds one for the newest package in `dist/`;
`just iso-test` installs from it into a fresh QEMU machine, unattended, and
checks that zendiloOS answers, takes its setup and makes a ZFS pool. A
newer Debian point release is taken by changing `packaging/iso/netinst.env`.

## The repository must be readable

The NAS asks GitHub's API without an account. While the repository is
private, it answers 404 and the NAS shows that zendiloOS's releases could
not be fetched; Debian's updates work all the same.
