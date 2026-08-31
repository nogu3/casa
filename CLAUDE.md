# CLAUDE.md

`casa` — a cross-protocol smart-home client. It calls protocol-specific CLIs (`enl`, etc.) as subprocesses and provides name mapping and a unified UX.

> Name: **`casa`** — decided.
> Repository: **public / standalone repository**.
> Config file: **managed in a separate repository** (not included in this repository).

---

## Project purpose and positioning

Smart-home targets span multiple protocols — ECHONET Lite / SwitchBot / Matter. The core idea is **not to merge these into a single binary**, but to implement each protocol as a thin CLI (`enl`, etc.) and to **layer a thin cross-cutting UX on top**. casa is that cross-cutting layer.

### casa's responsibilities
- Resolve human-friendly names → (protocol, address, object)
- Load and validate the config file
- Provide a consistent wrapper UX over protocol-specific CLIs

### Not casa's responsibilities
- Implementing protocols. It does not assemble byte sequences, does not send UDP, and does not carry a Matter stack. **Everything is delegated to child processes.**
- Scheduling, running long-running / resident, holding state.
- Owning the config file itself. The config is managed separately by the user (see below).

---

## Sibling CLI naming convention

Protocol-specific CLIs that are meant to be called by casa follow this policy:

- **For protocols that have an official CLI, adopt the official name as-is** (defer to the official name over the naming convention).
- **Self-authored CLIs use a short name based on the protocol's acronym** (`enl`, etc.).

| Protocol | CLI name | Status |
|---|---|---|
| ECHONET Lite | `enl` | Self-authored, in development (standalone repository) |
| SwitchBot | `swb` | Self-authored (cloud API v1.1 wrapper, standalone repository). Originally planned around the official CLI (`@switchbot/openapi-cli`), but `swb` unblocked the work first and is what casa's adapter actually calls. `swb` also has a BLE passive-scan plane (`scan`); casa deliberately does **not** integrate it (see the SwitchBot entry under Phase 4). |
| Matter | `mat` | Self-authored CLI (chip-tool wrapper, standalone repository). casa adapter supported. `mtr` is not used because it collides with the existing network diagnostic tool. |
| Android TV | `atv` | Self-authored CLI (Android TV Remote protocol v2 client, standalone repository). casa adapter supported. Deliberately minimal scope: `pair` / `status` / idempotent `on` / `off`. |

casa assumes **these exist on `PATH`**.

---

## Design principles that must always be upheld

1. **Do not speak protocols directly**
   Do not bring byte sequences, sockets, or protocol stacks into casa. If you feel the urge to, that is a sign you should build a new sibling CLI.
2. **stdout is pure structured JSON only**
   Parse the child CLI's output, normalize it into casa's schema, and re-emit it. Do not mix in human-oriented decoration.
3. **Diagnostics go to stderr as structured logs** (`tracing`)
   Do not swallow the child CLI's stderr either; keep it at least at debug level.
4. **Hold no state other than the config file**
   No cache DB, no daemon, no internal scheduler.

---

## Config file

### Location and ownership
- Default path: `$XDG_CONFIG_HOME/casa/devices.toml` (default `~/.config/casa/devices.toml`).
- The path can be overridden with `--config <path>` and the environment variable `CASA_CONFIG`.
- **The config file itself is not managed in the casa repository.** The user keeps it in a separate repository (assumed private) and places it in `~/.config/casa/` or symlinks it there.

### Format: TOML

```toml
version = 1

[devices.living_aircon]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x013001"

[devices.bedroom_light]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029101"

[devices.entry_lock]
protocol = "switchbot"
device_id = "DUMMY-XX-XX"
```

- Names (keys) are recommended to be snake_case.
- `protocol` determines which child CLI casa calls.
- Protocol-specific fields are passed through to the child CLI's arguments as-is.

### Samples and tests in the repository
- Samples included in this repository **must use dummy values only** (RFC 5737 `192.0.2.0/24`, etc.).
- Do not include real IPs, real MACs, or real device IDs in the repository (it is public).

### Migration
- Distinguished by the `version` field.
- **No automatic migration** (run it via an explicit command). The config file is the user's property, so do not silently rewrite it.

---

## Tech stack

| Area | Choice | Notes |
|---|---|---|
| Language | Rust | Same as enl |
| CLI | `clap` (derive) | |
| Subprocess | `std::process::Command` | Zero dependencies |
| Config parsing | `toml` crate | |
| JSON | `serde` + `serde_json` | Parse child CLI output |
| Logging | `tracing` + `tracing-subscriber` | stderr output |

Keep dependencies minimal. casa's core is spawning external processes and parsing JSON, so avoid excessive hand-rolling.

---

## Architecture

```
User / cron / n8n / other orchestrator
        │
        ▼
      casa  ◄── devices.toml (managed separately)
        │
        │  Command::new("enl") / "swb" / "mat"
        ▼
   protocol-specific CLI (stdout = JSON)
        │
        ▼
   real device (UDP / BLE / IP / cloud API)
```

### Resolving the child CLI binary
- Default: resolve `enl` / `swb` / `mat` / `atv` from `PATH`.
- Override: a full path can be specified via an environment variable (`CASA_ENL_BIN`, etc.) or the config file.
- Startup failure (binary missing / not executable) must be immediately distinguishable via a dedicated exit code.

### Version compatibility with the child CLI
- The coupling is **the stdout JSON schema only**. No crate dependency = no need to track SemVer.
- If a child CLI's schema makes a breaking change, absorb it on the casa side.
- Document the **minimum assumed child CLI version** in the README.

---

## Conventions

### stdout
- On success, emit the result data as JSON to stdout.
- Do not pass the child CLI's output through verbatim; **reconstruct it in casa's schema** (the protocol-abstraction responsibility).
- The **`timestamp` field is required** (ISO 8601, the time casa assembled the response). Upper layers (resident processes, caches) can use it for freshness determination.
- Example:
  ```json
  {
    "timestamp": "2026-06-02T12:34:56+09:00",
    "device": "living_aircon",
    "protocol": "echonet",
    "value": { "power": "on" }
  }
  ```

### Verb promotion criteria and invoke

A dedicated subcommand is added to casa only for operations that "have the same meaning across two or more protocols, or are used with high daily frequency." All other long-tail, protocol-specific operations are expressed via `casa invoke <name> <command> [args...]` (name resolution + address-flag injection + argument pass-through; `command` uses the child CLI's vocabulary directly). Group execution of invoke is allowed only when all members share the same protocol. casa's global flags go before invoke.

### stderr
- Child CLI errors are sent to stderr as structured logs.
- casa's own errors use the same shape: `{"error": {"kind": "...", "detail": "..."}}`.
- `kind` examples: `config_missing` / `config_parse` / `name_not_found` / `child_not_found` / `child_failed`.

### exit code
| code | meaning |
|---|---|
| 0 | Success |
| 2 | CLI argument error (clap default) |
| 10 | Config file missing / parse failure |
| 11 | Name not present in the config file |
| 12 | Child CLI binary not found / not executable |
| 13 | Child CLI's stdout cannot be parsed as JSON |
| 14 | Operation not supported for that protocol |
| 15 | In group execution, some (or all) members failed |
| other | **Propagate the child CLI's exit code as-is** |

By preserving the original code for errors originating from the child CLI, the caller can distinguish "timeout vs. rejection" and so on.

---

## Put use cases that need long-running / stateful behavior outside casa

In the future, requirements such as a self-built web page, calls from an LLM, subscription to state changes, and caching will arise. These are **not added to casa (the bin)**. Place another layer, `casad`, on top of casa to absorb them.

> **Important (actual state)**: `casad` is already implemented as a **separate crate in the same workspace (`crates/casad`), a separate binary**. The boundary to uphold is not "repository" but "**process / state**" —— casa(bin) stays stateless, while casad holds the long-running behavior, state, and scheduler. It is the same relationship as ssh and sshd being separate binaries in the same OpenSSH repository. Pure logic such as config loading and name resolution is shared by both via `casa-core`(lib), and real-device actions are performed by casad calling casa as a child process (hybrid).

```
Web page / LLM / other client
       │
       ▼
   casad (long-running, holds state. crates/casad, separate process)
       │
       │ spawns a process (calls casa as a CLI)
       ▼
   casa (stays stateless. crates/casa)
       │
       ▼
   enl / swb / mat
```

### Reasons to uphold this separation
- casa can be invoked equivalently from cron, from n8n, and from `casad`. Even if the resident process is down, you can still debug.
- `casad` can later be written in another language (TypeScript, etc.). For LLM work, the TS ecosystem is rich, making this a realistic option.
- `casad` can be thrown away and rebuilt. Because casa is intact, the blast radius stays contained.
- It upholds the principle that "whatever holds a cache is something that runs long-running." Adding a cache to casa would cascade into state management — the first step toward becoming Home Assistant.

### Responsibilities the `casad` side takes on (not casa's responsibilities)
- Evaluation engine for the automation rule DSL (`rules.toml`) — **implemented**:
  - Time triggers (internal scheduler) / event triggers (run `enl listen` in a
    one-shot loop for ECHONET INF notifications, and hold one unbounded
    `mat listen --count 0` stream (mat 1.5.0+) for Matter attribute changes via
    matd's resident Subscribe — respawning per event drops burst tails from
    matd's broadcast, losing `recovered` events; `priming: true` current-state
    redeliveries never fire rules, `recovered` events fire normally)
  - Per-rule active window (`active = { from = "HH:MM", to = "HH:MM" }`): a rule only fires
    inside its window. Half-open (`from` inclusive, `to` exclusive), `from` > `to` wraps over
    midnight, `from` == `to` is a config error, and omitting it means always in effect. It
    applies to every trigger kind.
  - Firing is dispatched asynchronously to per-device workers (FIFO per device, parallel across devices).
    `enl listen` does not stop even while an action is running. `--once` / `--listen-once` run synchronously.
  - Worker send policy (`[settings]`, optional): an **event-triggered** `off` is held for
    `off_grace_secs` (default 30) and discarded if an `on` for the same device arrives inside the
    window — a rapid off→on pair never reaches the device (mitigates cheap-plug firmware latch-up,
    issue #5). Time-triggered `off`s fire immediately. `min_gap_secs` (default 2) is the minimum
    interval between consecutive commands to one device. `--once` / `--listen-once` paths bypass
    the workers, so neither applies there.
  - Off no-op skip (issue #3): a Matter `off` is skipped when the resident `mat listen` stream
    recently (10 min TTL) observed the target's OnOff state as already off — Thread-traffic
    optimization. **`on` is never skipped** (firmware can latch `on` in reports while physically
    off, issue #5; trusting that would leave a light dark). Skips are DEBUG-logged; `--once`
    paths bypass this too.
  - On firing, casa is called as a child process (`casad run` / `check`). `then` supports `on` / `off` / `invoke`
    (`invoke` takes `device` / `command` / arbitrary `args` and delegates to `casa invoke`). `then` accepts either a
    single table or an array of actions; array members are dispatched per target device (same device = declaration
    order, different devices = parallel), and a failing action does not stop the rest.
- Subscription to ECHONET INF notifications (via `enl listen`; the enl side is designed so "listen is driven from an external loop") — **implemented**
- Subscription to Matter attribute changes (via `mat listen`, a thin client to
  `matd`'s resident Subscribe; requires a running `matd`) — **implemented**
- HTTP / WebSocket / MCP server — not implemented (future)
- Value caching and freshness management — not implemented (future)
- Endpoint for Function Calling from an LLM — not implemented (future; rules.toml is assumed to be LLM / UI generated)
- Authentication / authorization / rate limiting — not implemented (future)

### What the casa side must uphold for `casad` (already satisfied)
- The config file path can be passed via `--config <path>` (do not read `$XDG_CONFIG_HOME` every time).
- Always include `timestamp` in the stdout JSON (usable for cache decisions).
- Propagate the exit code from the child CLI (upper layers can make retry decisions).

`casad` is implemented in this workspace at `crates/casad` (it is not made a separate repository). As long as casa(bin)'s stateless principle is not broken, feature expansion of casad may proceed within this repository.

---

## Roadmap

A reference for when Claude Code implements casa. Proceed through the phases **in order**. Do not move to the next phase until the previous phase is fully done (all tests pass, acceptance criteria met).

Each phase defines the following:
- **Goal**: what gets built in that phase.
- **Scope**: what to do in this phase.
- **Out of scope**: things not to do in this phase, however small they seem.
- **Acceptance criteria**: clear acceptance standards.

---

### Phase 0 — Project skeleton

**Goal**: Make a Rust project buildable that can only read the config file and list devices. It does not call child CLIs yet.

**Scope**:
- A Cargo project with `clap`(derive), `serde`, `serde_json`, `toml`, `tracing`, `tracing-subscriber`.
- A single subcommand: a CLI skeleton with just `casa list`.
- Config loader: read TOML from the default path / `--config` / `CASA_CONFIG`.
- Config validation (required fields per protocol; unknown protocols are an error).
- `casa list` emits all devices as JSON to stdout.
- `tracing` logs go to stderr. The level is controlled by `RUST_LOG`.
- exit codes `0` / `2` / `10` / `11` work per the conventions.

**Out of scope**:
- Calling child CLIs.
- `get` / `set` / `describe` / `on` / `off`.

**Acceptance criteria**:
- `cargo build`, `cargo test`, `cargo clippy -- -D warnings` all pass.
- Unit tests for config parsing are in place: happy path, file missing, invalid TOML, unknown protocol, missing required field.
- With a dummy config (`192.0.2.0/24`), `casa list` emits correct JSON.
- Starting with no config file exits with exit code `10` and a structured error on stderr.

**No dependency on enl.** This phase can be completed and shipped even if enl is unfinished.

---

### Phase 1 — enl integration (get / set)

**Goal**: casa can read and write ECHONET Lite devices by name. The actual work is a subprocess call to `enl`.

**Prerequisite**: `enl get` and `enl set` have shipped, emitting stable JSON to stdout. Exit codes also follow the conventions in enl's CLAUDE.md.

**Scope**:
- A "child runner" module: takes a binary name and arguments, launches it, captures stdout/stderr, and returns either JSON or an error.
- The child binary is resolved from `PATH`. The full path can be overridden via `CASA_ENL_BIN` or the config file.
- `casa get <name> <epc>`:
  - Resolve `<name>` to (IP, EOJ) from the config.
  - Call `enl get --ip <ip> --eoj <eoj> --epc <epc>` (final flag names to match enl's shipped release).
  - Reshape enl's JSON into casa's schema and emit it to stdout.
- `casa set <name> <epc> <value>`: the same.
- exit code propagation: if enl exits with `3` (timeout) or `4` (device rejection), casa exits with the same code. casa's own errors are `10` / `11` / `12`.
- Do not swallow the child's stderr; forward it to casa's stderr at debug level.

**Out of scope**:
- SwitchBot, Matter, and other protocols.
- introspection (`describe`).
- ON/OFF shortcuts.

**Acceptance criteria**:
- `cargo test` includes an integration test using a **dummy `enl` binary** (a script or test helper that emits fixed JSON). Real enl is not needed in CI.
- Manual E2E tests against real devices are documented in the README (not run in CI).
- The `kind` values for stderr errors are stable and documented.

---

### Phase 2 — Introspection and shortcuts

**Goal**: Make casa pleasant for daily use.

**Prerequisite**: `enl describe` (property map introspection) has shipped.

**Scope**:
- `casa describe <name>`: call the child CLI's introspection (a property map for enl).
- `casa on <name>` / `casa off <name>`: shortcuts for high-frequency operations. For ECHONET Lite, map EPC `0x80` to `0x30`/`0x31`. The mapping table is hardcoded inside casa per protocol (this is UX, not protocol logic, so it is OK).
- Extend `casa list` so it can optionally include the latest property map obtained during that session (**do not add a persistent cache**).

**Out of scope**:
- A persistent cache or DB.
- SwitchBot/Matter support.

**Acceptance criteria**:
- `cargo test` covers the ECHONET Lite ON/OFF mapping.
- The README documents the `on`/`off` support status and mapping targets for each protocol.

---

### Phase 3 — Multi-protocol enablement (refactor only)

**Goal**: Reach a state where a second protocol can be added without a rewrite. No new protocol is added yet.

**Scope**:
- Refactor the child runner and subcommand handlers so that adding a new protocol requires only the following:
  1. Add a variant to the protocol enum.
  2. Add an adapter that builds that protocol's CLI arguments.
  3. Add tests for the adapter.
- The config's `protocol` field is the single source of truth for dispatch.

**Out of scope**:
- Actually adding SwitchBot or Matter (only once the self-authored `swb` / `mat`, etc. are ready).

**Acceptance criteria**:
- An adapter trait or function table clearly exists.
- Adding a virtual adapter in tests can be done without touching the subcommand handlers at all.

---

### Phase 4 onward — Adding real protocols

Each addition is one Phase 3-style adapter. Subcommands and the config schema do not change (other than a new value for `protocol`).

- **SwitchBot**: **Supported (cloud control plane only)**. Add an adapter that calls the self-authored `swb` (a SwitchBot cloud API v1.1 wrapper) — **not the official CLI**; `swb` unblocked the work before an official-CLI-based adapter was written, so it is what casa actually dispatches to. Authentication (`SWITCHBOT_TOKEN` / `SWITCHBOT_SECRET`) is handled entirely by `swb` via inherited environment variables; casa passes no credentials of its own. `on`/`off` map to `swb cmd <device_id> turnOn`/`turnOff`. The cloud API has no single-property read/write, so `get`/`set`/`describe` remain unsupported (trait default `None` → exit 14); reading state instead goes through `casa invoke <name> status` (`swb status <device_id>`), and `casa invoke <name> cmd <command> [args...]` sends an arbitrary SwitchBot cloud command (`swb cmd <device_id> <command> [args...]`, `device_id` injected as the positional argument right after the subcommand). `swb`'s BLE passive-scan plane (`scan`) is a separate, fully-local plane that casa **deliberately does not integrate** (decided 2026-07-19). BLE scanning is address-less ambient sensor reception, not the "resolve a name → operate one device" model casa is built on, and its JSONL-stream / `--follow`-resident output contradicts casa's single-shot, stateless, single-JSON contract. casa's SwitchBot support is the cloud control plane only; ambient BLE sensor collection, if ever needed, is a job for casad (resident subscription/cache) or a downstream pipe (`swb scan | jq`) — not casa(bin). This mirrors the existing stance that casa does not wrap `discover`.
- **Android TV**: **Supported**. Add an adapter that calls the self-authored `atv` (an Android TV Remote protocol v2 client; driven by the living-room REGZA X8900K, which speaks Remote v2 but not ECHONET Lite). In the config, a device is addressed by `host` (IP; atv does no name resolution). The address is injected as `--host` right after the subcommand: `on`/`off` map to `atv on/off --host <host>` (idempotent on the atv side — it reads the power state over the session and sends the power key only when the state differs), and reading state goes through `casa invoke <name> status`. Remote v2 has no single-property read/write or introspection, so `get`/`set`/`describe` remain unsupported (trait default `None` → exit 14). The one-time `atv pair` (reads the on-screen code from stdin) is interactive and is run directly, not through casa — though `casa invoke <name> pair` passes through and works. Pairing credentials (`~/.config/atv/`) are entirely atv's concern; casa passes nothing.
- **Matter**: **Supported**. Add an adapter that calls the self-authored `mat` (a chip-tool wrapper). Because Matter addresses by (node_id, endpoint, cluster, attribute), casa's single selector `<epc>` is interpreted as `endpoint/cluster/attribute` and assigned to `mat read`/`write`. `on`/`off` invoke the OnOff command (`mat on`/`off`). In the config, a Matter device is addressed by either `node_id` (unicast) or `group` (Matter wire groupcast) — exactly one is required — with `endpoint` optional. Only one variant is added to the Phase 3 adapter trait; the subcommand handlers are unchanged. A `group` device delegates `on`/`off`/`invoke` to `mat group ...` (multicast); `get`/`set`/`describe` are unsupported (groupcast is unacknowledged, exit 14).

---

### Explicitly deferred (do not implement without discussion)

These are **not implemented in casa itself**. When they become necessary, they are the responsibility of the upper layer (`casad`).

- **Cache / local DB.** Handle it with `casad`'s in-memory cache. Do not add a file cache to casa.
- **Automatic migration of the config file.** Explicit command only; automatic is not allowed.
- **Discovery.** The operational approach is to call `enl discover` directly. casa does not wrap it.
- **Monitoring state changes (waiting for INF notifications).** `casad`'s responsibility (implemented: run `enl listen` in a loop). casa(bin) holds no subscription.
- **Daemon / resident mode.** Not implemented in casa(bin). The long-running behavior is handled by the separate crate `casad` (implemented).
- **HTTP / WebSocket / MCP server.** `casad`'s responsibility.
- **Endpoint for LLM Function Calling.** `casad`'s responsibility.

---

## Things not to do

- Do not assemble/parse protocol byte sequences inside casa.
- Do not add a crate dependency on child CLIs (always subprocess).
- Do not add daemonization, long-running/resident behavior, or an internal scheduler to casa(bin) (those are handled by the separate crate `casad` in the same workspace).
- Do not add a cache or DB (`casad`'s responsibility).
- Do not embed an HTTP / WebSocket / MCP server into casa (`casad`'s responsibility).
- Do not commit real config files or real topology into this repository.

---

## Development commands

```bash
cargo build
cargo test
cargo clippy -- -D warnings
RUST_LOG=debug cargo run -- list
```
