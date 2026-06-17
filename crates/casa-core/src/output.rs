//! stdout に出す JSON の組み立て。
//!
//! stdout は純粋な構造化 JSON のみ。`timestamp`（ISO 8601、casa が応答を
//! 組み立てた時刻）を必ず含める。

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{Local, SecondsFormat};
use serde_json::{json, Value};

use crate::config::Device;

/// casa が応答を組み立てた時刻（ISO 8601、ローカルオフセット付き）。
pub fn timestamp() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// `casa list` の応答。
pub fn list_response(devices: Vec<Value>) -> Value {
    json!({
        "timestamp": timestamp(),
        "devices": devices,
    })
}

/// list 内の 1 デバイス分のエントリ。
pub fn device_entry(name: &str, device: &Device) -> Value {
    let mut entry = serde_json::to_value(device).expect("device serialization cannot fail");
    entry["name"] = json!(name);
    entry
}

/// デバイス操作（get / set / on / off）の応答。
pub fn device_response(name: &str, device: &Device, value: Value) -> Value {
    json!({
        "timestamp": timestamp(),
        "device": name,
        "protocol": device.protocol(),
        "value": value,
    })
}

/// `casa describe` の応答。
pub fn describe_response(name: &str, device: &Device, properties: Value) -> Value {
    json!({
        "timestamp": timestamp(),
        "device": name,
        "protocol": device.protocol(),
        "properties": properties,
    })
}

/// `casa validate` の応答。load を通った時点で設定は妥当なので `valid` は常に true。
/// `warnings` は妥当だが実行時に問題になりうる点（アダプタ未実装プロトコル等）。
pub fn validate_response(
    path: &Path,
    version: u32,
    device_count: usize,
    protocols: BTreeMap<&str, u32>,
    warnings: Vec<Value>,
) -> Value {
    json!({
        "timestamp": timestamp(),
        "config": path.display().to_string(),
        "version": version,
        "device_count": device_count,
        "protocols": protocols,
        "warnings": warnings,
        "valid": true,
    })
}

/// stdout に 1 行 JSON として出力する。
pub fn emit(value: &Value) {
    println!("{value}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_is_iso8601() {
        let ts = timestamp();
        assert!(
            chrono::DateTime::parse_from_rfc3339(&ts).is_ok(),
            "not ISO 8601: {ts}"
        );
    }

    #[test]
    fn device_entry_merges_name_and_fields() {
        let device = Device::Echonet {
            ip: "192.0.2.10".into(),
            eoj: "0x013001".into(),
        };
        let entry = device_entry("living_aircon", &device);
        assert_eq!(entry["name"], "living_aircon");
        assert_eq!(entry["protocol"], "echonet");
        assert_eq!(entry["ip"], "192.0.2.10");
        assert_eq!(entry["eoj"], "0x013001");
    }
}
