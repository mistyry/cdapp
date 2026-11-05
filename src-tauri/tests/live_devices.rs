//! 本机音频设备枚举：`cargo test --test live_devices -- --ignored --nocapture`
//! 只读取列表，不打开设备。输出设备数量与默认标记，不打印设备名称。

use cdapp_lib::audio::capture::input_devices;
use cdapp_lib::audio::output::output_devices;

#[test]
#[ignore = "需要本机音频设备"]
fn lists_inputs_and_outputs_by_id() {
    for (kind, list) in [
        (
            "输入",
            input_devices().map(|v| v.into_iter().map(|d| (d.id, d.default)).collect::<Vec<_>>()),
        ),
        (
            "输出",
            output_devices().map(|v| v.into_iter().map(|d| (d.id, d.default)).collect::<Vec<_>>()),
        ),
    ] {
        let list = list.expect("枚举失败");
        let ids: std::collections::HashSet<_> = list.iter().map(|d| &d.0).collect();
        assert_eq!(ids.len(), list.len(), "{kind}设备 ID 重复");
        assert!(
            list.iter().filter(|d| d.1).count() <= 1,
            "{kind}默认设备多于一个"
        );
        println!(
            "{kind}：{} 个，默认 {} 个",
            list.len(),
            list.iter().filter(|d| d.1).count()
        );
    }
}
