# invoke コマンドによる語彙閉鎖 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `casa invoke <name> <command> [args...]` で長尾のプロトコル固有操作を汎用吸収し、`color-temp` 専用サブコマンドを削除、casad の rules/exec からも invoke を使えるようにする。

**Architecture:** アダプタ trait に `invoke` メソッドを 1 個追加（アドレスフラグ注入 + 引数素通し）。ops 層はグループ対応（同一プロトコル限定）。casad は `Then` を tagged enum に再構成し、`Action`（ValueEnum）を廃止して `casad exec` をサブコマンド化する。

**Tech Stack:** Rust / clap derive（`trailing_var_arg` + `allow_hyphen_values`）/ serde（`tag = "action"`）。既存の統合テスト方式（`tests/fixtures/*.sh` のダミー子 CLI）を踏襲。

**Spec:** `docs/superpowers/specs/2026-07-14-invoke-command-design.md`

## Global Constraints

- `cargo build` / `cargo test` / `cargo clippy -- -D warnings` がすべて通ること（各タスクのコミット前に確認）。
- リポジトリに含める IP は RFC 5737 のダミー（`192.0.2.0/24`）のみ。
- stdout は純粋な構造化 JSON のみ。`timestamp` フィールド必須。
- 新しい exit code は追加しない（既存の 10/11/12/13/14/15 と子伝播で足りる）。
- コメント・コミットメッセージは既存流儀に合わせ日本語。コミット末尾に `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`。
- casa のグローバルフラグ（`--config`）は `invoke` のサブコマンドより**前**に置く規約（trailing 引数に呑まれるため）。

---

### Task 1: アダプタ trait に `invoke` を追加（Echonet / Matter 実装）

**Files:**
- Modify: `crates/casa-core/src/adapter/mod.rs`
- Modify: `crates/casa-core/src/adapter/echonet.rs`
- Modify: `crates/casa-core/src/adapter/matter.rs`

**Interfaces:**
- Consumes: 既存の `Invocation { bin: &'static str, args: Vec<String> }`、`Device`。
- Produces: `Adapter::invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation>`（既定 `None`）。Task 3 の ops 層がこれを呼ぶ。

- [ ] **Step 1: Echonet アダプタの失敗するテストを書く**

`crates/casa-core/src/adapter/echonet.rs` の `mod tests` に追加:

```rust
    #[test]
    fn invoke_injects_address_and_passes_args_through() {
        let extra: Vec<String> = vec!["--epc".into(), "0x80".into()];
        let inv = EchonetAdapter.invoke(&device(), "blink", &extra).unwrap();
        assert_eq!(inv.bin, "enl");
        assert_eq!(
            args(&inv),
            [
                "blink",
                "--ip",
                "192.0.2.10",
                "--eoj",
                "0x013001",
                "--epc",
                "0x80"
            ]
        );
    }

    #[test]
    fn invoke_with_no_extra_args_is_just_command_and_address() {
        let inv = EchonetAdapter.invoke(&device(), "blink", &[]).unwrap();
        assert_eq!(
            args(&inv),
            ["blink", "--ip", "192.0.2.10", "--eoj", "0x013001"]
        );
    }
```

`crates/casa-core/src/adapter/matter.rs` の `mod tests` に追加:

```rust
    #[test]
    fn invoke_injects_node_and_passes_args_through() {
        let extra: Vec<String> = vec!["--kelvin".into(), "2700".into()];
        let inv = MatterAdapter.invoke(&device(), "color-temp", &extra).unwrap();
        assert_eq!(inv.bin, "mat");
        assert_eq!(
            args(&inv),
            ["color-temp", "--node", "1234", "--kelvin", "2700"]
        );
    }

    #[test]
    fn invoke_with_endpoint_injects_endpoint_flag() {
        let extra: Vec<String> = vec!["--mireds".into(), "370".into()];
        let inv = MatterAdapter
            .invoke(&device_on_endpoint(2), "color-temp", &extra)
            .unwrap();
        assert_eq!(
            args(&inv),
            [
                "color-temp",
                "--node",
                "1234",
                "--endpoint",
                "2",
                "--mireds",
                "370"
            ]
        );
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p casa-core invoke`
Expected: コンパイルエラー（`no method named invoke`）。

- [ ] **Step 3: trait 既定メソッドと 2 アダプタの実装を書く**

`crates/casa-core/src/adapter/mod.rs` の `trait Adapter` に追加（`color_temp` メソッドの後）:

```rust
    /// 長尾のプロトコル固有操作の汎用動詞。`command` は子 CLI のサブコマンド名を
    /// そのまま受け取り（casa は解釈しない）、アドレスフラグを注入して `args` を
    /// 素通しする。アドレス注入がコマンドによらずプロトコルごとに一様であることが前提。
    fn invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation> {
        let _ = (device, command, args);
        None
    }
```

`crates/casa-core/src/adapter/echonet.rs` の `impl Adapter for EchonetAdapter` に追加:

```rust
    fn invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation> {
        let (ip, eoj) = address(device)?;
        let mut all = vec![
            command.to_string(),
            "--ip".to_string(),
            ip.to_string(),
            "--eoj".to_string(),
            eoj.to_string(),
        ];
        all.extend(args.iter().cloned());
        Some(Invocation { bin: BIN, args: all })
    }
```

`crates/casa-core/src/adapter/matter.rs` の `impl Adapter for MatterAdapter` に追加:

```rust
    /// endpoint は設定にあれば注入する（`power` と同じ流儀）。そのコマンドが
    /// `--endpoint` を取らない場合は mat 側のエラーが exit code 伝播で見える。
    fn invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation> {
        let (node, endpoint) = address(device)?;
        let mut all = vec![command.to_string(), "--node".to_string(), node.to_string()];
        if let Some(ep) = endpoint {
            all.push("--endpoint".to_string());
            all.push(ep.to_string());
        }
        all.extend(args.iter().cloned());
        Some(invocation(all))
    }
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p casa-core invoke`
Expected: 上記 4 テストすべて PASS。

- [ ] **Step 5: コミット**

```bash
git add crates/casa-core/src/adapter/
git commit -m "feat(adapter): 汎用 invoke メソッドを追加（アドレス注入 + 引数素通し）"
```

---

### Task 2: `output::invoke_response`（command フィールド付き envelope）

**Files:**
- Modify: `crates/casa-core/src/output.rs`

**Interfaces:**
- Consumes: 既存の `device_response(name, device, value)`。
- Produces: `pub fn invoke_response(name: &str, device: &Device, command: &str, value: Value) -> Value` — `device_response` の envelope に `"command"` キーを足したもの。Task 3 が使う。

- [ ] **Step 1: 失敗するテストを書く**

`crates/casa-core/src/output.rs` の `mod tests` に追加:

```rust
    #[test]
    fn invoke_response_includes_command_field() {
        let device = Device::Echonet {
            ip: "192.0.2.10".into(),
            eoj: "0x013001".into(),
        };
        let v = invoke_response(
            "living_aircon",
            &device,
            "blink",
            serde_json::json!({"ok": true}),
        );
        assert_eq!(v["device"], "living_aircon");
        assert_eq!(v["protocol"], "echonet");
        assert_eq!(v["command"], "blink");
        assert_eq!(v["value"]["ok"], true);
        assert!(v["timestamp"].is_string());
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p casa-core invoke_response`
Expected: コンパイルエラー（`cannot find function invoke_response`）。

- [ ] **Step 3: 実装する**

`crates/casa-core/src/output.rs` の `device_response` の直後に追加:

```rust
/// `casa invoke` の応答。何を invoke したか応答から追跡できるよう `command` を含める。
pub fn invoke_response(name: &str, device: &Device, command: &str, value: Value) -> Value {
    let mut response = device_response(name, device, value);
    response["command"] = json!(command);
    response
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p casa-core invoke_response`
Expected: PASS。

- [ ] **Step 5: コミット**

```bash
git add crates/casa-core/src/output.rs
git commit -m "feat(output): invoke 応答の envelope（command フィールド付き）を追加"
```

---

### Task 3: `ops::invoke`（グループ対応・同一プロトコル限定）

**Files:**
- Modify: `crates/casa-core/src/ops.rs`

**Interfaces:**
- Consumes: Task 1 の `Adapter::invoke`、Task 2 の `output::invoke_response`、既存の `run_group` / `require_adapter` / `unsupported` / `execute`。
- Produces: `pub fn invoke(config: &Config, name: &str, command: &str, args: &[String]) -> Result<Value, CasaError>`。Task 4 の CLI 層が使う。グループ応答（Ok / exit 15 の両方）のトップレベルに `command` を含める。

- [ ] **Step 1: 失敗するテストを書く**

`crates/casa-core/src/ops.rs` の `mod tests` に追加:

```rust
    const MIXED_GROUP_CONFIG: &str = r#"
version = 1

[devices.light1]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029101"

[devices.light2]
protocol = "matter"
node_id = "1234"

[groups.mixed]
members = ["light1", "light2"]
"#;

    #[test]
    fn invoke_rejects_mixed_protocol_group_before_spawn() {
        let config = config::parse(MIXED_GROUP_CONFIG).unwrap();

        let err = invoke(&config, "mixed", "blink", &[]).unwrap_err();

        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
        assert_eq!(err.exit_code(), 14);
        assert!(err.detail.contains("mixed"), "detail: {}", err.detail);
        assert!(err.detail.contains("echonet"), "detail: {}", err.detail);
        assert!(err.detail.contains("matter"), "detail: {}", err.detail);
        // spawn 前に拒否されるのでメンバー別結果は無い。
        assert!(err.response.is_none());
    }

    #[test]
    fn invoke_group_enters_group_pipeline_and_tags_command() {
        // enl を存在しないパスに向け、「グループ経路に入り exit 15 が返る」ことを
        // 実機なしで検証する（power のグループテストと同じ手法）。
        let text = format!("{GROUP_CONFIG}\n[binaries]\nenl = \"/nonexistent/enl\"\n");
        let config = config::parse(&text).unwrap();

        let err = invoke(&config, "living", "blink", &[]).unwrap_err();

        assert_eq!(err.kind, ErrorKind::GroupPartialFailure);
        let response = err.response.unwrap();
        assert_eq!(response["command"], "blink");
        assert_eq!(response["group"], "living");
        assert_eq!(response["results"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn invoke_on_protocol_without_adapter_is_protocol_unsupported() {
        let text = r#"
version = 1
[devices.lock]
protocol = "switchbot"
device_id = "DUMMY-XX-XX"
"#;
        let config = config::parse(text).unwrap();

        let err = invoke(&config, "lock", "press", &[]).unwrap_err();

        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p casa-core ops::tests::invoke`
Expected: コンパイルエラー（`cannot find function invoke`）。

- [ ] **Step 3: 実装する**

`crates/casa-core/src/ops.rs` の先頭 use に `BTreeSet` を足す（既存の `BTreeMap` と同じ行にまとめる）:

```rust
use std::collections::{BTreeMap, BTreeSet};
```

`ops.rs` の `color_temp` 関数の後に追加:

```rust
/// `casa invoke <name> <command> [args...]` — 長尾のプロトコル固有操作の汎用動詞。
/// `command` は子 CLI のサブコマンド名そのままで、casa は解釈しない。
pub fn invoke(
    config: &Config,
    name: &str,
    command: &str,
    args: &[String],
) -> Result<Value, CasaError> {
    if let Some(group) = config.groups.get(name) {
        ensure_uniform_protocol(config, name, group, command)?;
        return match run_group(config, name, group, command, |adapter, device| {
            adapter.invoke(device, command, args)
        }) {
            Ok(mut response) => {
                response["command"] = json!(command);
                Ok(response)
            }
            Err(mut err) => {
                if let Some(response) = err.response.as_mut() {
                    response["command"] = json!(command);
                }
                Err(err)
            }
        };
    }
    let device = config.device(name)?;
    let adapter = require_adapter(device, command)?;
    let invocation = adapter
        .invoke(device, command, args)
        .ok_or_else(|| unsupported(device, command))?;
    let value = execute(config, &invocation)?;
    Ok(output::invoke_response(name, device, command, value))
}

/// invoke のコマンド解釈はプロトコル依存なので、混在プロトコルのグループは
/// 「同名コマンドが別プロトコルで別の意味に実行される」事故を防ぐため spawn 前に拒否する。
fn ensure_uniform_protocol(
    config: &Config,
    group_name: &str,
    group: &Group,
    command: &str,
) -> Result<(), CasaError> {
    let protocols: BTreeSet<&str> = group
        .members
        .iter()
        .map(|m| Ok(config.device(m)?.protocol()))
        .collect::<Result<_, CasaError>>()?;
    if protocols.len() > 1 {
        let found: Vec<&str> = protocols.into_iter().collect();
        return Err(CasaError::new(
            ErrorKind::ProtocolUnsupported,
            format!(
                "invoke \"{command}\" on group \"{group_name}\" requires all members to \
                 share one protocol (found: {})",
                found.join(", ")
            ),
        ));
    }
    Ok(())
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p casa-core`
Expected: 新規 3 テスト含め全 PASS。

- [ ] **Step 5: コミット**

```bash
git add crates/casa-core/src/ops.rs
git commit -m "feat(ops): invoke 操作を追加（グループは同一プロトコル限定）"
```

---

### Task 4: casa CLI に `invoke` サブコマンドを配線（統合テスト）

**Files:**
- Modify: `crates/casa/src/cli.rs`
- Modify: `crates/casa/src/main.rs`
- Create: `crates/casa/tests/cli_invoke.rs`

**Interfaces:**
- Consumes: Task 3 の `ops::invoke(config, name, command, args)`。
- Produces: `casa invoke <name> <command> [args...]`（`--config` は invoke より前に置く規約）。

- [ ] **Step 1: 失敗する統合テストを書く**

`crates/casa/tests/cli_invoke.rs` を新規作成:

```rust
//! `casa invoke` の統合テスト。ダミー子 CLI で引数素通し・envelope・グループ・
//! エラー系を検証する。CI で実 enl / mat は不要。
//!
//! casa のグローバルフラグ（--config）は trailing 引数に呑まれないよう
//! invoke より前に置く（README の規約どおりの呼び方でテストする）。

mod common;

use common::*;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

const MATTER_CONFIG: &str = r#"
version = 1

[devices.living_light]
protocol = "matter"
node_id = "1234"
"#;

const GROUP_CONFIG: &str = r#"
version = 1

[devices.light1]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029101"

[devices.light2]
protocol = "echonet"
ip = "192.0.2.12"
eoj = "0x029101"

[groups.living]
members = ["light1", "light2"]
"#;

const MIXED_GROUP_CONFIG: &str = r#"
version = 1

[devices.light1]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029101"

[devices.light2]
protocol = "matter"
node_id = "1234"

[groups.mixed]
members = ["light1", "light2"]
"#;

#[test]
fn invoke_injects_echonet_address_and_passes_args_through() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "living_aircon",
            "blink",
            "--epc",
            "0x80",
        ],
        &[("CASA_ENL_BIN", &fixture("enl_args.sh"))],
    );

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = stdout_json(&out);
    assert_eq!(v["device"], "living_aircon");
    assert_eq!(v["protocol"], "echonet");
    assert_eq!(v["command"], "blink");
    assert!(v["timestamp"].is_string(), "timestamp missing: {v}");
    let expected = serde_json::json!([
        "blink",
        "--ip",
        "192.0.2.10",
        "--eoj",
        "0x013001",
        "--epc",
        "0x80"
    ]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn invoke_matter_color_temp_replaces_removed_shortcut() {
    // 旧 `casa color-temp` の代替経路。削除後もこの呼び方で同じ mat 引数列になる。
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "living_light",
            "color-temp",
            "--kelvin",
            "2700",
        ],
        &[("CASA_MAT_BIN", &fixture("mat_args.sh"))],
    );

    assert_eq!(out.status.code(), Some(0));
    let v = stdout_json(&out);
    assert_eq!(v["command"], "color-temp");
    let expected = serde_json::json!(["color-temp", "--node", "1234", "--kelvin", "2700"]);
    assert_eq!(v["value"]["args"], expected);
}

#[test]
fn invoke_group_runs_members_and_tags_command() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), GROUP_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "living",
            "blink",
        ],
        &[("CASA_ENL_BIN", &fixture("enl_args.sh"))],
    );

    assert_eq!(out.status.code(), Some(0));
    let v = stdout_json(&out);
    assert_eq!(v["group"], "living");
    assert_eq!(v["command"], "blink");
    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["ok"], true);
    assert_eq!(
        results[0]["value"]["args"],
        serde_json::json!(["blink", "--ip", "192.0.2.11", "--eoj", "0x029101"])
    );
}

#[test]
fn invoke_mixed_protocol_group_exits_14() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MIXED_GROUP_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "mixed",
            "blink",
        ],
        &[("CASA_ENL_BIN", &fixture("enl_args.sh"))],
    );

    assert_eq!(out.status.code(), Some(14));
    assert_eq!(
        stderr_error_json(&out)["error"]["kind"],
        "protocol_unsupported"
    );
}

#[test]
fn invoke_switchbot_exits_14() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "entry_lock",
            "press",
        ],
        &[],
    );

    assert_eq!(out.status.code(), Some(14));
    assert_eq!(
        stderr_error_json(&out)["error"]["kind"],
        "protocol_unsupported"
    );
}

#[test]
fn invoke_propagates_child_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casa(
        &[
            "--config",
            config.to_str().unwrap(),
            "invoke",
            "living_aircon",
            "blink",
        ],
        &[("CASA_ENL_BIN", &fixture("enl_exit3.sh"))],
    );

    // 子 CLI の exit code（3 = enl タイムアウト）をそのまま伝播する。
    assert_eq!(out.status.code(), Some(3));
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p casa --test cli_invoke`
Expected: 全テスト FAIL（`invoke` サブコマンドが無く clap が exit 2）。

- [ ] **Step 3: CLI variant と main の配線を書く**

`crates/casa/src/cli.rs` の `enum Command` の `Set` variant の後に追加:

```rust
    /// プロトコル固有 CLI のコマンドを名前解決付きで呼び出す（長尾操作の汎用動詞）。
    /// `<command>` と後続引数は解釈せず子 CLI へそのまま渡す。
    /// casa 自身のフラグ（--config 等）は invoke より前に置くこと。
    Invoke {
        /// 設定ファイル上のデバイス名またはグループ名（グループは同一プロトコルのみ）
        name: String,
        /// 子 CLI のサブコマンド名（例: color-temp）。casa は解釈しない
        command: String,
        /// 子 CLI にそのまま渡す引数
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
```

`crates/casa/src/main.rs` の `match cli.command` に追加（`Set` arm の後）:

```rust
        Command::Invoke {
            name,
            command,
            args,
        } => ops::invoke(&config, &name, &command, &args)?,
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p casa --test cli_invoke`
Expected: 全 6 テスト PASS。

Run: `cargo test`
Expected: 既存テスト含め全 PASS。

- [ ] **Step 5: コミット**

```bash
git add crates/casa/src/cli.rs crates/casa/src/main.rs crates/casa/tests/cli_invoke.rs
git commit -m "feat(casa): invoke サブコマンドを追加（長尾操作の汎用動詞）"
```

---

### Task 5: `color-temp` の削除（破壊変更）

**Files:**
- Modify: `crates/casa/src/cli.rs`（`ColorTemp` variant と `ArgGroup` import 削除）
- Modify: `crates/casa/src/main.rs`（`ColorTemp` arm 削除）
- Modify: `crates/casa-core/src/ops.rs`（`color_temp` 関数削除）
- Modify: `crates/casa-core/src/adapter/mod.rs`（`ColorTemp` 構造体・trait メソッド削除）
- Modify: `crates/casa-core/src/adapter/matter.rs`（`color_temp` 実装・テスト削除）
- Modify: `crates/casa/tests/cli_matter.rs`（color-temp 統合テスト削除）
- Modify: `crates/casa-core/src/config.rs:21`（doc コメントの color-temp 言及を更新）
- Modify: `crates/casa-core/src/output.rs:91`（doc コメントの color-temp 言及を更新）

**Interfaces:**
- Consumes: なし（削除のみ）。
- Produces: `casa color-temp` は clap の unknown subcommand（exit 2）になる。invoke による代替経路は Task 4 のテスト `invoke_matter_color_temp_replaces_removed_shortcut` が担保済み。

- [ ] **Step 1: color-temp 参照箇所を洗い出す**

Run: `grep -rn "color.temp\|ColorTemp" crates/`
Expected: 上記 Files 欄の 8 ファイルのみヒット（README は Task 8 で扱う）。

- [ ] **Step 2: コードから削除する**

以下を削除する:

1. `crates/casa/src/cli.rs`: `ColorTemp { ... }` variant 全体と、先頭の `use clap::{ArgGroup, Parser, Subcommand};` から `ArgGroup`（→ `use clap::{Parser, Subcommand};`）。
2. `crates/casa/src/main.rs`: `Command::ColorTemp { ... } => { ... }` の arm 全体。
3. `crates/casa-core/src/ops.rs`: `pub fn color_temp(...)` 関数全体と、use 行の `ColorTemp`（→ `use crate::adapter::{self, Adapter, Invocation};`）。
4. `crates/casa-core/src/adapter/mod.rs`: `pub struct ColorTemp { ... }`（doc コメント含む）と trait 内の `fn color_temp(...)` 既定メソッド。
5. `crates/casa-core/src/adapter/matter.rs`: `impl` 内の `fn color_temp(...)`（doc コメント含む）、`use super::{Adapter, ColorTemp, Invocation};` から `ColorTemp`、`mod tests` 内の `color_temp_kelvin_maps_to_mat_color_temp` / `color_temp_mireds_with_endpoint_passes_flags` / `color_temp_transition_is_appended` の 3 テスト。
6. `crates/casa/tests/cli_matter.rs`: `color-temp` を使うテスト関数（`grep -n "color" crates/casa/tests/cli_matter.rs` でヒットする関数）をすべて削除。
7. `crates/casa-core/src/config.rs:21` の doc コメント `書き系（on/off/color-temp/set）のみ対応。` を `書き系（on/off/set/invoke）のみ対応。` に変更。
8. `crates/casa-core/src/output.rs:91` の doc コメント `グループ操作（on / off / color-temp / set）の応答。` を `グループ操作（on / off / set / invoke）の応答。` に変更。

- [ ] **Step 3: ビルド・テスト・lint が通ることを確認する**

Run: `cargo build && cargo test && cargo clippy -- -D warnings`
Expected: すべて成功。`ColorTemp` 参照の残骸があればここでコンパイルエラーになる。

Run: `grep -rn "color.temp\|ColorTemp" crates/`
Expected: ヒット 0 件。

- [ ] **Step 4: コミット**

```bash
git add crates/
git commit -m "feat(casa)!: color-temp サブコマンドを削除（invoke で代替）

昇格基準（2プロトコル以上で同義 or 日常高頻度）を満たさない Matter 固有
コマンドのため削除。casa invoke <name> color-temp --kelvin 2700 で代替。"
```

---

### Task 6: casad `Then` の tagged enum 化（rules.toml の invoke 対応）

**Files:**
- Modify: `crates/casad/src/rules.rs`（`Then` 再構成、`Action` 依存を除去）
- Modify: `crates/casad/src/engine.rs`（`fire` を `then.casa_args` に変更、テスト修正）

**Interfaces:**
- Consumes: casa の CLI 表面（`--config <path>` はサブコマンドより前に置ける clap グローバルフラグ）。
- Produces:
  - `pub enum Then { On { device: String }, Off { device: String }, Invoke { device: String, command: String, args: Vec<String> } }`（serde `tag = "action"`、`args` は `#[serde(default)]`）。
  - `Then::device(&self) -> &str`
  - `Then::casa_args(&self, config: Option<&Path>) -> Vec<String>` — **`--config` を先頭に置く**（invoke の trailing 引数に呑まれないため）。Task 7 の CLI/main と exec テストがこの並びに依存する。
  - この時点では `action.rs` はまだ削除しない（`casad exec` が使用中。削除は Task 7）。

- [ ] **Step 1: 失敗するテストを書く**

`crates/casad/src/rules.rs` の `mod tests` を修正・追加:

既存テスト `parses_event_and_time_rules` の末尾行を変更:

```rust
        // 変更前: assert_eq!(file.rules[0].then.action, Action::On);
        assert!(matches!(file.rules[0].then, Then::On { .. }));
```

新規テストを追加:

```rust
    #[test]
    fn parses_invoke_rule_with_args() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "日没で電球色"
when = { at = "18:00" }
then = { action = "invoke", device = "hallway_light", command = "color-temp", args = ["--kelvin", "2700"] }
"#,
        )
        .unwrap();
        match &file.rules[0].then {
            Then::Invoke {
                device,
                command,
                args,
            } => {
                assert_eq!(device, "hallway_light");
                assert_eq!(command, "color-temp");
                assert_eq!(args, &["--kelvin", "2700"]);
            }
            other => panic!("unexpected then: {other:?}"),
        }
    }

    #[test]
    fn invoke_rule_args_default_to_empty() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "argsなし"
when = { at = "18:00" }
then = { action = "invoke", device = "hallway_light", command = "blink" }
"#,
        )
        .unwrap();
        match &file.rules[0].then {
            Then::Invoke { args, .. } => assert!(args.is_empty()),
            other => panic!("unexpected then: {other:?}"),
        }
    }

    #[test]
    fn casa_args_places_config_before_subcommand() {
        use std::path::Path;
        let then = Then::Invoke {
            device: "hallway_light".into(),
            command: "color-temp".into(),
            args: vec!["--kelvin".into(), "2700".into()],
        };
        assert_eq!(
            then.casa_args(Some(Path::new("/tmp/d.toml"))),
            [
                "--config",
                "/tmp/d.toml",
                "invoke",
                "hallway_light",
                "color-temp",
                "--kelvin",
                "2700"
            ]
        );
        // on/off も --config が先頭（casa の clap グローバルフラグ）。
        let on = Then::On {
            device: "hallway_light".into(),
        };
        assert_eq!(
            on.casa_args(Some(Path::new("/tmp/d.toml"))),
            ["--config", "/tmp/d.toml", "on", "hallway_light"]
        );
        assert_eq!(on.casa_args(None), ["on", "hallway_light"]);
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p casad rules`
Expected: コンパイルエラー（`Then` は struct であり variant を持たない）。

- [ ] **Step 3: `Then` を再構成する**

`crates/casad/src/rules.rs`:

1. `use crate::action::Action;` を削除し、`use std::path::Path;` が無ければ確認（既にある）。
2. `pub struct Then { pub action: Action, pub device: String }` を以下に置き換える:

```rust
/// アクション。`action` フィールドが serde の tag。
/// - `then = { action = "on", device = "hallway_light" }`
/// - `then = { action = "invoke", device = "desk_light", command = "color-temp", args = ["--kelvin", "2700"] }`
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Then {
    On {
        device: String,
    },
    Off {
        device: String,
    },
    Invoke {
        device: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl Then {
    /// アクション対象の名前（device または group。名前解決は casa 側が担う）。
    pub fn device(&self) -> &str {
        match self {
            Then::On { device } | Then::Off { device } | Then::Invoke { device, .. } => device,
        }
    }

    /// casa の引数列へ変換する。casa の CLI 表面に対する casad の知識はここに閉じる。
    /// `--config` は casa の clap グローバルフラグとしてサブコマンドより**前**に置く
    /// （invoke の trailing 引数に呑まれないため。on/off も並びを揃える）。
    pub fn casa_args(&self, config: Option<&Path>) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(path) = config {
            out.push("--config".to_string());
            out.push(path.to_string_lossy().into_owned());
        }
        match self {
            Then::On { device } => out.extend(["on".to_string(), device.clone()]),
            Then::Off { device } => out.extend(["off".to_string(), device.clone()]),
            Then::Invoke {
                device,
                command,
                args,
            } => {
                out.extend(["invoke".to_string(), device.clone(), command.clone()]);
                out.extend(args.iter().cloned());
            }
        }
        out
    }
}
```

3. `RuleFile::validate` 内の `check_target(config, &rule.name, &rule.then.device)?;` を
   `check_target(config, &rule.name, rule.then.device())?;` に変更。

`crates/casad/src/engine.rs` の `fire` を変更:

```rust
/// 1 つのルールの `then` を casa の spawn で実行する。
/// `config_path` は casa へ渡す `--config`（None なら casa が既定パスを解決）。
pub fn fire(rule: &Rule, config_path: Option<&Path>) -> Result<i32, CasaError> {
    let args = rule.then.casa_args(config_path);
    tracing::info!(rule = %rule.name, "firing rule");
    casa_runner::run_casa(&args)
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p casad`
Expected: 全 PASS（`check.rs` / `run.rs` 統合テストの既存 on/off ルール TOML は表記不変なのでそのまま通る。engine の単体テストも `then` を直接触らないため無変更で通る）。

- [ ] **Step 5: コミット**

```bash
git add crates/casad/src/rules.rs crates/casad/src/engine.rs
git commit -m "feat(casad): rules の Then を tagged enum 化し invoke アクションを追加"
```

---

### Task 7: `casad exec` のサブコマンド化と `action.rs` の削除

**Files:**
- Modify: `crates/casad/src/cli.rs`（`Exec` をサブコマンド化、`ExecAction` 追加）
- Modify: `crates/casad/src/main.rs`（`Exec` arm の書き換え、`mod action;` 削除）
- Delete: `crates/casad/src/action.rs`
- Modify: `crates/casad/tests/exec.rs`（`--config` 位置の期待値修正 + invoke テスト追加)

**Interfaces:**
- Consumes: Task 6 の `Then` / `Then::device()` / `Then::casa_args()`。
- Produces: `casad exec on <name>` / `casad exec off <name>`（表記は現行と同一）、`casad exec invoke <name> <command> [args...]`（新規）。casa へ渡す引数列は `--config` が先頭になる（Task 6 の `casa_args` 仕様）。

- [ ] **Step 1: 失敗する統合テストを書く**

`crates/casad/tests/exec.rs` を修正・追加:

既存テスト `exec_on_spawns_casa_with_mapped_args` のアサーションを、`--config` 先頭の新しい並びに変更:

```rust
    let stdout = String::from_utf8_lossy(&out.stdout);
    // casad は casa へ `--config <path> on living_aircon` を渡す
    // （--config は casa のグローバルフラグとして先頭。invoke の trailing 引数対策）。
    assert!(
        stdout.contains("on living_aircon"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("--config"), "stdout: {stdout}");
```

新規テストを追加:

```rust
#[test]
fn exec_invoke_spawns_casa_with_passthrough_args() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    // casad 自身の --config も exec より前に置く（invoke の trailing 引数対策）。
    let out = run_casad(
        &[
            "--config",
            config.to_str().unwrap(),
            "exec",
            "invoke",
            "living_aircon",
            "color-temp",
            "--kelvin",
            "2700",
        ],
        &[("CASA_BIN", &fixture("casa_stub.sh"))],
    );

    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("invoke living_aircon color-temp --kelvin 2700"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("--config"), "stdout: {stdout}");
}

#[test]
fn exec_invoke_unknown_name_fails_without_spawning_casa() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);

    let out = run_casad(
        &[
            "--config",
            config.to_str().unwrap(),
            "exec",
            "invoke",
            "nope",
            "blink",
        ],
        &[
            ("CASA_BIN", &fixture("casa_stub.sh")),
            ("CASA_FAKE_EXIT", "99"),
        ],
    );

    // link 側の名前解決で弾かれ、casa は起動されない。
    assert_eq!(out.status.code(), Some(11));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "name_not_found");
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p casad --test exec`
Expected: `exec_invoke_*` の 2 件が FAIL（clap が `invoke` を name 引数と解釈して余剰引数エラー exit 2）。`exec_on_spawns_casa_with_mapped_args` は旧実装の引数順（`--config` 末尾）でも `contains` は通るため PASS のまま。

- [ ] **Step 3: CLI と main を書き換え、action.rs を削除する**

`crates/casad/src/cli.rs`:

1. `use crate::action::Action;` を `use crate::rules::Then;` に変更。
2. `Exec` variant を置き換え:

```rust
    /// 名前を解決し、対応する casa アクションを実行する。
    /// ルールエンジンが発火時に使うアクション実行プリミティブの最小形。
    Exec {
        #[command(subcommand)]
        action: ExecAction,
    },
```

3. ファイル末尾に追加:

```rust
/// `casad exec` のアクション。rules の `then` と同じ語彙（on / off / invoke）。
#[derive(Debug, Subcommand)]
pub enum ExecAction {
    /// デバイス（またはグループ）の電源を入れる
    On {
        /// 設定ファイル上のデバイス名またはグループ名
        name: String,
    },
    /// デバイス（またはグループ）の電源を切る
    Off {
        /// 設定ファイル上のデバイス名またはグループ名
        name: String,
    },
    /// 子 CLI のコマンドを名前解決付きで呼び出す（casa invoke へ委譲）。
    /// casad 自身のフラグ（--config 等）は exec より前に置くこと。
    Invoke {
        /// 設定ファイル上のデバイス名またはグループ名
        name: String,
        /// 子 CLI のサブコマンド名（例: color-temp）
        command: String,
        /// 子 CLI にそのまま渡す引数
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

impl ExecAction {
    /// rules の `Then` へ変換する。exec と rules で casa 引数列の写像を共有する。
    pub fn into_then(self) -> Then {
        match self {
            ExecAction::On { name } => Then::On { device: name },
            ExecAction::Off { name } => Then::Off { device: name },
            ExecAction::Invoke {
                name,
                command,
                args,
            } => Then::Invoke {
                device: name,
                command,
                args,
            },
        }
    }
}
```

`crates/casad/src/main.rs`:

1. `mod action;` を削除。
2. `Command::Exec` arm を置き換え:

```rust
        Command::Exec { action } => {
            // link 側: 設定ロードと名前解決は casa-core で型安全に。未定義名は casa を
            // 起動する前に exit 11 で弾く（ルールエンジンが発火前にルールを検証できる根拠）。
            // device / group どちらでもよい（グループのメンバー展開は casa 側が担う）。
            let then = action.into_then();
            let config = config::load(cli.config.as_deref())?;
            config.ensure_target(then.device())?;

            // spawn 側: 実機アクションは casa を子プロセスとして起動し、exit code を伝播する。
            let args = then.casa_args(cli.config.as_deref());
            casa_runner::run_casa(&args)
        }
```

3. `crates/casad/src/action.rs` を削除:

```bash
git rm crates/casad/src/action.rs
```

- [ ] **Step 4: テスト・lint が通ることを確認する**

Run: `cargo test -p casad && cargo clippy -- -D warnings`
Expected: exec.rs の全テスト（既存 4 + 新規 2）含め全 PASS、clippy クリーン。

Run: `cargo test`
Expected: ワークスペース全体 PASS。

- [ ] **Step 5: コミット**

```bash
git add -A crates/casad
git commit -m "feat(casad): exec をサブコマンド化し invoke を追加（Action を Then に統合）"
```

---

### Task 8: ドキュメント更新とバージョン bump

**Files:**
- Modify: `Cargo.toml`（workspace version 0.5.0 → 0.6.0）
- Modify: `README.md`
- Modify: `CLAUDE.md`

**Interfaces:**
- Consumes: Task 1–7 の完成した挙動。
- Produces: リリース可能な v0.6.0。

- [ ] **Step 1: バージョンを bump する**

`Cargo.toml` の `[workspace.package]` の `version = "0.5.0"` を `version = "0.6.0"` に変更。
Run: `cargo build`（Cargo.lock のバージョン追従を確認）。

- [ ] **Step 2: README.md を更新する**

`grep -n "color" README.md` でヒットする箇所をすべて確認し、以下の方針で書き換える:

1. 「使い方」の color-temp 例を invoke に置き換える:

```bash
# プロトコル固有 CLI のコマンドを名前解決付きで呼び出す（長尾操作の汎用動詞）。
# <command> 以降は子 CLI にそのまま渡る。casa のフラグ（--config 等)は invoke より前に置く。
casa invoke living_light color-temp --kelvin 2700
casa invoke living_light color-temp --mireds 370 --transition 30
```

2. グループの記述 `on / off / color-temp / set` を `on / off / set / invoke` に変更し、「invoke はグループの全メンバーが同一プロトコルの場合のみ（混在グループは exit 14）」を追記。
3. 子 CLI 対応表・マッピング節の `casa color-temp` 記述を `casa invoke <name> color-temp ...` ベースに書き換える。
4. **動詞の昇格基準**の節を追加:

```markdown
## 動詞の昇格基準

casa に専用サブコマンドを足すのは、**2 プロトコル以上で同じ意味を持つ、または日常高頻度の操作**のみ
（例: `on` / `off` / `get` / `set` / `describe`）。それ以外のプロトコル固有操作は `casa invoke` で表現する。
invoke の応答は envelope（`timestamp` / `device` / `protocol` / `command`）を casa が保証し、
`value` は子 CLI の JSON をそのまま格納する。
```

5. 破壊変更の告知（バージョン節 or 冒頭の適切な場所）: 「v0.6.0: `casa color-temp` を削除。`casa invoke <name> color-temp --kelvin 2700` で代替」。

- [ ] **Step 3: CLAUDE.md を更新する**

1. 「規約」節に invoke と昇格基準を追記（stdout 節の後に小節を追加）:

```markdown
### 動詞の昇格基準と invoke

casa に専用サブコマンドを足すのは「2 プロトコル以上で同じ意味を持つ、または日常高頻度」の
操作のみ。それ以外の長尾のプロトコル固有操作は `casa invoke <name> <command> [args...]` で
表現する（名前解決 + アドレスフラグ注入 + 引数素通し。`command` は子 CLI の語彙そのまま）。
invoke のグループ実行は全メンバー同一プロトコルの場合のみ。casa のグローバルフラグは
invoke より前に置く。
```

2. Phase 4 の Matter 節にある `on`/`off` の記述はそのまま。`casad` 責務節の rules.toml 説明に「then は on / off / invoke」と一言追記。

- [ ] **Step 4: 最終検証**

Run: `cargo build && cargo test && cargo clippy -- -D warnings`
Expected: すべて成功。

Run: `grep -rn "color.temp\|ColorTemp" crates/ README.md CLAUDE.md | grep -v invoke`
Expected: ヒットするのは README の invoke 例・破壊変更告知・マッピング説明のみ（コード参照ゼロ）。

- [ ] **Step 5: コミット**

```bash
git add Cargo.toml Cargo.lock README.md CLAUDE.md
git commit -m "docs: invoke の使い方と動詞の昇格基準を記載、バージョンを 0.6.0 に bump"
```
