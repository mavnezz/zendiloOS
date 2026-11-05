# Zendiloos

Zendiloos is a NAS operating system based on Debian 13. It is managed
through a web desktop (Vue 3) backed by a Rust server.

> **Status:** early development (milestone M0). Not for real data.

## Documentation

- [Architecture](docs/architecture.md): target architecture, modules, security model, decisions
- [Roadmap](docs/roadmap.md): MVP, milestones, next steps

## Development

Tooling (Rust as pinned in `rust-toolchain.toml`, Node, just, …) comes from
the Nix devShell; direnv loads it when entering the directory, otherwise use
`nix develop`. The product itself contains no Nix.

```sh
just               # list all recipes
just web-install   # once, and after dependency changes: frontend packages
just check         # everything CI checks: backend and frontend
just dev           # API server on http://127.0.0.1:8780, state in dev/state
just web-dev       # interface on http://localhost:5173, talks to `just dev`
just api-types     # after API changes: regenerate the TypeScript types
just sqlx-prepare  # after changing SQL or migrations
```

For the interface, run `just dev` and `just web-dev` side by side and open
http://localhost:5173. On a workstation the privileged agent is simulated:
nothing is rebooted for real.

### Package and test VM

```sh
just deb           # build dist/zendiloos_*.deb in a debian:trixie container
just vm create     # download Debian 13, create a VM with 4 SATA and 1 NVMe disk
just vm start      # boot it; console output in dev/vm/console.log
just vm install    # install the newest package from dist/
just vm ssh        # log in (user debian)
just vm stop
```

The installed system answers on https://127.0.0.1:8443 (self-signed
certificate); the setup code is on the VM's console and in
`/var/lib/zendiloos/setup-code`.

On first start the server logs a setup code (also in
`dev/state/setup-code`); the initial setup needs it to create the first
administrator.

```sh
curl http://127.0.0.1:8780/api/v1/health
curl http://127.0.0.1:8780/api/v1/setup
```

Every push to `main` and every pull request runs `just check` and
`just deb` on GitHub Actions; the package is attached to the run.

## Layout

```text
backend/    Cargo workspace (crates/zendiloos-*, zendiloosd, zendiloos-agent)
frontend/   Vue 3 interface; openapi.json and the TypeScript types are generated
debian/     Debian packaging
packaging/  systemd units, nginx configuration, default configuration, build image
scripts/    test VM
docs/       architecture and roadmap
```


## License

[Apache-2.0](LICENSE)
