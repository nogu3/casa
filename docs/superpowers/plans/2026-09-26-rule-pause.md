# ルールの一時停止（pause / resume）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** casad のルールを 1 本ずつ停止／再開でき、スマホの mando 画面から操作できるようにする。

**Architecture:** 停止中ルール名を JSON の状態ファイル（pause file）に持つ。casad はルール発火の直前に毎回それを読んで停止中ルールを落とし、`casad rules list|pause|resume` CLI で読み書きする。mando はデバイス操作と同じく CLI を subprocess で呼ぶだけの API と画面セクションを持つ。hogar の compose で casad / mando が同じ `data/casa` を共有する。

**Tech Stack:** Rust（casad: clap / serde_json / chrono、mando: axum / tokio）、素の JS（mando index.html）、docker compose（hogar-iac）

**Spec:** `~/ghq/github.com/nogu3/casa/docs/superpowers/specs/2026-09-26-rule-pause-design.md`

**リポジトリ:**
- casa: `~/ghq/github.com/nogu3/casa`（Task 1〜4）
- mando: `~/ghq/github.com/nogu3/mando`（Task 5〜6）
- hogar-iac: `~/ghq/github.com/nogu3/hogar-iac`（Task 7）

各リポジトリの作業は main 上で直接コミットしてよい（本人の運用: ブランチを切った場合も最後はローカルで main にマージして push）。

## Global Constraints

- casa workspace version: `1.10.0` → `1.11.0`（ルート `Cargo.toml` の `[workspace.package] version`）
- pause file のパス: 環境変数 `CASA_PAUSE_FILE`。未設定時は `$XDG_STATE_HOME/casa/paused.json`、無ければ `$HOME/.local/state/casa/paused.json`
- pause file 形式: `{ "paused": [ { "name": "<ルール名>", "since": "<RFC3339 ローカル時刻>" } ] }`
- rules.toml のパス（`casad rules` 用）: `--rules <PATH>`、省略時は環境変数 `CASA_RULES`
- hogar: `CASA_PAUSE_FILE=/data/casa/paused.json`、`CASA_RULES=/config/casa/rules.toml`、ボリューム `./data/casa:/data/casa`
- 停止中で落としたルールのログ: `info` レベル、メッセージ `rule skipped: paused`、フィールド `rule`
- pause file が壊れている／読めないとき、エンジンは「停止中なし」とみなし warn（ルールを動かす側へ倒す）
- 停止はトグルのみ（期限・1 回スキップは作らない）。mando の停止・再開は確認ダイアログなし
- casa の検証コマンド: `cargo test -p casad` と `cargo clippy --all-targets -- -D warnings`。mando: `cargo test` と `cargo clippy -- -D warnings`
- コミット末尾に `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`

## Review Focus

1. **ルール名に空白・日本語・スラッシュ等が含まれる**（例 `"Alexa シャッターを開ける"`）— mando の `/api/rules/:name/pause` は URL エンコードされた名前を受けて正しく解決し、CLI には名前を 1 引数としてそのまま渡すこと（Task 5 に `%20` と日本語のテストを入れる）。
2. **pause file のディレクトリが存在しない初回の pause** — ディレクトリを作って書けること（Task 2 のテスト）。
3. **pause file が壊れている状態で pause/resume** — 壊れたファイルを上書きせずエラー終了すること、かつエンジンは全ルールを動かし続けること（Task 2・Task 4 のテスト）。
4. **rules.toml からルールが消えた／改名された後に古い名前が pause file に残る** — エンジンは無視（起動時 warn）、`rules list` にも出ない、`rules resume <古い名前>` は未知名としてエラーになるので **掃除手段が要る** → resume だけは「rules.toml に無くても pause file にあれば外せる」ことにする（Task 4 のテスト）。
5. **同名ルールの重複** — `casad check` / `run` / `rules` のすべてで設定エラーになること（Task 1 のテスト）。

---

### Task 1: ルール名の重複を設定エラーにする（casa）

**Files:**
- Modify: `crates/casad/src/rules.rs`（`parse` のルールごとの検証ループ、~L244）
- Test: `crates/casad/src/rules.rs` の `mod tests`、`crates/casad/tests/check.rs`

**Interfaces:**
- Produces: `rules::parse` が同名ルールを `ErrorKind::ConfigParse`（exit 10）で拒否する。detail は `duplicate rule name "<name>"`。

- [ ] **Step 1: 失敗するテストを書く**（`rules.rs` の `mod tests` に追加）

```rust
#[test]
fn parse_rejects_duplicate_rule_names() {
    let text = r#"
version = 1
[[rules]]
name = "同じ"
when = { at = "07:00" }
then = { action = "on", device = "a" }
[[rules]]
name = "同じ"
when = { at = "08:00" }
then = { action = "off", device = "a" }
"#;
    let err = parse(text).unwrap_err();
    assert_eq!(err.kind, ErrorKind::ConfigParse);
    assert!(err.detail.contains("duplicate rule name \"同じ\""), "{}", err.detail);
}
```

`tests/check.rs` にも追加:

```rust
#[test]
fn check_rejects_duplicate_rule_names() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(
        dir.path(),
        r#"
version = 1
[[rules]]
name = "dup"
when = { at = "07:00" }
then = { action = "on", device = "living_aircon" }
[[rules]]
name = "dup"
when = { at = "08:00" }
then = { action = "off", device = "living_aircon" }
"#,
    );
    let out = run_casad(
        &["check", rules.to_str().unwrap(), "--config", config.to_str().unwrap()],
        &[],
    );
    assert_eq!(out.status.code(), Some(10));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "config_parse");
}
```

- [ ] **Step 2: 失敗を確認** — `cargo test -p casad duplicate_rule_names` → 2 件 FAIL（重複を受理してしまう）

- [ ] **Step 3: 実装** — `parse` の `for rule in &file.rules {` の直前に `let mut seen = std::collections::HashSet::new();` を置き、ループ先頭に:

```rust
        // 停止（pause）のキーはルール名なので一意でなければならない。
        if !seen.insert(rule.name.as_str()) {
            return Err(CasaError::new(
                ErrorKind::ConfigParse,
                format!("duplicate rule name \"{}\"", rule.name),
            ));
        }
```

- [ ] **Step 4: 通過を確認** — `cargo test -p casad` 全件 PASS

- [ ] **Step 5: Commit**

```bash
git add crates/casad/src/rules.rs crates/casad/tests/check.rs
git commit -m "feat(casad): ルール名の重複を設定エラーにする"
```

---

### Task 2: pause file モジュール（casa）

**Files:**
- Create: `crates/casad/src/pause.rs`
- Modify: `crates/casad/src/main.rs`（`mod pause;` 追加）
- Test: `crates/casad/src/pause.rs` の `mod tests`

**Interfaces:**
- Produces（`crate::pause`）:
  - `pub struct PauseFile { pub paused: Vec<PausedRule> }`（`Default`, `Serialize`, `Deserialize`, `Debug`, `PartialEq`）
  - `pub struct PausedRule { pub name: String, pub since: String }`
  - `pub fn path() -> PathBuf` — Global Constraints のパス解決
  - `pub fn load(path: &Path) -> Result<PauseFile, CasaError>` — 無い→`Ok(default)`、読めない／壊れ→`Err(ConfigParse)`
  - `pub fn load_lenient(path: &Path) -> PauseFile` — エラーは warn ログを出して `default`
  - `pub fn save(path: &Path, file: &PauseFile) -> Result<(), CasaError>` — 親ディレクトリ作成＋一時ファイル→rename
  - `impl PauseFile { pub fn is_paused(&self, name: &str) -> bool; pub fn since(&self, name: &str) -> Option<&str>; pub fn pause(&mut self, name: &str, since: String) -> bool /* 追加したら true */; pub fn resume(&mut self, name: &str) -> bool /* 外したら true */ }`
  - `pub fn now_rfc3339() -> String` — `chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)`

- [ ] **Step 1: 失敗するテストを書く**（`pause.rs` 末尾。先にモジュールの型だけ空で置いてもよい）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let f = load(&dir.path().join("none.json")).unwrap();
        assert!(f.paused.is_empty());
    }

    #[test]
    fn save_creates_parent_dir_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b/paused.json");
        let mut f = PauseFile::default();
        assert!(f.pause("リビングエアコン 0時OFF", "2026-09-26T21:00:00+09:00".into()));
        save(&p, &f).unwrap();
        let back = load(&p).unwrap();
        assert_eq!(back, f);
        assert!(back.is_paused("リビングエアコン 0時OFF"));
        assert_eq!(back.since("リビングエアコン 0時OFF"), Some("2026-09-26T21:00:00+09:00"));
        // 一時ファイルが残っていない
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap()).unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(names, vec!["paused.json".to_string()]);
    }

    #[test]
    fn pause_and_resume_are_idempotent() {
        let mut f = PauseFile::default();
        assert!(f.pause("x", "t1".into()));
        assert!(!f.pause("x", "t2".into()), "二重 pause は何もしない");
        assert_eq!(f.since("x"), Some("t1"), "since は最初の値を保つ");
        assert!(f.resume("x"));
        assert!(!f.resume("x"));
        assert!(!f.is_paused("x"));
    }

    #[test]
    fn broken_file_is_error_strict_and_empty_lenient() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("paused.json");
        std::fs::write(&p, "{not json").unwrap();
        assert_eq!(load(&p).unwrap_err().kind, ErrorKind::ConfigParse);
        assert!(load_lenient(&p).paused.is_empty());
    }
}
```

- [ ] **Step 2: 失敗を確認** — `cargo test -p casad pause::` → コンパイルエラー／FAIL

- [ ] **Step 3: 実装** — `crates/casad/src/pause.rs`:

```rust
//! ルールの一時停止状態（pause file）。
//!
//! 停止中のルール名を JSON に持つ。casad エンジンは発火直前に毎回読み（即時反映・
//! 再起動耐性）、`casad rules pause|resume` が書く。mando はその CLI を呼ぶだけ。
//! 壊れたファイルはエンジンでは「停止中なし」とみなす（ルールを動かす側へ倒す。
//! 動くべき朝のシャッターが黙って止まるより、止めたはずのものが動く方が気づける）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use casa_core::error::{CasaError, ErrorKind};

#[derive(Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct PauseFile {
    #[serde(default)]
    pub paused: Vec<PausedRule>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct PausedRule {
    pub name: String,
    pub since: String,
}

impl PauseFile {
    pub fn is_paused(&self, name: &str) -> bool {
        self.paused.iter().any(|p| p.name == name)
    }

    pub fn since(&self, name: &str) -> Option<&str> {
        self.paused.iter().find(|p| p.name == name).map(|p| p.since.as_str())
    }

    /// 停止中に加える。既に停止中なら何もせず false（since は最初の値を保つ）。
    pub fn pause(&mut self, name: &str, since: String) -> bool {
        if self.is_paused(name) {
            return false;
        }
        self.paused.push(PausedRule { name: name.to_string(), since });
        true
    }

    /// 停止中から外す。停止中でなければ何もせず false。
    pub fn resume(&mut self, name: &str) -> bool {
        let before = self.paused.len();
        self.paused.retain(|p| p.name != name);
        self.paused.len() != before
    }
}

/// pause file のパス。`CASA_PAUSE_FILE` → `$XDG_STATE_HOME/casa/paused.json` →
/// `$HOME/.local/state/casa/paused.json`。
pub fn path() -> PathBuf {
    if let Some(p) = std::env::var_os("CASA_PAUSE_FILE") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("casa").join("paused.json")
}

/// 厳密に読む。無ければ空、読めない／壊れていればエラー（書き込み前の読み用）。
pub fn load(path: &Path) -> Result<PauseFile, CasaError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(PauseFile::default()),
        Err(e) => {
            return Err(CasaError::new(
                ErrorKind::ConfigParse,
                format!("failed to read pause file {}: {e}", path.display()),
            ))
        }
    };
    serde_json::from_str(&text).map_err(|e| {
        CasaError::new(
            ErrorKind::ConfigParse,
            format!("failed to parse pause file {}: {e}", path.display()),
        )
    })
}

/// エンジン用。エラーは warn に残して「停止中なし」とみなす。
pub fn load_lenient(path: &Path) -> PauseFile {
    load(path).unwrap_or_else(|e| {
        tracing::warn!(error = %e.detail, "pause file unreadable; treating as no paused rules");
        PauseFile::default()
    })
}

/// 同じディレクトリの一時ファイルへ書いて rename する（読み手に途中状態を見せない）。
pub fn save(path: &Path, file: &PauseFile) -> Result<(), CasaError> {
    let io_err = |what: &str, e: std::io::Error| {
        CasaError::new(
            ErrorKind::ConfigParse,
            format!("failed to {what} pause file {}: {e}", path.display()),
        )
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| io_err("create dir for", e))?;
    }
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    let text = serde_json::to_string_pretty(file).expect("PauseFile serializes");
    std::fs::write(&tmp, text).map_err(|e| io_err("write", e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io_err("replace", e)
    })
}

pub fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}
```

`main.rs` の `mod mat;` の後に `mod pause;` を追加。`casa_core::error::ErrorKind` がテストから見えるよう `use super::*;` で足りる（`ErrorKind` は上で import 済み）。

- [ ] **Step 4: 通過を確認** — `cargo test -p casad pause::` PASS、`cargo clippy --all-targets -- -D warnings`（未使用警告が出る場合は Task 3 で使うまで `#[allow(dead_code)]` をモジュール先頭に付けず、Task 3 と同じコミットにまとめてもよい。ここでは `pub` なので bin crate の dead_code 警告が出たら `#![allow(dead_code)]` を一時的に付け、Task 3 で外す）

- [ ] **Step 5: Commit**

```bash
git add crates/casad/src/pause.rs crates/casad/src/main.rs
git commit -m "feat(casad): ルールの停止状態を持つ pause file モジュールを追加"
```

---

### Task 3: エンジンで停止中ルールを発火させない（casa）

**Files:**
- Modify: `crates/casad/src/engine.rs`（`tick` L206、`fire_due_events` L262、`drain_matter_events_once` L398、`time_loop` L536、`event_loop` L549、`matter_event_loop` L580）
- Modify: `crates/casad/src/main.rs`（`Command::Run` で起動時に未知名を warn）
- Modify: `crates/casad/tests/common/mod.rs`（`run_casad` が既定で存在しない pause file を指す）
- Test: `crates/casad/src/engine.rs` の `mod tests`、`crates/casad/tests/run.rs`

**Interfaces:**
- Consumes: Task 2 の `pause::{PauseFile, path, load_lenient}`
- Produces:
  - `engine::without_paused<'a>(rules: Vec<&'a Rule>, paused: &PauseFile) -> Vec<&'a Rule>`（純関数・テスト用）
  - `engine::drop_paused<'a>(rules: Vec<&'a Rule>) -> Vec<&'a Rule>`（空なら読まずに返す。非空なら `pause::load_lenient(&pause::path())` を 1 回読む）
  - `pause::warn_unknown(file: &RuleFile, paused: &PauseFile)` — rules.toml に無い名前ごとに warn

- [ ] **Step 1: 失敗するテストを書く**

`engine.rs` の `mod tests`:

```rust
#[test]
fn without_paused_drops_only_paused_rules() {
    let f = rules(
        r#"
version = 1
[[rules]]
name = "A"
when = { at = "00:00" }
then = { action = "off", device = "x" }
[[rules]]
name = "B"
when = { at = "00:00" }
then = { action = "off", device = "y" }
"#,
    );
    let mut paused = crate::pause::PauseFile::default();
    paused.pause("A", "t".into());
    let due = due_time_rules(&f, at(0, 0));
    let names: Vec<_> = without_paused(due, &paused).iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["B"]);
}
```

`tests/common/mod.rs` の `run_casad` で `cmd.env_remove("CASA_FAKE_EXIT");` の後に:

```rust
    // 開発機の ~/.local/state/casa/paused.json を拾わないよう、既定は存在しないパス。
    // 個々のテストが envs で CASA_PAUSE_FILE を渡せば後勝ちで上書きされる。
    cmd.env("CASA_PAUSE_FILE", "/nonexistent/casad-test/paused.json")
        .env_remove("CASA_RULES");
```

`tests/run.rs`:

```rust
#[test]
fn run_once_skips_paused_time_rule() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), SCHEDULE);
    let pause = dir.path().join("paused.json");
    std::fs::write(&pause, r#"{"paused":[{"name":"9時にエアコンOFF","since":"t"}]}"#).unwrap();

    let out = run_casad(
        &["run", rules.to_str().unwrap(), "--once", "--now", "09:00",
          "--config", config.to_str().unwrap()],
        &[("CASA_BIN", &fixture("casa_stub.sh")),
          ("CASA_PAUSE_FILE", pause.to_str().unwrap()),
          ("RUST_LOG", "info")],
    );
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("off living_aircon"), "停止中は発火しない: {stdout}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("rule skipped: paused"), "stderr: {stderr}");
}

#[test]
fn run_once_fires_when_pause_file_is_broken() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(dir.path(), SCHEDULE);
    let pause = dir.path().join("paused.json");
    std::fs::write(&pause, "{broken").unwrap();

    let out = run_casad(
        &["run", rules.to_str().unwrap(), "--once", "--now", "09:00",
          "--config", config.to_str().unwrap()],
        &[("CASA_BIN", &fixture("casa_stub.sh")),
          ("CASA_PAUSE_FILE", pause.to_str().unwrap())],
    );
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("off living_aircon"), "壊れていたら動かす側: {stdout}");
}
```

- [ ] **Step 2: 失敗を確認** — `cargo test -p casad paused` → `without_paused` 未定義／`run_once_skips_paused_time_rule` FAIL

- [ ] **Step 3: 実装**

`engine.rs`（`active_or_log` の後）:

```rust
/// 停止中（pause file に載っている）ルールを落とす。落としたものは info に残す
/// （「なぜ動かなかったか」を発火ログと同じ粒度で追えるように）。
pub fn without_paused<'a>(rules: Vec<&'a Rule>, paused: &PauseFile) -> Vec<&'a Rule> {
    rules
        .into_iter()
        .filter(|r| {
            let p = paused.is_paused(&r.name);
            if p {
                tracing::info!(rule = %r.name, "rule skipped: paused");
            }
            !p
        })
        .collect()
}

/// 発火直前に pause file を読んで停止中ルールを落とす。候補が無ければ読まない
/// （毎分の時刻評価で無駄に読まない・壊れたファイルの warn を連打しない）。
pub fn drop_paused(rules: Vec<&Rule>) -> Vec<&Rule> {
    if rules.is_empty() {
        return rules;
    }
    without_paused(rules, &pause::load_lenient(&pause::path()))
}
```

import に `use crate::pause::{self, PauseFile};` を追加。6 箇所を差し替える:

- `tick`: `fire_all(drop_paused(due_time_rules(file, now)), config_path)`
- `fire_due_events`: `fire_all(drop_paused(due_event_rules(file, config, events, now)), config_path)`
- `drain_matter_events_once`: `fire_all(drop_paused(due_matter_rules(file, config, &lines, now_or(now))), config_path)`
- `time_loop`: `dispatcher.dispatch_all(drop_paused(due_time_rules(file, now)))`
- `event_loop`: `dispatcher.dispatch_all(drop_paused(due_event_rules(file, config, &events, now)))`
- `matter_event_loop`: `dispatcher.dispatch_all(drop_paused(due_matter_rules(file, config, std::slice::from_ref(&incoming), now)))`

`pause.rs` に追加:

```rust
/// rules.toml に無い名前が pause file に残っていたら warn する（改名・削除の取り残し）。
pub fn warn_unknown(file: &crate::rules::RuleFile, paused: &PauseFile) {
    for p in &paused.paused {
        if !file.rules.iter().any(|r| r.name == p.name) {
            tracing::warn!(rule = %p.name, "paused rule not found in rules file; ignored");
        }
    }
}
```

`main.rs` の `Command::Run` で `engine::validate_schedule(&rule_file)?;` の直後に:

```rust
            pause::warn_unknown(&rule_file, &pause::load_lenient(&pause::path()));
```

Task 2 で一時的に付けた `#![allow(dead_code)]` があれば外す。

- [ ] **Step 4: 通過を確認** — `cargo test -p casad` 全件 PASS、`cargo clippy --all-targets -- -D warnings` クリーン

- [ ] **Step 5: Commit**

```bash
git add crates/casad/src/engine.rs crates/casad/src/pause.rs crates/casad/src/main.rs crates/casad/tests/common/mod.rs crates/casad/tests/run.rs
git commit -m "feat(casad): 停止中のルールを時刻・イベントとも発火させない"
```

---

### Task 4: `casad rules list|pause|resume` CLI と 1.11.0（casa）

**Files:**
- Modify: `crates/casad/src/cli.rs`（`Command::Rules` と `RulesAction`）
- Modify: `crates/casad/src/main.rs`（ディスパッチ）
- Modify: `Cargo.toml`（workspace version 1.11.0）、`Cargo.lock`（`cargo build` で更新）
- Modify: `README.md`（casad の節に `rules` サブコマンドと `CASA_PAUSE_FILE` を追記。README に casad のコマンド一覧が無ければ `CLAUDE.md` の該当節へ）
- Create: `crates/casad/tests/rules_cmd.rs`

**Interfaces:**
- Consumes: Task 2 の `pause::*`、Task 1 の重複検証済み `rules::load`
- Produces（mando が依存する CLI 契約）:
  - `casad rules list --json` → stdout 1 行の JSON 配列、rules.toml 記載順。要素: `{"name": str, "trigger": "time"|"event", "at": str /*time のみ*/, "paused": bool, "since": str /*paused のみ*/}`
  - `casad rules pause <NAME>` / `casad rules resume <NAME>` → exit 0、stdout なし
  - 未知名の pause → exit 11（`name_not_found`）。resume は rules.toml に無くても pause file にあれば外して exit 0、どちらにも無ければ exit 11
  - pause file が壊れている → pause/resume は exit 10、ファイルは変更しない。list は exit 10
  - `--rules` 省略かつ `CASA_RULES` 未設定 → clap のエラー（exit 2）

- [ ] **Step 1: 失敗するテストを書く** — `crates/casad/tests/rules_cmd.rs`:

```rust
//! casad rules（list / pause / resume）の統合テスト。

mod common;

use common::*;

const RULES: &str = r#"
version = 1
[[rules]]
name = "リビングエアコン 0時OFF"
when = { at = "00:00" }
then = { action = "off", device = "living_aircon" }
[[rules]]
name = "状変でON"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "living_aircon" }
"#;

struct Env {
    _dir: tempfile::TempDir,
    config: String,
    rules: String,
    pause: std::path::PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG).to_str().unwrap().to_string();
    let rules_path = dir.path().join("rules.toml");
    std::fs::write(&rules_path, RULES).unwrap();
    // 親ディレクトリが無い状態から始める（初回 pause でディレクトリを作れること）
    let pause = dir.path().join("state/casa/paused.json");
    Env { config, rules: rules_path.to_str().unwrap().to_string(), pause, _dir: dir }
}

fn casad(e: &Env, args: &[&str]) -> std::process::Output {
    let mut full = vec!["--config", e.config.as_str(), "rules"];
    full.extend_from_slice(args);
    run_casad(
        &full,
        &[("CASA_RULES", e.rules.as_str()), ("CASA_PAUSE_FILE", e.pause.to_str().unwrap())],
    )
}

fn list(e: &Env) -> serde_json::Value {
    let out = casad(e, &["list", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn list_shows_all_rules_in_order_with_trigger_kind() {
    let e = env();
    let v = list(&e);
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "リビングエアコン 0時OFF");
    assert_eq!(arr[0]["trigger"], "time");
    assert_eq!(arr[0]["at"], "00:00");
    assert_eq!(arr[0]["paused"], false);
    assert!(arr[0].get("since").is_none());
    assert_eq!(arr[1]["trigger"], "event");
    assert!(arr[1].get("at").is_none());
}

#[test]
fn pause_then_resume_roundtrip_and_idempotent() {
    let e = env();
    assert_eq!(casad(&e, &["pause", "リビングエアコン 0時OFF"]).status.code(), Some(0));
    assert_eq!(casad(&e, &["pause", "リビングエアコン 0時OFF"]).status.code(), Some(0));
    let v = list(&e);
    assert_eq!(v[0]["paused"], true);
    assert!(v[0]["since"].as_str().unwrap().starts_with("20"));
    assert_eq!(casad(&e, &["resume", "リビングエアコン 0時OFF"]).status.code(), Some(0));
    assert_eq!(casad(&e, &["resume", "リビングエアコン 0時OFF"]).status.code(), Some(0));
    assert_eq!(list(&e)[0]["paused"], false);
}

#[test]
fn pause_unknown_name_is_name_not_found_and_writes_nothing() {
    let e = env();
    let out = casad(&e, &["pause", "無いルール"]);
    assert_eq!(out.status.code(), Some(11));
    assert!(!e.pause.exists());
}

#[test]
fn resume_removes_stale_name_missing_from_rules() {
    let e = env();
    std::fs::create_dir_all(e.pause.parent().unwrap()).unwrap();
    std::fs::write(&e.pause, r#"{"paused":[{"name":"消えたルール","since":"t"}]}"#).unwrap();
    assert_eq!(casad(&e, &["resume", "消えたルール"]).status.code(), Some(0));
    let text = std::fs::read_to_string(&e.pause).unwrap();
    assert!(!text.contains("消えたルール"));
    assert_eq!(casad(&e, &["resume", "消えたルール"]).status.code(), Some(11));
}

#[test]
fn broken_pause_file_is_not_overwritten() {
    let e = env();
    std::fs::create_dir_all(e.pause.parent().unwrap()).unwrap();
    std::fs::write(&e.pause, "{broken").unwrap();
    assert_eq!(casad(&e, &["pause", "リビングエアコン 0時OFF"]).status.code(), Some(10));
    assert_eq!(std::fs::read_to_string(&e.pause).unwrap(), "{broken");
}
```

- [ ] **Step 2: 失敗を確認** — `cargo test -p casad --test rules_cmd` → FAIL（`rules` サブコマンドが無い）

- [ ] **Step 3: 実装**

`cli.rs` の `Command` に追加:

```rust
    /// ルールの一覧と一時停止 / 再開。停止状態は pause file（CASA_PAUSE_FILE）に持ち、
    /// 常駐中の casad は発火直前に毎回読むので再起動なしで効く。
    Rules {
        /// ルールファイル（rules.toml）のパス
        #[arg(long, env = "CASA_RULES", value_name = "PATH")]
        rules: PathBuf,
        #[command(subcommand)]
        action: RulesAction,
    },
```

```rust
/// `casad rules` の操作。
#[derive(Debug, Subcommand)]
pub enum RulesAction {
    /// 全ルールを記載順に出す（停止中かどうか付き）
    List {
        /// 1 行の JSON 配列で出す（mando 等の機械向け）
        #[arg(long)]
        json: bool,
    },
    /// ルールを停止する（再開するまで発火しない）
    Pause {
        /// ルール名（rules.toml の name）
        name: String,
    },
    /// 停止中のルールを再開する
    Resume {
        /// ルール名（rules.toml の name）
        name: String,
    },
}
```

`main.rs` の `match cli.command` に追加（`use crate::cli::{Cli, Command, RulesAction};`、`use crate::rules::Trigger;`）:

```rust
        Command::Rules { rules, action } => {
            let config = config::load(cli.config.as_deref())?;
            let rule_file = rules::load(&rules)?;
            rule_file.validate(&config)?;
            let path = pause::path();
            match action {
                RulesAction::List { json } => {
                    let paused = pause::load(&path)?;
                    let items: Vec<serde_json::Value> = rule_file
                        .rules
                        .iter()
                        .map(|r| {
                            let mut o = serde_json::json!({
                                "name": r.name,
                                "trigger": if matches!(r.when, Trigger::Time { .. }) { "time" } else { "event" },
                                "paused": paused.is_paused(&r.name),
                            });
                            if let Trigger::Time { at } = &r.when {
                                o["at"] = at.clone().into();
                            }
                            if let Some(since) = paused.since(&r.name) {
                                o["since"] = since.into();
                            }
                            o
                        })
                        .collect();
                    if json {
                        println!("{}", serde_json::Value::Array(items));
                    } else {
                        for i in &items {
                            let mark = if i["paused"] == true { "⏸" } else { " " };
                            let at = i["at"].as_str().unwrap_or("-");
                            println!("{mark} {at:>5}  {}", i["name"].as_str().unwrap_or(""));
                        }
                    }
                    pause::warn_unknown(&rule_file, &paused);
                    Ok(0)
                }
                RulesAction::Pause { name } => {
                    if !rule_file.rules.iter().any(|r| r.name == name) {
                        return Err(CasaError::new(
                            ErrorKind::NameNotFound,
                            format!("rule not found: {name}"),
                        ));
                    }
                    let mut paused = pause::load(&path)?;
                    if paused.pause(&name, pause::now_rfc3339()) {
                        pause::save(&path, &paused)?;
                        tracing::info!(rule = %name, "rule paused");
                    }
                    Ok(0)
                }
                RulesAction::Resume { name } => {
                    let mut paused = pause::load(&path)?;
                    if paused.resume(&name) {
                        pause::save(&path, &paused)?;
                        tracing::info!(rule = %name, "rule resumed");
                    } else if !rule_file.rules.iter().any(|r| r.name == name) {
                        // rules.toml にも pause file にも無い＝打ち間違い。停止中でない既知ルールの
                        // resume は冪等に成功させる。
                        return Err(CasaError::new(
                            ErrorKind::NameNotFound,
                            format!("rule not found: {name}"),
                        ));
                    }
                    Ok(0)
                }
            }
        }
```

（`use casa_core::error::{CasaError, ErrorKind};` を main.rs に合わせて調整。）

ルート `Cargo.toml` の `version = "1.10.0"` を `"1.11.0"` に。README（または CLAUDE.md）の casad 節に追記:

```markdown
- `casad rules list [--json]` / `casad rules pause <NAME>` / `casad rules resume <NAME>` — ルールの一時停止。
  状態は `CASA_PAUSE_FILE`（既定 `$XDG_STATE_HOME/casa/paused.json`）に持ち、常駐中の casad は発火直前に
  毎回読むので再起動不要。rules.toml は `--rules` か `CASA_RULES`。
```

- [ ] **Step 4: 通過を確認** — `cargo test -p casad` 全件 PASS、`cargo clippy --all-targets -- -D warnings`、`cargo fmt --check`

- [ ] **Step 5: Commit & push**

```bash
git add Cargo.toml Cargo.lock README.md crates/casad/src/cli.rs crates/casad/src/main.rs crates/casad/tests/rules_cmd.rs
git commit -m "feat(casad): rules list/pause/resume でルールを一時停止できるようにする (1.11.0)"
git push
```

（README ではなく CLAUDE.md を直した場合は add 対象を差し替える。hogar の Dockerfile は GitHub から clone するので push 必須。）

---

### Task 5: mando の `/api/rules` API（mando）

**Files:**
- Modify: `src/config.rs`（`Rules` 構造体と `Config.rules`）
- Modify: `src/main.rs`（ルート 3 本・ハンドラ・テスト）
- Modify: `config.example.toml`（`[rules]` 例）

**Interfaces:**
- Consumes: Task 4 の CLI 契約
- Produces（Task 6 の UI が依存）:
  - `GET /api/rules` → 200 + list JSON 配列そのまま。`[rules]` 未設定 404 `{"error":"rules not configured"}`。exec 失敗・非 JSON 配列は 502 `{"error":"rules query failed"}`
  - `POST /api/rules/:name/pause`・`/resume` → 成功なら 200 + 実行後の list 配列。コマンド非成功は 502 `{"error":"rules action failed","name":...,"detail":<stderr 先頭行>}`

- [ ] **Step 1: 失敗するテストを書く**（`src/main.rs` の tests、`MINIMAL_DEVICE` を使う）

呼ばれた引数をファイルに記録するフェイクを sh で作る。`pause`/`resume` は最後の引数をファイルに書き、`list` はそれを見て JSON を返す:

```rust
    fn rules_cfg(dir: &std::path::Path) -> String {
        let log = dir.join("argv");
        format!(
            r#"{MINIMAL_DEVICE}
[rules]
list   = ["sh", "-c", "if [ -f {log} ]; then n=$(cat {log}); printf '[{{\"name\":\"%s\",\"trigger\":\"time\",\"at\":\"00:00\",\"paused\":true}}]' \"$n\"; else printf '[]'; fi"]
pause  = ["sh", "-c", "printf '%s' \"$1\" > {log}", "fake"]
resume = ["sh", "-c", "exit 11", "fake"]
"#,
            log = log.display()
        )
    }

    #[tokio::test]
    async fn rules_not_configured_is_404() {
        let (st, v) = call_with_cfg(MINIMAL_DEVICE, "GET", "/api/rules").await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert_eq!(v["error"], "rules not configured");
    }

    #[tokio::test]
    async fn rules_list_passes_through_array() {
        let dir = tempfile::tempdir().unwrap();
        let (st, v) = call_with_cfg(&rules_cfg(dir.path()), "GET", "/api/rules").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v, serde_json::json!([]));
    }

    #[tokio::test]
    async fn rules_pause_passes_decoded_name_as_one_arg_and_returns_list() {
        let dir = tempfile::tempdir().unwrap();
        // "リビングエアコン 0時OFF" を URL エンコード
        let path = "/api/rules/%E3%83%AA%E3%83%93%E3%83%B3%E3%82%B0%E3%82%A8%E3%82%A2%E3%82%B3%E3%83%B3%200%E6%99%82OFF/pause";
        let (st, v) = call_with_cfg(&rules_cfg(dir.path()), "POST", path).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v[0]["name"], "リビングエアコン 0時OFF");
        assert_eq!(v[0]["paused"], true);
    }

    #[tokio::test]
    async fn rules_action_failure_is_502() {
        let dir = tempfile::tempdir().unwrap();
        let (st, v) = call_with_cfg(&rules_cfg(dir.path()), "POST", "/api/rules/x/resume").await;
        assert_eq!(st, StatusCode::BAD_GATEWAY);
        assert_eq!(v["error"], "rules action failed");
        assert_eq!(v["name"], "x");
    }

    #[tokio::test]
    async fn rules_list_non_json_is_502() {
        let cfg = format!(
            "{MINIMAL_DEVICE}\n[rules]\nlist = [\"sh\", \"-c\", \"printf nope\"]\npause = [\"true\"]\nresume = [\"true\"]\n"
        );
        let (st, v) = call_with_cfg(&cfg, "GET", "/api/rules").await;
        assert_eq!(st, StatusCode::BAD_GATEWAY);
        assert_eq!(v["error"], "rules query failed");
    }
```

`tempfile` が mando の dev-dependencies に無ければ `Cargo.toml` の `[dev-dependencies]` に `tempfile = "3"` を追加する。

- [ ] **Step 2: 失敗を確認** — `cargo test rules_` → FAIL（404 / ルート無し）

- [ ] **Step 3: 実装**

`config.rs`（`Health` の近く）:

```rust
/// casad ルールの一覧・一時停止のコマンド定義（任意）。未設定なら /api/rules ごと無効。
/// pause / resume はコマンド配列の末尾にルール名を 1 引数として足して exec する。
#[derive(Debug, Clone, Deserialize)]
pub struct Rules {
    pub list: Vec<String>,
    pub pause: Vec<String>,
    pub resume: Vec<String>,
}
```

`Config` に:

```rust
    /// casad ルールの一時停止（任意）。
    #[serde(default)]
    pub rules: Option<Rules>,
```

`validate` の末尾近くで、`rules` が Some なら 3 配列が空でないこと（空なら `ConfigError::EmptyCommand("rules".into())`）。

`main.rs` ルーター:

```rust
        .route("/api/rules", get(list_rules))
        .route("/api/rules/:name/pause", post(pause_rule))
        .route("/api/rules/:name/resume", post(resume_rule))
```

ハンドラ（`get_health` の後）:

```rust
/// ルール操作は devices の executor（3610 レーン）とは別レーンで直列化する。
const RULES_LANE: &str = "rules";

fn rules_not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"error":"rules not configured"}"#.to_string(),
    )
        .into_response()
}

async fn rules_list_response(app: &App, list: &[String]) -> Response {
    let result = run_bounded(&app.executor, RULES_LANE, list, app.exec_timeout()).await;
    if result.outcome != ExecOutcome::Success {
        tracing::warn!(outcome = ?result.outcome, stderr = %result.stderr.trim(), "rules list 非成功");
        return rules_query_failed();
    }
    match serde_json::from_str::<Value>(&result.stdout) {
        Ok(v @ Value::Array(_)) => Json(v).into_response(),
        _ => {
            tracing::warn!("rules list の stdout が JSON 配列でない");
            rules_query_failed()
        }
    }
}

fn rules_query_failed() -> Response {
    (
        StatusCode::BAD_GATEWAY,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"error":"rules query failed"}"#.to_string(),
    )
        .into_response()
}

async fn list_rules(State(app): State<Shared>) -> Response {
    let Some(rules) = &app.config.rules else {
        return rules_not_configured();
    };
    rules_list_response(&app, &rules.list).await
}

async fn rule_action(app: &App, name: &str, pause: bool) -> Response {
    let Some(rules) = &app.config.rules else {
        return rules_not_configured();
    };
    let mut cmd = if pause { rules.pause.clone() } else { rules.resume.clone() };
    cmd.push(name.to_string());
    let result = run_bounded(&app.executor, RULES_LANE, &cmd, app.exec_timeout()).await;
    if result.outcome != ExecOutcome::Success {
        tracing::warn!(rule = %name, outcome = ?result.outcome, stderr = %result.stderr.trim(), "rules action 非成功");
        let detail = result.stderr.lines().next().unwrap_or("").to_string();
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": "rules action failed", "name": name, "detail": detail })),
        )
            .into_response();
    }
    rules_list_response(app, &rules.list).await
}

async fn pause_rule(State(app): State<Shared>, Path(name): Path<String>) -> Response {
    rule_action(&app, &name, true).await
}

async fn resume_rule(State(app): State<Shared>, Path(name): Path<String>) -> Response {
    rule_action(&app, &name, false).await
}
```

（axum の `Path<String>` はパーセントデコード済みの値を渡す。テストで確認済みになる。）

`config.example.toml` の末尾に:

```toml
# casad ルールの一覧・一時停止（任意）。未設定なら画面の「ルール」セクションごと出ない。
# pause / resume は末尾にルール名を 1 引数として足して実行する。
[rules]
list   = ["casad", "rules", "list", "--json"]
pause  = ["casad", "rules", "pause"]
resume = ["casad", "rules", "resume"]
```

- [ ] **Step 4: 通過を確認** — `cargo test` 全件 PASS、`cargo clippy -- -D warnings`、`cargo fmt --check`。`cargo run -- --check --config config.example.toml` が `check OK`

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/config.rs src/main.rs config.example.toml
git commit -m "feat: casad ルールの一覧・一時停止 API（/api/rules）を追加"
```

---

### Task 6: mando 画面に「ルール」セクション（mando）

**Files:**
- Modify: `index.html`（CSS・`boot()` 末尾・新関数 `loadRules` / `renderRules` / `ruleAct`）
- Modify: `src/main.rs` tests（index に `/api/rules` を叩く JS が含まれることの軽い確認）

**Interfaces:**
- Consumes: Task 5 の `GET /api/rules`、`POST /api/rules/:name/pause|resume`（戻り値は最新 list 配列）

- [ ] **Step 1: 失敗するテストを書く**（既存の index 系テストの流儀で）

```rust
    #[tokio::test]
    async fn index_has_rules_section_script() {
        let res = router(test_app())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("/api/rules"));
        assert!(body.contains("その他のルール"));
    }
```

- [ ] **Step 2: 失敗を確認** — `cargo test index_has_rules_section_script` FAIL

- [ ] **Step 3: 実装** — `index.html`

CSS（`</style>` の前。既存の変数名・配色に合わせて調整してよい）:

```css
.rules { display: flex; flex-direction: column; gap: 6px; }
.rule { display: flex; align-items: center; gap: 8px; padding: 10px 12px; border-radius: 12px; background: var(--card, #1b1f27); }
.rule .rname { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.rule .rat { font-variant-numeric: tabular-nums; opacity: .7; }
.rule.paused { outline: 2px solid #e0a030; }
.rule button { min-width: 72px; }
.rules-more > summary { cursor: pointer; padding: 8px 4px; opacity: .8; }
.rule-msg { color: #e06060; font-size: .85em; }
```

JS（`fetchHealth` の後）:

```js
// ルールの一時停止。停止中を最上段、時刻ルール、その他（イベント）は折りたたみ。
let rulesBox = null;

async function loadRules() {
  let list;
  try {
    list = await api("GET", "/api/rules");
  } catch (e) {
    return; // [rules] 未設定（404）や取得失敗ではセクションを出さない
  }
  if (!rulesBox) {
    app.appendChild(sectionHeading("⏸ ルール"));
    rulesBox = document.createElement("div");
    app.appendChild(rulesBox);
  }
  renderRules(list);
}

function ruleRow(r) {
  const row = document.createElement("div");
  row.className = "rule" + (r.paused ? " paused" : "");
  const name = document.createElement("span");
  name.className = "rname";
  name.textContent = r.name;
  const at = document.createElement("span");
  at.className = "rat";
  at.textContent = r.at || "";
  const btn = document.createElement("button");
  btn.textContent = r.paused ? "再開" : "止める";
  const msg = document.createElement("div");
  msg.className = "rule-msg";
  btn.addEventListener("click", () => ruleAct(r, btn, msg));
  row.append(name, at, btn);
  const wrap = document.createElement("div");
  wrap.append(row, msg);
  return wrap;
}

function renderRules(list) {
  const openMore = rulesBox.querySelector("details")?.open ?? false;
  rulesBox.innerHTML = "";
  const box = document.createElement("div");
  box.className = "rules";
  const paused = list.filter((r) => r.paused);
  const timed = list.filter((r) => !r.paused && r.trigger === "time");
  const others = list.filter((r) => !r.paused && r.trigger !== "time");
  for (const r of paused) box.appendChild(ruleRow(r));
  for (const r of timed) box.appendChild(ruleRow(r));
  rulesBox.appendChild(box);
  if (others.length) {
    const more = document.createElement("details");
    more.className = "rules-more";
    more.open = openMore;
    const sum = document.createElement("summary");
    sum.textContent = `その他のルール（${others.length}）`;
    const inner = document.createElement("div");
    inner.className = "rules";
    for (const r of others) inner.appendChild(ruleRow(r));
    more.append(sum, inner);
    rulesBox.appendChild(more);
  }
}

async function ruleAct(r, btn, msg) {
  const verb = r.paused ? "resume" : "pause";
  btn.disabled = true;
  msg.textContent = "";
  try {
    const list = await api("POST", `/api/rules/${encodeURIComponent(r.name)}/${verb}`);
    renderRules(list);
  } catch (e) {
    btn.disabled = false;
    msg.textContent = "操作に失敗しました";
  }
}
```

`api()` が非 2xx で throw することを確認し（しない場合は `ruleAct`・`loadRules` で `res.error` を見て分岐する）、`boot()` の最後（グラフ読み込みの後）に `loadRules();` を追加する。

- [ ] **Step 4: 通過を確認** — `cargo test` PASS。`task run`（または `MANDO_CONFIG=config.example.toml cargo run`）で起動し、ブラウザで「⏸ ルール」セクションが出ること・フェイクでなく実 casad が無い環境ではセクションが出ないこと（取得失敗）を目視確認。可能なら `config.example.toml` の `[rules]` を上記 Task 5 テストと同様の `sh -c` フェイクに差し替えたローカル config で、止める→最上段に移動・再開→元の位置、が動くことを確認

- [ ] **Step 5: Commit & push**

```bash
git add index.html src/main.rs
git commit -m "feat(ui): ルールの停止・再開セクション（イベント系は折りたたみ）"
git push
```

---

### Task 7: hogar へ配備し実機で確認（hogar-iac）

**Files:**
- Modify: `hogar/compose.yml`（x-apps の volumes、x-apps-env）
- Modify: `hogar/config/mando/config.toml`（`[rules]`）
- Modify: `hogar/config/casa/rules.toml`（2026-09-25 のコメントアウトを戻す）
- Modify: `hogar/apps/Dockerfile`（`CASA_REF`・`MANDO_REF`）

**Interfaces:**
- Consumes: Task 4・Task 6 で push 済みの casa / mando の HEAD sha

- [ ] **Step 1: config を書き換える**

`compose.yml` の `x-apps: &apps` の `volumes:` に追加:

```yaml
    - ./data/casa:/data/casa                               # casad ルールの一時停止状態 (paused.json)
```

`x-apps-env: &apps-env` に追加:

```yaml
  CASA_PAUSE_FILE: /data/casa/paused.json   # casad rules pause/resume と常駐 casad が共有
  CASA_RULES: /config/casa/rules.toml       # mando が呼ぶ casad rules の既定
```

`hogar/config/mando/config.toml` の末尾に Task 5 の `[rules]` ブロックをそのまま追加。

`hogar/config/casa/rules.toml` の「リビングエアコン 0時OFF」を元に戻す（コメント 1 行を消し、`# ` を外す）:

```toml
[[rules]]
name = "リビングエアコン 0時OFF"
when = { at = "00:00" }
then = { action = "off", device = "living_aircon" }
```

`hogar/apps/Dockerfile` の `CASA_REF` / `MANDO_REF` を各リポジトリの `git rev-parse --short=12 HEAD`（push 済み）に更新。

- [ ] **Step 2: 事前検証** — ローカルで新 casad を使い本番 rules.toml を検査（重複名が無いこと）:

```bash
cd ~/ghq/github.com/nogu3/casa && cargo run -q -p casad -- check ~/ghq/github.com/nogu3/hogar-iac/hogar/config/casa/rules.toml --config ~/ghq/github.com/nogu3/hogar-iac/hogar/config/casa/devices.toml | head -c 200
```

Expected: `{"ok":true,...`。`duplicate rule name` が出たら rules.toml の名前を直す。

- [ ] **Step 3: NAS に data/casa を作る**

```bash
ssh nas 'mkdir -p /volume1/docker/hogar/data/casa && sudo chown 1001:10 /volume1/docker/hogar/data/casa && ls -ld /volume1/docker/hogar/data/casa'
```

（sudo が要らない／使えない場合は nas-docker skill の手順に従う。所有者が 1001 であること。）

- [ ] **Step 4: Commit・build・deploy**

```bash
cd ~/ghq/github.com/nogu3/hogar-iac
git add hogar/compose.yml hogar/config/mando/config.toml hogar/config/casa/rules.toml hogar/apps/Dockerfile
git commit -m "apps: casa 1.11.0 / mando を pin（ルールの一時停止）、リビングエアコン 0時OFF を戻す"
git push
hogar/scripts/build.sh apps
hogar/scripts/deploy.sh
```

- [ ] **Step 5: 実機確認**

```bash
ssh nas 'docker exec hogar-casad casad rules list | head -40'
ssh nas 'docker exec hogar-mando casad rules list --json | head -c 300'
curl -s http://<NAS の IP>:8080/api/rules | head -c 300
```

Expected: 全ルールが出る。mando コンテナからも同じ list が取れる。

続いて副作用の無い時刻ルールで往復を確認（実機を動かさないため、発火時刻が近くないルールを選ぶ）:

```bash
ssh nas 'docker exec hogar-mando casad rules pause "玄関照明 5時に点灯" && cat /volume1/docker/hogar/data/casa/paused.json && docker exec hogar-mando casad rules resume "玄関照明 5時に点灯" && cat /volume1/docker/hogar/data/casa/paused.json'
```

Expected: 1 回目の cat に名前が載り、2 回目は `"paused": []`。スマホで mando を開き「⏸ ルール」セクションで止める→最上段に出る→再開、を本人に確認してもらう。`rule skipped: paused` のログ確認は、発火時刻が来る時刻ルールを止めた状態で `docker logs hogar-casad | grep "rule skipped"` を後日見る（即時確認が必要なら、Alexa 仮想スイッチ系のイベントルールを 1 本止めて声で試す）。

- [ ] **Step 6: vault 側の記録**

vault の該当ノート（`Money/エアコン定時オンオフの効果 2026-08.md` の関連、または Tech MOC 配下の casad / mando ノート）に「ルールは mando の ⏸ ルールから一時停止できる（2026-09-26〜）」を 1 行追記し、`python3 tools/vault_check.py` → 合計 0 件を確認してコミット・push（ファイル名を列挙して `git add`）。
