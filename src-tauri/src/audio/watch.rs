//! 音频设备变化监视。cpal 在 macOS 与 Linux 上没有设备增减通知，按 2 秒间隔比较设备列表，
//! 变化时回调。Windows 上 cpal 只在已打开的流上报告默认设备变化，同样使用轮询。

use std::time::Duration;

use super::capture::{InputDevice, input_devices};
use super::output::{OutputDevice, output_devices};

/// 轮询间隔。
pub const INTERVAL: Duration = Duration::from_secs(2);

/// 一次设备快照，失败的一侧为 None。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub inputs: Option<Vec<InputDevice>>,
    pub outputs: Option<Vec<OutputDevice>>,
}

#[must_use]
pub fn snapshot() -> Snapshot {
    Snapshot {
        inputs: input_devices().ok(),
        outputs: output_devices().ok(),
    }
}

fn changed<T: PartialEq>(a: Option<&T>, b: Option<&T>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a != b)
}

/// 设备名列表，默认设备后加星号。
fn names<'a>(v: Option<impl Iterator<Item = (&'a str, bool)>>) -> String {
    v.map(|it| {
        it.map(|(n, d)| if d { format!("{n}*") } else { n.to_owned() })
            .collect::<Vec<_>>()
            .join("、")
    })
    .unwrap_or_default()
}

/// 比较两次快照，返回变化的一侧：(输入变化, 输出变化)。读取失败视为未变化。
#[must_use]
pub fn diff(old: &Snapshot, new: &Snapshot) -> (bool, bool) {
    (
        changed(old.inputs.as_ref(), new.inputs.as_ref()),
        changed(old.outputs.as_ref(), new.outputs.as_ref()),
    )
}

/// 启动监视线程。`on_change` 参数为 (输入变化, 输出变化)。
pub fn start(on_change: impl Fn(bool, bool) + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("audio-watch".into())
        .spawn(move || {
            let mut last = snapshot();
            loop {
                std::thread::sleep(INTERVAL);
                let now = snapshot();
                let (i, o) = diff(&last, &now);
                if i || o {
                    log::info!(
                        "音频设备变化：输入 [{}]，输出 [{}]",
                        names(
                            now.inputs
                                .as_ref()
                                .map(|v| v.iter().map(|d| (d.name.as_str(), d.default)))
                        ),
                        names(
                            now.outputs
                                .as_ref()
                                .map(|v| v.iter().map(|d| (d.name.as_str(), d.default)))
                        ),
                    );
                    on_change(i, o);
                }
                if now.inputs.is_some() {
                    last.inputs = now.inputs;
                }
                if now.outputs.is_some() {
                    last.outputs = now.outputs;
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("无法启动设备监视线程：{e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, default: bool) -> InputDevice {
        InputDevice {
            id: id.into(),
            name: id.into(),
            default,
        }
    }

    #[test]
    fn diff_reports_changes_and_ignores_failures() {
        let a = Snapshot {
            inputs: Some(vec![dev("a", true)]),
            outputs: Some(vec![]),
        };
        assert_eq!(diff(&a, &a.clone()), (false, false));
        let b = Snapshot {
            inputs: Some(vec![dev("a", false), dev("b", true)]),
            ..a.clone()
        };
        assert_eq!(diff(&a, &b), (true, false));
        // 默认设备变化同样算变化。
        let c = Snapshot {
            inputs: Some(vec![dev("a", false)]),
            ..a.clone()
        };
        assert_eq!(diff(&a, &c), (true, false));
        let failed = Snapshot {
            inputs: None,
            outputs: None,
        };
        assert_eq!(diff(&a, &failed), (false, false));
    }
}
