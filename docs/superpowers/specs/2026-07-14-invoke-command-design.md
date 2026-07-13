# invoke コマンドによる語彙の閉鎖 — 設計

日付: 2026-07-14
状態: 承認済み

## 背景と目的

`casa color-temp` の追加で「enl/mat が機能を増やすたびに casa にサブコマンドを足し続ける」
軌道に乗りかけた。この設計は casa の動詞表面を**閉じ**、長尾のプロトコル固有操作を
汎用動詞 `invoke` 1 個で吸収する。

casa の不可替な価値（名前解決・出力スキーマ統一・操作の抽象化・グループ実行）は保つ。
「ルールエンジン特化に降る」案は不成立と確認済み — casad のルール語彙は casa の
サブコマンドへ写像されるため、casa の動詞表面が常にボトルネックになる。

## 決定事項

1. **昇格基準の明文化**: casa に専用動詞を足すのは「2 プロトコル以上で同じ意味を持つ、
   または日常高頻度の操作」のみ。それ以外は `invoke` で表現する。CLAUDE.md に記載する。
2. **`casa color-temp` は削除**（破壊変更）。昇格基準を満たさない（Matter 固有）。
   `casa invoke <name> color-temp --kelvin 2700` で代替できる。
3. **casad も同時対応**: rules.toml の `then` と `casad exec` から invoke を使えるようにする。

## casa CLI 表面

```
casa invoke <name> <command> [args...]
```

- `<name>`: デバイス名またはグループ名。
- `<command>`: 子 CLI のサブコマンド名をそのまま。casa は解釈しない（写像表を持たない）。
- `[args...]`: clap の `trailing_var_arg` + `allow_hyphen_values` で素通し。
  casa 自身のグローバルフラグ（`--config`）は `invoke` より前に置く規約。
- 削除: `ColorTemp` CLI variant、`ops::color_temp`、`Adapter::color_temp`、
  `adapter::ColorTemp` 構造体、関連テスト。

## アダプタ層

trait にメソッドを 1 個追加。既定は `None`（→ `protocol_unsupported`、exit 14）:

```rust
fn invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation>
```

- **Echonet**: `enl <command> --ip <ip> --eoj <eoj> <args...>`
- **Matter**: `mat <command> --node <id> [--endpoint <ep>] <args...>`。
  endpoint は設定にあれば注入（既存 `power` と同じ流儀）。そのコマンドが
  `--endpoint` を取らない場合は mat 側のエラーが exit code 伝播で見える —
  casa がコマンドごとの知識を持たないための意図的な割り切り。

アドレス注入がコマンドによらずプロトコルごとに一様（enl: `--ip/--eoj`、
mat: `--node/--endpoint`）であることがこの設計の前提。子 CLI 側でこの一様性が
崩れる変更が入った場合はアダプタで吸収する。

## ops 層・出力・グループ

- 単体デバイス: 既存 envelope（`timestamp`/`device`/`protocol`/`value`）に
  **`command` フィールドを追加**する。`value` は子 CLI の JSON をそのまま格納
  （長尾操作のスキーマ正規化は放棄する — 抽象化しようがない部分なので失うものはない）。
  グループ応答もトップレベルに `command` を含める（メンバー結果には付けない）。
- グループ: `run_group` を再利用して許可。ただし**全メンバーが同一プロトコルの
  グループのみ**。コマンド名の解釈がプロトコル依存なので、混在グループへの invoke は
  「同名コマンドが別プロトコルで別の意味に実行される」事故を防ぐため明示エラーで拒否
  （`protocol_unsupported`、exit 14）。実機を動かす系なので安全側に倒す。
- 新しい exit code は増やさない。既存の 12/13/14/15 と子伝播で足りる。

## casad

- **rules.toml**: `Then` を `#[serde(tag = "action", rename_all = "lowercase")]` の
  enum（`On` / `Off` / `Invoke { command, args }`）に再構成し、casa 引数列への写像も
  `Then` に移す。既存の `on`/`off` ルールの TOML 表記は変わらない。

  ```toml
  then = { action = "invoke", device = "desk_light", command = "color-temp", args = ["--kelvin", "2700"] }
  ```

  `args` は省略可（既定は空配列）。

- **`casad exec`**: action をサブコマンド化する。`casad exec on <name>` は現行表記の
  まま動き、`casad exec invoke <name> <command> [args...]` が加わる。
  （現行の `Action` ValueEnum は payload を持てないため構造変更が必要。）
- ルール検証: `then.device` は従来どおりグループ可（`check_target`）。
  同一プロトコル制約は casa が実行時に判定し、casad は関知しない。

## テスト

- casa 統合テスト（ダミー子 CLI）: 引数素通し、グループ invoke 成功、
  混在プロトコルグループの拒否、アダプタ未実装プロトコルで exit 14。
- アダプタ単体テスト: enl / mat それぞれの引数組み立て（endpoint あり/なし含む）。
- casad: `Then` のパース/検証テスト（invoke の TOML 表記、既存 on/off の後方互換）、
  `exec invoke` の引数写像テスト。
- color-temp 削除に伴う既存テストの整理。

## ドキュメント

- README: invoke の使い方、color-temp 削除の告知（破壊変更）、昇格基準。
- CLAUDE.md: 昇格基準の明文化、invoke を規約セクションに追記。
