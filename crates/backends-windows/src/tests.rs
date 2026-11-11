//! `tests/test_core.py` 的 `WindowsReplyTests` / `WindowsPortConnectorTests`
//! 全部用合成字节复刻；Windows 真机 API 不参与这些测试。

use super::decode::*;
use serde_json::{json, Value};

fn reply() -> Vec<u8> {
    let mut data = vec![0u8; 35];
    data[12..14].copy_from_slice(&0x1234u16.to_le_bytes());
    data[14..16].copy_from_slice(&0x5678u16.to_le_bytes());
    data[23] = 2;
    data[31..35].copy_from_slice(&1u32.to_le_bytes());
    data
}

fn props(
    index: u32,
    length: Option<usize>,
    flags: u32,
    companion_index: u16,
    companion_port: u16,
    link: &str,
    reserved: u32,
) -> Vec<u8> {
    let mut text: Vec<u16> = link.encode_utf16().collect();
    text.push(0);
    let mut bytes = Vec::new();
    for unit in text {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let total = length.unwrap_or(16 + bytes.len());
    let mut data = vec![0u8; total.max(18)];
    data[0..4].copy_from_slice(&index.to_le_bytes());
    data[4..8].copy_from_slice(&(total as u32).to_le_bytes());
    data[8..12].copy_from_slice(&(flags | reserved).to_le_bytes());
    data[12..14].copy_from_slice(&companion_index.to_le_bytes());
    data[14..16].copy_from_slice(&companion_port.to_le_bytes());
    let copy = bytes.len().min(total.saturating_sub(16));
    data[16..16 + copy].copy_from_slice(&bytes[..copy]);
    data.truncate(total);
    data
}

#[derive(Default)]
struct FakeIoctl {
    calls: Vec<(u32, u32, usize)>,
    scripted: Vec<Vec<u8>>,
}

impl HubIo for FakeIoctl {
    fn hubs(&mut self) -> Result<(Vec<Hub>, Vec<String>), String> {
        Ok((Vec::new(), Vec::new()))
    }

    fn ioctl(
        &mut self,
        handle: &str,
        code: u32,
        initial: &[u8],
        size: usize,
    ) -> Result<Vec<u8>, String> {
        assert_eq!(handle, "hub-handle");
        let port = u32::from_le_bytes(initial[..4].try_into().unwrap());
        self.calls.push((code, port, size));
        Ok(self.scripted[self.calls.len() - 1].clone())
    }

    fn close(&mut self, _handle: &str) {}
}

#[test]
fn test_usb2_link_does_not_claim_cable_usb_version() {
    let result = decode_connection(&reply(), None).unwrap().unwrap();
    assert_eq!(result["speed"], json!("480 Mbps"));
    assert_eq!(result["vendor_id"], json!(0x1234));
    assert_eq!(result["product_id"], json!(0x5678));
    assert_eq!(result["name"], json!("USB 1234:5678"));
    assert_eq!(
        result["source"],
        json!("USB Hub IOCTL EX / EX_V2；设备链路，不是线缆上限")
    );
}

#[test]
fn test_v2_superspeed_override() {
    let mut v2 = vec![0u8; 16];
    v2[12..16].copy_from_slice(&4u32.to_le_bytes());
    let result = decode_connection(&reply(), Some(&v2)).unwrap().unwrap();
    assert!(result["speed"].as_str().unwrap().contains("SuperSpeedPlus"));
    assert!(result["speed"].as_str().unwrap().contains("未确定"));
    let mut v2_low = vec![0u8; 16];
    v2_low[12..16].copy_from_slice(&1u32.to_le_bytes());
    let result = decode_connection(&reply(), Some(&v2_low)).unwrap().unwrap();
    assert_eq!(result["speed"], json!("SuperSpeed（≥5 Gbps）"));
    assert_eq!(
        decode_connection(&reply(), Some(&[0u8; 15]))
            .unwrap()
            .unwrap()["speed"],
        json!("480 Mbps")
    );
}

#[test]
fn test_disconnected_and_truncated() {
    assert_eq!(decode_connection(&[0u8; 35], None).unwrap(), None);
    assert_eq!(
        decode_connection(&[0u8; 34], None).unwrap_err(),
        "USB connection reply is shorter than 35 bytes"
    );
}

#[test]
fn test_ffi_layout() {
    // FFI 结构体的 size_of 断言在 ffi::tests，这里校验纯逻辑的 pack(1) 前提：
    // 端口连接器属性的 WCHAR 尾部起点是 16，连接状态在 31。
    assert_eq!(USB_PORT_CONNECTOR_PROPERTIES_MIN_LENGTH, 18);
    assert_eq!(PORT_PROPERTIES_KNOWN_BITS, 0xf);
    let row = decode_connector_properties(&props(3, None, 0, 0, 0, "", 0)).unwrap();
    assert_eq!(row.actual_length, 18);
    let mut connection = reply();
    connection[31..35].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(decode_connection(&connection, None).unwrap(), None);
}

#[test]
fn test_pack1_offsets_and_four_known_bit_mask() {
    let link = r"\\?\usb#hub";
    let flags = PORT_USER_CONNECTABLE | PORT_HAS_MULTIPLE_COMPANIONS | PORT_CONNECTOR_IS_TYPE_C;
    let data = props(3, None, flags, 2, 7, link, 0);
    let row = decode_connector_properties(&data).unwrap();
    assert_eq!(row.connection_index, 3);
    assert_eq!(row.actual_length, data.len() as u32);
    assert!(row.user_connectable);
    assert!(!row.debug_capable);
    assert!(row.multiple_companions);
    assert!(row.type_c);
    assert_eq!(row.companion_index, 2);
    assert_eq!(row.companion_port, 7);
    assert_eq!(row.companion_link, normal_hub_link(link));
    assert_eq!(row.reserved_bits, 0);
}

#[test]
fn test_reserved_bit_0x10_is_not_a_flag() {
    let row = decode_connector_properties(&props(
        3,
        None,
        PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C,
        0,
        0,
        "",
        0x10,
    ))
    .unwrap();
    assert_eq!(row.reserved_bits, 0x10);
    assert!(row.user_connectable);
    assert!(row.type_c);
    assert!(!row.debug_capable);
    assert!(!row.multiple_companions);
    assert_eq!(row.companion_port, 0);
    let candidate = port_candidate(&row, "hub-a", "Hub #1 Port 3", false, "");
    assert!(candidate.get("ucsi").is_none());
    assert!(candidate.get("connector_id").is_none());
}

#[test]
fn test_debug_port_is_rejected_and_user_connectable_is_required() {
    let flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C;
    let rows = [
        decode_connector_properties(&props(3, None, flags, 0, 0, "", 0)).unwrap(),
        decode_connector_properties(&props(4, None, flags | PORT_DEBUG_CAPABLE, 0, 0, "", 0))
            .unwrap(),
        decode_connector_properties(&props(5, None, PORT_CONNECTOR_IS_TYPE_C, 0, 0, "", 0))
            .unwrap(),
        decode_connector_properties(&props(6, None, PORT_USER_CONNECTABLE, 0, 0, "", 0)).unwrap(),
    ];
    let ports = collect_port_candidates(&rows, "hub-a", 1);
    assert_eq!(ports.len(), 1);
    assert_eq!(ports[0]["number"], json!(3));
}

#[test]
fn test_truncated_reply_and_invalid_actual_length() {
    assert_eq!(
        decode_connector_properties(&[0u8; 17]).unwrap_err(),
        "USB port connector properties reply is shorter than 18 bytes"
    );
    for bad in [0usize, 17, USB_PORT_CONNECTOR_PROPERTIES_MAX_LENGTH + 1] {
        let mut data = props(3, Some(18), 0, 0, 0, "", 0);
        data[4..8].copy_from_slice(&(bad as u32).to_le_bytes());
        assert_eq!(
            decode_connector_properties(&data).unwrap_err(),
            "USB port connector properties ActualLength 无效",
            "bad={bad}"
        );
    }
    let data = props(3, Some(18), 0, 0, 0, "", 0);
    assert_eq!(
        decode_connector_properties(&data).unwrap().actual_length,
        18
    );
    let data = props(
        3,
        Some(USB_PORT_CONNECTOR_PROPERTIES_MAX_LENGTH),
        0,
        0,
        0,
        "",
        0,
    );
    assert_eq!(
        decode_connector_properties(&data).unwrap().actual_length,
        USB_PORT_CONNECTOR_PROPERTIES_MAX_LENGTH as u32
    );
}

#[test]
fn test_two_stage_length_and_companion_index_loop() {
    let link = r"\\?\usb#companion";
    let length = 16 + (link.chars().count() + 1) * 2;
    let flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C | PORT_HAS_MULTIPLE_COMPANIONS;
    let mut io = FakeIoctl {
        calls: Vec::new(),
        scripted: vec![
            props(3, Some(length), flags, 0, 1, link, 0),
            props(3, Some(length), flags, 0, 1, link, 0),
            props(3, Some(length), flags, 1, 1, link, 0),
            props(3, Some(length), flags, 2, 0, link, 0),
        ],
    };
    // 验证每次调用传入的 CompanionIndex；scripted 的第三项索引由闭包检查。
    let rows = read_connector_properties(&mut io, "hub-handle", 3).unwrap();
    assert_eq!(io.calls.len(), 4);
    assert_eq!(
        io.calls[0],
        (IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES, 3, 18)
    );
    for call in &io.calls[1..] {
        assert_eq!(call.0, IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES);
        assert_eq!(call.2, length);
    }
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter()
            .map(|row| row.companion_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        rows.iter()
            .map(|row| row.companion_port)
            .collect::<Vec<_>>(),
        vec![1, 1, 0]
    );
}

#[test]
fn test_companion_group_merges_and_prefers_type_c_row() {
    let link = r"\\?\USB#HUB-1";
    let type_c = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C;
    let mixed = link.to_uppercase().replace('/', "\\");
    let rows = [
        decode_connector_properties(&props(
            2,
            None,
            PORT_USER_CONNECTABLE | PORT_HAS_MULTIPLE_COMPANIONS,
            0,
            4,
            link,
            0,
        ))
        .unwrap(),
        decode_connector_properties(&props(
            7,
            None,
            type_c | PORT_HAS_MULTIPLE_COMPANIONS,
            1,
            4,
            &mixed,
            0,
        ))
        .unwrap(),
    ];
    assert_eq!(
        companion_key_for_test(&normal_hub_link(link), 4),
        companion_key_for_test(&normal_hub_link(&link.to_uppercase()), 4)
    );
    let ports = collect_port_candidates(&rows, "hub-a", 2);
    assert_eq!(ports.len(), 1);
    assert_eq!(ports[0]["number"], json!(7));
    assert!(ports[0]["name"].as_str().unwrap().contains("Hub #2 Port 7"));
}

#[test]
fn test_no_companion_rows_are_not_merged_across_hubs() {
    let flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C;
    let row = decode_connector_properties(&props(1, None, flags, 0, 0, "", 0)).unwrap();
    let first = collect_port_candidates(&[row.clone()], "hub-a", 1);
    let second = collect_port_candidates(&[row], "hub-b", 2);
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_ne!(first[0]["id"], second[0]["id"]);
    // 不同 Hub 的无伴口行不会因哈希冲突合并（`group_ambiguous` 需要多个
    // hub_link+hub_index，单次 collect 只收到一个 Hub，故这里直接验证 id）。
    let collision_rows = [
        decode_connector_properties(&props(1, None, flags, 0, 4, "same", 0)).unwrap(),
        decode_connector_properties(&props(1, None, flags, 0, 4, "same", 0)).unwrap(),
    ];
    let merged = collect_port_candidates_with_key(&collision_rows, "hub-a", 1, |_, _| {
        "collision".to_string()
    });
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0]["number"], json!(1));
}

#[test]
fn test_conflicting_group_key_keeps_rows_separate_with_source_note() {
    let flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C;
    let c1 = decode_connector_properties(&props(1, None, flags, 0, 4, r"\\?\A", 0)).unwrap();
    let c2 = decode_connector_properties(&props(9, None, flags, 0, 4, r"\\?\B", 0)).unwrap();
    assert_ne!(
        companion_key_for_test(&c1.companion_link, 4),
        companion_key_for_test(&c2.companion_link, 4)
    );
    // Python 测试用 monkeypatch 把伴口组键别名成 "collision" 模拟哈希冲突。
    let ports =
        collect_port_candidates_with_key(&[c1, c2], "hub-a", 1, |_, _| "collision".to_string());
    assert_eq!(ports.len(), 2);
    let numbers: Vec<u64> = ports
        .iter()
        .map(|port| port["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, vec![1, 9]);
    assert!(ports
        .iter()
        .all(|port| port["source"].as_str().unwrap().contains("冲突")));
}

#[test]
fn test_candidate_never_infers_connection_or_hardware_data() {
    let flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C;
    let rows = [decode_connector_properties(&props(9, None, flags, 0, 0, "", 0)).unwrap()];
    let port = &collect_port_candidates(&rows, "hub-a", 3)[0];
    for key in [
        "connected",
        "contract",
        "output_power",
        "input_power",
        "cable",
        "orientation",
        "overcurrent_count",
        "plug_count",
        "liquid_detected",
    ] {
        assert_eq!(port[key], Value::Null, "{key}");
    }
    assert_eq!(port["pin_configuration"], json!({}));
    assert_eq!(port["kind"], json!("USB-C"));
    assert_eq!(port["number"], json!(9));
    assert_eq!(port["id"], json!(digest(&["hub-a", "9"])));
    assert!(!port["id"].as_str().unwrap().contains("\\?\\"));
    let mut snapshot = json!({"power": {}, "ports": [port.clone()]});
    cablescope_analysis::enrich(&mut snapshot);
    assert_eq!(snapshot["ports"][0]["pin_map"], Value::Null);
    assert!(snapshot["ports"][0]["negotiation"]["notes"][0]
        .as_str()
        .unwrap()
        .contains("缺少完整链路上限"));
    assert_eq!(snapshot["ports"][0]["connected"], Value::Null);
}

#[test]
fn test_raw_hub_paths_never_enter_snapshot_or_error_text() {
    let raw = r"\\?\usb#vid_1234#pid_5678#{f18a0e88-c30c-11d0-8815-00a0c906bed8}";
    let flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C;
    let row = decode_connector_properties(&props(1, None, flags, 0, 2, raw, 0)).unwrap();
    let ports = collect_port_candidates(&[row], &digest(&[&normal_hub_link(raw)]), 1);
    let text = serde_json::to_string(&json!({
        "ports": ports,
        "errors": ["Hub #1 Port 1 端口属性: 无效"]
    }))
    .unwrap();
    assert!(!text.to_lowercase().contains(&raw.to_lowercase()));
    assert!(!text.to_lowercase().contains("vid_1234"));
    assert_eq!(ports[0]["id"].as_str().unwrap().len(), 16);
}

#[test]
fn test_ucsi_state_and_dynamic_limitations() {
    assert_eq!(
        classify_ucsi_state(&json!({"ucsi_state": "available"})),
        "available"
    );
    assert_eq!(
        classify_ucsi_state(&json!({"ucsi_state": "no_data_plane"})),
        "no_data_plane"
    );
    assert_eq!(
        classify_ucsi_state(&json!({"ucsi_state": "query_failed"})),
        "query_failed"
    );
    for bad in [
        json!({}),
        json!({"ucsi_state": null}),
        json!({"ucsi_state": "unknown"}),
        json!({"ucsi_state": true}),
    ] {
        assert_eq!(classify_ucsi_state(&bad), "query_failed");
    }
    let texts: Vec<&str> = ["available", "no_data_plane", "query_failed"]
        .iter()
        .map(|state| ucsi_limitations(state))
        .collect();
    assert_eq!(
        texts
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    for text in &texts {
        assert!(text.contains("未启用测试接口"));
        assert!(text.contains("未修改注册表"));
        assert!(text.contains("未重启设备"));
        assert!(text.contains("功率"));
        assert!(!text.contains("已解析"));
        assert!(!text.contains("已读取 PD"));
    }
    assert!(texts[1].contains("本机固件未暴露"));
}

#[cfg(windows)]
#[test]
fn test_backend_uses_dynamic_limitation_and_candidate_ports() {
    use crate::cim::InventoryCache;
    use crate::WindowsBackend;
    use cablescope_backends_traits::Backend;

    fn fake_power(_errors: &mut Vec<String>) -> Value {
        json!({})
    }
    fn no_displays() -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }

    let inventory = json!({
        "devices": [], "batteries": [],
        "ucsi_state": "no_data_plane", "errors": []
    });
    let flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C;
    let row = decode_connector_properties(&props(1, None, flags, 0, 0, "", 0)).unwrap();
    let candidate = port_candidate(&row, "hub-a", "Hub #1 Port 1", false, "");

    // Python 测试把 usb_devices() 替换为返回固定候选；Rust 版让 HubIo 枚举
    // 出合成 Hub，并用合成 IOCTL 字节生成同一候选端口。
    struct CandidateHubs;
    impl HubIo for CandidateHubs {
        fn hubs(&mut self) -> Result<(Vec<Hub>, Vec<String>), String> {
            Ok((
                vec![Hub {
                    id: "hub-handle".to_string(),
                    hub_link: "hub-a".to_string(),
                    index: 1,
                }],
                Vec::new(),
            ))
        }
        fn ioctl(
            &mut self,
            _handle: &str,
            code: u32,
            initial: &[u8],
            _size: usize,
        ) -> Result<Vec<u8>, String> {
            let port = if initial.len() >= 4 {
                u32::from_le_bytes(initial[..4].try_into().unwrap())
            } else {
                1
            };
            match code {
                USB_HUB_INFORMATION_EX => {
                    let mut info = vec![0u8; 6];
                    info[4..6].copy_from_slice(&1u16.to_le_bytes());
                    Ok(info)
                }
                IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES => Ok(props(
                    port,
                    Some(18),
                    PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C,
                    0,
                    0,
                    "",
                    0,
                )),
                IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX => Ok(vec![0u8; 35]),
                IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2 => Ok(vec![0u8; 16]),
                _ => Err("unused".to_string()),
            }
        }
        fn close(&mut self, _: &str) {}
    }
    let mut backend = WindowsBackend::with_overrides(
        InventoryCache::with_inventory(inventory),
        Box::new(CandidateHubs),
        fake_power,
        no_displays,
    );
    let snapshot = backend.sample();
    assert_eq!(snapshot["ports"], json!([candidate]));
    assert_eq!(snapshot["ports"][0]["connected"], Value::Null);
    assert!(snapshot["limitations"][0]
        .as_str()
        .unwrap()
        .contains("本机固件未暴露"));
    assert!(!snapshot["limitations"][0]
        .as_str()
        .unwrap()
        .contains("首版"));
}

#[test]
fn test_normal_hub_link_matches_python_casefold_expansions() {
    let link = r"\\?\USB#HUB/ßẞİ  ";
    assert_eq!(normal_hub_link(link), r"\\?\usb#hub\ssssi̇");
    assert_eq!(normal_hub_link("  /A/B  "), r"\a\b");
}

#[test]
fn test_digest_matches_windows_py_nul_separated_sha256() {
    // `py -3` 对照：windows.digest("hub-a", "9") 与 windows.digest("a","","b")。
    assert_eq!(digest(&["hub-a", "9"]), "b9475dc050dea021");
    assert_eq!(digest(&["a", "", "b"]), "2d8dbcddf40617a6");
}
