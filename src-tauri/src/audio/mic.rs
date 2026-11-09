//! 麦克风状态机。
//! 根据连接、发言权限与静音状态决定采集与推流动作，不直接操作设备，便于测试。

use serde::Serialize;

/// 麦克风状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MicStatus {
    /// 未采集。
    Off,
    /// 正在启动。
    Starting,
    /// 正在采集。
    Active,
    /// 已静音。
    Muted,
    /// 系统拒绝麦克风权限。
    Denied,
    /// 没有可用的麦克风。
    NoDevice,
    /// 麦克风已断开。
    DeviceLost,
    /// 其他错误。
    Error,
}

/// 状态机输出的动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicAction {
    /// 开始采集。
    Start,
    /// 停止采集并释放设备。
    Stop,
    /// 请求服务端暂停推流。
    Pause,
    /// 请求服务端恢复推流，成功后开始采集。
    Resume,
    Report(MicStatus, Option<String>),
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct MicCtl {
    muted: bool,
    say: bool,
    connected: bool,
    running: bool,
    /// 服务端推流已暂停。
    paused: bool,
    /// 正在等待恢复推流的结果。
    resuming: bool,
    /// 尚未返回结果的恢复请求数。
    resumes: u32,
}

impl MicCtl {
    /// 创建状态机，`muted` 为初始静音状态。
    #[must_use]
    pub fn new(muted: bool) -> Self {
        Self {
            muted,
            say: false,
            connected: false,
            running: false,
            paused: false,
            resuming: false,
            resumes: 0,
        }
    }

    #[must_use]
    pub fn muted(&self) -> bool {
        self.muted
    }

    #[must_use]
    pub fn running(&self) -> bool {
        self.running
    }

    /// 开始发送。推流已暂停时先请求恢复，恢复成功后再采集。
    /// 恢复请求在途时不重复发送。
    fn start(&mut self, out: &mut Vec<MicAction>) {
        if self.resuming {
            // 等待在途的恢复结果。
        } else if self.paused {
            self.paused = false;
            self.resuming = true;
            self.resumes += 1;
            out.push(MicAction::Resume);
        } else {
            self.running = true;
            out.push(MicAction::Start);
        }
        out.push(MicAction::Report(MicStatus::Starting, None));
    }

    fn stop(&mut self, out: &mut Vec<MicAction>) {
        if self.running {
            self.running = false;
            out.push(MicAction::Stop);
        }
    }

    fn pause(&mut self, out: &mut Vec<MicAction>) {
        self.resuming = false;
        if !self.paused {
            self.paused = true;
            out.push(MicAction::Pause);
        }
    }

    /// 媒体连接建立。没有发言权限时不采集，静音时暂停推流。
    /// `paused` 为服务端记录的推流暂停状态。
    pub fn on_connected(&mut self, say: bool, paused: bool) -> Vec<MicAction> {
        let mut out = Vec::new();
        self.connected = true;
        self.say = say;
        self.paused = paused;
        self.resuming = false;
        self.resumes = 0;
        if !say {
            out.push(MicAction::Report(MicStatus::Off, Some(NO_SAY.into())));
        } else if self.muted {
            self.pause(&mut out);
            out.push(MicAction::Report(MicStatus::Muted, None));
        } else {
            self.start(&mut out);
        }
        out
    }

    /// 发言权限变化。
    pub fn set_say(&mut self, say: bool) -> Vec<MicAction> {
        let mut out = Vec::new();
        if say == self.say {
            return out;
        }
        self.say = say;
        if !self.connected {
            return out;
        }
        if !say {
            self.stop(&mut out);
            self.pause(&mut out);
            out.push(MicAction::Report(MicStatus::Off, Some(NO_SAY.into())));
        } else if self.muted {
            self.pause(&mut out);
            out.push(MicAction::Report(MicStatus::Muted, None));
        } else {
            self.start(&mut out);
        }
        out
    }

    /// 设置静音。未连接或没有发言权限时只更新状态。
    pub fn set_muted(&mut self, muted: bool) -> Vec<MicAction> {
        let mut out = Vec::new();
        self.muted = muted;
        if !self.connected || !self.say {
            let status = if muted {
                MicStatus::Muted
            } else {
                MicStatus::Off
            };
            out.push(MicAction::Report(status, None));
            return out;
        }
        if muted {
            self.stop(&mut out);
            self.pause(&mut out);
            out.push(MicAction::Report(MicStatus::Muted, None));
        } else {
            // 重新打开设备，用于失败后的重试。
            self.stop(&mut out);
            self.start(&mut out);
        }
        out
    }

    /// 推流暂停或恢复的结果。
    /// 只处理最后一次恢复请求的结果。恢复失败时回到静音，不采集。
    pub fn on_producer(&mut self, pause: bool, result: Result<(), String>) -> Vec<MicAction> {
        if pause {
            return match result {
                Err(e) if self.muted && self.connected => vec![MicAction::Report(
                    MicStatus::Muted,
                    Some(format!("暂停推流失败：{e}")),
                )],
                _ => Vec::new(),
            };
        }
        self.resumes = self.resumes.saturating_sub(1);
        if self.resumes > 0 || !self.resuming {
            // 已被后续请求取代。
            return Vec::new();
        }
        self.resuming = false;
        match result {
            Ok(()) => {
                self.running = true;
                vec![MicAction::Start]
            }
            Err(e) => {
                self.paused = true;
                self.muted = true;
                vec![MicAction::Report(
                    MicStatus::Muted,
                    Some(format!("恢复推流失败：{e}")),
                )]
            }
        }
    }

    /// 采集已开始。
    pub fn on_started(&mut self, device: Option<String>) -> Vec<MicAction> {
        if !self.running {
            return Vec::new();
        }
        vec![MicAction::Report(MicStatus::Active, device)]
    }

    /// 采集失败。停止采集并暂停推流。
    pub fn on_failed(&mut self, status: MicStatus, message: String) -> Vec<MicAction> {
        if !self.running {
            return Vec::new();
        }
        let mut out = Vec::new();
        self.stop(&mut out);
        self.pause(&mut out);
        out.push(MicAction::Report(status, Some(message)));
        out
    }

    /// 切换输入设备，正在采集时重新打开。
    pub fn on_device_changed(&mut self) -> Vec<MicAction> {
        let mut out = Vec::new();
        if self.running {
            self.stop(&mut out);
            self.start(&mut out);
        }
        out
    }

    /// 连接断开，停止采集。
    pub fn on_disconnected(&mut self) -> Vec<MicAction> {
        let mut out = Vec::new();
        self.connected = false;
        self.resuming = false;
        self.stop(&mut out);
        out
    }
}

const NO_SAY: &str = "没有发言权限，其他成员听不到你的声音";

#[cfg(test)]
mod tests {
    use super::MicAction::{Pause, Report, Resume, Start, Stop};
    use super::*;

    #[test]
    fn starts_only_when_connected_allowed_and_unmuted() {
        let mut m = MicCtl::new(false);
        assert_eq!(
            m.on_connected(true, false),
            vec![Start, Report(MicStatus::Starting, None)]
        );
        assert_eq!(
            m.on_started(Some("内置麦克风".into())),
            vec![Report(MicStatus::Active, Some("内置麦克风".into()))]
        );

        let mut m = MicCtl::new(false);
        let a = m.on_connected(false, false);
        assert!(matches!(a.as_slice(), [Report(MicStatus::Off, Some(_))]));
        assert!(!m.running());
        // 没有发言权限时取消静音也不采集。
        assert_eq!(m.set_muted(false), vec![Report(MicStatus::Off, None)]);

        let mut m = MicCtl::new(true);
        assert_eq!(
            m.on_connected(true, false),
            vec![Pause, Report(MicStatus::Muted, None)]
        );
        assert!(!m.running());
    }

    #[test]
    fn mute_releases_device_and_unmute_resumes() {
        let mut m = MicCtl::new(false);
        m.on_connected(true, false);
        assert_eq!(
            m.set_muted(true),
            vec![Stop, Pause, Report(MicStatus::Muted, None)]
        );
        assert!(!m.running());
        // 重复静音不再暂停。
        assert_eq!(m.set_muted(true), vec![Report(MicStatus::Muted, None)]);
        // 取消静音先恢复推流，成功后才采集。
        assert_eq!(
            m.set_muted(false),
            vec![Resume, Report(MicStatus::Starting, None)]
        );
        assert!(!m.running());
        assert_eq!(m.on_producer(false, Ok(())), vec![Start]);
        assert!(m.running());
    }

    #[test]
    fn resume_failure_returns_to_muted_without_capturing() {
        let mut m = MicCtl::new(true);
        m.on_connected(true, false);
        assert_eq!(
            m.set_muted(false),
            vec![Resume, Report(MicStatus::Starting, None)]
        );
        assert!(!m.running(), "恢复结果返回前不采集");
        let a = m.on_producer(false, Err("超时".into()));
        assert!(matches!(a.as_slice(), [Report(MicStatus::Muted, Some(_))]));
        assert!(m.muted());
        assert!(!m.running());
        // 可以再次尝试。
        assert_eq!(
            m.set_muted(false),
            vec![Resume, Report(MicStatus::Starting, None)]
        );
    }

    #[test]
    fn stale_resume_result_is_ignored_after_mute() {
        let mut m = MicCtl::new(true);
        m.on_connected(true, false);
        m.set_muted(false);
        assert_eq!(
            m.set_muted(true),
            vec![Pause, Report(MicStatus::Muted, None)]
        );
        assert_eq!(m.on_producer(false, Ok(())), vec![]);
        assert!(!m.running());
        // 多次切换只采用最后一次恢复请求的结果。
        m.set_muted(false);
        m.set_muted(true);
        m.set_muted(false);
        assert_eq!(m.on_producer(true, Ok(())), vec![]);
        assert_eq!(m.on_producer(false, Ok(())), vec![]);
        assert_eq!(m.on_producer(false, Ok(())), vec![Start]);
    }

    #[test]
    fn pause_failure_keeps_backend_muted_and_reports_it() {
        let mut m = MicCtl::new(false);
        m.on_connected(true, false);
        m.set_muted(true);
        let a = m.on_producer(true, Err("超时".into()));
        assert!(matches!(a.as_slice(), [Report(MicStatus::Muted, Some(_))]));
        assert!(m.muted());
        assert!(!m.running());
    }

    #[test]
    fn say_permission_changes_after_connect() {
        let mut m = MicCtl::new(false);
        m.on_connected(true, false);
        assert!(m.running());
        let a = m.set_say(false);
        assert!(matches!(
            a.as_slice(),
            [Stop, Pause, Report(MicStatus::Off, Some(_))]
        ));
        assert!(!m.running());
        assert_eq!(m.set_say(false), vec![]);
        assert_eq!(
            m.set_say(true),
            vec![Resume, Report(MicStatus::Starting, None)]
        );
        assert_eq!(m.on_producer(false, Ok(())), vec![Start]);
        // 静音状态下获得发言权限，保持静音。
        let mut m = MicCtl::new(true);
        m.on_connected(false, true);
        assert_eq!(m.set_say(true), vec![Report(MicStatus::Muted, None)]);
        assert!(!m.running());
    }

    #[test]
    fn joined_paused_when_muted() {
        // 服务端已暂停推流时不重复暂停。
        let mut m = MicCtl::new(true);
        assert_eq!(
            m.on_connected(true, true),
            vec![Report(MicStatus::Muted, None)]
        );
        // 服务端未暂停时请求暂停。
        let mut m = MicCtl::new(true);
        assert_eq!(
            m.on_connected(true, false),
            vec![Pause, Report(MicStatus::Muted, None)]
        );
    }

    #[test]
    fn failure_pauses_and_retry_restarts() {
        let mut m = MicCtl::new(false);
        m.on_connected(true, false);
        assert_eq!(
            m.on_failed(MicStatus::Denied, "权限被拒绝".into()),
            vec![
                Stop,
                Pause,
                Report(MicStatus::Denied, Some("权限被拒绝".into()))
            ]
        );
        // 失败后不再处理设备事件。
        assert_eq!(m.on_failed(MicStatus::Error, "x".into()).len(), 0);
        assert_eq!(m.on_started(None).len(), 0);
        // 取消静音即重试。
        assert_eq!(
            m.set_muted(false),
            vec![Resume, Report(MicStatus::Starting, None)]
        );
        assert_eq!(m.on_producer(false, Ok(())), vec![Start]);
    }

    #[test]
    fn device_switch() {
        let mut m = MicCtl::new(false);
        m.on_connected(true, false);
        assert_eq!(
            m.on_device_changed(),
            vec![Stop, Start, Report(MicStatus::Starting, None)]
        );
        m.set_muted(true);
        assert_eq!(m.on_device_changed().len(), 0, "静音时不打开设备");
    }

    #[test]
    fn disconnect_stops_and_rebuild_restarts() {
        let mut m = MicCtl::new(false);
        m.on_connected(true, false);
        assert_eq!(m.on_disconnected(), vec![Stop]);
        assert_eq!(m.on_disconnected().len(), 0);
        // 重建后保持静音。
        m.set_muted(true);
        assert_eq!(
            m.on_connected(true, false),
            vec![Pause, Report(MicStatus::Muted, None)]
        );
        // 断开期间取消静音，重建后开始采集。
        m.on_disconnected();
        assert_eq!(m.set_muted(false), vec![Report(MicStatus::Off, None)]);
        assert_eq!(
            m.on_connected(true, false),
            vec![Start, Report(MicStatus::Starting, None)]
        );
    }
}
