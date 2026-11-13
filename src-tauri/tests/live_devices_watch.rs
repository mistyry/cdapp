#[test]
#[ignore = "读取本机音频设备"]
fn snapshot_lists_devices() {
    let s = cdapp_lib::audio::watch::snapshot();
    println!(
        "输入 {:?}",
        s.inputs
            .as_ref()
            .map(|v| v.iter().map(|d| (&d.name, d.default)).collect::<Vec<_>>())
    );
    println!(
        "输出 {:?}",
        s.outputs
            .as_ref()
            .map(|v| v.iter().map(|d| (&d.name, d.default)).collect::<Vec<_>>())
    );
    assert!(s.inputs.is_some() && s.outputs.is_some());
}
