//! stdout に出す JSON の組み立て。
//!
//! stdout は純粋な構造化 JSON のみ。`timestamp`（ISO 8601、casa が応答を
//! 組み立てた時刻）を必ず含める。

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{Local, SecondsFormat};
use serde_json::{json, Value};

use crate::config::{Device, Group};
use crate::error::CasaError;

/// casa が応答を組み立てた時刻（ISO 8601、ローカルオフセット付き）。
pub fn timestamp() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// `casa list` の応答。
pub fn list_response(devices: Vec<Value>, groups: Vec<Value>) -> Value {
    json!({
        "timestamp": timestamp(),
        "devices": devices,
        "groups": groups,
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

/// list 内の 1 グループ分のエントリ。
pub fn group_entry(name: &str, group: &Group) -> Value {
    json!({
        "name": name,
        "members": group.members,
    })
}

/// グループ操作のメンバー 1 件分の結果。エラーは子 CLI の exit code を
/// `error.exit_code` に保存する（単体操作の「exit code 伝播」の等価物）。
pub fn group_member_result(
    name: &str,
    device: &Device,
    outcome: &Result<Value, CasaError>,
) -> Value {
    match outcome {
        Ok(value) => json!({
            "device": name,
            "protocol": device.protocol(),
            "ok": true,
            "value": value,
        }),
        Err(err) => json!({
            "device": name,
            "protocol": device.protocol(),
            "ok": false,
            "error": {
                "kind": err.kind.as_str(),
                "exit_code": err.exit_code(),
                "detail": err.detail,
            },
        }),
    }
}

/// グループ操作（on / off / color-temp / set）の応答。
/// `results` の順序は設定ファイル上のメンバー記載順。
pub fn group_response(group: &str, results: Vec<Value>) -> Value {
    json!({
        "timestamp": timestamp(),
        "group": group,
        "results": results,
    })
}

/// `casa validate` の応答。load を通った時点で設定は妥当なので `valid` は常に true。
/// `warnings` は妥当だが実行時に問題になりうる点（アダプタ未実装プロトコル等）。
pub fn validate_response(
    path: &Path,
    version: u32,
    device_count: usize,
    group_count: usize,
    protocols: BTreeMap<&str, u32>,
    warnings: Vec<Value>,
) -> Value {
    json!({
        "timestamp": timestamp(),
        "config": path.display().to_string(),
        "version": version,
        "device_count": device_count,
        "group_count": group_count,
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
    use crate::config::Group;
    use crate::error::{CasaError, ErrorKind};

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

    #[test]
    fn group_member_result_ok_shape() {
        let device = Device::Echonet {
            ip: "192.0.2.10".into(),
            eoj: "0x013001".into(),
        };
        let entry =
            group_member_result("living_aircon", &device, &Ok(serde_json::json!({"power": "on"})));
        assert_eq!(entry["device"], "living_aircon");
        assert_eq!(entry["protocol"], "echonet");
        assert_eq!(entry["ok"], true);
        assert_eq!(entry["value"]["power"], "on");
    }

    #[test]
    fn group_member_result_error_shape() {
        let device = Device::Echonet {
            ip: "192.0.2.10".into(),
            eoj: "0x013001".into(),
        };
        let err = CasaError::new(ErrorKind::ChildFailed(3), "timeout");
        let entry = group_member_result("living_aircon", &device, &Err(err));
        assert_eq!(entry["ok"], false);
        assert_eq!(entry["error"]["kind"], "child_failed");
        assert_eq!(entry["error"]["exit_code"], 3);
        assert_eq!(entry["error"]["detail"], "timeout");
        assert!(entry.get("value").is_none());
    }

    #[test]
    fn group_response_has_timestamp_group_results() {
        let response = group_response("living", vec![serde_json::json!({"ok": true})]);
        assert!(response["timestamp"].is_string());
        assert_eq!(response["group"], "living");
        assert_eq!(response["results"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn list_response_includes_groups() {
        let group = Group {
            members: vec!["a".into(), "b".into()],
        };
        let response = list_response(vec![], vec![group_entry("living", &group)]);
        assert_eq!(response["groups"][0]["name"], "living");
        assert_eq!(response["groups"][0]["members"][1], "b");
    }
}
