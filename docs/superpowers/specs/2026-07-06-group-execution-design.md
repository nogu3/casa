# casa グループ実行 設計

日付: 2026-07-06
ステータス: 承認済み

## 目的

複数デバイス（例: mat のリビング系照明）をひとつの名前でまとめ、`casa on living` のように
1 コマンドで同時に操作できるようにする。casa のステートレス原則・子 CLI 委譲原則は維持する。

## 設定（devices.toml）

```toml
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
```

- `groups` はトップレベルの新テーブル。`Group { members: Vec<String> }`。
- 混在プロトコル可（アダプタ層が吸収するため制限しない）。
- ロード時バリデーション（すべて `config_parse` / exit 10）:
  - メンバー名が `devices` に存在しない。
  - グループ名がデバイス名と衝突する。
  - `members` が空。
  - メンバーにグループ名を指定（ネスト不可）。
- `version` は 1 のまま（`groups` 省略時は従来と完全互換のため bump しない）。

## コマンド UX

- **グループ対応（書き系のみ）**: `on` / `off` / `color-temp` / `set`。
  既存コマンドの `<name>` がグループ名を透過的に受け付ける。
  デバイス名との衝突は設定エラーにするので名前解決に曖昧さはない。
- **グループ非対応（読み系）**: `get` / `describe` にグループ名を渡すと
  `protocol_unsupported`（exit 14）で「groups are not supported for "get"」等の明示エラー。
- `casa list` の出力に `groups` フィールドを追加（グループ名とメンバー一覧）。
- `casa validate` のサマリにグループ数（`group_count`）を含める。
  メンバー整合性は load 時点で検証済みなので追加チェックは不要。

## 実行モデル（並列）

- `runner` に複数 Invocation を並列実行する関数を追加:
  全子プロセスを `Command::spawn()`（stdout/stderr piped）で先に起動し、
  その後順に `wait_with_output()` で回収する。
- スレッド・非同期ランタイムは使わない（依存ゼロ維持）。
  子 CLI の出力は小さい JSON なのでパイプバッファ詰まりは実質問題にならない。
- 失敗があっても全メンバー分を実行・回収する（fail-fast しない）。

## 出力スキーマ

```json
{
  "timestamp": "2026-07-06T12:34:56+09:00",
  "group": "living",
  "results": [
    {"device": "living_light", "protocol": "matter", "ok": true, "value": {"...": "..."}},
    {"device": "living_aircon", "protocol": "echonet", "ok": false,
     "error": {"kind": "child_failed", "exit_code": 3, "detail": "..."}}
  ]
}
```

- 単体デバイス操作の出力スキーマは変更しない。
- `results` の順序は設定ファイル上のメンバー記載順。

## exit code

| code | 意味 |
|---|---|
| 0 | グループ全員成功 |
| 15 | グループ内に 1 件以上の失敗（新 kind: `group_partial_failure`） |

- 13（child_invalid_output）・14（protocol_unsupported）は使用済みのため 15。
- 部分失敗時も stdout にはメンバー別結果 JSON を出し、stderr に
  `{"error": {"kind": "group_partial_failure", ...}}` を 1 行出す。
  呼び出し側（casad / cron）は JSON で詳細を判断できる。
- メンバー個別の子 CLI exit code は `results[].error.exit_code` に保存する
  （単体操作の「exit code 伝播」の等価物）。

## 変更範囲

| ファイル | 変更 |
|---|---|
| `crates/casa-core/src/config.rs` | `groups: BTreeMap<String, Group>` 追加 + ロード時バリデーション |
| `crates/casa-core/src/runner.rs` | 並列 spawn/wait 関数追加 |
| `crates/casa-core/src/ops.rs` | 書き系 4 操作で名前がグループなら並列実行に分岐 |
| `crates/casa-core/src/output.rs` | `group_response` 追加、`list_response` に groups |
| `crates/casa-core/src/error.rs` | `ErrorKind::GroupPartialFailure` → exit 15 |
| `crates/casa/src/cli.rs`, `main.rs` | 変更ほぼなし（名前解決が透過的なため） |
| `crates/casa-core/src/config.rs` | `Config::ensure_target` 追加（device/group どちらでも存在チェックのみ行う） |
| `crates/casad/src/main.rs` | `exec` の spawn 前検証を `config.device()` → `config.ensure_target()` に変更（グループ名を許可） |
| `crates/casad/src/rules.rs` | `then.device`（アクション対象）の検証を `ensure_target` 経由に変更しグループを許可。`when.device`（イベント発火元）は実デバイスが必要なため `config.device()` のまま |

casad は casa を子プロセスとして呼ぶだけだが、casad 自身も spawn 前に名前を検証しており
（`casad exec` と rules.toml の `then.device`）、そこは devices のみ参照だったためグループ名を
弾いていた。最終レビューで発見し、上記 2 箇所を `Config::ensure_target` 経由に変更して
グループ名を通すよう修正した（当初の「casad 側変更ゼロ」という想定は誤りだった）。

## テスト

- config: groups 正常系 / メンバー不在 / 名前衝突 / 空 members / ネスト指定の各エラー。
- ops: ダミー子 CLI（echo 等）でグループ実行の成功・部分失敗・exit 15 を統合テスト。
- get / describe にグループ名を渡すと exit 14 になること。
- `casa list` / `casa validate` の groups 反映。

## スコープ外

- ネストグループ。
- グループへの読み系操作（get / describe の集約）。
- 並列度の制御（同時実行数の上限）。
- シーン（メンバーごとに異なる値を設定する機能）。必要になったら別途設計。
