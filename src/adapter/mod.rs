//! プロトコルアダプタ層。デバイス定義から子 CLI の呼び出し（バイナリ名 + 引数）を組む。
//!
//! 新しいプロトコルの追加は以下の 3 点だけで済む（サブコマンドハンドラは触らない）:
//! 1. `config::Device` enum に variant を追加する。
//! 2. その variant の引数を組むアダプタを実装し、`adapter_for` に 1 行足す。
//! 3. アダプタのテストを追加する。

pub mod echonet;
pub mod matter;

use crate::config::Device;
use crate::error::{CasaError, ErrorKind};

/// 子 CLI の 1 回分の呼び出し。バイナリの実パス解決（PATH / `CASA_<BIN>_BIN` /
/// `[binaries]`）は runner の責務なので、ここでは論理名だけを持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub bin: &'static str,
    pub args: Vec<String>,
}

/// プロトコル固有 CLI への引数組み立てを担う。プロトコルの知識はここに閉じる。
///
/// 既定実装はすべて `protocol_unsupported`（exit 14）を返すので、アダプタは
/// 対応する操作だけ実装すればよい。プロトコル固有の引数形式エラーは
/// `invalid_argument`（exit 2）で返す。
pub trait Adapter {
    /// プロトコル名（エラーメッセージ用）。
    fn protocol(&self) -> &'static str;

    fn get(&self, device: &Device, property: &str) -> Result<Invocation, CasaError> {
        let _ = (device, property);
        Err(unsupported(self.protocol(), "get"))
    }

    fn set(&self, device: &Device, property: &str, value: &str) -> Result<Invocation, CasaError> {
        let _ = (device, property, value);
        Err(unsupported(self.protocol(), "set"))
    }

    fn describe(&self, device: &Device) -> Result<Invocation, CasaError> {
        let _ = device;
        Err(unsupported(self.protocol(), "describe"))
    }

    fn power(&self, device: &Device, on: bool) -> Result<Invocation, CasaError> {
        let _ = device;
        Err(unsupported(self.protocol(), if on { "on" } else { "off" }))
    }
}

/// 設定の `protocol` フィールド（= `Device` の variant）が dispatch の唯一の真実。
/// アダプタ未実装のプロトコルは `None`。
pub fn adapter_for(device: &Device) -> Option<&'static dyn Adapter> {
    match device {
        Device::Echonet { .. } => Some(&echonet::EchonetAdapter),
        Device::Matter { .. } => Some(&matter::MatterAdapter),
        // 公式 switchbot CLI（@switchbot/openapi-cli）を呼ぶアダプタを追加予定。
        Device::Switchbot { .. } => None,
    }
}

/// 未対応操作のエラー（exit 14）。
pub fn unsupported(protocol: &str, operation: &str) -> CasaError {
    CasaError::new(
        ErrorKind::ProtocolUnsupported,
        format!("operation \"{operation}\" is not yet supported for protocol \"{protocol}\""),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echonet_devices_dispatch_to_echonet_adapter() {
        let device = Device::Echonet {
            ip: "192.0.2.10".into(),
            eoj: "0x013001".into(),
        };
        let adapter = adapter_for(&device).unwrap();
        assert_eq!(adapter.get(&device, "0x80").unwrap().bin, "enl");
    }

    #[test]
    fn matter_devices_dispatch_to_matter_adapter() {
        let device = Device::Matter {
            node_id: 5,
            endpoint: 1,
        };
        let adapter = adapter_for(&device).unwrap();
        assert_eq!(adapter.describe(&device).unwrap().bin, "mat");
    }

    #[test]
    fn switchbot_has_no_adapter_yet() {
        let device = Device::Switchbot {
            device_id: "DUMMY-XX-XX".into(),
        };
        assert!(adapter_for(&device).is_none());
    }
}
