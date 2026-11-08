# Security policy

zendiloOS runs on a machine that holds people's files, so reports of
vulnerabilities are welcome and handled before anything else.

## Reporting a vulnerability

Please report it privately through GitHub: **Security → Report a
vulnerability** in this repository. Do not open a public issue or pull
request for it.

A useful report says what is affected (the version, from Settings →
Update & restore, or the package version), how it can be reproduced, and
what an attacker gains: from where (the local network, the internet,
an account of which role) and with what result.

You will get an answer within a few days. A fix ships as a release on the
stable channel, and the advisory is published once that release is out.

## Supported versions

| Version | Supported |
| --- | --- |
| The newest stable release | yes |
| The rolling development build (`dev`) | yes, it gets fixes first |
| Older releases | no: a NAS updates itself to the newest release |

## What zendiloOS does to stay safe

The design is described in [docs/architecture.md](docs/architecture.md):
`zendiloosd` runs without privileges and reaches the system only through
agents that check every request; releases are signed with minisign and a
NAS installs only what the release key signed (see
[docs/releases.md](docs/releases.md)). Administrators are as good as root
on the machine by design.
