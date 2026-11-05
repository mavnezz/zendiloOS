# Zendiloos Architecture

> Status: draft of 2026-11-05.
> **[Assumption]** marks assumptions, **[Open]** questions still to be
> decided, **[Later]** features deliberately postponed.

Zendiloos is a NAS operating system based on **Debian 13 "trixie"**. It is
managed entirely through a web desktop (Vue 3) that talks to a Rust backend.

The name `zendiloos` is used everywhere: crates `zendiloos-*`, services
`zendiloosd` and `zendiloos-agent`, system user `zendiloos`, paths
`/var/lib/zendiloos`, packages `zendiloos*`.

---

## 1. Target Architecture

### 1.1 Runtime View

```text
Browser (Vue desktop)
   │  HTTPS :443 (HTTP :80 optional)
   │  REST  /api/v1/…      WebSocket  /api/v1/ws
   v
nginx  ─────────────── /  →  static frontend  /usr/share/zendiloos/webui
   │  HTTP over Unix socket  /run/zendiloos/api.sock
   v
zendiloosd          user "zendiloos", no capabilities, never root
   ├─ SQLite        /var/lib/zendiloos/zendiloos.db  (source of truth)
   ├─ reads         /proc, /sys  (metrics, hardware information)
   ├─ tasks         metrics sampler, SMART/RAID monitoring, jobs, notifications
   │
   │  typed requests over Unix socket  /run/zendiloos/agent.sock
   │  (root:zendiloos 0660, plus SO_PEERCRED check)
   v
zendiloos-agent     root, socket-activated, no network, closed set of operations
   │  execve of fixed paths │ atomic files at fixed locations │ systemd via D-Bus │ Docker socket
   v
mdadm · sgdisk · wipefs · mkfs.btrfs/ext4 · btrfs · smartctl · nvme · useradd · smbpasswd · …
```

| Process | Runs as | Responsibility | Network |
|---|---|---|---|
| nginx | `www-data` | TLS, static files, rate limiting, proxying | yes (80/443) |
| `zendiloosd` | `zendiloos` | API, authentication, business logic, database, live data | Unix socket only |
| `zendiloos-agent` | `root` | privileged, validated single operations | none (`PrivateNetwork`) |

### 1.2 Two Deployment Modes, One Set of Packages

1. **Packages on an existing Debian 13** (phases 1–4): the `.deb` packages
   are installed on an existing system, at first in a test VM.
2. **Appliance from an ISO** (phase 5): the same packages, preinstalled in a
   system image with its own installer and update mechanism.

This leads to a ground rule that applies from day one:

> **The database is the source of truth. System configuration (Samba, NFS
> exports, mount units, network, user accounts) is generated from it and
> reconciled at startup.**

This makes image-based updates possible later (the root filesystem can be
replaced without losing configuration), detects drift, and keeps Zendiloos
from editing configuration files it does not own. Zendiloos writes only its
own files (for example `/etc/samba/zendiloos.conf`, pulled in with
`include`) instead of rewriting `smb.conf`.

### 1.3 Installed Layout

The Debian package `zendiloos` (built with `just deb`) installs:

| Path | Purpose |
|---|---|
| `/usr/sbin/zendiloosd` | management service, user `zendiloos` (created through `sysusers.d`) |
| `/usr/libexec/zendiloos/zendiloos-agent` | privileged agent, root, started on demand |
| `/usr/share/zendiloos/webui/` | built web interface, served by nginx |
| `/etc/zendiloos/zendiloosd.toml`, `agent.toml` | static configuration (conffiles, defaults commented out) |
| `/etc/zendiloos/tls/` | self-signed certificate, created at installation or first boot (`zendiloos-certificate.service`) |
| `/etc/nginx/conf.d/zendiloos.conf` | HTTP → HTTPS redirect, TLS, security headers, rate limits, proxy to the API socket |
| `/run/zendiloos/api.sock` | `zendiloosd.socket`, `zendiloos:www-data 0660`: only nginx reaches the API |
| `/run/zendiloos/agent.sock` | `zendiloos-agent.socket`, `root:zendiloos 0660`: only zendiloosd reaches the agent |
| `/var/lib/zendiloos/` | database, backups, setup code, console text (`0700`) |
| `/run/issue.d/zendiloos.issue` | link to the console text: the login prompt shows the address and, until setup is done, the setup code |

`zendiloosd` uses `Type=notify` and is ordered before `getty.target`, so the
console prompt appears only once the setup code is written. Both services
run with strict systemd sandboxing; `systemd-analyze security` rates
zendiloosd 1.5 and the agent 1.3 ("OK"). The agent has no capabilities and
no network as long as reboot and power off are its only operations.

### 1.4 Alternatives Considered

**Reverse proxy: nginx** rather than exposing axum directly. nginx provides
proven TLS, rate limiting, efficient static file serving, and later
forwarding to container apps. `zendiloosd` therefore never listens on a
network port. Downside: one more component to configure, which the package
takes care of.

**Privileged operations: a dedicated agent.**

| Option | Pros | Cons |
|---|---|---|
| **Dedicated agent with a closed set of operations** (chosen) | one point of validation and audit; validation in the type system; independent of polkit rules | our own root code, which must stay small |
| System D-Bus services (udisks2, NetworkManager, logind) + polkit | proven root code maintained elsewhere | generic object model, policy scattered across polkit rules; Samba, users and Docker still unsolved |
| `sudo` with a command list | quick to build | argument validation in sudoers is barely manageable; shell-adjacent attack surface |
| API runs as root | simplest | any flaw in the web service is immediately root |

---

## 2. Module Structure

### 2.1 Backend: DDD and Hexagonal Architecture

The backend follows domain-driven design with a hexagonal (ports and
adapters) architecture. The goal is the dependency rule: everything points
inward, and the domain knows no frameworks, databases or operating system.
**Crate boundaries enforce it**, so a violation is a build error, not a
review comment.

| Concept | In Zendiloos |
|---|---|
| Bounded context | one core crate each: `identity`, `system`, `storage`, `sharing`, `files`, `containers`, `network` |
| Domain layer | entities, value objects, aggregates, domain rules and errors; pure Rust, no I/O |
| Application layer | use cases (commands and queries) and the **ports** they need, as traits |
| Primary (driving) adapter | `zendiloos-api`: HTTP and WebSocket |
| Secondary (driven) adapters | `zendiloos-infra`: SQLite repositories, agent gateways, host probes (`sysinfo`, `/proc`, journal) |
| Shared kernel | `zendiloos-kernel`: IDs, validated names, `Secret`, time, common port errors |
| Composition root | `zendiloosd`: reads the configuration and plugs adapters into ports |

| Crate | Role | May depend on (internal) |
|---|---|---|
| `zendiloos-kernel` | shared kernel: time, `Secret`, request context, the audit entry and its port, common port errors | – |
| `zendiloos-identity` | accounts, roles and permissions, sessions, passwords, throttling, audit | kernel |
| `zendiloos-system` | system information, metrics, power, logs | kernel |
| `zendiloos-storage` | disks, SMART, pools, volumes | kernel |
| `zendiloos-sharing` | SMB and NFS shares, share permissions | kernel |
| `zendiloos-files` | file manager, recycle bin | kernel |
| `zendiloos-containers` | containers, images, volumes, networks, apps | kernel |
| `zendiloos-network` | interfaces, IP, DNS, hostname | kernel |
| `zendiloos-infra` | all secondary adapters, SQLite migrations | kernel, contexts, agent-proto |
| `zendiloos-api` | primary adapter: axum routes, DTOs, OpenAPI, auth middleware, WebSocket | kernel, contexts |
| `zendiloosd` | composition root (binary) | everything except agent |
| `zendiloos-agent-proto` | wire protocol to the agent, `AgentTransport` trait, Unix client | – |
| `zendiloos-exec` | the only way to run programs | – |
| `zendiloos-agent` | privileged root service (binary) | agent-proto, exec |
| `zendiloos-sim` | simulated agent behind `AgentTransport`; executes nothing | agent-proto |

Crates are created in the step that needs them; there are no empty
placeholder crates.

```text
                         zendiloosd  (composition root)
               ┌──────────────┼──────────────────┬──────────────┐
               v              v                  v              v
        zendiloos-api   zendiloos-infra    zendiloos-sim   (agent side)
        primary adapter secondary adapters       │
               │         │            │           │
               v         v            v           v
      bounded contexts (core)      zendiloos-agent-proto <── zendiloos-agent ──> zendiloos-exec
      identity · system · storage          │                  (root)
      sharing · files · containers         │
      network                              │
               │                           │
               └─────────────┬─────────────┘
                             v
                      zendiloos-kernel
```

Rules:

- **Context crates contain no I/O.** Their only dependencies are the kernel,
  `thiserror` and `async-trait`: no sqlx, axum, tokio, sysinfo or serde.
- **Contexts do not depend on each other.** If one context needs something
  from another (sharing needs to know volumes), it declares a port in its
  own terms, and `zendiloos-infra` or the composition root implements that
  port. This is the anti-corruption layer between contexts.
- **`zendiloos-api` calls use cases, never adapters.** It does not depend on
  `zendiloos-infra`.
- **The agent side knows nothing about contexts, the database or HTTP.**
  A bug in a parser, a repository or a handler therefore never runs as
  root. Infra's agent gateways translate between context ports and the agent
  protocol; behind `AgentTransport` sits either the Unix socket client or
  `zendiloos-sim`, so gateway code including its parsers runs in tests and
  on a workstation without root or real disks.

Inside a context crate:

```text
zendiloos-storage/src/
├── lib.rs
├── domain/            entities, value objects, rules (is this RAID layout valid?)
│   ├── disk.rs
│   └── pool.rs
└── application/
    ├── ports.rs       traits the context needs: PoolRepository, DiskInventory, …
    ├── commands/      use cases that change state, through the domain: CreatePool, …
    └── queries/       use cases that read: ListDisks, GetSmartReport, …
```

Deliberate simplifications:

- **No generic command or query bus.** A use case is a struct with an
  `execute` method; that is explicit and type-safe.
- **Lightweight CQRS.** Commands go through the domain model; queries may
  read a port and return a read model directly.
- **Depth per context.** `storage`, `sharing` and `identity` have real rules
  and use every layer. `system` is mostly a pass-through and stays thin.
- **Async ports** are traits with `#[async_trait]`, held as `Arc<dyn Port>`.
  Rust does not yet allow `dyn` traits with native `async fn`.
- **Domain events** (for example `ShareCreated` → regenerate the Samba
  configuration) are published in-process through a port; no broker.

**Alternatives considered:**

| Split | Pros | Cons |
|---|---|---|
| **Core crate per context + one infra crate** (chosen) | domain purity and context separation enforced by the compiler; root code isolated | more crates; infra grows with every context |
| One crate per context with domain, application and adapters as modules | a context in one place, fewer crates | domain purity only by convention, since the crate depends on sqlx and sysinfo |
| One core crate with contexts as modules, plus infra and api | fewest crates; domain purity enforced | separation between contexts only by convention (`pub(crate)`) |

### 2.2 Frontend

| Purpose | Choice | Rationale |
|---|---|---|
| Framework | Vue 3 (Composition API, `<script setup>`), TypeScript 5.9 strict, Vite | TypeScript 7 once vue-tsc, typescript-eslint and openapi-typescript support it |
| Styling/components | Tailwind CSS 4, shadcn-vue (on Reka UI) | components live as source in the repo, accessible, adaptable |
| Client state | Pinia | windows, session, notifications, preferences |
| Server state | `@tanstack/vue-query` | caching, invalidation, loading and error states; avoids rebuilding this in every Pinia store |
| API types | `openapi-typescript` + `openapi-fetch`, generated from utoipa | one source of truth, no drift between Rust and TypeScript |
| Routing | Vue Router: `/setup`, `/login`, `/` (desktop) | apps are opened through the window store, not through URLs |
| i18n | vue-i18n; English first (the default and the source of all texts), German as a second language chosen in the settings | retrofitting internationalization is expensive |
| Icons | lucide-vue-next | matches shadcn-vue |
| Tests | Vitest with Vue Test Utils, later Playwright for end-to-end tests | |
| Formatting and linting | Prettier; ESLint with the Vue and TypeScript rules and `import-x/no-restricted-paths` for module boundaries | Prettier formats, ESLint checks code only |

```text
frontend/src/
├── main.ts
├── app/                  composition root: App.vue, router, guards, plugins
├── desktop/              shell
│   ├── windowManager.ts  pure logic (open, focus, minimize, move, resize,
│   │                     clamp geometry), unit tested with Vitest
│   ├── windows.store.ts  Pinia store around windowManager, layout in localStorage
│   ├── apps.ts           app registry: id, title, icon, window size, permission,
│   │                     lazily loaded component or sections
│   ├── Desktop.vue · Taskbar.vue · Launcher.vue · AppWindow.vue
│   └── NotificationCenter.vue
├── features/             one module per backend bounded context
│   ├── auth/  system/  storage/  shares/  files/  users/  containers/  network/
│   │   ├── api.ts        query/mutation composables (vue-query)
│   │   ├── model.ts      types and pure logic
│   │   └── components/   components and pages
└── shared/
    ├── api/              fetch client (CSRF, error format), WebSocket client, generated schema
    ├── ui/               shadcn-vue components, own building blocks (meter, sparkline, confirm dialog)
    ├── lib/              formatting, helpers
    └── i18n/             translations
```

- Features do not import from each other, only from `shared/`.
  `desktop/apps.ts` composes feature pages into apps; features know nothing
  about windows. Enforced with an ESLint rule (`import/no-restricted-paths`).
- Pages adapt to the window size with **container queries**, not to the
  screen size.
- On phones there are no windows: the desktop becomes a home screen and apps
  open full screen.

---

## 3. Communication Paths

### 3.1 Browser ↔ nginx ↔ zendiloosd

- **REST** under `/api/v1/…`, JSON with `snake_case` field names.
  Timestamps are integers: Unix time in seconds (UTC); the frontend formats
  them in the viewer's locale. Errors use RFC 9457
  (`application/problem+json`) with a stable `code` for the frontend;
  internal details go to the log only.
- **Versioning:** the path carries the major version. The OpenAPI document
  is generated, checked in, and verified in CI against unintended changes.
  A breaking change means `/api/v2` alongside `/api/v1`.
- **Long-running operations** (creating a pool, RAID resync, large copies,
  image pulls) answer `202 Accepted` with a job ID. Progress arrives over the
  WebSocket; jobs are also stored in the database so a restart can mark them
  as interrupted.
- **One WebSocket per browser tab** (`/api/v1/ws`) with topic subscriptions
  instead of one connection per data stream:

  ```json
  → {"type":"subscribe","topic":"metrics"}
  ← {"type":"event","topic":"metrics","data":{…}}
  ← {"type":"event","topic":"jobs","data":{"id":"…","progress":0.42}}
  ← {"type":"event","topic":"notifications","data":{…}}
  ```

  A single sampler task produces metrics (every 2 s) and distributes them
  through `tokio::sync::broadcast`/`watch`; the load does not depend on the
  number of open browsers.
- In production `zendiloosd` listens on `/run/zendiloos/api.sock`, during
  development on `127.0.0.1:8080` (Vite proxies to it).

### 3.2 zendiloosd ↔ zendiloos-agent

| Property | Decision |
|---|---|
| Transport | Unix socket `/run/zendiloos/agent.sock`, created by systemd (`root:zendiloos 0660`), agent socket-activated |
| Authentication | file permissions + `SO_PEERCRED`: only the UID of `zendiloos` is served |
| Framing | one request per connection; JSON lines (NDJSON); size limits (requests 64 KiB) |
| Operations | closed `enum Request` with typed arguments validated during deserialization; `deny_unknown_fields` |
| Responses | a single response; **[Later]** a stream of frames (progress, logs) ending with a result |
| Files | for the file manager the agent passes open file descriptors via `SCM_RIGHTS` (see 4.4) |
| Version | `proto_version` handshake field; API and agent ship from the same package version, a mismatch is rejected |
| Traceability | every operation carries a request ID that appears in the API's audit log and in the agent's journal |
| Limits | timeout per operation, bounded concurrency, destructive storage operations serialized |

Alternatives: D-Bus (zbus) would bring introspection and polkit, but costs
considerably more complexity for a single client. Varlink (increasingly used
by systemd) is interesting, but still rare in Rust. gRPC is oversized for a
local channel. A JSON protocol modelled with serde enums is fully typed and
simple to audit.

### 3.3 Agent ↔ Linux

- **Programs** only through `zendiloos-exec`: absolute paths from a fixed
  list in the agent configuration, no shell, arguments as a list, empty
  environment, no stdin except for secrets (passwords never on the command
  line), timeout, output limit.
- **Files** only at fixed locations known to the agent code, written
  atomically (temporary file, `fsync`, `rename`), for example
  `/etc/samba/zendiloos.conf`, `/etc/exports.d/zendiloos.exports`,
  `/etc/systemd/network/50-zendiloos-*.network`.
- **Mounts** through generated systemd mount units that the agent asks
  systemd to start. The mount then lives in the host's namespace (the
  hardened agent has its own), dependencies on Samba/NFS are explicit, and
  mounting at boot needs no `fstab` edits.
- **systemd** through `systemctl` with fixed arguments (`reboot`,
  `poweroff`). **[Later]** D-Bus (zbus) once operations need more than
  queueing a job, such as following a unit's state.
- **Interpretation happens unprivileged.** The agent returns raw tool output;
  parsing happens in `zendiloosd`. Exceptions are safety checks the agent
  must make itself (is this device the system disk? is it mounted?); for
  those it reads `/sys` and `/proc/self/mountinfo` directly and as narrowly
  as possible.

### 3.4 zendiloosd ↔ Linux (without the agent)

Read-only and unprivileged: `/proc` and `/sys` for metrics (`sysinfo` for
CPU, memory, processes, sensors; own parsers for `/proc/mdstat` and
`/proc/diskstats`), the journal for logs (group `systemd-journal`).

---

## 4. Security and Permission Model

### 4.1 Threat Model

| Attacker | Goal | Main countermeasures |
|---|---|---|
| Device on the LAN without an account | log in, take over during initial setup | throttling, setup code, no default password, TLS |
| Signed-in user without admin rights | other users' files, privilege escalation | server-side permission checks, file access with the user's own kernel permissions |
| Web page in the same browser | CSRF, WebSocket hijacking | `SameSite=Strict`, CSRF header, origin check including the WebSocket upgrade, strict CSP |
| Compromised `zendiloosd` | root | the agent offers only validated domain operations, no generic execution; system disk protected |
| Malicious container | host | Docker access only through agent policy (see 4.9) |

### 4.2 Accounts

**Decided: unified accounts.** One account is valid for the web interface,
SMB, the file manager, and later NFS and apps. "Admin" is a role, not a
separate account.

- The database is authoritative (role, groups, Argon2id hash for the web
  login, preferences).
- For every account the agent creates a Linux account: UID from 2000, group
  `zendiloos-users`, shell `nologin`, no home directory, locked Linux
  password. Files on shares therefore belong to the right user.
- When a password is set, Samba receives its own hash (`smbpasswd` via
  stdin). The plaintext password exists only during that request.
- The agent refuses any change to accounts outside `zendiloos-users`. On an
  existing Debian system, system accounts and the owner's own user (UID 1000)
  stay untouched.

Alternative: separate web accounts and share accounts. Simpler to build, but
unexpected for users, and the file manager would still need a mapping from
web account to file permissions.

### 4.3 Roles and Permissions

Handlers ask for a **permission**, never for a role. Roles are fixed and
bundle permissions, so the model can change in one place.

| Permission | admin | user | readonly |
|---|:-:|:-:|:-:|
| `system.view` (dashboard, information) | ✓ | ✓ | ✓ |
| `system.manage` (settings, power, updates) | ✓ | | |
| `logs.view` | ✓ | | ✓ |
| `users.manage` | ✓ | | |
| `storage.view` | ✓ | | ✓ |
| `storage.manage` (create/destroy pools, wipe disks) | ✓ | | |
| `shares.manage` | ✓ | | |
| `files.read` (own shares in the file manager) | ✓ | ✓ | ✓ |
| `files.write` (change files in own shares) | ✓ | ✓ | |
| `containers.manage` | ✓ | | |
| `account.self` (own password, sessions, preferences) | ✓ | ✓ | ✓ |

The frontend hides apps the user lacks the permission for; that is a
convenience. Enforcement happens only in the backend (axum extractor
`RequirePermission<P>`).

### 4.4 Share and File Permissions

- Share permissions (per user/group: no access, read, read+write) live in
  the database and are applied twice: in the Samba configuration
  (`valid users`, `read list`, `write list`) and as POSIX ACLs on the share
  directory. They therefore also apply to the file manager and NFS.
- **File manager: the kernel checks, not our code.** `zendiloosd` cannot
  read shares itself. For every access it asks the agent to open the file
  *as the signed-in user*: the agent switches to that user's UID/GID for
  this call only (`setfsuid`/`setfsgid`), opens relative to the share root
  with `openat2(RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS)` and passes the
  descriptor back via `SCM_RIGHTS`. `zendiloosd` then streams the data
  directly. The kernel prevents path traversal and symlink escapes, new
  files get the right owner, and uploads/downloads do not pass through the
  root process. **[Assumption]** to be confirmed with a prototype in phase 3.
- Alternative: the file manager runs with broad rights and checks ACLs
  itself. Simpler, but every gap in the checks becomes a data leak, and new
  files belong to the service account.
- NFS (v3/v4 with `sec=sys`) trusts the client's UID. Exports are therefore
  restricted to hosts/networks, and the interface states the risk.
  **[Later]** Kerberos.

### 4.5 Login, Sessions, CSRF

- Passwords: Argon2id (19 MiB, 2 passes). At least 10 characters, at most
  1024 bytes, must not contain the account name; no composition rules.
  **[Later]** check against a list of common passwords. Unknown account
  names cost as much time as wrong passwords, so response times do not
  reveal which accounts exist.
- Sessions: 256-bit random token in a cookie (`HttpOnly`, `SameSite=Strict`,
  `Path=/`; with secure cookies also `Secure` and the `__Host-` prefix). The
  cookie has no `Max-Age` and ends with the browser session. The database
  stores only the token's SHA-256 hash. Sessions end after 30 minutes idle
  and 12 hours at most; a password change ends all other sessions.
  **[Later]** users list and end their sessions.
- CSRF: per-session token in the `X-CSRF-Token` header on every
  state-changing request, compared in constant time. In addition,
  `Sec-Fetch-Site` must be `same-origin` and `Origin` must match `Host` when
  the browser sends them; login and setup check this too, so a foreign page
  cannot sign a victim into an attacker's account. The WebSocket upgrade
  will get the same checks.
- Throttling in the backend, in memory: 5 failures per account and 20 per
  address within 15 minutes, for passwords and setup codes alike. Plus
  `limit_req` in nginx **[Step 7]**.
- **Initial setup:** as long as no admin exists, the interface shows a setup
  wizard. It requires a one-time **setup code**, stored in
  `/var/lib/zendiloos/setup-code` (0600) and logged to the journal as a
  warning; showing it on the console (HDMI/serial) follows with the
  packaging **[Step 7]**. Nobody on the LAN can complete the setup before
  the owner. The first administrator is created atomically, so two
  concurrent setups cannot both succeed.
- **[Later]** TOTP second factor, passkeys.

### 4.6 Agent Hardening

- A closed set of operations; no "run this command", no paths from the API
  that the agent does not check again against fixed roots.
- Validated types are the only form in which values reach a command line
  (`DiskName` accepts only `sda`, `nvme0n1` and the like: no paths, no `-`
  prefix).
- `Secret` has a redacted `Debug`/`Display` and is never logged.
- Every operation is logged to the journal with request ID, peer UID/PID and
  result.
- systemd hardening: `PrivateNetwork`, `ProtectSystem=strict` with explicit
  `ReadWritePaths`, `ProtectHome`, `CapabilityBoundingSet` limited to what
  is needed, `SystemCallFilter`, `NoNewPrivileges` where possible. New
  operations extend this deliberately.

### 4.7 Storage Guard Rails

Destructive operations (wiping disks, creating/destroying pools, reshaping
RAID) go through:

1. **Plan:** the API produces a plan ("these disks will be wiped: model,
   serial number, size") and a confirmation token bound to exactly these
   serial numbers and `/dev/disk/by-id` paths.
2. **Confirm:** the user confirms (for destruction also by typing the pool
   name). If a different disk has been inserted in the meantime, the token
   no longer matches.
3. **The agent checks again, independently of the API:** no device of the
   system disk (root, `/boot`, EFI, swap), not mounted, not a member of an
   active array; opening exclusively (`O_EXCL`) must succeed.
4. **Development mode:** the agent configuration defaults to
   `destructive_operations = "deny"`. For testing, individual devices are
   allowed (`allowed_devices = ["/dev/disk/by-id/virtio-zdl-test-1", …]`).
   On a real machine with data, the agent stays harmless.

Destructive features are developed and tested only against `zendiloos-sim`,
loop devices and virtual disks in a Debian VM.

### 4.8 Audit and Log Hygiene

- Audit table (append-only from the API's point of view): time, user, source
  IP, action, target, outcome, request ID. Includes logins and failed
  attempts. Every context writes it through the shared `AuditLog` port of
  the kernel. Actions that may end the process (reboot, power off) are
  recorded before they are carried out, and a failure is recorded after.
  The request ID also appears in the agent's journal.
- Structured logs through `tracing` (journald in production, readable during
  development). Passwords, tokens and cookies never appear in logs; the types
  ensure this, not discipline.

### 4.9 Containers **[Phase 4]**

Access to the Docker socket is equivalent to root. Recommendation:
`zendiloosd` does not talk to Docker directly but through agent operations
(bollard in the agent) that enforce a policy: no `privileged` containers, no
host PID/network, bind mounts only below shared paths; exceptions only
explicitly per app by an admin. Logs and statistics arrive as a frame
stream. Final decision in phase 4.

---

## 5. Storage Concept

| Term | Implementation |
|---|---|
| Disk | detected with `lsblk --json`, identified by serial number and `/dev/disk/by-id`, never by `sdX` |
| Pool | redundancy: mdadm array (RAID 1, 5, 6, 10) or a single disk ("basic") |
| Volume | filesystem in a pool: **Btrfs** (default) or ext4; mount unit at `/srv/zendiloos/<volume>` **[Assumption]**; layer in between see below |
| Share | Btrfs subvolume (snapshots per share, quotas via Btrfs simple quotas) or a directory on ext4 |
| Monitoring | `/proc/mdstat`, `mdadm --detail`, Btrfs device statistics, SMART (`smartctl --json`, `nvme`), periodic scrub |
| ZFS | optional module from phase 4 (`zfs-dkms` from contrib) |

| Option | Pros | Cons |
|---|---|---|
| **mdadm + Btrfs** (default) | in-kernel, extremely well tested, all RAID levels, growth by reshape; Btrfs adds checksums, snapshots, subvolumes | Btrfs detects bit rot but cannot repair it on top of mdadm |
| Btrfs native RAID | self-healing | RAID 5/6 has known problems (write hole); two mechanisms in the interface |
| ZFS | self-healing, mature | out-of-tree module (DKMS) that can break on kernel updates; licensing questions for prebuilt modules in the ISO |

**[Open] Layer between RAID and filesystem.** Must be decided before the
first real pool (phase 3), because the on-disk layout cannot be changed
later.

| Variant | Pros | Cons |
|---|---|---|
| mdadm → filesystem (one volume per pool) | simplest chain, few failure modes | one pool = one volume; mixed disk sizes only with wasted space |
| **mdadm → LVM (thick) → filesystem** (recommended) | several volumes per pool, growing volumes; later several arrays in one volume group to use mixed disk sizes | one more layer for the agent to manage and monitor |
| mdadm → LVM thin → filesystem | overcommit, block-level snapshots | thin metadata is a failure source of its own; full thin pools are dangerous; Btrfs already has snapshots |

From phase 3 the domain model distinguishes between **pool** (redundancy)
and **volume** (filesystem), even if the first version creates only one
volume per pool.

---

## 6. Further Decisions

| Area | Decision | Alternatives / rationale |
|---|---|---|
| Base | Debian 13 "trixie" (kernel 6.12 LTS), amd64; arm64 from phase 5 | current stable; backports kernel for new hardware if needed |
| Target hardware | x86-64 NAS appliances with Intel CPUs and generic x86-64 systems with UEFI (self-built, mini PCs) | phase 5: boot medium, LEDs and fans (often via I²C), Intel GPU drivers |
| Development environment | Nix devShell (`flake.nix`, `.envrc`) for tooling on the development machine only; toolchain version from `rust-toolchain.toml` | the product contains no Nix; packages and images are built in a `debian:trixie` container |
| Database | SQLite (WAL) with sqlx; compile-time checked queries (`query!`), offline data in `backend/.sqlx` | after changing SQL or migrations run `just sqlx-prepare`; `just check` fails if the data is stale |
| Migrations | forward only, embedded, transactional; before each migration a copy of the database in `/var/lib/zendiloos/backup/` | safe migrations without down scripts |
| Configuration | static: `/etc/zendiloos/*.toml` (listen address, paths, log level, development flags); everything the admin manages lives in the database | |
| System metrics | `sysinfo` for CPU, memory, processes, sensors; own parsers for `/proc/mdstat` and `/proc/diskstats` | sysinfo does not cover storage specifics |
| Network | systemd-networkd + systemd-resolved **[Phase 4]**; on an existing Debian with ifupdown/NetworkManager display only | declarative files the agent writes atomically |
| Docker | Debian packages `docker.io` and `docker-compose`, apps as Compose projects **[Phase 4]** | security updates through Debian; Docker CE optional |
| Packaging | `debian/` with debhelper (compat 13 plus `dh-sequence-installsysusers`), built in a `debian:trixie` container with Rust from `rustup` and Node.js 24 pinned by checksum | native maintainer scripts, `sysusers.d`, `tmpfiles.d`, `dh_installsystemd`; works on any host with Docker |
| Test VM | `scripts/vm.sh` (`just vm …`): Debian 13 cloud image verified by SHA512, cloud-init, 4 SATA disks with emulated SMART and 1 NVMe disk, ports on 127.0.0.1 only | the cloud kernel lacks some drivers; storage work (M3) may need `linux-image-amd64` in the VM **[Assumption]** |
| Reproducibility | `rust-toolchain.toml`, `Cargo.lock`, `package-lock.json`, `SOURCE_DATE_EPOCH`, pinned build container; system images built with mmdebstrap against a fixed snapshot.debian.org state, failing on any error | |
| Licensing | Zendiloos code under Apache-2.0; the ISO build produces a package manifest/SBOM and ships the copyright files | required when distributing a Debian-based image |
| Quality | rustfmt, Clippy (`-D warnings` in CI), `unsafe_code = "forbid"` outside clearly bounded exceptions, `cargo deny`, ESLint, `vue-tsc --strict`, Vitest | |
| Installation and updates | **[Phase 5]** recommendation: root image built with mmdebstrap as a signed image, A/B partitions, updates with systemd-sysupdate, automatic rollback via systemd-boot boot counting, data and `/var` on separate partitions | alternatives: Debian installer with preseeding (apt based, not atomic), live-build with Calamares, mkosi. Decided in phase 5; the ground rule in 1.2 keeps all options open |

---

## 7. Open Questions

Decided on 2026-11-05: name `zendiloos`, unified accounts, Nix devShell for
tooling only, target hardware x86-64 NAS appliances and generic x86-64.
HTTPS by default: plain HTTP only redirects, the session cookie is
HTTPS-only, a self-signed certificate is created on the device; installing
an own certificate and ACME follow later.

1. Mount path of volumes (`/srv/zendiloos/<volume>` or something shorter
   such as `/volume1`).
2. CI service. The repository is on GitHub (https://github.com/mavnezz/zendiloOS);
   recommendation: GitHub Actions running `just check` and `just deb`.
3. LVM between mdadm and filesystem (section 5); at the latest at the start
   of phase 3.
