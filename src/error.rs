//! casa のエラー表現。
//!
//! stderr へは `{"error": {"kind": "...", "detail": "..."}}` の 1 行 JSON を出し、
//! exit code は CLAUDE.md の規約に従う。

use std::fmt;

/// エラー種別。stderr の `kind` フィールドと exit code の唯一の対応表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// 設定ファイルが存在しない。
    ConfigMissing,
    /// 設定ファイルの読み込み・パース・バリデーション失敗。
    ConfigParse,
    /// 指定された名前が設定ファイルに無い。
    NameNotFound,
    /// 子 CLI バイナリが見つからない / 実行不可。
    ChildNotFound,
    /// 子 CLI が非ゼロ exit code で終了した。コードはそのまま伝播する。
    ChildFailed(i32),
    /// 子 CLI の stdout が JSON としてパースできない。
    ChildInvalidOutput,
    /// その操作に対応するアダプタが未実装のプロトコル。
    ProtocolUnsupported,
}

impl ErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::ConfigMissing => "config_missing",
            ErrorKind::ConfigParse => "config_parse",
            ErrorKind::NameNotFound => "name_not_found",
            ErrorKind::ChildNotFound => "child_not_found",
            ErrorKind::ChildFailed(_) => "child_failed",
            ErrorKind::ChildInvalidOutput => "child_invalid_output",
            ErrorKind::ProtocolUnsupported => "protocol_unsupported",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            ErrorKind::ConfigMissing | ErrorKind::ConfigParse => 10,
            ErrorKind::NameNotFound => 11,
            ErrorKind::ChildNotFound => 12,
            ErrorKind::ChildFailed(code) => *code,
            ErrorKind::ChildInvalidOutput => 13,
            ErrorKind::ProtocolUnsupported => 14,
        }
    }
}

#[derive(Debug)]
pub struct CasaError {
    pub kind: ErrorKind,
    pub detail: String,
}

impl CasaError {
    pub fn new(kind: ErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub fn exit_code(&self) -> i32 {
        self.kind.exit_code()
    }

    /// stderr に出す 1 行 JSON。
    pub fn to_stderr_json(&self) -> String {
        serde_json::json!({
            "error": {
                "kind": self.kind.as_str(),
                "detail": self.detail,
            }
        })
        .to_string()
    }
}

impl fmt::Display for CasaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.detail)
    }
}

impl std::error::Error for CasaError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_convention() {
        assert_eq!(ErrorKind::ConfigMissing.exit_code(), 10);
        assert_eq!(ErrorKind::ConfigParse.exit_code(), 10);
        assert_eq!(ErrorKind::NameNotFound.exit_code(), 11);
        assert_eq!(ErrorKind::ChildNotFound.exit_code(), 12);
        assert_eq!(ErrorKind::ChildFailed(3).exit_code(), 3);
        assert_eq!(ErrorKind::ChildFailed(4).exit_code(), 4);
        assert_eq!(ErrorKind::ChildInvalidOutput.exit_code(), 13);
        assert_eq!(ErrorKind::ProtocolUnsupported.exit_code(), 14);
    }

    #[test]
    fn stderr_json_shape() {
        let err = CasaError::new(ErrorKind::ConfigMissing, "no such file");
        let v: serde_json::Value = serde_json::from_str(&err.to_stderr_json()).unwrap();
        assert_eq!(v["error"]["kind"], "config_missing");
        assert_eq!(v["error"]["detail"], "no such file");
    }
}
