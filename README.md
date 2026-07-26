# casa

A cross-protocol smart-home client. It calls protocol-specific CLIs (`enl`, etc.)
as subprocesses and provides human-friendly name mapping and a unified CLI UX.

casa does not speak any protocol directly. It holds no byte strings or sockets;
everything is delegated to sibling CLIs. See [CLAUDE.md](CLAUDE.md) for the full
design overview.

> **Breaking change (v0.6.0)**: `casa color-temp` was removed.
> Use `casa invoke <name> color-temp --kelvin 2700` instead (see the "Verb promotion criteria" section).

## Usage

```bash
# List configured devices
casa list

# Read a property (for ECHONET Lite, specify an EPC. Calls enl as a subprocess)
casa get living_aircon 0x80

# Write a property
casa set living_aircon 0x80 0x30

# Power ON / OFF shortcuts
casa on living_aircon
casa off living_aircon

# Group operation (defined under [groups]. Runs on all members in parallel at once)
casa on living
casa off living

# Call a protocol-specific CLI command with name resolution (the general-purpose verb for long-tail operations).
# Everything after <command> is passed straight through to the child CLI. Put casa's flags (--config etc.) before invoke.
casa invoke living_light color-temp --kelvin 2700
casa invoke living_light color-temp --mireds 370 --transition 30

# Property map (introspection)
casa describe living_aircon

# Include each device's property map in the listing (fetched on the spot; no persistent cache)
casa list --describe

# Validate the config file (does not call real devices). Warns about protocols with no adapter yet
casa validate
```

The interpretation of the second argument `<property>` to `get` / `set` is
protocol-dependent: for ECHONET Lite it is an EPC (e.g. `0x80`), for Matter it is
`endpoint/cluster/attribute` (e.g. `1/onoff/on-off`).

`casa validate` reads the config and reports its validity as JSON (version,
required fields, and unknown protocols are already checked at load time). In
addition, it lists protocols that are valid as config but have no adapter yet —
which would fail at runtime with `protocol_unsupported` (exit 14) — under
`warnings`. All protocols currently in the `Device` enum (echonet / matter /
switchbot) have an adapter, so `warnings` is empty for any config built from
them; the field exists for the (currently hypothetical) case of a protocol
that parses but has no adapter implementation yet:

```json
{
  "timestamp": "2026-06-02T12:34:56+09:00",
  "config": "/home/you/.config/casa/devices.toml",
  "version": 1,
  "device_count": 2,
  "protocols": { "echonet": 1, "switchbot": 1 },
  "warnings": [],
  "valid": true
}
```

## Verb promotion criteria

A dedicated subcommand is added to casa only for operations that **have the same
meaning across two or more protocols, or are used frequently in daily use**
(e.g. `on` / `off` / `get` / `set` / `describe`). Any other protocol-specific
operation is expressed via `casa invoke`. casa guarantees the envelope of an
invoke response (`timestamp` / `device` / `protocol` / `command`), while `value`
stores the child CLI's JSON verbatim. `casa color-temp` (Matter-only, removed in
v0.6.0) is the textbook example of something that did not meet this criterion; it
was replaced by `casa invoke <name> color-temp --kelvin <k> | --mireds <m>`.

### Groups

You can bundle multiple devices under a single name via `[groups]` in
`devices.toml`. `on` / `off` / `set` / `invoke` accept a group name
transparently, spawning the child CLIs of all members in parallel and running
them at once (`on` / `off` / `set` allow mixed protocols). `invoke` can only run
when all members of the group share the same protocol (a mixed group is rejected
with exit 14 before spawning). `get` / `describe` do not support groups
(exit 14). The result is per-member JSON:

```json
{
  "timestamp": "2026-06-02T12:34:56+09:00",
  "group": "living",
  "results": [
    {"device": "living_light", "protocol": "matter", "ok": true, "value": {}},
    {"device": "living_aircon", "protocol": "echonet", "ok": false,
     "error": {"kind": "child_failed", "exit_code": 3, "detail": "..."}}
  ]
}
```

Exit 0 if everyone succeeds; exit 15 (`group_partial_failure`) if even one
member fails.

Because `set`'s property selector is protocol-dependent, a mixed group (e.g.
echonet + matter) sends a selector that is meaningless to one of the protocols.
It is only practical for a group of the same protocol.

stdout emits only pure structured JSON. It always includes `timestamp`
(ISO 8601).

```json
{
  "timestamp": "2026-06-02T12:34:56+09:00",
  "devices": [
    { "name": "living_aircon", "protocol": "echonet", "ip": "192.0.2.10", "eoj": "0x013001" }
  ]
}
```

`get` / `set` reshape the child CLI's output into casa's schema before emitting:

```json
{
  "timestamp": "2026-06-02T12:34:56+09:00",
  "device": "living_aircon",
  "protocol": "echonet",
  "value": { "power": "on" }
}
```

The `invoke` response uses the same envelope except that a `command` field is
added:

```json
{
  "timestamp": "2026-06-02T12:34:56+09:00",
  "device": "living_light",
  "protocol": "matter",
  "command": "color-temp",
  "value": {}
}
```

Diagnostic logs are emitted to stderr in structured form (JSON). The level is
controlled by `RUST_LOG`.

```bash
RUST_LOG=debug casa list
```

## Config file

Default path: `$XDG_CONFIG_HOME/casa/devices.toml` (default
`~/.config/casa/devices.toml`). It can be overridden with `--config <path>` or
the environment variable `CASA_CONFIG`.

The config file itself is not managed in this repository. Users keep it in a
separate repository and place it in `~/.config/casa/` or symlink it there.

Sample (dummy values only): [examples/devices.toml](examples/devices.toml)

```toml
version = 1

[devices.living_aircon]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x013001"

[devices.entry_lock]
protocol = "switchbot"
device_id = "DUMMY-XX-XX"

# A group for operating multiple devices at once (only on / off / set / invoke are supported)
[groups.living]
members = ["living_light", "living_aircon"]
```

## Child CLIs (sibling CLIs)

casa assumes that the protocol-specific CLIs exist on `PATH`.

| Protocol | CLI | Assumed minimum version | Status |
|---|---|---|---|
| ECHONET Lite | `enl` | 1.5.0 for both casa / casad (a CLI that takes the address as positional arguments; has `listen`, with a coexistence model of resident listen and one-shot 3610) | Supported |
| Matter | `mat` | 1.0.0 (`read` / `write` / `invoke` / `on` / `off` / `color-temp` / `describe`; casad event triggers additionally need `listen`, which requires a running `matd`) | Supported |
| SwitchBot | `swb` | 0.1.0 (`status` / `cmd` subcommands and exit code conventions) | Supported (self-authored, cloud API v1.1 wrapper; on / off / invoke). BLE scan plane not yet integrated. |

The enl interface that casa calls (it follows enl's shipped releases):

```
enl get <ip> <eoj> <epc>
enl set <ip> <eoj> <epc> <edt>
enl describe <ip> <eoj>
```

(enl 1.5 series. EOJ / EPC values with a `0x` prefix are normalized and accepted
on the enl side.)

The mat interface that casa calls (Matter addresses by (node_id, endpoint,
cluster, attribute)):

```
# casa get <name> <property>  property = endpoint/cluster/attribute (chip-tool notation)
mat read <node_id> <endpoint> <cluster> <attribute>
# casa set <name> <property> <value>
mat write <node_id> <endpoint> <cluster> <attribute> <value>
mat describe <node_id>
# casa invoke <name> color-temp --kelvin <k> | --mireds <m> [--transition <t>]
mat color-temp --node <node_id> [--endpoint <ep>] --kelvin <k> | --mireds <m> [--transition <t>]
```

A Matter device requires either `node_id` or `group` in the config and
optionally holds `endpoint` (for the on/off shortcut; the default is 1 on the
mat side):

```toml
[devices.living_light]
protocol = "matter"
node_id = "1234"          # identifier of a commissioned node

[devices.power_strip_outlet2]
protocol = "matter"
node_id = "5678"
endpoint = 2              # the endpoint that on/off targets
```

The `<property>` of `get` / `set` is in `endpoint/cluster/attribute` form
(e.g. `casa get living_light 1/onoff/on-off`,
`casa set living_light 1/levelcontrol/current-level 128`). casa does not
interpret this selector; it just splits it on `/` and passes it to mat, and
validity is verified on the mat (chip-tool) side.

A Matter device can specify `group` instead of `node_id` (unicast) to target a
Matter wire group (groupcast / multicast). The value of `group` is a `mat`
group alias or GroupId; casa does not interpret it and passes it straight
through to `mat group ... --group <value>` (alias→GroupId resolution is mat's
responsibility). Exactly one of `node_id` or `group` may be specified (both or
neither is a config error, exit 10).

Groupcast supports only `on` / `off` / `invoke`. Because groupcast is
unacknowledged (no response comes back), `get` / `set` / `describe` are
unsupported (exit 14).

```toml
[devices.desk_room_lights]
protocol = "matter"
group = "desk_room_lights"
```

The swb interface that casa calls (SwitchBot's cloud API has no single-property
read/write; `device_id` is injected as the first positional argument right
after the subcommand):

```
# casa on <name> / casa off <name>
swb cmd <device_id> turnOn
swb cmd <device_id> turnOff
# casa invoke <name> status  (the only way to read state; returns the full status)
swb status <device_id>
# casa invoke <name> cmd <switchbot-command> [args...]  (send an arbitrary cloud command)
swb cmd <device_id> <switchbot-command> [args...]
```

casa passes no authentication of its own; `SWITCHBOT_TOKEN` / `SWITCHBOT_SECRET`
are swb's concern and reach it via inherited environment variables on the child
process. `get` / `set` / `describe` are not supported for SwitchBot (exit 14
`protocol_unsupported`) because the SwitchBot cloud API has no single-property
read/write and swb has no property-map introspection.

### `on` / `off` support and mapping targets

The shortcut mappings are hardcoded inside casa as UX, not as protocol logic.

| Protocol | `on` | `off` | Mapping target |
|---|---|---|---|
| ECHONET Lite | Yes | Yes | set EPC `0x80` to `0x30` (ON) / `0x31` (OFF) |
| Matter | Yes | Yes | invoke the On / Off commands of the OnOff cluster (`mat on`/`off`, the endpoint is `endpoint` from the config) |
| SwitchBot | Yes | Yes | send `swb cmd <device_id> turnOn` / `turnOff` |

`get` / `set` have no SwitchBot equivalent (the cloud API has no single-property
read/write), so both return exit 14 `protocol_unsupported` for that protocol;
reading state instead goes through `casa invoke <name> status` (see below).

`describe` follows the same shape: ECHONET Lite uses `enl describe` (property
map), Matter uses `mat describe` (endpoint / cluster introspection of the
node), and SwitchBot is not supported (`casa describe` returns exit 14, and
`casa list --describe` yields `properties: null`) because swb has no
property-map introspection.

Color-temperature changes have no dedicated subcommand in casa (`casa color-temp`
was removed in v0.6.0; see the "Verb promotion criteria" section). Calling
`casa invoke <name> color-temp --kelvin <k> | --mireds <m> [--transition <t>]`
delegates directly to `mat color-temp --node <node_id> [--endpoint <ep>]` on
Matter. Mutual-exclusion validation of `--kelvin` / `--mireds` and clamping of
out-of-range values are the responsibility of mat / the device, not casa (casa
passes the arguments through without interpreting them). ECHONET Lite does have
an adapter, but `enl` has no command equivalent to `color-temp`, so passing it
propagates enl's own "unknown subcommand" error with the child CLI's exit code
intact.

SwitchBot's adapter supports `invoke` for the general case: `casa invoke <name>
status` runs `swb status <device_id>` and returns the full status (there is no
single-property read on the cloud API, so this is also how state is read in
place of `get`). `casa invoke <name> cmd <switchbot-command> [args...]` runs
`swb cmd <device_id> <switchbot-command> [args...]`, sending any SwitchBot cloud
command through unchanged.

Binary resolution defaults to `PATH`. It can be overridden as follows (the
environment variable takes precedence):

- Environment variable: `CASA_ENL_BIN=/path/to/enl`
- Config file:

  ```toml
  [binaries]
  enl = "/path/to/enl"
  ```

The child CLI's stderr is not swallowed; with `RUST_LOG=debug` it is forwarded to
casa's stderr.

## Exit codes

| code | Meaning |
|---|---|
| 0 | Success |
| 2 | CLI argument error (clap default) |
| 10 | Config file missing / parse failure |
| 11 | Name not in the config file |
| 12 | Child CLI binary not found / not executable |
| 13 | Child CLI's stdout cannot be parsed as JSON |
| 14 | Operation not supported for that protocol |
| 15 | Some (or all) members of a group execution failed |
| other | Propagates the child CLI's exit code as-is |

Errors originating from the child CLI keep their original exit code (e.g. if enl
exits with `3` on timeout, casa also exits with `3`), so the caller can
distinguish "timeout vs. rejection" and the like.

Even on a partial failure of a group execution (exit 15), stdout emits per-member
result JSON. Which member failed with what exit code can be determined from
`results[].error.exit_code`.

## stderr error format

casa's own errors are emitted to stderr as single-line JSON:

```json
{"error": {"kind": "config_missing", "detail": "config file not found: ..."}}
```

The `kind` values are stable, and these are all of them:

| kind | Meaning | exit code |
|---|---|---|
| `config_missing` | Config file does not exist | 10 |
| `config_parse` | Config file parse / validation failure | 10 |
| `name_not_found` | Name not in the config file | 11 |
| `child_not_found` | Child CLI binary not found / not executable | 12 |
| `child_failed` | Child CLI exited non-zero (code propagated) | child CLI's code |
| `child_invalid_output` | Child CLI's stdout is not JSON | 13 |
| `protocol_unsupported` | Operation not supported for that protocol | 14 |
| `group_partial_failure` | Some or all members of a group execution failed | 15 |

## casad (resident layer: automation rules)

Automations like "when A happens, do B" or "when the time comes, do C" are **not
put into casa itself** (casa stays stateless). Instead, a separate binary `casad`
in the same workspace handles them. See the "Resident / state" section of
[CLAUDE.md](CLAUDE.md) for details.

- **casa** = the stateless executor (CLI).
- **casad** = resides, evaluates rules, and **calls casa as a child process** on
  firing. It shares (links) `casa-core` for config loading and name resolution,
  and delegates real-device actions to casa (hybrid).

Rules are written in TOML (authors are expected to be LLMs / UIs. Sample:
[examples/rules.toml](examples/rules.toml)):

```toml
version = 1

# Event trigger: when living_aircon's power (EPC 0x80) turns ON (0x30), turn on the bedroom light
[[rules]]
name = "bedroom light ON when aircon starts"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "bedroom_light" }

# Time trigger: turn off the bedroom light every day at 22:00
[[rules]]
name = "bedroom light off at 22:00"
when = { at = "22:00" }
then = { action = "off", device = "bedroom_light" }

# Matter event trigger: when study_motion's occupancy becomes 0 (vacant), turn off the desk light
[[rules]]
name = "desk light off when study becomes vacant"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }

# Multiple actions: one trigger, several actions. Same-device actions run in
# declaration order; different-device actions run in parallel.
[[rules]]
name = "desk lights on when study becomes occupied"
when = { device = "study_motion", attribute = "occupancy", equals = 1 }
then = [
  { action = "on", device = "desk_tape_light" },
  { action = "on", device = "desk_light" },
  { action = "invoke", device = "desk_light", command = "color-temp", args = ["--kelvin", "2700"] },
]

# Active window: this rule only fires between 06:00 and 21:00.
[[rules]]
name = "circulate air in the study while nobody is there"
when   = { device = "study_motion", attribute = "occupancy", equals = 0 }
active = { from = "06:00", to = "21:00" }
then   = { action = "on", device = "study_fan" }
```

`then` accepts either a single table or an array of them. With an array, each
action is dispatched to its target device's worker: actions aimed at the same
device run in declaration order, actions aimed at different devices run in
parallel. A failing action does not stop the remaining ones. Use a group
(`[groups.x] members = [...]`) instead when every target takes the same action.
`then = []` (an empty array) is a config error. Ordering is guaranteed within
a single device's queue; actions from concurrently firing rules may interleave.

`active = { from = "HH:MM", to = "HH:MM" }` limits a rule to a time window. The
window is half-open: `from` is included and `to` is excluded, so a rule with
`to = "21:00"` is already inactive at 21:00 sharp. A `from` later than `to`
wraps over midnight (`{ from = "21:00", to = "06:00" }` covers 21:00–23:59 and
00:00–05:59). `from` equal to `to` is a config error, because an empty window
and an all-day window cannot be told apart. Omit `active` and the rule is in
effect at all times. The window applies to every trigger kind — time, ECHONET
event, and Matter event — and is evaluated against local time when the trigger
matches. A rule dropped because it is outside its window is logged at debug
level.

A time trigger whose `at` falls outside its own `active` window is a config
error too: since `to` is exclusive, `when = { at = "21:00" }` combined with
`active = { from = "06:00", to = "21:00" }` could never fire, so `casad check`
and `casad run` reject it up front instead of loading a rule that silently does
nothing.

```bash
# Parse and validate the rules, returning casad's interpretation as JSON
casad check rules.toml

# Start resident (running the time scheduler + the enl listen event listener concurrently)
casad run rules.toml

# Debug: evaluate time triggers exactly once (also the form for delegating from a per-minute cron)
casad run rules.toml --once --now 22:00

# Debug: run enl listen exactly once to evaluate event triggers
casad run rules.toml --listen-once

# Debug: run mat listen exactly once to evaluate Matter event triggers
casad run rules.toml --listen-once-mat

# Debug: evaluate as if it were 22:00 (works with --once / --listen-once / --listen-once-mat).
# Drives both time-trigger evaluation and the `active` window check.
casad run rules.toml --listen-once-mat --now 22:00
```

Event triggers are realized by running the child CLI's `listen` in a loop:
`enl listen` (waiting for ECHONET INF notifications) for `epc` triggers, and
`mat listen` (a thin client streaming matd's resident Subscribe; requires a
running `matd`) for `attribute` triggers. Events with `priming: true` (matd's
current-state redelivery on (re)subscribe) never fire rules. Binary resolution
and stderr forwarding follow the same conventions as casa
(`CASA_ENL_BIN` / `CASA_MAT_BIN` / `[binaries]` / `PATH`).

## Development

Workspace layout (`crates/`):

| crate | kind | role |
|---|---|---|
| `casa-core` | lib | Config loading, name resolution, adapters, child CLI runner (shared by casa and casad) |
| `casa` | bin | Stateless CLI |
| `casad` | bin | Resident layer (rule DSL engine) |

```bash
cargo build
cargo test
cargo clippy --workspace -- -D warnings
RUST_LOG=debug cargo run -p casa -- list --config examples/devices.toml
RUST_LOG=debug cargo run -p casad -- check examples/rules.toml --config examples/devices.toml
```

### Adding a new protocol

Protocol-specific knowledge is confined to `crates/casa-core/src/adapter/`.
Adding a new protocol takes only the following 3 steps, and the subcommand
handlers (`crates/casa/src/main.rs` / `crates/casa-core/src/ops.rs`) are left
unchanged:

1. Add a variant to the `Device` enum in `crates/casa-core/src/config.rs`.
2. Implement an adapter in `crates/casa-core/src/adapter/` that builds the child
   CLI arguments for that variant, and add one line to `adapter_for`.
3. Add unit tests for the adapter.

CI does not use real enl. Integration tests are done with dummies in
`crates/casa/tests/fixtures/` (casa) and `crates/casad/tests/fixtures/`
(casad: stand-in stubs for casa / enl).

### Manual E2E tests against real devices (not run in CI)

In an environment with real enl and real devices, verify the following:

```bash
# 1. Prepare a config file pointing at real devices (managed in a separate repo), and confirm the device list is emitted
casa list

# 2. Confirm the operating status (EPC 0x80) can be read
casa get living_aircon 0x80

# 3. Confirm a write takes effect (0x30 = ON), and verify with a re-read
casa set living_aircon 0x80 0x30
casa get living_aircon 0x80

# 4. Make the device unreachable (e.g. by powering it off) and confirm enl's timeout exit code
#    is returned from casa as-is (check with echo $?)
casa get living_aircon 0x80; echo $?
```

In an environment with real swb and a real SwitchBot cloud device, verify the
following (swb reads `SWITCHBOT_TOKEN` / `SWITCHBOT_SECRET` from the
environment; casa passes no credentials of its own):

```bash
# 0. Export the credentials swb needs (casa does not read or pass these itself)
export SWITCHBOT_TOKEN=...
export SWITCHBOT_SECRET=...

# 1. Confirm the device turns on / off
casa on entry_lock
casa off entry_lock

# 2. Confirm the full status can be read (there is no single-property get for SwitchBot)
casa invoke entry_lock status
```
