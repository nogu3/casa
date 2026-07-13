//! ECHONET Lite アダプタ。実体は `enl` のサブプロセス呼び出し。

use super::{Adapter, Invocation};
use crate::config::Device;

const BIN: &str = "enl";

/// ON/OFF ショートカットのマッピング先。
/// プロトコルロジックではなく UX としてのハードコード（CLAUDE.md 参照）。
pub const POWER_EPC: &str = "0x80";
pub const POWER_ON: &str = "0x30";
pub const POWER_OFF: &str = "0x31";

pub struct EchonetAdapter;

/// デバイス定義から (ip, eoj) を取り出す。dispatch は `adapter_for` が variant で
/// 行うので、他 variant が来ることはない。
fn address(device: &Device) -> Option<(&str, &str)> {
    match device {
        Device::Echonet { ip, eoj } => Some((ip, eoj)),
        _ => None,
    }
}

fn invocation(parts: &[&str]) -> Invocation {
    Invocation {
        bin: BIN,
        args: parts.iter().map(|s| s.to_string()).collect(),
    }
}

impl Adapter for EchonetAdapter {
    fn get(&self, device: &Device, property: &str) -> Option<Invocation> {
        let (ip, eoj) = address(device)?;
        Some(invocation(&[
            "get", "--ip", ip, "--eoj", eoj, "--epc", property,
        ]))
    }

    fn set(&self, device: &Device, property: &str, value: &str) -> Option<Invocation> {
        let (ip, eoj) = address(device)?;
        Some(invocation(&[
            "set", "--ip", ip, "--eoj", eoj, "--epc", property, "--value", value,
        ]))
    }

    fn describe(&self, device: &Device) -> Option<Invocation> {
        let (ip, eoj) = address(device)?;
        Some(invocation(&["describe", "--ip", ip, "--eoj", eoj]))
    }

    fn power(&self, device: &Device, on: bool) -> Option<Invocation> {
        let value = if on { POWER_ON } else { POWER_OFF };
        self.set(device, POWER_EPC, value)
    }

    fn invoke(&self, device: &Device, command: &str, args: &[String]) -> Option<Invocation> {
        let (ip, eoj) = address(device)?;
        let mut all = vec![
            command.to_string(),
            "--ip".to_string(),
            ip.to_string(),
            "--eoj".to_string(),
            eoj.to_string(),
        ];
        all.extend(args.iter().cloned());
        Some(Invocation { bin: BIN, args: all })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device::Echonet {
            ip: "192.0.2.10".into(),
            eoj: "0x013001".into(),
        }
    }

    fn args(inv: &Invocation) -> Vec<&str> {
        inv.args.iter().map(String::as_str).collect()
    }

    #[test]
    fn get_builds_enl_args() {
        let inv = EchonetAdapter.get(&device(), "0x80").unwrap();
        assert_eq!(inv.bin, "enl");
        assert_eq!(
            args(&inv),
            [
                "get",
                "--ip",
                "192.0.2.10",
                "--eoj",
                "0x013001",
                "--epc",
                "0x80"
            ]
        );
    }

    #[test]
    fn set_builds_enl_args() {
        let inv = EchonetAdapter.set(&device(), "0xb0", "0x42").unwrap();
        assert_eq!(
            args(&inv),
            [
                "set",
                "--ip",
                "192.0.2.10",
                "--eoj",
                "0x013001",
                "--epc",
                "0xb0",
                "--value",
                "0x42"
            ]
        );
    }

    #[test]
    fn describe_builds_enl_args() {
        let inv = EchonetAdapter.describe(&device()).unwrap();
        assert_eq!(
            args(&inv),
            ["describe", "--ip", "192.0.2.10", "--eoj", "0x013001"]
        );
    }

    #[test]
    fn power_on_maps_to_epc_0x80_value_0x30() {
        let inv = EchonetAdapter.power(&device(), true).unwrap();
        assert_eq!(
            args(&inv),
            [
                "set",
                "--ip",
                "192.0.2.10",
                "--eoj",
                "0x013001",
                "--epc",
                "0x80",
                "--value",
                "0x30"
            ]
        );
    }

    #[test]
    fn power_off_maps_to_epc_0x80_value_0x31() {
        let inv = EchonetAdapter.power(&device(), false).unwrap();
        assert_eq!(
            args(&inv),
            [
                "set",
                "--ip",
                "192.0.2.10",
                "--eoj",
                "0x013001",
                "--epc",
                "0x80",
                "--value",
                "0x31"
            ]
        );
    }

    #[test]
    fn invoke_injects_address_and_passes_args_through() {
        let extra: Vec<String> = vec!["--epc".into(), "0x80".into()];
        let inv = EchonetAdapter.invoke(&device(), "blink", &extra).unwrap();
        assert_eq!(inv.bin, "enl");
        assert_eq!(
            args(&inv),
            [
                "blink",
                "--ip",
                "192.0.2.10",
                "--eoj",
                "0x013001",
                "--epc",
                "0x80"
            ]
        );
    }

    #[test]
    fn invoke_with_no_extra_args_is_just_command_and_address() {
        let inv = EchonetAdapter.invoke(&device(), "blink", &[]).unwrap();
        assert_eq!(
            args(&inv),
            ["blink", "--ip", "192.0.2.10", "--eoj", "0x013001"]
        );
    }
}
