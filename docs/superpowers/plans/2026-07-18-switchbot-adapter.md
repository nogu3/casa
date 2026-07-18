# SwitchBot アダプタ（クラウド制御）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** casa が SwitchBot デバイスをクラウド API 経由（自作 `swb` CLI）で on/off・invoke 操作できるようにする。

**Architecture:** 既存のアダプタ trait パターンに従い `SwitchbotAdapter` を追加する。アダプタは `Invocation { bin, args }` を組むだけで、ops 層・subcommand ハンドラは一切変更しない。`power`（on/off → `swb cmd <id> turnOn/turnOff`）と `invoke`（`swb <command> <id> [args]`、device_id を位置引数注入）のみ実装し、`get`/`set`/`describe` は trait 既定の `None`（exit 14）のまま。

**Tech Stack:** Rust, 既存 `casa-core` crate。外部依存の追加なし。

## Global Constraints

- バイナリ名は `swb`（`const BIN: &str = "swb"`）。
- casa は認証情報を扱わない（`SWITCHBOT_TOKEN`/`SWITCHBOT_SECRET` は子プロセスに環境変数継承で swb 側が使う）。
- config スキーマ変更なし。`Device::Switchbot { device_id }` は既存。
- テスト・サンプルの device_id はダミー値のみ（公開リポジトリ・実 ID 禁止。例 `"DUMMY-XX-XX"`）。
- `cargo build` / `cargo test` / `cargo clippy -- -D warnings` が通ること。
- 最低前提 `swb` バージョン: `0.1.0`（`status` / `cmd` サブコマンドと exit code 規約を前提）。

---

### Task 1: SwitchbotAdapter（power / invoke）を追加

**Files:**
- Create: `crates/casa-core/src/adapter/switchbot.rs`
- Modify: `crates/casa-core/src/adapter/mod.rs`（`pub mod switchbot;` 追加、`adapter_for` の switchbot 分岐を実装に差し替え、`switchbot_has_no_adapter_yet` テスト書き換え）

**Interfaces:**
- Consumes: `super::{Adapter, Invocation}`、`crate::config::Device`（既存 `Device::Switchbot { device_id }`）。`Adapter` trait は `power(&self, device, on: bool)` と `invoke(&self, device, command: &str, args: &[String])` を含み、いずれも `Option<Invocation>` を返す。未実装メソッド（`get`/`set`/`describe`）は trait 既定で `None`。
- Produces: `pub struct SwitchbotAdapter;`（`impl Adapter`）。`adapter_for(&Device::Switchbot{..})` が `Some(&switchbot::SwitchbotAdapter)` を返す。

- [ ] **Step 1: 失敗するアダプタ単体テストを書く**

`crates/casa-core/src/adapter/switchbot.rs` を新規作成し、まずモジュール本体の空実装とテストを書く（テストが参照する `SwitchbotAdapter` はまだ無いのでコンパイル失敗する）。ファイル全体を以下にする:

```rust
//! SwitchBot アダプタ。実体は自作 `swb`（SwitchBot クラウド API v1.1 ラッパ）の
//! サブプロセス呼び出し。**公式 CLI ではない**。
//!
//! SwitchBot クラウド API には単一プロパティ read/write が無い（`status` は全状態を
//! 一括で返す GET のみ、制御は `cmd` によるコマンド送信）。そのため casa の
//! `get`/`set`/`describe` はこのプロトコルでは未対応（trait 既定の `None` = exit 14）。
//! 読み取りは `casa invoke <name> status`、制御は `on`/`off` と `casa invoke <name> cmd <command>`。
//!
//! アドレス（device_id）は swb では常にサブコマンド直後の第 1 位置引数に来る
//! （`status <device>` / `cmd <device> <command>`）。Matter の `--node` フラグ注入とは
//! 対照的に、swb は位置引数注入。認証は swb 側の責務で、casa は何も渡さない。

use super::{Adapter, Invocation};
use crate::config::Device;

const BIN: &str = "swb";

pub struct SwitchbotAdapter;

/// デバイス定義から device_id を取り出す。dispatch は `adapter_for` が variant で
/// 行うので、他 variant が来ることはない。
fn device_id(device: &Device) -> Option<&str> {
    match device {
        Device::Switchbot { device_id } => Some(device_id),
        _ => None,
    }
}

fn invocation(args: Vec<String>) -> Invocation {
    Invocation { bin: BIN, args }
}

impl Adapter for SwitchbotAdapter {
    /// `on`/`off` は SwitchBot の turnOn/turnOff コマンド送信。
    fn power(&self, device: &Device, on: bool) -> Option<Invocation> {
        let id = device_id(device)?;
        let command = if on { "turnOn" } else { "turnOff" };
        Some(invocation(vec![
            "cmd".to_string(),
            id.to_string(),
            command.to_string(),
        ]))
    }

    /// 長尾のプロトコル固有操作。`command` は swb のサブコマンド名（`status` / `cmd` 等）を
    /// そのまま受け取り、device_id をサブコマンド直後の第 1 位置引数に注入して後続 `args` を
    /// 素通しする。
    fn invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation> {
        let id = device_id(device)?;
        let mut all = vec![command.to_string(), id.to_string()];
        all.extend(args.iter().cloned());
        Some(invocation(all))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device::Switchbot {
            device_id: "DUMMY-XX-XX".into(),
        }
    }

    fn args(inv: &Invocation) -> Vec<&str> {
        inv.args.iter().map(String::as_str).collect()
    }

    #[test]
    fn power_on_sends_turn_on_command() {
        let inv = SwitchbotAdapter.power(&device(), true).unwrap();
        assert_eq!(inv.bin, "swb");
        assert_eq!(args(&inv), ["cmd", "DUMMY-XX-XX", "turnOn"]);
    }

    #[test]
    fn power_off_sends_turn_off_command() {
        let inv = SwitchbotAdapter.power(&device(), false).unwrap();
        assert_eq!(args(&inv), ["cmd", "DUMMY-XX-XX", "turnOff"]);
    }

    #[test]
    fn invoke_status_injects_device_id_as_positional() {
        let inv = SwitchbotAdapter.invoke(&device(), "status", &[]).unwrap();
        assert_eq!(inv.bin, "swb");
        assert_eq!(args(&inv), ["status", "DUMMY-XX-XX"]);
    }

    #[test]
    fn invoke_cmd_passes_command_and_args_through() {
        let extra: Vec<String> = vec!["turnOn".into()];
        let inv = SwitchbotAdapter.invoke(&device(), "cmd", &extra).unwrap();
        assert_eq!(args(&inv), ["cmd", "DUMMY-XX-XX", "turnOn"]);
    }

    #[test]
    fn invoke_passes_trailing_flags_through() {
        let extra: Vec<String> = vec!["setBrightness".into(), "--param".into(), "50".into()];
        let inv = SwitchbotAdapter.invoke(&device(), "cmd", &extra).unwrap();
        assert_eq!(
            args(&inv),
            ["cmd", "DUMMY-XX-XX", "setBrightness", "--param", "50"]
        );
    }

    #[test]
    fn get_set_describe_are_unsupported() {
        assert!(SwitchbotAdapter.get(&device(), "power").is_none());
        assert!(SwitchbotAdapter.set(&device(), "power", "on").is_none());
        assert!(SwitchbotAdapter.describe(&device()).is_none());
    }
}
```

- [ ] **Step 2: `mod.rs` に module 宣言と dispatch を配線し、既存テストを書き換える**

`crates/casa-core/src/adapter/mod.rs` を編集する。

module 宣言に追加（`pub mod matter;` の下）:

```rust
pub mod echonet;
pub mod matter;
pub mod switchbot;
```

`adapter_for` の switchbot 分岐を差し替え、Phase 4 コメントを更新:

```rust
pub fn adapter_for(device: &Device) -> Option<&'static dyn Adapter> {
    match device {
        Device::Echonet { .. } => Some(&echonet::EchonetAdapter),
        Device::Matter { .. } => Some(&matter::MatterAdapter),
        // SwitchBot: 自作 `swb`（クラウド API v1.1 ラッパ）を呼ぶ。公式 CLI ではない。
        Device::Switchbot { .. } => Some(&switchbot::SwitchbotAdapter),
    }
}
```

`switchbot_has_no_adapter_yet` テストを削除し、以下に置き換える:

```rust
    #[test]
    fn switchbot_devices_dispatch_to_switchbot_adapter() {
        let device = Device::Switchbot {
            device_id: "DUMMY-XX-XX".into(),
        };
        let adapter = adapter_for(&device).unwrap();
        assert_eq!(adapter.power(&device, true).unwrap().bin, "swb");
    }
```

- [ ] **Step 3: テストを実行して通ることを確認**

Run: `cargo test -p casa-core adapter`
Expected: PASS（`switchbot::tests` の 6 テスト、`adapter::tests::switchbot_devices_dispatch_to_switchbot_adapter` を含む全アダプタテストが通る）

- [ ] **Step 4: clippy を通す**

Run: `cargo clippy -p casa-core -- -D warnings`
Expected: 警告なしで終了。

- [ ] **Step 5: コミット**

```bash
git add crates/casa-core/src/adapter/switchbot.rs crates/casa-core/src/adapter/mod.rs
git commit -m "feat(adapter): SwitchBot アダプタ（swb クラウド制御）を追加

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CFmcbkE2SePyJq3w3RvLgj"
```

---

### Task 2: validate の no_adapter テストを更新

**Files:**
- Modify: `crates/casa-core/src/ops.rs`（`validate_reports_summary_and_flags_protocols_without_adapter` テスト）

**Interfaces:**
- Consumes: Task 1 で switchbot がアダプタを持つようになった事実。`validate(&Config, &Path) -> Value` は既存。`validate` は各デバイスについて `adapter_for` が `None` のとき `warnings` に `{"kind": "no_adapter", ...}` を積む。全プロトコルにアダプタがある今、実 `Device` variant では no_adapter 警告は発生しない。
- Produces: なし（テストのみの変更）。

- [ ] **Step 1: 失敗を確認する**

Run: `cargo test -p casa-core validate_reports_summary_and_flags_protocols_without_adapter`
Expected: FAIL。既存テストは switchbot デバイスに no_adapter 警告 1 件を期待するが、Task 1 で switchbot がアダプタを持ったため `warnings` が空になり、`assert_eq!(warnings.len(), 1)` で落ちる。

- [ ] **Step 2: テストを「全デバイスにアダプタがあり警告ゼロ」を検証する形に書き換える**

`ops.rs` の `validate_reports_summary_and_flags_protocols_without_adapter` テスト関数を丸ごと以下に置き換える（関数名も改める）:

```rust
    #[test]
    fn validate_reports_summary_and_emits_no_adapter_warnings_when_all_have_adapters() {
        let text = r#"
version = 1
[devices.aircon]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x013001"
[devices.lock]
protocol = "switchbot"
device_id = "DUMMY-XX-XX"
"#;
        let config = config::parse(text).unwrap();
        let report = validate(&config, std::path::Path::new("/tmp/devices.toml"));

        assert_eq!(report["valid"], true);
        assert_eq!(report["version"], 1);
        assert_eq!(report["device_count"], 2);
        assert_eq!(report["config"], "/tmp/devices.toml");
        assert_eq!(report["protocols"]["echonet"], 1);
        assert_eq!(report["protocols"]["switchbot"], 1);
        assert_eq!(report["group_count"], 0);
        assert!(report["timestamp"].is_string());

        // echonet / switchbot ともアダプタ実装済みなので no_adapter 警告は出ない。
        let warnings = report["warnings"].as_array().unwrap();
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
    }
```

- [ ] **Step 3: テストが通ることを確認**

Run: `cargo test -p casa-core validate_reports_summary_and_emits_no_adapter_warnings_when_all_have_adapters`
Expected: PASS

- [ ] **Step 4: crate 全体のテストを実行**

Run: `cargo test -p casa-core`
Expected: 全 PASS

- [ ] **Step 5: コミット**

```bash
git add crates/casa-core/src/ops.rs
git commit -m "test(adapter): switchbot 実装に伴い validate の no_adapter テストを更新

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CFmcbkE2SePyJq3w3RvLgj"
```

---

### Task 3: ドキュメントと version bump

**Files:**
- Modify: `README.md`（兄弟 CLI 表、on/off 対応表、invoke/describe の switchbot 記述、最低 swb バージョン）
- Modify: `CLAUDE.md`（Phase 4 の SwitchBot 節）
- Modify: `Cargo.toml`（ワークスペース version `0.7.1` → `0.8.0`）

**Interfaces:**
- Consumes: Task 1〜2 で確定した挙動（on/off = `swb cmd turnOn/turnOff`、読み取り = `invoke status`、get/set/describe 未対応）。
- Produces: なし（ドキュメント・メタデータのみ）。

- [ ] **Step 1: README の SwitchBot 記述を更新**

`README.md` を編集する。まず現状の該当箇所を確認する:

Run: `grep -n "SwitchBot\|switchbot" README.md`

以下を反映する（既存の表現・行に合わせて編集する。行番号は grep 結果に従う）:

1. 兄弟 CLI 表の SwitchBot 行（`| SwitchBot | \`switchbot\` | — | Not supported (Phase 4) |` 付近）を、self-authored `swb` を呼ぶ・クラウド制御対応済みに更新する。例:
   `| SwitchBot | \`swb\` | on / off / invoke | Self-authored (cloud API v1.1). BLE scan plane not yet integrated. |`
2. on/off 対応表（`| SwitchBot | No | No | Not supported ... |` 付近）を、on/off 対応済み（`swb cmd turnOn`/`turnOff`）に更新する。get/set は「単一プロパティ read/write が SwitchBot クラウドに無いため未対応」と明記する。
3. describe の段落（`SwitchBot is not supported (\`casa describe\` returns exit 14 ...)` 付近）は挙動としては変わらない（describe は引き続き exit 14）ので、理由を「swb にプロパティマップ introspection が無い」に更新する。
4. invoke の段落（`Because SwitchBot has no adapter yet, \`invoke\` itself returns exit 14` 付近）を、switchbot は invoke 対応済み・`casa invoke <name> status` で全状態取得、`casa invoke <name> cmd <command>` で任意コマンド送信、と更新する。
5. 最低 `swb` バージョン `0.1.0` を、他の子 CLI 最低バージョン記述に倣って追記する。
6. 実機 E2E の手順（`SWITCHBOT_TOKEN`/`SWITCHBOT_SECRET` を export し、`casa on <name>` / `casa invoke <name> status` を実デバイスに対して叩く）を、既存の enl/mat 手動テスト節と同じ体裁で README に追記する（CI では回さない旨も明記）。

- [ ] **Step 2: CLAUDE.md の Phase 4 SwitchBot 節を更新**

`CLAUDE.md` の Phase 4「SwitchBot」節（`**SwitchBot**: Write an adapter that calls the official \`switchbot\` CLI` で始まる箇所）を、自作 `swb`（クラウド API v1.1 ラッパ）を呼ぶ前提に書き換える。要点:
- 公式 CLI ではなく自作 `swb` を呼ぶ。認証は swb 側に閉じ、casa は認証情報を渡さない。
- on/off = `swb cmd turnOn`/`turnOff`。読み取りは単一プロパティ read が無いため `casa invoke <name> status` 経由（`get`/`set`/`describe` は未対応 = exit 14）。
- BLE スキャン平面は将来の別作業でスコープ外。

兄弟 CLI 命名表（`| SwitchBot | \`switchbot\` | Uses the official CLI ...`）も `swb`・self-authored に更新する。

- [ ] **Step 3: version を bump**

`Cargo.toml`（ワークスペースルート）の `version = "0.7.1"` を `version = "0.8.0"` に変更する。

- [ ] **Step 4: ビルドとテストで整合を確認**

Run: `cargo build && cargo test`
Expected: 全 PASS（version 変更で `Cargo.lock` が更新される）。

- [ ] **Step 5: コミット**

```bash
git add README.md CLAUDE.md Cargo.toml Cargo.lock
git commit -m "docs: SwitchBot（swb クラウド制御）対応を記載し 0.8.0 に bump

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CFmcbkE2SePyJq3w3RvLgj"
```

---

## 最終確認

- [ ] `cargo build` / `cargo test` / `cargo clippy -- -D warnings` が全て通る。
- [ ] `casa validate` が switchbot デバイスに対し no_adapter 警告を出さない（Task 2 で検証済み）。
- [ ] README / CLAUDE.md が swb クラウド制御対応を反映している。
