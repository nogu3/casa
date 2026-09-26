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
        self.paused
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.since.as_str())
    }

    /// 停止中に加える。既に停止中なら何もせず false（since は最初の値を保つ）。
    pub fn pause(&mut self, name: &str, since: String) -> bool {
        if self.is_paused(name) {
            return false;
        }
        self.paused.push(PausedRule {
            name: name.to_string(),
            since,
        });
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
    (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()
    })()
    .map_err(|e| io_err("write", e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io_err("replace", e)
    })
}

pub fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// rules.toml に無い名前が pause file に残っていたら warn する（改名・削除の取り残し）。
pub fn warn_unknown(file: &crate::rules::RuleFile, paused: &PauseFile) {
    for p in &paused.paused {
        if !file.rules.iter().any(|r| r.name == p.name) {
            tracing::warn!(rule = %p.name, "paused rule not found in rules file; ignored");
        }
    }
}

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
        assert!(f.pause(
            "リビングエアコン 0時OFF",
            "2026-09-26T21:00:00+09:00".into()
        ));
        save(&p, &f).unwrap();
        let back = load(&p).unwrap();
        assert_eq!(back, f);
        assert!(back.is_paused("リビングエアコン 0時OFF"));
        assert_eq!(
            back.since("リビングエアコン 0時OFF"),
            Some("2026-09-26T21:00:00+09:00")
        );
        // 一時ファイルが残っていない
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
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
