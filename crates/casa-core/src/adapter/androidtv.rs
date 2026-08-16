//! Android TV アダプタ。実体は自作 `atv`（Android TV Remote protocol v2 の
//! 薄い CLI）のサブプロセス呼び出し。
//!
//! Remote v2 に単一プロパティ read/write や introspection は無いので、casa の
//! `get`/`set`/`describe` はこのプロトコルでは未対応（trait 既定の `None` = exit 14）。
//! `on`/`off` は atv 側が冪等（状態を見て必要なときだけ power key を送る）。
//! 状態読み取りは `casa invoke <name> status`。初回ペアリング（`atv pair`、stdin で
//! PIN 入力）は対話的なので casa 経由ではなく atv を直接叩く運用を想定するが、
//! invoke は素通しするので `casa invoke <name> pair` も動く。
//!
//! アドレス（host）は atv では常に `--host` フラグ（サブコマンド直後に注入）。
//! ペアリング証明書は atv 側の責務（`~/.config/atv/`）で、casa は何も渡さない。

use super::{Adapter, Invocation};
use crate::config::Device;

const BIN: &str = "atv";

pub struct AndroidtvAdapter;

/// デバイス定義から host を取り出す。dispatch は `adapter_for` が variant で
/// 行うので、他 variant が来ることはない。
fn host(device: &Device) -> Option<&str> {
    match device {
        Device::Androidtv { host } => Some(host),
        _ => None,
    }
}

impl Adapter for AndroidtvAdapter {
    /// `on`/`off` は atv の同名サブコマンドへ。冪等性は atv 側が保証する。
    fn power(&self, device: &Device, on: bool) -> Option<Invocation> {
        let command = if on { "on" } else { "off" };
        self.invoke(device, command, &[])
    }

    /// 長尾のプロトコル固有操作。`command` は atv のサブコマンド名（`status` 等）を
    /// そのまま受け取り、`--host` をサブコマンド直後に注入して後続 `args` を素通しする。
    fn invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation> {
        let host = host(device)?;
        let mut all = vec![command.to_string(), "--host".to_string(), host.to_string()];
        all.extend(args.iter().cloned());
        Some(Invocation {
            bin: BIN,
            args: all,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Device;

    fn device() -> Device {
        Device::Androidtv {
            host: "192.0.2.10".into(),
        }
    }

    fn args(inv: &Invocation) -> Vec<&str> {
        inv.args.iter().map(String::as_str).collect()
    }

    #[test]
    fn power_on_builds_atv_on_with_host() {
        let inv = AndroidtvAdapter.power(&device(), true).unwrap();
        assert_eq!(inv.bin, "atv");
        assert_eq!(args(&inv), ["on", "--host", "192.0.2.10"]);
    }

    #[test]
    fn power_off_builds_atv_off_with_host() {
        let inv = AndroidtvAdapter.power(&device(), false).unwrap();
        assert_eq!(args(&inv), ["off", "--host", "192.0.2.10"]);
    }

    #[test]
    fn invoke_injects_host_flag_after_command() {
        let inv = AndroidtvAdapter.invoke(&device(), "status", &[]).unwrap();
        assert_eq!(inv.bin, "atv");
        assert_eq!(args(&inv), ["status", "--host", "192.0.2.10"]);
    }

    #[test]
    fn invoke_passes_trailing_args_through() {
        let extra: Vec<String> = vec!["--port".into(), "6467".into()];
        let inv = AndroidtvAdapter.invoke(&device(), "pair", &extra).unwrap();
        assert_eq!(
            args(&inv),
            ["pair", "--host", "192.0.2.10", "--port", "6467"]
        );
    }

    #[test]
    fn get_set_describe_are_unsupported() {
        assert!(AndroidtvAdapter.get(&device(), "power").is_none());
        assert!(AndroidtvAdapter.set(&device(), "power", "on").is_none());
        assert!(AndroidtvAdapter.describe(&device()).is_none());
    }
}
