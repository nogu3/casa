# casa グループ実行 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `devices.toml` の `[groups]` で複数デバイスをまとめ、`casa on/off/color-temp/set <group>` で全メンバーを並列に同時操作できるようにする。

**Architecture:** 名前解決レイヤの拡張。ops 層で名前がグループなら「メンバーごとに Invocation を組む → 全子プロセスを並列 spawn → メンバー記載順に回収 → メンバー別結果 JSON」に分岐する。プロトコル知識は既存アダプタ層のまま、casa のステートレス原則を維持する。

**Tech Stack:** Rust / clap / serde / serde_json / toml / tracing（新規依存なし。並列化は `std::process::Command::spawn` のみで実現）

**Spec:** `docs/superpowers/specs/2026-07-06-group-execution-design.md`

## Global Constraints

- 新規クレート依存を追加しない（並列化にスレッド・async ランタイムを使わない）。
- stdout は純粋な構造化 JSON のみ。`timestamp`（ISO 8601）必須。
- casa 自体のエラーは stderr に `{"error": {"kind": "...", "detail": "..."}}` の 1 行 JSON。
- exit code: 0 成功 / 10 config / 11 name_not_found / 12 child_not_found / 13 child_invalid_output / 14 protocol_unsupported / **15 group_partial_failure（新規）** / その他は子 CLI 伝播。
- グループ対応は書き系のみ: `on` / `off` / `color-temp` / `set`。`get` / `describe` にグループ名は exit 14。
- 単体デバイス操作の出力スキーマ・挙動は一切変えない。
- 設定 `version` は 1 のまま（`groups` 省略時は完全互換）。
- サンプル値はダミーのみ（RFC 5737 `192.0.2.0/24` 等）。実 IP・実機 ID をコミットしない。
- 各タスク完了時に `cargo test` と `cargo clippy -- -D warnings` が通ること。
- コミットメッセージは既存リポジトリの慣例（日本語 + conventional commits、`Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` を末尾に付ける）。

---

### Task 1: error — `GroupPartialFailure`（exit 15）と部分失敗レスポンスの運搬

**Files:**
- Modify: `crates/casa-core/src/error.rs`

**Interfaces:**
- Produces: `ErrorKind::GroupPartialFailure`（`as_str() == "group_partial_failure"`、`exit_code() == 15`）
- Produces: `CasaError` の新フィールド `pub response: Option<serde_json::Value>` と `pub fn with_response(self, response: serde_json::Value) -> Self`。部分失敗時に「stdout に出すべきグループ結果 JSON」をエラーに載せて main まで運ぶための器。

**背景:** 部分失敗は「stdout にメンバー別結果 JSON を出す」と「exit 15 + stderr エラー」の両方が必要。現行の `main.rs` は `Err` なら stderr だけ出して exit する構造なので、エラーに任意の stdout ボディを添付できるようにする。

- [ ] **Step 1: 失敗するテストを書く**

`crates/casa-core/src/error.rs` の `mod tests` に追加:

```rust
    #[test]
    fn group_partial_failure_is_exit_15() {
        assert_eq!(ErrorKind::GroupPartialFailure.exit_code(), 15);
        assert_eq!(ErrorKind::GroupPartialFailure.as_str(), "group_partial_failure");
    }

    #[test]
    fn with_response_attaches_stdout_body() {
        let err = CasaError::new(ErrorKind::GroupPartialFailure, "1/2 failed");
        assert!(err.response.is_none());
        let err = err.with_response(serde_json::json!({"group": "living"}));
        assert_eq!(err.response.unwrap()["group"], "living");
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p casa-core error::`
Expected: コンパイルエラー（`GroupPartialFailure` / `response` / `with_response` が未定義）

- [ ] **Step 3: 実装**

`ErrorKind` enum に variant を追加（`ProtocolUnsupported` の後）:

```rust
    /// その操作に対応するアダプタが未実装のプロトコル。
    ProtocolUnsupported,
    /// グループ実行でメンバーの一部（または全部）が失敗した。
    /// メンバー別の成否は stdout のグループ結果 JSON（`CasaError::response`）で判別する。
    GroupPartialFailure,
```

`as_str()` に追加:

```rust
            ErrorKind::GroupPartialFailure => "group_partial_failure",
```

`exit_code()` に追加:

```rust
            ErrorKind::GroupPartialFailure => 15,
```

`CasaError` 構造体とコンストラクタを変更:

```rust
#[derive(Debug)]
pub struct CasaError {
    pub kind: ErrorKind,
    pub detail: String,
    /// エラーでも stdout に出すべき応答（グループ部分失敗のメンバー別結果）。
    /// main が emit してから exit する。
    pub response: Option<serde_json::Value>,
}

impl CasaError {
    pub fn new(kind: ErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            response: None,
        }
    }

    pub fn with_response(mut self, response: serde_json::Value) -> Self {
        self.response = Some(response);
        self
    }
    // exit_code / to_stderr_json は変更なし
```

既存テスト `exit_codes_follow_convention` にも 1 行追加:

```rust
        assert_eq!(ErrorKind::GroupPartialFailure.exit_code(), 15);
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p casa-core error::`
Expected: PASS（既存含め全件）

- [ ] **Step 5: clippy とワークスペース全体のテスト**

Run: `cargo clippy -- -D warnings && cargo test`
Expected: PASS（`CasaError::new` 経由の構築のみなので他モジュールは壊れない）

- [ ] **Step 6: Commit**

```bash
git add crates/casa-core/src/error.rs
git commit -m "feat(error): group_partial_failure (exit 15) と部分失敗レスポンス運搬を追加

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: config — `[groups]` のパースとロード時バリデーション

**Files:**
- Modify: `crates/casa-core/src/config.rs`

**Interfaces:**
- Produces: `pub struct Group { pub members: Vec<String> }`（`Debug, Clone, Serialize, Deserialize` derive）
- Produces: `Config` の新フィールド `pub groups: BTreeMap<String, Group>`（`#[serde(default)]`）
- バリデーション（すべて `ErrorKind::ConfigParse` / exit 10）: メンバーが `devices` に不在 / グループ名がデバイス名と衝突 / `members` が空 / メンバーがグループ名（ネスト）

- [ ] **Step 1: 失敗するテストを書く**

`crates/casa-core/src/config.rs` の `mod tests` に追加:

```rust
    const VALID_WITH_GROUPS: &str = r#"
version = 1

[devices.living_light]
protocol = "matter"
node_id = "1234"

[devices.living_aircon]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x013001"

[groups.living]
members = ["living_light", "living_aircon"]
"#;

    #[test]
    fn parses_groups() {
        let config = parse(VALID_WITH_GROUPS).unwrap();
        let group = config.groups.get("living").unwrap();
        assert_eq!(group.members, vec!["living_light", "living_aircon"]);
    }

    #[test]
    fn config_without_groups_stays_compatible() {
        let config = parse(VALID).unwrap();
        assert!(config.groups.is_empty());
    }

    #[test]
    fn group_member_not_in_devices_is_config_parse() {
        let text = r#"
version = 1
[devices.a]
protocol = "matter"
node_id = "1"
[groups.g]
members = ["a", "ghost"]
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("ghost"), "detail: {}", err.detail);
    }

    #[test]
    fn group_name_colliding_with_device_is_config_parse() {
        let text = r#"
version = 1
[devices.living]
protocol = "matter"
node_id = "1"
[groups.living]
members = ["living"]
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("living"), "detail: {}", err.detail);
    }

    #[test]
    fn empty_group_is_config_parse() {
        let text = r#"
version = 1
[groups.g]
members = []
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("no members"), "detail: {}", err.detail);
    }

    #[test]
    fn nested_group_is_config_parse() {
        let text = r#"
version = 1
[devices.a]
protocol = "matter"
node_id = "1"
[groups.inner]
members = ["a"]
[groups.outer]
members = ["inner"]
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("nested"), "detail: {}", err.detail);
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p casa-core config::`
Expected: コンパイルエラー（`groups` フィールド未定義）

- [ ] **Step 3: 実装**

`Config` にフィールド追加:

```rust
#[derive(Debug, Deserialize)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub devices: BTreeMap<String, Device>,
    /// デバイスをまとめて操作するグループ。書き系（on/off/color-temp/set）のみ対応。
    /// メンバー整合性はロード時に検証済みなので、実行時の名前解決は失敗しない。
    #[serde(default)]
    pub groups: BTreeMap<String, Group>,
    /// 子 CLI バイナリのフルパス上書き（例: `enl = "/opt/bin/enl"`）。
    /// 環境変数 `CASA_<BIN>_BIN` の方が優先される。
    #[serde(default)]
    pub binaries: BTreeMap<String, String>,
}

/// デバイスグループ。ネスト（メンバーにグループ名）は不可。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub members: Vec<String>,
}
```

`parse()` の version チェックの後（`Ok(config)` の前）にバリデーションを追加:

```rust
    for (name, group) in &config.groups {
        if config.devices.contains_key(name) {
            return Err(CasaError::new(
                ErrorKind::ConfigParse,
                format!("group \"{name}\" collides with a device of the same name"),
            ));
        }
        if group.members.is_empty() {
            return Err(CasaError::new(
                ErrorKind::ConfigParse,
                format!("group \"{name}\" has no members"),
            ));
        }
        for member in &group.members {
            if config.devices.contains_key(member) {
                continue;
            }
            let detail = if config.groups.contains_key(member) {
                format!("group \"{name}\" member \"{member}\" is a group; groups cannot be nested")
            } else {
                format!("group \"{name}\" member \"{member}\" is not defined in [devices]")
            };
            return Err(CasaError::new(ErrorKind::ConfigParse, detail));
        }
    }
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p casa-core config::`
Expected: PASS（既存含め全件）

- [ ] **Step 5: clippy とワークスペース全体のテスト**

Run: `cargo clippy -- -D warnings && cargo test`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/casa-core/src/config.rs
git commit -m "feat(config): [groups] のパースとロード時バリデーションを追加

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: runner — 並列 spawn / 回収の `run_parallel`

**Files:**
- Modify: `crates/casa-core/src/runner.rs`

**Interfaces:**
- Consumes: `CasaError` / `ErrorKind`（Task 1 以前から存在）
- Produces: `pub fn run_parallel(commands: &[(String, Vec<String>)]) -> Vec<Result<serde_json::Value, CasaError>>`
  - `commands` は（解決済みバイナリパス, 引数）の列。全子プロセスを先に spawn してから記載順に回収する。
  - 1 メンバーの失敗は他メンバーに影響しない（結果の要素ごとに Ok / Err）。
- 既存 `run(bin, args)` の挙動は不変（内部を共通ヘルパ `collect` に整理するだけ）。

- [ ] **Step 1: 失敗するテストを書く**

`crates/casa-core/src/runner.rs` の `mod tests` に追加:

```rust
    #[test]
    fn run_parallel_returns_results_in_order() {
        let commands = vec![
            ("echo".to_string(), vec![r#"{"n": 1}"#.to_string()]),
            ("echo".to_string(), vec![r#"{"n": 2}"#.to_string()]),
        ];
        let results = run_parallel(&commands);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap()["n"], 1);
        assert_eq!(results[1].as_ref().unwrap()["n"], 2);
    }

    #[test]
    fn run_parallel_isolates_member_failures() {
        let commands = vec![
            ("/nonexistent/casa-child".to_string(), vec![]),
            ("echo".to_string(), vec![r#"{"ok": true}"#.to_string()]),
        ];
        let results = run_parallel(&commands);
        assert_eq!(
            results[0].as_ref().unwrap_err().kind,
            ErrorKind::ChildNotFound
        );
        assert_eq!(results[1].as_ref().unwrap()["ok"], true);
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p casa-core runner::`
Expected: コンパイルエラー（`run_parallel` 未定義）

- [ ] **Step 3: 実装**

まず既存 `run()` の「output → Result<Value>」部分をヘルパに抽出する（挙動は不変）:

```rust
use std::process::{Command, Stdio};
```

```rust
/// 子 CLI を起動し、stdout を JSON としてパースして返す。
///
/// - 起動失敗（バイナリ無し / 実行不可）→ `child_not_found`（exit 12）
/// - 非ゼロ終了 → `child_failed`（子の exit code をそのまま伝播）
/// - stdout が JSON でない → `child_invalid_output`（exit 13）
/// - 子の stderr は呑まず、debug レベルで casa の stderr に転送する
pub fn run(bin: &str, args: &[String]) -> Result<serde_json::Value, CasaError> {
    tracing::debug!(bin, ?args, "spawning child CLI");

    let output = Command::new(bin).args(args).output().map_err(|e| {
        CasaError::new(
            ErrorKind::ChildNotFound,
            format!("failed to execute child CLI \"{bin}\": {e}"),
        )
    })?;

    collect(bin, output)
}

/// 複数の子 CLI を並列に実行する。全子プロセスを先に spawn してから
/// 記載順に回収するので、体感で「同時」に動く。スレッド・非同期ランタイム
/// は使わない（依存ゼロ維持）。子 CLI の出力は小さい JSON なので、逐次
/// 回収でもパイプバッファ詰まりは実質問題にならない。
///
/// 1 要素の失敗（spawn 失敗・非ゼロ終了・不正 JSON）は他の要素に影響しない。
pub fn run_parallel(commands: &[(String, Vec<String>)]) -> Vec<Result<serde_json::Value, CasaError>> {
    let children: Vec<Result<std::process::Child, CasaError>> = commands
        .iter()
        .map(|(bin, args)| {
            tracing::debug!(bin, ?args, "spawning child CLI (parallel)");
            Command::new(bin)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| {
                    CasaError::new(
                        ErrorKind::ChildNotFound,
                        format!("failed to execute child CLI \"{bin}\": {e}"),
                    )
                })
        })
        .collect();

    children
        .into_iter()
        .zip(commands)
        .map(|(child, (bin, _))| {
            let output = child?.wait_with_output().map_err(|e| {
                CasaError::new(
                    ErrorKind::ChildFailed(1),
                    format!("failed to wait for child CLI \"{bin}\": {e}"),
                )
            })?;
            collect(bin, output)
        })
        .collect()
}

/// 終了した子プロセスの output を casa の Result に変換する共通処理。
fn collect(bin: &str, output: std::process::Output) -> Result<serde_json::Value, CasaError> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr.lines().filter(|l| !l.trim().is_empty()) {
        tracing::debug!(child = bin, stderr = line, "child CLI stderr");
    }

    if !output.status.success() {
        // 子 CLI 由来のエラーは元の exit code を保ち、呼び出し側が
        // 「タイムアウトかリジェクトか」等を区別できるようにする。
        let code = output.status.code().unwrap_or(1);
        return Err(CasaError::new(
            ErrorKind::ChildFailed(code),
            format!("\"{bin}\" exited with code {code}: {}", stderr.trim()),
        ));
    }

    serde_json::from_slice(&output.stdout).map_err(|e| {
        CasaError::new(
            ErrorKind::ChildInvalidOutput,
            format!("\"{bin}\" stdout is not valid JSON: {e}"),
        )
    })
}
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p casa-core runner::`
Expected: PASS（既存 2 件 + 新規 2 件）

- [ ] **Step 5: clippy とワークスペース全体のテスト**

Run: `cargo clippy -- -D warnings && cargo test`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/casa-core/src/runner.rs
git commit -m "feat(runner): 子 CLI を並列 spawn する run_parallel を追加

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: output — グループ応答の組み立てと list / validate への groups 反映

**Files:**
- Modify: `crates/casa-core/src/output.rs`
- Modify: `crates/casa-core/src/ops.rs`（`validate` の呼び出し側）
- Modify: `crates/casa/src/main.rs`（`list` の呼び出し側）

**Interfaces:**
- Consumes: `Group`（Task 2）、`CasaError`（Task 1 の `kind` / `exit_code()` / `detail`）
- Produces:
  - `pub fn group_member_result(name: &str, device: &Device, outcome: &Result<Value, CasaError>) -> Value`
    — `{"device", "protocol", "ok", "value"}` または `{"device", "protocol", "ok": false, "error": {"kind", "exit_code", "detail"}}`
  - `pub fn group_response(group: &str, results: Vec<Value>) -> Value` — `{"timestamp", "group", "results"}`
  - `pub fn group_entry(name: &str, group: &Group) -> Value` — list 用 `{"name", "members"}`
  - `pub fn list_response(devices: Vec<Value>, groups: Vec<Value>) -> Value` — **シグネチャ変更**、`"groups"` フィールド追加
  - `pub fn validate_response(path, version, device_count, group_count: usize, protocols, warnings) -> Value` — **シグネチャ変更**、`"group_count"` フィールド追加

- [ ] **Step 1: 失敗するテストを書く**

`crates/casa-core/src/output.rs` の `mod tests` に追加:

```rust
    use crate::config::Group;
    use crate::error::{CasaError, ErrorKind};

    #[test]
    fn group_member_result_ok_shape() {
        let device = Device::Echonet {
            ip: "192.0.2.10".into(),
            eoj: "0x013001".into(),
        };
        let entry =
            group_member_result("living_aircon", &device, &Ok(serde_json::json!({"power": "on"})));
        assert_eq!(entry["device"], "living_aircon");
        assert_eq!(entry["protocol"], "echonet");
        assert_eq!(entry["ok"], true);
        assert_eq!(entry["value"]["power"], "on");
    }

    #[test]
    fn group_member_result_error_shape() {
        let device = Device::Echonet {
            ip: "192.0.2.10".into(),
            eoj: "0x013001".into(),
        };
        let err = CasaError::new(ErrorKind::ChildFailed(3), "timeout");
        let entry = group_member_result("living_aircon", &device, &Err(err));
        assert_eq!(entry["ok"], false);
        assert_eq!(entry["error"]["kind"], "child_failed");
        assert_eq!(entry["error"]["exit_code"], 3);
        assert_eq!(entry["error"]["detail"], "timeout");
        assert!(entry.get("value").is_none());
    }

    #[test]
    fn group_response_has_timestamp_group_results() {
        let response = group_response("living", vec![serde_json::json!({"ok": true})]);
        assert!(response["timestamp"].is_string());
        assert_eq!(response["group"], "living");
        assert_eq!(response["results"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn list_response_includes_groups() {
        let group = Group {
            members: vec!["a".into(), "b".into()],
        };
        let response = list_response(vec![], vec![group_entry("living", &group)]);
        assert_eq!(response["groups"][0]["name"], "living");
        assert_eq!(response["groups"][0]["members"][1], "b");
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p casa-core output::`
Expected: コンパイルエラー（新関数未定義 / `list_response` の引数数不一致）

- [ ] **Step 3: 実装**

`crates/casa-core/src/output.rs` の import を変更:

```rust
use crate::config::{Device, Group};
use crate::error::CasaError;
```

`list_response` を変更:

```rust
/// `casa list` の応答。
pub fn list_response(devices: Vec<Value>, groups: Vec<Value>) -> Value {
    json!({
        "timestamp": timestamp(),
        "devices": devices,
        "groups": groups,
    })
}
```

新関数を追加（`describe_response` の後あたり）:

```rust
/// list 内の 1 グループ分のエントリ。
pub fn group_entry(name: &str, group: &Group) -> Value {
    json!({
        "name": name,
        "members": group.members,
    })
}

/// グループ操作のメンバー 1 件分の結果。エラーは子 CLI の exit code を
/// `error.exit_code` に保存する（単体操作の「exit code 伝播」の等価物）。
pub fn group_member_result(
    name: &str,
    device: &Device,
    outcome: &Result<Value, CasaError>,
) -> Value {
    match outcome {
        Ok(value) => json!({
            "device": name,
            "protocol": device.protocol(),
            "ok": true,
            "value": value,
        }),
        Err(err) => json!({
            "device": name,
            "protocol": device.protocol(),
            "ok": false,
            "error": {
                "kind": err.kind.as_str(),
                "exit_code": err.exit_code(),
                "detail": err.detail,
            },
        }),
    }
}

/// グループ操作（on / off / color-temp / set）の応答。
/// `results` の順序は設定ファイル上のメンバー記載順。
pub fn group_response(group: &str, results: Vec<Value>) -> Value {
    json!({
        "timestamp": timestamp(),
        "group": group,
        "results": results,
    })
}
```

`validate_response` を変更:

```rust
/// `casa validate` の応答。load を通った時点で設定は妥当なので `valid` は常に true。
/// `warnings` は妥当だが実行時に問題になりうる点（アダプタ未実装プロトコル等）。
pub fn validate_response(
    path: &Path,
    version: u32,
    device_count: usize,
    group_count: usize,
    protocols: BTreeMap<&str, u32>,
    warnings: Vec<Value>,
) -> Value {
    json!({
        "timestamp": timestamp(),
        "config": path.display().to_string(),
        "version": version,
        "device_count": device_count,
        "group_count": group_count,
        "protocols": protocols,
        "warnings": warnings,
        "valid": true,
    })
}
```

呼び出し側 1: `crates/casa-core/src/ops.rs` の `validate()` 末尾を変更:

```rust
    output::validate_response(
        path,
        config.version,
        config.devices.len(),
        config.groups.len(),
        protocols,
        warnings,
    )
```

同ファイルの既存テスト `validate_reports_summary_and_flags_protocols_without_adapter` に追加:

```rust
        assert_eq!(report["group_count"], 0);
```

呼び出し側 2: `crates/casa/src/main.rs` の `Command::List` アームを変更:

```rust
        Command::List { describe } => {
            let mut devices = Vec::with_capacity(config.devices.len());
            for (name, device) in &config.devices {
                let mut entry = output::device_entry(name, device);
                if describe {
                    // introspection 未対応のプロトコルは properties: null。
                    entry["properties"] =
                        ops::describe_device(&config, device)?.unwrap_or(serde_json::Value::Null);
                }
                devices.push(entry);
            }
            let groups = config
                .groups
                .iter()
                .map(|(name, group)| output::group_entry(name, group))
                .collect();
            output::list_response(devices, groups)
        }
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test`
Expected: PASS（ワークスペース全体。casad は output を使っていないので影響なし）

- [ ] **Step 5: clippy**

Run: `cargo clippy -- -D warnings`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/casa-core/src/output.rs crates/casa-core/src/ops.rs crates/casa/src/main.rs
git commit -m "feat(output): グループ応答の組み立てと list/validate への groups 反映

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: ops — グループディスパッチ（並列実行・部分失敗 exit 15・読み系の拒否）

**Files:**
- Modify: `crates/casa-core/src/ops.rs`
- Modify: `crates/casa/src/main.rs`（部分失敗時の stdout 出力）

**Interfaces:**
- Consumes: `Config::groups`（Task 2）、`runner::run_parallel`（Task 3）、`output::group_member_result` / `group_response`（Task 4）、`ErrorKind::GroupPartialFailure` / `CasaError::with_response` / `CasaError::response`（Task 1）
- Produces: 既存公開関数のシグネチャは**不変**。`power` / `set` / `color_temp` が `<name>` にグループ名を透過的に受け付け、`get` / `describe` はグループ名を `protocol_unsupported`（exit 14）で拒否する。

- [ ] **Step 1: 失敗するテストを書く**

`crates/casa-core/src/ops.rs` の `mod tests` に追加:

```rust
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

    /// echo を子 CLI の代役にして、run_group の成功パスを通す。
    fn echo_invocation(device: &Device) -> Option<Invocation> {
        let Device::Echonet { ip, .. } = device else {
            panic!("test config only has echonet devices");
        };
        Some(Invocation {
            bin: "echo",
            args: vec![format!(r#"{{"ip": "{ip}"}}"#)],
        })
    }

    #[test]
    fn run_group_collects_member_results_in_config_order() {
        let config = config::parse(GROUP_CONFIG).unwrap();
        let group = config.groups.get("living").unwrap();

        let response =
            run_group(&config, "living", group, "on", |_, device| echo_invocation(device))
                .unwrap();

        assert_eq!(response["group"], "living");
        assert!(response["timestamp"].is_string());
        let results = response["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["device"], "light1");
        assert_eq!(results[0]["ok"], true);
        assert_eq!(results[0]["value"]["ip"], "192.0.2.11");
        assert_eq!(results[1]["device"], "light2");
        assert_eq!(results[1]["value"]["ip"], "192.0.2.12");
    }

    #[test]
    fn run_group_partial_failure_is_exit_15_with_response() {
        let config = config::parse(GROUP_CONFIG).unwrap();
        let group = config.groups.get("living").unwrap();

        // light2 だけ操作未対応（None）にして部分失敗を作る。
        let err = run_group(&config, "living", group, "on", |_, device| match device {
            Device::Echonet { ip, .. } if ip == "192.0.2.11" => echo_invocation(device),
            _ => None,
        })
        .unwrap_err();

        assert_eq!(err.kind, ErrorKind::GroupPartialFailure);
        assert_eq!(err.exit_code(), 15);
        let response = err.response.unwrap();
        let results = response["results"].as_array().unwrap();
        assert_eq!(results[0]["ok"], true);
        assert_eq!(results[1]["ok"], false);
        assert_eq!(results[1]["error"]["kind"], "protocol_unsupported");
    }

    #[test]
    fn power_dispatches_group_names_to_group_pipeline() {
        // enl を存在しないパスに向けることで、「グループ経路に入り、メンバーごとに
        // child_not_found で失敗し、exit 15 が返る」ことを実機なしで検証する。
        let text = format!("{GROUP_CONFIG}\n[binaries]\nenl = \"/nonexistent/enl\"\n");
        let config = config::parse(&text).unwrap();

        let err = power(&config, "living", true).unwrap_err();

        assert_eq!(err.kind, ErrorKind::GroupPartialFailure);
        let response = err.response.unwrap();
        let results = response["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["error"]["kind"], "child_not_found");
        assert_eq!(results[0]["error"]["exit_code"], 12);
    }

    #[test]
    fn get_and_describe_reject_group_names() {
        let config = config::parse(GROUP_CONFIG).unwrap();

        let err = get(&config, "living", "0x80").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
        assert!(err.detail.contains("get"), "detail: {}", err.detail);

        let err = describe(&config, "living").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
        assert!(err.detail.contains("describe"), "detail: {}", err.detail);
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p casa-core ops::`
Expected: コンパイルエラー（`run_group` 未定義）

- [ ] **Step 3: 実装**

`crates/casa-core/src/ops.rs` の import を変更:

```rust
use crate::config::{Config, Device, Group};
```

グループヘルパを追加（`run_for_value` の前あたり）:

```rust
/// グループ書き系操作の共通パイプライン。各メンバーの Invocation を組み、
/// 全子プロセスを並列に spawn してから、設定ファイル上のメンバー記載順に回収する。
///
/// - Invocation を組めないメンバー（アダプタ未実装 / 操作未対応）は spawn せず
///   メンバー別エラーとして results に載せる。
/// - 1 件でも失敗があれば `group_partial_failure`（exit 15）。stdout に出すべき
///   メンバー別結果は `CasaError::response` に載せて main まで運ぶ。
fn run_group(
    config: &Config,
    group_name: &str,
    group: &Group,
    operation: &str,
    build: impl Fn(&'static dyn Adapter, &Device) -> Option<Invocation>,
) -> Result<Value, CasaError> {
    // メンバー名はロード時に検証済みなので device() は失敗しない。
    let members: Vec<(&String, &Device)> = group
        .members
        .iter()
        .map(|m| Ok((m, config.device(m)?)))
        .collect::<Result<_, CasaError>>()?;

    let prepared: Vec<Result<Invocation, CasaError>> = members
        .iter()
        .map(|(_, device)| {
            let adapter = require_adapter(device, operation)?;
            build(adapter, device).ok_or_else(|| unsupported(device, operation))
        })
        .collect();

    let commands: Vec<(String, Vec<String>)> = prepared
        .iter()
        .filter_map(|p| p.as_ref().ok())
        .map(|inv| (runner::resolve_bin(inv.bin, config), inv.args.clone()))
        .collect();
    let mut spawned = runner::run_parallel(&commands).into_iter();

    let outcomes: Vec<Result<Value, CasaError>> = prepared
        .into_iter()
        .map(|p| match p {
            Ok(_) => spawned
                .next()
                .expect("run_parallel returns one result per command"),
            Err(e) => Err(e),
        })
        .collect();

    let failed = outcomes.iter().filter(|o| o.is_err()).count();
    let results: Vec<Value> = members
        .iter()
        .zip(&outcomes)
        .map(|((name, device), outcome)| output::group_member_result(name, device, outcome))
        .collect();
    let response = output::group_response(group_name, results);

    if failed == 0 {
        Ok(response)
    } else {
        Err(CasaError::new(
            ErrorKind::GroupPartialFailure,
            format!(
                "{failed}/{} member(s) of group \"{group_name}\" failed during \"{operation}\"",
                group.members.len()
            ),
        )
        .with_response(response))
    }
}

/// 読み系（get / describe）はグループ非対応。グループ名なら明示エラーにする
/// （黙って name_not_found にすると「なぜ list には出るのに」と混乱するため）。
fn reject_group(config: &Config, name: &str, operation: &str) -> Result<(), CasaError> {
    if config.groups.contains_key(name) {
        return Err(CasaError::new(
            ErrorKind::ProtocolUnsupported,
            format!("groups are not supported for \"{operation}\"; specify a device name"),
        ));
    }
    Ok(())
}
```

書き系 3 関数を変更（シグネチャ不変、冒頭にグループ分岐を追加）:

```rust
/// `casa get <name> <property>`
pub fn get(config: &Config, name: &str, property: &str) -> Result<Value, CasaError> {
    reject_group(config, name, "get")?;
    let device = config.device(name)?;
    let adapter = require_adapter(device, "get")?;
    run_for_value(adapter.get(device, property), config, name, device, "get")
}

/// `casa set <name> <property> <value>`
pub fn set(config: &Config, name: &str, property: &str, value: &str) -> Result<Value, CasaError> {
    if let Some(group) = config.groups.get(name) {
        return run_group(config, name, group, "set", |adapter, device| {
            adapter.set(device, property, value)
        });
    }
    let device = config.device(name)?;
    let adapter = require_adapter(device, "set")?;
    run_for_value(
        adapter.set(device, property, value),
        config,
        name,
        device,
        "set",
    )
}
```

```rust
/// `casa on <name>` / `casa off <name>`
pub fn power(config: &Config, name: &str, on: bool) -> Result<Value, CasaError> {
    let op = if on { "on" } else { "off" };
    if let Some(group) = config.groups.get(name) {
        return run_group(config, name, group, op, |adapter, device| {
            adapter.power(device, on)
        });
    }
    let device = config.device(name)?;
    let adapter = require_adapter(device, op)?;
    run_for_value(adapter.power(device, on), config, name, device, op)
}

/// `casa color-temp <name> --kelvin <k> | --mireds <m> [--transition <s>]`
pub fn color_temp(config: &Config, name: &str, color: &ColorTemp) -> Result<Value, CasaError> {
    if let Some(group) = config.groups.get(name) {
        return run_group(config, name, group, "color-temp", |adapter, device| {
            adapter.color_temp(device, color)
        });
    }
    let device = config.device(name)?;
    let adapter = require_adapter(device, "color-temp")?;
    run_for_value(
        adapter.color_temp(device, color),
        config,
        name,
        device,
        "color-temp",
    )
}
```

`describe` の冒頭にも拒否を追加:

```rust
/// `casa describe <name>`
pub fn describe(config: &Config, name: &str) -> Result<Value, CasaError> {
    reject_group(config, name, "describe")?;
    let device = config.device(name)?;
    match describe_device(config, device)? {
        Some(properties) => Ok(output::describe_response(name, device, properties)),
        None => Err(unsupported(device, "describe")),
    }
}
```

`crates/casa/src/main.rs` のエラーハンドリングを変更（部分失敗でも stdout にグループ結果を出す）:

```rust
    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        // グループ部分失敗はメンバー別結果を stdout に出してから exit 15 する。
        if let Some(response) = &err.response {
            output::emit(response);
        }
        eprintln!("{}", err.to_stderr_json());
        std::process::exit(err.exit_code());
    }
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test`
Expected: PASS（ワークスペース全体）

- [ ] **Step 5: clippy**

Run: `cargo clippy -- -D warnings`
Expected: PASS

- [ ] **Step 6: 手動スモークテスト（ダミー設定・実機なし）**

```bash
cat > /tmp/casa-group-smoke.toml <<'EOF'
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

[binaries]
enl = "/nonexistent/enl"
EOF
cargo run -q -p casa -- --config /tmp/casa-group-smoke.toml list
cargo run -q -p casa -- --config /tmp/casa-group-smoke.toml on living; echo "exit: $?"
cargo run -q -p casa -- --config /tmp/casa-group-smoke.toml get living 0x80; echo "exit: $?"
```

Expected:
- `list` の JSON に `"groups": [{"name": "living", "members": ["light1", "light2"]}]` が含まれる。
- `on living` は stdout にメンバー 2 件の結果 JSON（両方 `"ok": false`, `child_not_found`）、stderr に `group_partial_failure`、`exit: 15`。
- `get living 0x80` は stderr に `protocol_unsupported`、`exit: 14`。

- [ ] **Step 7: Commit**

```bash
git add crates/casa-core/src/ops.rs crates/casa/src/main.rs
git commit -m "feat(ops): グループの並列実行と部分失敗 exit 15 を追加

on/off/color-temp/set の <name> がグループ名を透過的に受け付ける。
get/describe はグループ名を明示エラー（exit 14）で拒否する。

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: ドキュメント — CLI ヘルプと README の更新

**Files:**
- Modify: `crates/casa/src/cli.rs`（ヘルプ文言のみ、引数構造は不変）
- Modify: `README.md`

**Interfaces:** なし（ドキュメントのみ）

- [ ] **Step 1: cli.rs のヘルプ文言を更新**

`On` / `Off` / `Set` / `ColorTemp` の `name` フィールドの doc コメントを
「設定ファイル上のデバイス名」から以下に変更する（`Get` / `Describe` / `List` は変えない）:

```rust
        /// 設定ファイル上のデバイス名またはグループ名
        name: String,
```

- [ ] **Step 2: README を更新**

(a) 「使い方」のコードブロック内、`casa off living_aircon` の行の後に追加:

```bash

# グループ操作（[groups] で定義。メンバー全員へ並列に同時実行）
casa on living
casa off living
```

(b) 「設定ファイル」セクションの TOML 例の末尾に追加:

```toml
# 複数デバイスをまとめて操作するグループ（on / off / color-temp / set のみ対応）
[groups.living]
members = ["living_light", "living_aircon"]
```

(c) exit code の表の `| 14 | そのプロトコルでは未対応の操作 |` の行の直後に追加:

```markdown
| 15 | グループ実行でメンバーの一部（または全部）が失敗 |
```

(d) exit code の表の直後（「子 CLI 由来のエラーは…」の段落の後）に追加:

```markdown
グループ実行の部分失敗（exit 15）でも stdout にはメンバー別結果の JSON が出る。
どのメンバーが何の exit code で失敗したかは `results[].error.exit_code` で判別できる。
```

(e) 「使い方」の下の `<property>` の説明段落の近く（`casa validate` の説明の後など、流れが自然な位置）にグループの説明を追加。以下の `~~~~` 内をそのまま README に書く（内側の ``` はそのまま）:

~~~~markdown
### グループ

`devices.toml` の `[groups]` で複数デバイスをひとつの名前にまとめられる。
`on` / `off` / `color-temp` / `set` はグループ名を透過的に受け付け、全メンバーの
子 CLI を並列に spawn して同時実行する（プロトコル混在可）。`get` / `describe` は
グループ非対応（exit 14）。結果はメンバー別の JSON:

```json
{
  "timestamp": "2026-07-06T12:34:56+09:00",
  "group": "living",
  "results": [
    {"device": "living_light", "protocol": "matter", "ok": true, "value": {}},
    {"device": "living_aircon", "protocol": "echonet", "ok": false,
     "error": {"kind": "child_failed", "exit_code": 3, "detail": "..."}}
  ]
}
```

全員成功なら exit 0、1 件でも失敗したら exit 15（`group_partial_failure`）。
~~~~

- [ ] **Step 3: ビルドとテストの確認**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: PASS（ヘルプ文言変更のみなので通るはず）

- [ ] **Step 4: Commit**

```bash
git add crates/casa/src/cli.rs README.md
git commit -m "docs: グループ実行の使い方と exit 15 を README / ヘルプに追記

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: バージョン bump

**Files:**
- Modify: `Cargo.toml`（workspace 共通 version）
- Modify: `Cargo.lock`（ビルドで自動更新）

**Interfaces:** なし

- [ ] **Step 1: バージョンを上げる**

`Cargo.toml` の 6 行目 `version = "0.4.0"` を:

```toml
version = "0.5.0"
```

- [ ] **Step 2: Cargo.lock を更新してテスト**

Run: `cargo build && cargo test && cargo clippy -- -D warnings`
Expected: PASS。`casa --version` が `casa 0.5.0` を出す（`cargo run -q -p casa -- --version` で確認）。

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: バージョンを 0.5.0 に bump（グループ実行追加）

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```
