# ルールの一時停止（pause / resume）と mando からの操作

- 日付: 2026-09-26
- 対象: casad（rules / engine / cli）、mando（API・画面）、hogar-iac（compose・config・pin）
- version bump: workspace `1.10.0` → `1.11.0`（新機能・minor）

## 背景と課題

2026-09-25 の夜、妻がリビングで寝るため「リビングエアコン 0時OFF」を一晩止めたかった。
現状 casad のルールを止める手段は rules.toml を書き換えて hogar-iac から deploy する
（casad 再起動）しかなく、Claude に頼まないと止められない。

こうした「一時的にルールを止めたい／戻したい」は今後も起きるので、**スマホの mando
画面からルール単位で停止・再開できる**ようにする。

## 要件

1. mando 画面からルールを 1 本ずつ **停止 / 再開** できる（本人・妻どちらでも操作可）
2. 停止は **手動で再開するまで続く** トグル（期限・1 回スキップは持たない）
3. 停止・再開は **即時に効き**、casad の再起動を要さない。casad が再起動しても停止状態は残る
4. mando には **全ルール** を出す。ただし普段止めない系（イベントトリガ）は折りたたみ、
   タップしないと見えない。停止中のルールは常に目立つ位置に出す
5. rules.toml への追記（フラグ等）は不要

## 検討した方式

| 方式 | 内容 | 判定 |
| --- | --- | --- |
| A. 状態ファイル | 停止中ルール名を JSON に保存。casad は発火判定時に読む。操作は casad CLI、mando は CLI を呼ぶ | **採用** |
| B. casad に unix socket 制御口 | matd 同様の常駐 API。状態はメモリ＋永続化 | 常駐側の変更が大きく、A と利点が変わらない |
| C. mando が状態を持ち casad が問い合わせ | mando HTTP に依存 | 依存の向きが逆（下層が上層に依存）。不採用 |

A は mando の「プロトコルを喋らず CLI を subprocess で呼ぶだけ」という既存方針と一致する。

## 設計

### 状態ファイル（pause file）

- パス: 環境変数 `CASA_PAUSE_FILE`（hogar では `/data/casa/paused.json`）。
  未設定時の既定は `$XDG_STATE_HOME/casa/paused.json`（なければ `$HOME/.local/state/casa/paused.json`）。
- 形式:

  ```json
  { "paused": [ { "name": "リビングエアコン 0時OFF", "since": "2026-09-26T21:03:00+09:00" } ] }
  ```

- 書き込みは同じディレクトリの一時ファイルへ書いて `rename`（アトミック）。ディレクトリが無ければ作る。
- ファイルが無い ＝ 停止中ルールなし（正常）。
- パース不能・読み取りエラー ＝ **停止中ルールなしとみなし warn ログ**（ルールが動く側へ倒す。
  止まるべきものが動く方が、動くべきもの（朝のシャッター等）が黙って止まるより発見しやすい）。
- rules.toml に存在しない名前が入っていたら無視し、`casad run` 起動時と `rules list` で warn。

### ルール名の一意性

停止のキーはルール名。現状は重複を検出していないので、`casad check` と `casad run` 起動時の
検証に **ルール名の重複をエラー** として加える（現行 rules.toml に重複が無いことは実装時に確認）。

### casad エンジン

- 時刻トリガ・イベントトリガとも、トリガ一致 → `active` 窓判定 → **停止判定** → 発火、の順。
- 停止判定は評価 1 回（毎分の時刻評価 1 回 / イベント 1 件）ごとに pause file を 1 回読む。
  ファイルは小さく頻度も低いので、キャッシュ・ファイル監視は持たない（即時反映を単純に得る）。
- 停止中で落としたルールは `info` ログ `rule skipped: paused`（rule 名付き）。発火ログと同じ粒度で
  「なぜ動かなかったか」を残す。

### casad CLI（新サブコマンド `rules`）

rules.toml のパスは `--rules <PATH>`、省略時は環境変数 `CASA_RULES`。

- `casad rules list [--json]` — 全ルールを出す。JSON は 1 行の配列:

  ```json
  [ { "name": "...", "trigger": "time", "at": "00:00", "paused": true, "since": "..." },
    { "name": "...", "trigger": "event", "paused": false } ]
  ```

  `trigger` は `time` / `event` の 2 値（echonet・matter のイベントはまとめて `event`）。
  並び順は rules.toml の記載順。
- `casad rules pause <NAME>` — 停止中に加える。既に停止中なら何もしない（exit 0）。
- `casad rules resume <NAME>` — 停止中から外す。停止中でなければ何もしない（exit 0）。
- 存在しないルール名は exit 非 0 ＋ stderr にエラー（誤操作で幽霊エントリを作らない）。

### mando

- config に `[rules]` セクションを追加し、呼ぶコマンドをデバイスと同じ流儀で宣言する:

  ```toml
  [rules]
  list   = ["casad", "rules", "list", "--json"]
  pause  = ["casad", "rules", "pause"]    # 末尾にルール名を付けて実行
  resume = ["casad", "rules", "resume"]
  ```

  `[rules]` が無ければ API は 404、画面のセクションも出さない。
- API:
  - `GET  /api/rules` — list の JSON をそのまま返す
  - `POST /api/rules/:name/pause` / `POST /api/rules/:name/resume` — 実行して結果の list を返す
  - 実行は既存 Executor（lane `"rules"`、`[exec] timeout_ms`）を通す
- 画面: 既存のカード群の下に「ルール」セクション。
  1. **停止中** — 停止中ルールを最上段に強調表示（「再開」ボタン）
  2. **時刻ルール** — `trigger = time` を記載順に（`at` を併記、「止める」ボタン）
  3. **その他のルール ▸** — `trigger = event` は折りたたみ。タップで展開
- 停止・再開はワンタップで即実行（確認ダイアログなし。取り消しは逆ボタンで即座にできるため）。

### hogar-iac

- compose: casad と mando の両方に `./data/casa:/data/casa` と環境変数
  `CASA_PAUSE_FILE=/data/casa/paused.json`、`CASA_RULES=/config/casa/rules.toml` を追加
  （x-apps-env / x-apps に入れる）。`data/casa` は uid 1001 で作成。
- mando の config.toml に `[rules]` を追加。
- `hogar/apps/Dockerfile` の casa・mando の pin を更新 → build → deploy。
- 2026-09-25 にコメントアウトした「リビングエアコン 0時OFF」は、本機能の deploy 時に
  rules.toml へ戻す（戻すかその後 pause するかは本人判断）。

## エラー時の振る舞い

| 状況 | 振る舞い |
| --- | --- |
| pause file が無い | 停止中なし（正常） |
| pause file が壊れている / 読めない | 停止中なしとみなし warn。`rules pause/resume` はエラー終了（壊れたファイルを上書きしない） |
| 書き込み失敗 | `rules pause/resume` が exit 非 0。mando は失敗を UI に表示 |
| 未知のルール名を pause/resume | exit 非 0 |
| pause file に rules.toml に無い名前 | 無視＋warn |

## テスト

- casad 単体: pause file の読み書き（無い・壊れ・アトミック書き込み・冪等）、ルール名重複の検出、
  時刻・イベント両トリガで停止中ルールが発火しないこと、`rules list --json` の出力形。
- mando: `/api/rules` 系のハンドラテスト（Executor のフェイク経由）、`[rules]` 未設定で 404。
- 実機: deploy 後に mando から任意の時刻ルールを pause → casad ログに `rule skipped: paused` が
  出ること、resume で発火に戻ることを確認。

## スコープ外

- 期限付き停止・1 回スキップ（要件 2 により持たない。必要になったら `until` を足す）
- Alexa・Claude からの操作（CLI があるので Claude からは `casad rules pause` で可能）
- 停止状態の embalse 記録・Slack 通知
