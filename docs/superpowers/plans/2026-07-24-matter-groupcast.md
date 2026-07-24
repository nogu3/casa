# Matter groupcast 対応 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** casa の Matter デバイスに `group` アドレッシングを足し、`on`/`off`/`invoke` を `mat group invoke`（wire groupcast）へ委譲できるようにして、書斎 `desk_light` の groupcast 化に対応する。

**Architecture:** `protocol` は `matter` のまま、既存 `Device::Matter` variant を `node_id`（unicast）と `group`（groupcast）の排他 2 択に拡張する。matter adapter が addressing mode で分岐し、group のとき `mat group <cmd> --group <g>` を組む。casad は無変更（`casa on/off` を spawn するだけ）だが、variant 変更に伴う compile 追随のみ行う。実装後 jarvis へ casa バイナリを再配布し、実機 config を書き換える。

**Tech Stack:** Rust（workspace: casa-core / casa / casad）、clap、serde、toml、cargo test / clippy。子 CLI は `mat`。

## Global Constraints

- Rust edition 2021、workspace 統一 version。本作業で `1.2.0` → `1.3.0`（minor）へ bump。
- `cargo build` / `cargo test` / `cargo clippy -- -D warnings` が全て通ること（clippy は警告もエラー扱い＝dead_code 不可）。
- casa の設計原則を破らない: プロトコルのバイト列を組まない、stdout は純 JSON、状態を持たない。group の値（alias / GroupId）は casa が解釈せず `--group` にパススルー、解決は `mat`。
- 出力 JSON の `protocol` は node/group いずれも `"matter"`。
- 後方互換: 既存の `node_id` 指定 Matter デバイスの挙動・引数列は不変。
- コミットはセッション中に編集したファイルのみ add する。
- コミットメッセージ末尾に以下を付ける:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01QM1GLB9m2ZaGSKGSFhS83L
  ```

---

### Task 1: config に group フィールドと排他バリデーションを足し、workspace を green に保つ

`Device::Matter` の `node_id` を `Option` 化し `group` を追加。ロード時に「node_id / group のちょうど一方」を検証。variant 変更で壊れる全 compile サイト（adapter の interim 対応、casad の 2 サイト、既存テスト）を追随させ、`cargo test` / `clippy` を green に戻す。この時点では group はまだ機能せず（`mat group` 呼び出しは Task 2）、group デバイスへの操作は exit 14 になる。

**Files:**
- Modify: `crates/casa-core/src/config.rs`（Matter variant / parse() バリデーション / 既存テスト2件）
- Modify: `crates/casa-core/src/adapter/matter.rs:27-32`（`address()` を Option 対応の interim に）
- Modify: `crates/casa-core/src/adapter/mod.rs:84-87`（テスト構築に `group: None`）
- Modify: `crates/casa-core/src/adapter/matter.rs:115-127`（テスト構築に `group: None`）
- Modify: `crates/casad/src/engine.rs:172-175`（Matter マッチを Option 対応に）
- Modify: `crates/casad/src/rules.rs:273-284`（Matter マッチを Option 対応に、group は trigger 源として拒否）

**Interfaces:**
- Produces: `Device::Matter { node_id: Option<String>, group: Option<String>, endpoint: Option<u32> }`。
  ロード後は「node_id と group のちょうど一方が Some」が保証される（parse() が検証）。
- Consumes: なし（起点タスク）。

- [ ] **Step 1: config テストを追加（group 正常・両指定・両欠落）**

`crates/casa-core/src/config.rs` の `mod tests` 内に追加:

```rust
    #[test]
    fn parses_matter_group_device() {
        let text = r#"
version = 1
[devices.desk_room_lights]
protocol = "matter"
group = "desk_room_lights"
"#;
        let config = parse(text).unwrap();
        match config.device("desk_room_lights").unwrap() {
            Device::Matter {
                node_id,
                group,
                endpoint,
            } => {
                assert_eq!(*node_id, None);
                assert_eq!(group.as_deref(), Some("desk_room_lights"));
                assert_eq!(*endpoint, None);
            }
            other => panic!("unexpected device: {other:?}"),
        }
    }

    #[test]
    fn matter_with_both_node_id_and_group_is_config_parse() {
        let text = r#"
version = 1
[devices.x]
protocol = "matter"
node_id = "17"
group = "desk_room_lights"
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(
            err.detail.contains("both node_id and group"),
            "detail: {}",
            err.detail
        );
    }

    #[test]
    fn matter_with_neither_node_id_nor_group_is_config_parse() {
        let text = r#"
version = 1
[devices.x]
protocol = "matter"
endpoint = 1
"#;
        let err = parse(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(
            err.detail.contains("exactly one of node_id or group"),
            "detail: {}",
            err.detail
        );
    }
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `cargo test -p casa-core parses_matter_group_device`
Expected: コンパイルエラー（`group` フィールドがまだ無い）で FAIL。

- [ ] **Step 3: Matter variant を変更**

`crates/casa-core/src/config.rs` の Matter variant（現 49-56 行）を差し替え:

```rust
    Matter {
        /// unicast 宛先の node_id。`group` と排他（ロード時に検証）。
        #[serde(default)]
        node_id: Option<String>,
        /// Matter wire group（groupcast）の宛先。mat の group alias / GroupId を
        /// パススルーする（casa は解釈しない。alias→GroupId 解決は mat）。`node_id` と排他。
        #[serde(default)]
        group: Option<String>,
        /// OnOff/invoke ショートカットが使うエンドポイント。未指定なら mat 既定(1)。
        #[serde(default)]
        endpoint: Option<u32>,
    },
```

- [ ] **Step 4: parse() に排他バリデーションを追加**

`crates/casa-core/src/config.rs` の `parse()` 内、groups バリデーション `for` ループの直後（現 174 行の `}` の後、`Ok(config)` の前）に追加:

```rust
    // Matter デバイスは node_id / group のちょうど一方が必須（排他）。
    for (name, device) in &config.devices {
        if let Device::Matter {
            node_id, group, ..
        } = device
        {
            match (node_id, group) {
                (Some(_), None) | (None, Some(_)) => {}
                (Some(_), Some(_)) => {
                    return Err(CasaError::new(
                        ErrorKind::ConfigParse,
                        format!(
                            "matter device \"{name}\" sets both node_id and group; exactly one is required"
                        ),
                    ));
                }
                (None, None) => {
                    return Err(CasaError::new(
                        ErrorKind::ConfigParse,
                        format!(
                            "matter device \"{name}\" requires exactly one of node_id or group"
                        ),
                    ));
                }
            }
        }
    }
```

- [ ] **Step 5: 既存 config テストのパターンを Option 対応に更新**

`crates/casa-core/src/config.rs` の `parses_matter_devices` 相当テスト（現 245-258 行）を差し替え:

```rust
        match config.device("living_light").unwrap() {
            Device::Matter {
                node_id, endpoint, ..
            } => {
                assert_eq!(node_id.as_deref(), Some("1234"));
                assert_eq!(*endpoint, None);
            }
            other => panic!("unexpected device: {other:?}"),
        }
        match config.device("strip_outlet2").unwrap() {
            Device::Matter {
                node_id, endpoint, ..
            } => {
                assert_eq!(node_id.as_deref(), Some("5678"));
                assert_eq!(*endpoint, Some(2));
            }
            other => panic!("unexpected device: {other:?}"),
        }
```

（既存 `matter_missing_node_id_is_config_parse` は「endpoint のみ」= 両欠落なので、新メッセージ `requires exactly one of node_id or group` が `node_id` を含み、既存アサーション `contains("node_id")` はそのまま通る。変更不要。）

- [ ] **Step 6: adapter `address()` を Option 対応の interim に更新**

`crates/casa-core/src/adapter/matter.rs` の `address()`（現 27-32 行）を差し替え。group（node_id None）は interim では `None` を返し、全操作が exit 14 になる（group 機能は Task 2）:

```rust
/// デバイス定義から (node_id, on/off 用エンドポイント) を取り出す。group 指定
/// （node_id なし）の場合は interim で None（Task 2 で groupcast 対応）。
fn address(device: &Device) -> Option<(&str, Option<u32>)> {
    match device {
        Device::Matter {
            node_id: Some(node_id),
            endpoint,
            ..
        } => Some((node_id, *endpoint)),
        _ => None,
    }
}
```

- [ ] **Step 7: adapter テストの Device 構築に `group: None` を足す**

`crates/casa-core/src/adapter/matter.rs` のテストヘルパ（現 115-127 行）を差し替え:

```rust
    fn device() -> Device {
        Device::Matter {
            node_id: Some("1234".into()),
            group: None,
            endpoint: None,
        }
    }

    fn device_on_endpoint(ep: u32) -> Device {
        Device::Matter {
            node_id: Some("1234".into()),
            group: None,
            endpoint: Some(ep),
        }
    }
```

`crates/casa-core/src/adapter/mod.rs` のテスト（現 84-87 行）を差し替え:

```rust
        let device = Device::Matter {
            node_id: Some("1234".into()),
            group: None,
            endpoint: None,
        };
```

- [ ] **Step 8: casad の Matter マッチ 2 サイトを Option 対応に更新**

`crates/casad/src/engine.rs` の `matter_event_matches`（現 172-175 行）を差し替え。group デバイスは event trigger 源になれない（node_id が無いので mat listen の node_id と突合不能）ため `false`:

```rust
    let (node_id, endpoint) = match config.device(device) {
        Ok(Device::Matter {
            node_id: Some(node_id),
            endpoint,
            ..
        }) => (node_id, endpoint),
        _ => return false,
    };
```

`crates/casad/src/rules.rs` の `check_matter_device`（現 273-284 行の match アーム）を差し替え。group Matter は trigger 源として明示的に拒否:

```rust
        casa_core::config::Device::Matter {
            node_id: Some(node_id),
            ..
        } => {
            if parse_node_id(node_id).is_none() {
                return Err(CasaError::new(
                    ErrorKind::ConfigParse,
                    format!(
                        "rule \"{rule_name}\": device \"{device}\" node_id \"{node_id}\" is not numeric"
                    ),
                ));
            }
            Ok(())
        }
        casa_core::config::Device::Matter { node_id: None, .. } => Err(CasaError::new(
            ErrorKind::ConfigParse,
            format!(
                "rule \"{rule_name}\": device \"{device}\" is a matter group (groupcast) with no node_id; a matter event trigger requires a node_id device"
            ),
        )),
```

- [ ] **Step 9: workspace 全体をビルド・テスト**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: 全 PASS、clippy 警告 0。新規 3 テスト（group 正常・両指定・両欠落）が通る。

- [ ] **Step 10: Commit**

```bash
git add crates/casa-core/src/config.rs crates/casa-core/src/adapter/matter.rs crates/casa-core/src/adapter/mod.rs crates/casad/src/engine.rs crates/casad/src/rules.rs
git commit -m "$(cat <<'EOF'
feat(config): Matter デバイスに group フィールドを追加（node_id と排他）

protocol=matter のまま node_id を Option 化し、Matter wire group（groupcast）
宛先の group を追加。ロード時に「node_id/group のちょうど一方」を検証する。
groupcast の実処理は次コミット。casad は group デバイスを event trigger 源
として拒否する。

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01QM1GLB9m2ZaGSKGSFhS83L
EOF
)"
```

---

### Task 2: matter adapter に group アドレッシング（power / invoke）を実装

`address()` を「node か group か」を表す内部 enum に置き換え、`power`（on/off）と `invoke` に group アームを足す。group の `get`/`set`/`describe` は非対応（`None` → exit 14）。node 側の引数列は完全維持。

**Files:**
- Modify: `crates/casa-core/src/adapter/matter.rs`（`address()` 置換、`power`/`invoke` に group 分岐、group テスト追加）

**Interfaces:**
- Consumes: `Device::Matter { node_id: Option<String>, group: Option<String>, endpoint: Option<u32> }`（Task 1）。ロード後は一方が Some で保証。
- Produces: なし（adapter 内部の変更。外部シグネチャ不変）。

- [ ] **Step 1: group の失敗するテストを追加**

`crates/casa-core/src/adapter/matter.rs` の `mod tests` に、group デバイスヘルパと各テストを追加:

```rust
    fn group_device() -> Device {
        Device::Matter {
            node_id: None,
            group: Some("desk_room_lights".into()),
            endpoint: None,
        }
    }

    #[test]
    fn group_power_on_builds_mat_group_invoke() {
        let inv = MatterAdapter.power(&group_device(), true).unwrap();
        assert_eq!(inv.bin, "mat");
        assert_eq!(
            args(&inv),
            [
                "group",
                "invoke",
                "--group",
                "desk_room_lights",
                "--cluster",
                "onoff",
                "--command",
                "on"
            ]
        );
    }

    #[test]
    fn group_power_off_builds_mat_group_invoke() {
        let inv = MatterAdapter.power(&group_device(), false).unwrap();
        assert_eq!(
            args(&inv),
            [
                "group",
                "invoke",
                "--group",
                "desk_room_lights",
                "--cluster",
                "onoff",
                "--command",
                "off"
            ]
        );
    }

    #[test]
    fn group_power_with_endpoint_passes_flag() {
        let dev = Device::Matter {
            node_id: None,
            group: Some("desk_room_lights".into()),
            endpoint: Some(2),
        };
        let inv = MatterAdapter.power(&dev, true).unwrap();
        assert_eq!(
            args(&inv),
            [
                "group",
                "invoke",
                "--group",
                "desk_room_lights",
                "--cluster",
                "onoff",
                "--command",
                "on",
                "--endpoint",
                "2"
            ]
        );
    }

    #[test]
    fn group_invoke_shortcut_injects_group_and_passes_args() {
        let extra: Vec<String> = vec!["--kelvin".into(), "2700".into()];
        let inv = MatterAdapter
            .invoke(&group_device(), "color-temp", &extra)
            .unwrap();
        assert_eq!(
            args(&inv),
            [
                "group",
                "color-temp",
                "--group",
                "desk_room_lights",
                "--kelvin",
                "2700"
            ]
        );
    }

    #[test]
    fn group_invoke_arbitrary_passes_through() {
        let extra: Vec<String> = vec![
            "--cluster".into(),
            "onoff".into(),
            "--command".into(),
            "on".into(),
        ];
        let inv = MatterAdapter
            .invoke(&group_device(), "invoke", &extra)
            .unwrap();
        assert_eq!(
            args(&inv),
            [
                "group",
                "invoke",
                "--group",
                "desk_room_lights",
                "--cluster",
                "onoff",
                "--command",
                "on"
            ]
        );
    }

    #[test]
    fn group_get_set_describe_are_unsupported() {
        assert!(MatterAdapter.get(&group_device(), "onoff/on-off").is_none());
        assert!(MatterAdapter
            .set(&group_device(), "onoff/on-off", "1")
            .is_none());
        assert!(MatterAdapter.describe(&group_device()).is_none());
    }
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `cargo test -p casa-core group_power_on_builds_mat_group_invoke`
Expected: FAIL（power が group で `None` を返す interim 実装のため `.unwrap()` が panic）。

- [ ] **Step 3: `address()` を MatterAddr enum に置換**

`crates/casa-core/src/adapter/matter.rs` の interim `address()`（Task 1 で入れたもの）を差し替え:

```rust
/// Matter の addressing mode。node（unicast）か group（groupcast）。
/// ロード時バリデーションでちょうど一方が保証されるので、両立/両欠落は来ない。
enum MatterAddr<'a> {
    Node {
        node_id: &'a str,
        endpoint: Option<u32>,
    },
    Group {
        group: &'a str,
        endpoint: Option<u32>,
    },
}

fn address(device: &Device) -> Option<MatterAddr<'_>> {
    match device {
        Device::Matter {
            node_id: Some(node_id),
            endpoint,
            ..
        } => Some(MatterAddr::Node {
            node_id,
            endpoint: *endpoint,
        }),
        Device::Matter {
            group: Some(group),
            endpoint,
            ..
        } => Some(MatterAddr::Group {
            group,
            endpoint: *endpoint,
        }),
        _ => None,
    }
}

/// `--endpoint <ep>` を（設定にあれば）末尾に足す。node/group 共通。
fn push_endpoint(args: &mut Vec<String>, endpoint: Option<u32>) {
    if let Some(ep) = endpoint {
        args.push("--endpoint".to_string());
        args.push(ep.to_string());
    }
}
```

- [ ] **Step 4: `get`/`set`/`describe` を node 限定に更新**

`crates/casa-core/src/adapter/matter.rs` の `get`/`set`/`describe`（現 61-84 行）を、`MatterAddr::Node` のときだけ組むよう差し替え。group は `None`（exit 14）:

```rust
    fn get(&self, device: &Device, property: &str) -> Option<Invocation> {
        let MatterAddr::Node { node_id, .. } = address(device)? else {
            return None;
        };
        let mut args = vec!["read".to_string(), "--node".to_string(), node_id.to_string()];
        args.extend(selector_flags(property)?);
        Some(invocation(args))
    }

    fn set(&self, device: &Device, property: &str, value: &str) -> Option<Invocation> {
        let MatterAddr::Node { node_id, .. } = address(device)? else {
            return None;
        };
        let mut args = vec!["write".to_string(), "--node".to_string(), node_id.to_string()];
        args.extend(selector_flags(property)?);
        args.push("--value".to_string());
        args.push(value.to_string());
        Some(invocation(args))
    }

    fn describe(&self, device: &Device) -> Option<Invocation> {
        let MatterAddr::Node { node_id, .. } = address(device)? else {
            return None;
        };
        Some(invocation(vec![
            "describe".to_string(),
            "--node".to_string(),
            node_id.to_string(),
        ]))
    }
```

- [ ] **Step 5: `power` に group 分岐を実装**

`crates/casa-core/src/adapter/matter.rs` の `power`（現 86-95 行）を差し替え:

```rust
    fn power(&self, device: &Device, on: bool) -> Option<Invocation> {
        let cmd = if on { "on" } else { "off" };
        match address(device)? {
            MatterAddr::Node { node_id, endpoint } => {
                let mut args =
                    vec![cmd.to_string(), "--node".to_string(), node_id.to_string()];
                push_endpoint(&mut args, endpoint);
                Some(invocation(args))
            }
            MatterAddr::Group { group, endpoint } => {
                // groupcast: `mat group invoke --group <g> --cluster onoff --command on|off`
                let mut args = vec![
                    "group".to_string(),
                    "invoke".to_string(),
                    "--group".to_string(),
                    group.to_string(),
                    "--cluster".to_string(),
                    "onoff".to_string(),
                    "--command".to_string(),
                    cmd.to_string(),
                ];
                push_endpoint(&mut args, endpoint);
                Some(invocation(args))
            }
        }
    }
```

- [ ] **Step 6: `invoke` に group 分岐を実装**

`crates/casa-core/src/adapter/matter.rs` の `invoke`（現 99-108 行）を差し替え:

```rust
    /// endpoint は設定にあれば注入する（`power` と同じ流儀）。group では `mat group`
    /// サブコマンドを 1 語 prepend し、`--group` を注入して残りを素通しする。
    fn invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation> {
        match address(device)? {
            MatterAddr::Node { node_id, endpoint } => {
                let mut all =
                    vec![command.to_string(), "--node".to_string(), node_id.to_string()];
                push_endpoint(&mut all, endpoint);
                all.extend(args.iter().cloned());
                Some(invocation(all))
            }
            MatterAddr::Group { group, endpoint } => {
                let mut all = vec![
                    "group".to_string(),
                    command.to_string(),
                    "--group".to_string(),
                    group.to_string(),
                ];
                push_endpoint(&mut all, endpoint);
                all.extend(args.iter().cloned());
                Some(invocation(all))
            }
        }
    }
```

- [ ] **Step 7: 全テスト・clippy を実行**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: 全 PASS（group 6 テスト＋既存 node テスト回帰無し）、clippy 警告 0。

- [ ] **Step 8: Commit**

```bash
git add crates/casa-core/src/adapter/matter.rs
git commit -m "$(cat <<'EOF'
feat(matter): group アドレッシングで groupcast on/off/invoke を実装

group 指定の Matter デバイスは `mat group invoke --group <g> --cluster onoff
--command on|off`（on/off）、`mat group <cmd> --group <g> args`（invoke）へ委譲。
get/set/describe は非対応（exit 14、groupcast は unacknowledged で読めない）。
node 側の引数列は不変。

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01QM1GLB9m2ZaGSKGSFhS83L
EOF
)"
```

---

### Task 3: docs 追記と version bump

README と CLAUDE.md に Matter group アドレッシングを記載し、workspace version を `1.3.0` へ上げる。

**Files:**
- Modify: `README.md`（Matter デバイスの項）
- Modify: `CLAUDE.md`（Phase 4 の Matter 節）
- Modify: `Cargo.toml`（`version = "1.3.0"`）

**Interfaces:**
- Consumes: Task 2 完了時点の group 挙動（on/off/invoke 対応、get/set/describe 非対応）。
- Produces: なし。

- [ ] **Step 1: README に group アドレッシングを追記**

`README.md` の Matter 説明箇所（`node_id` / `on`/`off` を説明しているデバイス設定セクション）に、次の趣旨の段落を追加する（既存の記述スタイル・見出し階層に合わせること）:

```markdown
Matter デバイスは `node_id`（unicast）の代わりに `group` を指定すると、Matter の
wire group（groupcast / multicast）宛てになる。`group` の値は `mat` の group alias
または GroupId で、casa は解釈せず `mat group ... --group <値>` にパススルーする
（alias→GroupId 解決は `mat`）。`node_id` と `group` はちょうど一方のみ指定できる
（両方・両欠落は config エラー、exit 10）。

groupcast で対応する操作は `on` / `off` / `invoke` のみ。groupcast は unacknowledged
（応答が返らない）ため `get` / `set` / `describe` は非対応（exit 14）。

    [devices.desk_room_lights]
    protocol = "matter"
    group = "desk_room_lights"
```

- [ ] **Step 2: CLAUDE.md の Matter 節に 1 行追記**

`CLAUDE.md` の「Phase 4 onward」内 **Matter** の項の末尾に追記:

```markdown
  In the config, a Matter device is addressed by either `node_id` (unicast) or `group` (Matter wire groupcast); exactly one is required. A `group` device delegates `on`/`off`/`invoke` to `mat group ...` (multicast); `get`/`set`/`describe` are unsupported (groupcast is unacknowledged, exit 14).
```

- [ ] **Step 3: version を bump**

`Cargo.toml` の `[workspace.package]` の `version = "1.2.0"` を `version = "1.3.0"` に変更。

- [ ] **Step 4: ビルドして version 反映を確認**

Run: `cargo build && cargo run -p casa -- --version`
Expected: `casa 1.3.0`（clap の version 表示）。ビルド成功。

- [ ] **Step 5: Commit**

```bash
git add README.md CLAUDE.md Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
docs: Matter group アドレッシングを文書化し 1.3.0 に bump

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01QM1GLB9m2ZaGSKGSFhS83L
EOF
)"
```

---

### Task 4: casa バイナリを jarvis へ再配布

despliegue skill に従い casa を aarch64 クロスビルドして jarvis へ配布・差し替えする。casad は無変更なので配布対象は casa バイナリのみ。

**Files:**
- なし（ビルド成果物の配布のみ。リポジトリ変更なし）

**Interfaces:**
- Consumes: Task 3 までのコミット済み実装（version 1.3.0）。
- Produces: jarvis 上の `casa` バイナリが 1.3.0（group 対応版）になる。

- [ ] **Step 1: despliegue skill を起動して手順に従う**

`Skill(despliegue)` を起動。casa（1.3.0）を aarch64 クロスビルド → jarvis へ scp → 差し替え。
skill の手順（ビルド先・配置パス・権限）に完全に従うこと。

- [ ] **Step 2: jarvis 上で version を確認**

Run: `ssh jarvis 'casa --version'`
Expected: `casa 1.3.0`。

- [ ] **Step 3: group 未反映状態での回帰確認（既存 node 動作）**

Run: `ssh jarvis 'casa off desk_tape_light'`
Expected: 従来どおり成功（`mat off --node 6` 相当）、exit 0。既存挙動に回帰が無いことを確認。

---

### Task 5: jarvis の config を反映し実機検証

`desk_room_lights` デバイスを追加し、書斎ルールの `desk_light` を groupcast へ差し替え、実機で検証する。これらの config は jarvis-iac 管理外の手管理ファイルのため ssh 直編集する。

**Files:**
- Modify（jarvis 実機）: `~/.config/casa/devices.toml`
- Modify（jarvis 実機）: `~/.config/casa/rules.toml`

**Interfaces:**
- Consumes: Task 4 で配布した group 対応 casa。
- Produces: 書斎 人感トリガが desk_light を groupcast で制御する運用状態。

- [ ] **Step 1: 既存 config を退避**

Run:
```bash
ssh jarvis 'cp ~/.config/casa/devices.toml ~/.config/casa/devices.toml.bak-groupcast && cp ~/.config/casa/rules.toml ~/.config/casa/rules.toml.bak-groupcast && echo backed-up'
```
Expected: `backed-up`。

- [ ] **Step 2: devices.toml に desk_room_lights を追加**

`~/.config/casa/devices.toml` の末尾（`desk_light` エントリの後）に追記する。desk_light エントリ自体は残す（他用途・回帰確認用）。追記内容:

```toml
# 書斎デスクライトの Matter wire group（groupcast）。mat aliases.toml の
# [groups] desk_room_lights = 11 と対応。人感トリガはこちらを groupcast で叩く。
[devices.desk_room_lights]
protocol = "matter"
group = "desk_room_lights"
```

- [ ] **Step 3: devices.toml が valid か検証**

Run: `ssh jarvis 'casa --config ~/.config/casa/devices.toml list >/dev/null && echo config-ok'`
Expected: `config-ok`（exit 0）。パースエラー（両指定/両欠落）が無いこと。

- [ ] **Step 4: rules.toml の書斎 2 ルールを groupcast へ差し替え**

`~/.config/casa/rules.toml` の 2 ルールを次のとおり編集（`desk_tape_light` はそのまま、`desk_light` → `desk_room_lights`）:

```toml
[[rules]]
name = "書斎 人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = [
  { action = "off", device = "desk_tape_light" },
  { action = "off", device = "desk_room_lights" },
]

[[rules]]
name = "書斎 人感ONで点灯"
when = { device = "study_motion", attribute = "occupancy", equals = 1 }
then = [
  { action = "on", device = "desk_tape_light" },
  { action = "on", device = "desk_room_lights" },
]
```

- [ ] **Step 5: groupcast 単発を実機で検証**

Run: `ssh jarvis 'casa off desk_room_lights'`
Expected: exit 0。casa の stdout JSON に `protocol: "matter"` と mat の "sent" 相当の `value` が入る。実機の書斎デスクライトが消灯する。続けて `casa on desk_room_lights` で点灯を確認。
（RUST_LOG=debug を付ければ子 CLI 呼び出しが `mat group invoke --group desk_room_lights --cluster onoff --command off` であることを stderr で確認できる。）

- [ ] **Step 6: casad を再起動してルールを反映**

Run: `ssh jarvis 'sudo systemctl restart casad'`（サービス名は jarvis skill の ENVIRONMENT に従う。restart は確認不要の許可操作）
Expected: 起動成功。`ssh jarvis 'systemctl status casad --no-pager'` で active、rules.toml のパースエラー（group が trigger 源でない等）が無いこと。

- [ ] **Step 7: 人感トリガの発火を検証**

書斎の人感センサー（study_motion）を実際に反応させる（在室→退室、または既知の手段）。
Run: `ssh jarvis 'journalctl -u casad --since "2 min ago" --no-pager | tail -30'`
Expected: `firing rule ... device="desk_room_lights" action="on"/"off"` のログが出て、実機のデスクライトが groupcast で点灯/消灯する。desk_tape_light も従来どおり連動する。

- [ ] **Step 8: 検証結果を報告**

groupcast 単発・人感トリガ両方の検証結果（成功/ログ抜粋）をユーザーへ報告する。config 変更は手管理のため、将来の IaC 化（jarvis-iac issue #3）対象であることも申し添える。

---

## 完了条件

- casa-core: Matter が node_id / group の排他 2 択、group で `mat group invoke` を組む。`cargo test` / `cargo clippy -- -D warnings` green。
- version 1.3.0、README / CLAUDE.md 追記済み。
- jarvis の casa が 1.3.0。`casa on/off desk_room_lights` が groupcast を撃つ。
- 書斎 人感トリガが desk_room_lights を groupcast 制御。desk_tape_light は unicast のまま連動。
