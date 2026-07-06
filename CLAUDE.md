# CLAUDE.md

`casa` — スマートホーム横断クライアント。プロトコル固有 CLI（`enl` 等）をサブプロセスで呼び、名前マッピングと統一 UX を提供する。

> 名前: **`casa`** 確定。
> リポジトリ: **パブリック / 独立リポジトリ**。
> 設定ファイル: **別リポジトリで管理**（このリポジトリには含めない）。

---

## プロジェクトの目的と立ち位置

スマートホームの操作対象は ECHONET Lite / SwitchBot / Matter と複数プロトコルにまたがる。これらを**ひとつのバイナリに統合せず**、プロトコルごとの薄い CLI（`enl` 等）で実装し、**横断 UX をその上に薄く乗せる**のが基本構想。casa はその横断レイヤを担う。

### casa の責務
- 人間に優しい名前 → (プロトコル, アドレス, オブジェクト) の解決
- 設定ファイルの読み込み・バリデーション
- プロトコル固有 CLI への一貫したラッパ UX

### casa の非責務
- プロトコルの実装。バイト列を組まない、UDP を投げない、Matter スタックを抱えない。**すべて子プロセスに委譲する。**
- スケジューリング・常駐・状態保持。
- 設定ファイル自体の所有。設定は利用者が別管理する（後述）。

---

## 兄弟 CLI 命名規則

casa から呼ばれる前提のプロトコル固有 CLI は、以下の方針で揃える:

- **公式 CLI が存在するプロトコルは公式名をそのまま採用する**（命名規則より公式に従う）。
- **自作 CLI はプロトコル頭字語の短い名前**で揃える（`enl` 等）。

| プロトコル | CLI 名 | 状態 |
|---|---|---|
| ECHONET Lite | `enl` | 自作・開発中（独立リポジトリ） |
| SwitchBot | `switchbot` | 公式 CLI を利用（`@switchbot/openapi-cli`、OpenAPI 経由）。将来自作に切り替える可能性あり。 |
| Matter | `mat` | 自作 CLI（chip-tool ラッパ、独立リポジトリ）。casa アダプタ対応済み。`mtr` は既存 network diagnostic と衝突するため使わない |

casa は **これらが `PATH` 上に存在すること**を前提とする。

---

## 絶対に守る設計原則

1. **プロトコルを直接喋らない**
   バイト列・ソケット・プロトコルスタックを casa に持ち込まない。持ち込みたくなったら、それは新しい兄弟 CLI を作るべきサイン。
2. **stdout は純粋な構造化 JSON のみ**
   子 CLI の出力をパースし、casa のスキーマに正規化して再出力する。人間装飾は混ぜない。
3. **診断は stderr に構造化ログ**（`tracing`）
   子 CLI の stderr も呑み込まず、少なくとも debug レベルで残す。
4. **設定ファイル以外の状態を持たない**
   キャッシュ DB なし、デーモンなし、内部スケジューラなし。

---

## 設定ファイル

### 場所と所有
- 既定パス: `$XDG_CONFIG_HOME/casa/devices.toml`（既定 `~/.config/casa/devices.toml`）。
- パスは `--config <path>` および環境変数 `CASA_CONFIG` で上書き可能。
- **設定ファイル自体は casa リポジトリで管理しない**。利用者が別リポジトリ（プライベート想定）で持ち、`~/.config/casa/` に配置 or シンボリックリンクする運用とする。

### フォーマット: TOML

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

- 名前（キー）は snake_case 推奨。
- `protocol` で casa が呼ぶ子 CLI を決める。
- プロトコル固有フィールドは子 CLI の引数にそのまま渡す。

### リポジトリ内のサンプル・テスト
- このリポジトリに含めるサンプルは **必ずダミー値のみ**（RFC 5737 `192.0.2.0/24` 等）。
- 実 IP・実 MAC・実機 ID をリポジトリに含めない（パブリックのため）。

### マイグレーション
- `version` フィールドで判別。
- **自動マイグレーションはしない**（明示コマンドで実行）。設定ファイルは利用者の所有物なので静かに書き換えない。

---

## 技術スタック

| 領域 | 採用 | 備考 |
|---|---|---|
| 言語 | Rust | enl と同一 |
| CLI | `clap` (derive) | |
| サブプロセス | `std::process::Command` | 依存ゼロ |
| 設定パース | `toml` crate | |
| JSON | `serde` + `serde_json` | 子 CLI の出力パース |
| ログ | `tracing` + `tracing-subscriber` | stderr 出力 |

依存は最小限。casa の核は外部プロセス起動と JSON パースなので過度な手書きはしない。

---

## アーキテクチャ

```
ユーザー / cron / n8n / その他オーケストレータ
        │
        ▼
      casa  ◄── devices.toml (別管理)
        │
        │  Command::new("enl") / "switchbot" / "mat"
        ▼
   プロトコル固有 CLI (stdout = JSON)
        │
        ▼
   実機（UDP / BLE / IP / クラウド API）
```

### 子 CLI バイナリの解決
- 既定: `PATH` から `enl` / `switchbot` / `mat` を解決。
- オーバーライド: 環境変数（`CASA_ENL_BIN` 等）または設定ファイルでフルパス指定可。
- 起動失敗（バイナリ無し／実行不可）は専用 exit code で即判別できること。

### 子 CLI とのバージョン互換
- 結合は **stdout JSON スキーマのみ**。crate 依存しない＝SemVer 追従不要。
- 子 CLI のスキーマが破壊変更されたら casa 側で吸収する。
- README に **想定する子 CLI 最低バージョン**を書く。

---

## 規約

### stdout
- 成功時は結果データを JSON で stdout に出す。
- 子 CLI 出力をそのまま流さず、**casa のスキーマで再構成**する（プロトコル抽象化の責務）。
- **`timestamp` フィールドを必須**とする（ISO 8601、casa が応答を組み立てた時刻）。上層（常駐プロセス・キャッシュ）がフレッシュネス判定に使える。
- 例:
  ```json
  {
    "timestamp": "2026-06-02T12:34:56+09:00",
    "device": "living_aircon",
    "protocol": "echonet",
    "value": { "power": "on" }
  }
  ```

### stderr
- 子 CLI のエラーは構造化ログで stderr に流す。
- casa 自体のエラーも同じ形式: `{"error": {"kind": "...", "detail": "..."}}`。
- `kind` 例: `config_missing` / `config_parse` / `name_not_found` / `child_not_found` / `child_failed`。

### exit code
| code | 意味 |
|---|---|
| 0 | 成功 |
| 2 | CLI 引数エラー（clap 既定） |
| 10 | 設定ファイル無し / パース失敗 |
| 11 | 名前が設定ファイルに無い |
| 12 | 子 CLI バイナリが見つからない / 実行不可 |
| 13 | 子 CLI の stdout が JSON としてパースできない |
| 14 | そのプロトコルでは未対応の操作 |
| 15 | グループ実行でメンバーの一部（または全部）が失敗 |
| その他 | **子 CLI の exit code をそのまま伝播** |

子 CLI 由来のエラーは元のコードを保つことで、呼び出し側が「タイムアウトかリジェクトか」等を区別できる。

---

## 常駐・状態が必要なユースケースは casa の外に置く

将来、自作 Web ページや LLM からの呼び出し、状態変化の購読、キャッシュなどが要件として出てくる。これらは **casa（bin）に足さない**。casa の上にもう一層 `casad` を置いて吸収する。

> **重要（実態）**: `casad` は**同一ワークスペースの別 crate（`crates/casad`）・別バイナリ**として実装済み。守るべき境界は「リポジトリ」ではなく「**プロセス／状態**」—— casa(bin) はステートレスのまま、casad が常駐・状態・スケジューラを持つ。ssh と sshd が同じ OpenSSH リポジトリの別バイナリであるのと同じ関係。設定ロード・名前解決などの純ロジックは `casa-core`(lib) を両者で共有し、実機アクションは casad が casa を子プロセスとして呼ぶ（ハイブリッド）。

```
Web ページ / LLM / その他クライアント
       │
       ▼
   casad（常駐・状態を持つ。crates/casad・別プロセス）
       │
       │ プロセス起動（casa を CLI として呼ぶ）
       ▼
   casa（ステートレスを維持。crates/casa）
       │
       ▼
   enl / switchbot / mat
```

### この分離を守る理由
- casa は cron からも n8n からも `casad` からも等価に叩ける。常駐が落ちていてもデバッグできる。
- `casad` を後から別言語（TypeScript 等）で書ける。LLM 系は TS エコシステムが厚いため現実的な選択肢になる。
- `casad` を捨てて作り直せる。casa が無事なので影響範囲が閉じる。
- 「キャッシュを持つ主体は常駐するもの」という原則を守れる。casa にキャッシュを足すと状態管理が連鎖し、Home Assistant 化の第一歩になる。

### `casad` 側が担う責務（casa の責務ではない）
- 自動化ルール DSL（`rules.toml`）の評価エンジン — **実装済み**:
  - 時刻トリガ（内部スケジューラ）/ イベントトリガ（`enl listen` をループで回して INF 通知に反応）
  - 発火時は casa を子プロセスとして呼ぶ（`casad run` / `check`）
- ECHONET の INF 通知の購読（`enl listen` 経由。enl 側が「listen は外部ループから回す」設計）— **実装済み**
- HTTP / WebSocket / MCP サーバ — 未実装（将来）
- 値のキャッシュとフレッシュネス管理 — 未実装（将来）
- LLM からの Function Calling 受け口 — 未実装（将来。rules.toml は LLM / UI 生成を想定）
- 認証・認可・レート制限 — 未実装（将来）

### casa 側が `casad` のために守るべきこと（既に満たしている）
- `--config <path>` で設定ファイルパスを渡せる（毎回 `$XDG_CONFIG_HOME` を読まない）。
- stdout JSON に `timestamp` を必ず含める（キャッシュ判断に使える）。
- exit code を子 CLI から伝播する（上層がリトライ判断できる）。

`casad` は本ワークスペースの `crates/casad` に実装する（別リポジトリにはしない）。casa(bin) の
ステートレス原則を壊さない限り、casad の機能拡張はこのリポジトリ内で進めてよい。

---

## ロードマップ

Claude Code が casa を実装するときのリファレンス。フェーズは**順番に**進める。前フェーズが完全に終わる（全テストが通る・受け入れ基準を満たす）まで次フェーズに進まない。

各フェーズは以下を定義する:
- **ゴール**: そのフェーズで何が出来上がるか。
- **スコープ**: このフェーズでやること。
- **スコープ外**: 小さく見えてもこのフェーズではやらないこと。
- **完了条件**: 明確な受け入れ基準。

---

### Phase 0 — プロジェクト雛形

**ゴール**: 設定ファイルを読んでデバイス一覧を出せるだけの Rust プロジェクトをビルド可能にする。子 CLI はまだ呼ばない。

**スコープ**:
- `clap`(derive)・`serde`・`serde_json`・`toml`・`tracing`・`tracing-subscriber` を入れた Cargo プロジェクト。
- サブコマンド1つ: `casa list` だけの CLI 雛形。
- 設定ローダ: 既定パス／`--config`／`CASA_CONFIG` から TOML を読む。
- 設定のバリデーション（プロトコルごとの必須フィールド、未知プロトコルはエラー）。
- `casa list` は全デバイスを JSON で stdout に出す。
- `tracing` のログは stderr へ。レベルは `RUST_LOG` で制御。
- exit code `0` / `2` / `10` / `11` が規約通り動く。

**スコープ外**:
- 子 CLI の呼び出し。
- `get` / `set` / `describe` / `on` / `off`。

**完了条件**:
- `cargo build`・`cargo test`・`cargo clippy -- -D warnings` がすべて通る。
- 設定パースのユニットテストを揃える: 正常系、ファイル無し、TOML 不正、未知プロトコル、必須フィールド欠落。
- ダミー設定（`192.0.2.0/24`）で `casa list` が正しい JSON を出す。
- 設定ファイル無しで起動すると exit code `10` で stderr に構造化エラー。

**enl への依存なし**。enl が未完成でもこのフェーズは完成・出荷できる。

---

### Phase 1 — enl 連携（get / set）

**ゴール**: casa が名前で ECHONET Lite 機器を読み書きできる。実体は `enl` をサブプロセス呼び出し。

**前提**: `enl get` と `enl set` が stdout に安定した JSON を出して出荷されていること。exit code も enl 側 CLAUDE.md の規約通り。

**スコープ**:
- 「子ランナー」モジュール: バイナリ名と引数を受け取って起動し、stdout/stderr を捕捉、JSON を返すかエラーを返す。
- 子バイナリは `PATH` 解決。`CASA_ENL_BIN` または設定ファイルでフルパス上書き可。
- `casa get <name> <epc>`:
  - `<name>` を設定から (IP, EOJ) に解決。
  - `enl get --ip <ip> --eoj <eoj> --epc <epc>`（最終的なフラグ名は enl の出荷に合わせる）を呼ぶ。
  - enl の JSON を casa スキーマに再整形して stdout に出す。
- `casa set <name> <epc> <value>`: 同様。
- exit code 伝播: enl が `3`（タイムアウト）や `4`（機器リジェクト）で終了したら casa も同じコードで終了。casa 自身のエラーは `10` / `11` / `12`。
- 子 stderr は呑まず debug レベルで casa の stderr へ転送。

**スコープ外**:
- SwitchBot・Matter・その他プロトコル。
- introspection（`describe`）。
- ON/OFF ショートカット。

**完了条件**:
- `cargo test` が**ダミー `enl` バイナリ**（固定 JSON を吐くスクリプト or テストヘルパ）を使った統合テストを含む。CI で実 enl は不要。
- 実機相手の手動 E2E テストが README に記載されている（CI には載せない）。
- stderr エラーの `kind` 値が安定していて文書化されている。

---

### Phase 2 — Introspection とショートカット

**ゴール**: casa を日常使いで気持ちよくする。

**前提**: `enl describe`（プロパティマップ introspection）が出荷済み。

**スコープ**:
- `casa describe <name>`: 子 CLI の introspection を呼ぶ（enl ならプロパティマップ）。
- `casa on <name>` / `casa off <name>`: 高頻度操作のショートカット。ECHONET Lite では EPC `0x80` に `0x30`/`0x31` をマップする。マッピング表はプロトコルごとに casa 内ハードコード（プロトコルロジックではなく UX なので OK）。
- `casa list` に、その session 中の最新プロパティマップを任意でインクルードできるよう拡張（**永続キャッシュは追加しない**）。

**スコープ外**:
- 永続キャッシュや DB。
- SwitchBot/Matter 対応。

**完了条件**:
- `cargo test` が ECHONET Lite の ON/OFF マッピングをカバー。
- README に各プロトコルの `on`/`off` 対応状況とマッピング先を記載。

---

### Phase 3 — マルチプロトコル化（リファクタのみ）

**ゴール**: 2 つ目のプロトコルを書き直しなしで追加できる状態にする。新プロトコルはまだ足さない。

**スコープ**:
- 子ランナーとサブコマンドハンドラをリファクタし、新プロトコル追加が以下だけで済むようにする:
  1. プロトコル enum に variant 追加。
  2. そのプロトコルの CLI 引数を組むアダプタを追加。
  3. アダプタのテストを追加。
- 設定の `protocol` フィールドが dispatch の唯一の真実とする。

**スコープ外**:
- 実際に SwitchBot や Matter を足すこと（公式 `switchbot` / 自作 `mat` 等の準備ができてから）。

**完了条件**:
- アダプタ trait か関数テーブルが明確に存在する。
- テストで仮想アダプタを足してもサブコマンドハンドラを一切触らずに済む。

---

### Phase 4 以降 — 実プロトコル追加

各追加は Phase 3 形式のアダプタ 1 個分。サブコマンドや設定スキーマは（`protocol` の新値以外）変更しない。

- **SwitchBot**: 公式 `switchbot` CLI（`@switchbot/openapi-cli`）を呼ぶアダプタを書く。**自作 CLI は書かない**。認証は公式 CLI 側が完結させるので casa は credentials を扱わない（環境変数や設定ファイルから何かを渡す必要も基本的にない）。OpenAPI 経由＝クラウド往復のためレイテンシ特性が enl と異なる点だけ呼び出し側に伝える。
  - 将来、BLE 直接制御やローカル完結が必要になった場合は自作 CLI（`sbl` 等）に切り替える可能性がある。その場合も casa 側は `protocol = "switchbot"` のディスパッチ先バイナリを変えるだけで済むよう、アダプタは公式 CLI の存在を前提に閉じ込めること。
- **Matter**: **対応済み**。自作 `mat`（chip-tool ラッパ）を呼ぶアダプタを追加。Matter は (node_id, endpoint, cluster, attribute) でアドレスするため、casa の単一セレクタ `<epc>` を `endpoint/cluster/attribute` として解釈し `mat read`/`write` に割り当てる。`on`/`off` は OnOff コマンドの invoke（`mat on`/`off`）。設定は `node_id` 必須・`endpoint` 任意。Phase 3 のアダプタ trait に variant 1 個追加のみでサブコマンドハンドラは無変更。

---

### 明示的に保留（議論なしに実装しない）

これらは **casa 本体には実装しない**。必要になったら上層（`casad`）の責務とする。

- **キャッシュ / ローカル DB**。`casad` のメモリキャッシュで対応する。casa にファイルキャッシュを足さない。
- **設定ファイルの自動マイグレーション**。明示コマンドのみ、自動は不可。
- **ディスカバリ**。`enl discover` を直接叩く運用。casa はラップしない。
- **状態変化の監視（INF 通知の待受）**。`casad` の責務（実装済み: `enl listen` をループで回す）。casa(bin) は購読を持たない。
- **デーモン・常駐モード**。casa(bin) には実装しない。常駐は別 crate `casad` が担う（実装済み）。
- **HTTP / WebSocket / MCP サーバ**。`casad` の責務。
- **LLM Function Calling 受け口**。`casad` の責務。

---

## やらないこと

- プロトコルバイト列を casa 内で組まない／パースしない。
- 子 CLI を crate 依存しない（必ずサブプロセス）。
- casa(bin) にデーモン化・常駐・内部スケジューラを足さない（それらは同一ワークスペースの別 crate `casad` が担う）。
- キャッシュ・DB を足さない（`casad` の責務）。
- HTTP / WebSocket / MCP サーバを casa に組み込まない（`casad` の責務）。
- 実設定ファイル・実トポロジをこのリポジトリにコミットしない。

---

## 開発コマンド

```bash
cargo build
cargo test
cargo clippy -- -D warnings
RUST_LOG=debug cargo run -- list
```
