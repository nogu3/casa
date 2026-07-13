//! プロトコルアダプタ層。デバイス定義から子 CLI の呼び出し（バイナリ名 + 引数）を組む。
//!
//! 新しいプロトコルの追加は以下の 3 点だけで済む（サブコマンドハンドラは触らない）:
//! 1. `config::Device` enum に variant を追加する。
//! 2. その variant の引数を組むアダプタを実装し、`adapter_for` に 1 行足す。
//! 3. アダプタのテストを追加する。

pub mod echonet;
pub mod matter;

use crate::config::Device;

/// 子 CLI の 1 回分の呼び出し。バイナリの実パス解決（PATH / `CASA_<BIN>_BIN` /
/// `[binaries]`）は runner の責務なので、ここでは論理名だけを持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub bin: &'static str,
    pub args: Vec<String>,
}

/// `color-temp` ショートカットの指定値。kelvin / mireds の排他は CLI 層（clap）が
/// 保証する。値は事前検証せず子 CLI にそのまま渡す（範囲外は mat / デバイス側が clamp）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorTemp {
    pub kelvin: Option<String>,
    pub mireds: Option<String>,
    pub transition: Option<String>,
}

/// プロトコル固有 CLI への引数組み立てを担う。プロトコルの知識はここに閉じる。
///
/// 未対応の操作は `None` を返し、呼び出し側（ops 層)が `protocol_unsupported`
/// （exit 14）に変換する。既定はすべて未対応なので、アダプタは対応する操作だけ
/// 実装すればよい。
pub trait Adapter {
    fn get(&self, device: &Device, property: &str) -> Option<Invocation> {
        let _ = (device, property);
        None
    }

    fn set(&self, device: &Device, property: &str, value: &str) -> Option<Invocation> {
        let _ = (device, property, value);
        None
    }

    fn describe(&self, device: &Device) -> Option<Invocation> {
        let _ = device;
        None
    }

    fn power(&self, device: &Device, on: bool) -> Option<Invocation> {
        let _ = (device, on);
        None
    }

    fn color_temp(&self, device: &Device, color: &ColorTemp) -> Option<Invocation> {
        let _ = (device, color);
        None
    }

    /// 長尾のプロトコル固有操作の汎用動詞。`command` は子 CLI のサブコマンド名を
    /// そのまま受け取り（casa は解釈しない）、アドレスフラグを注入して `args` を
    /// 素通しする。アドレス注入がコマンドによらずプロトコルごとに一様であることが前提。
    fn invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation> {
        let _ = (device, command, args);
        None
    }
}

/// 設定の `protocol` フィールド（= `Device` の variant）が dispatch の唯一の真実。
/// アダプタ未実装のプロトコルは `None`。
pub fn adapter_for(device: &Device) -> Option<&'static dyn Adapter> {
    match device {
        Device::Echonet { .. } => Some(&echonet::EchonetAdapter),
        Device::Matter { .. } => Some(&matter::MatterAdapter),
        // Phase 4: 公式 switchbot CLI（@switchbot/openapi-cli）を呼ぶアダプタを追加する。
        Device::Switchbot { .. } => None,
    }
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
            node_id: "1234".into(),
            endpoint: None,
        };
        let adapter = adapter_for(&device).unwrap();
        assert_eq!(adapter.get(&device, "1/onoff/on-off").unwrap().bin, "mat");
    }

    #[test]
    fn switchbot_has_no_adapter_yet() {
        let device = Device::Switchbot {
            device_id: "DUMMY-XX-XX".into(),
        };
        assert!(adapter_for(&device).is_none());
    }
}
