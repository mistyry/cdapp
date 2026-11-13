use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MicStatus {
    Off,
    Starting,
    Active,
    Muted,
    Denied,
    NoDevice,
    DeviceLost,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicAction {
    Start,
    Stop,
    Pause,
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
    paused: bool,
    resuming: bool,
    resumes: u32,
}

impl MicCtl {
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

    fn start(&mut self, out: &mut Vec<MicAction>) {
        if self.resuming {
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
            self.stop(&mut out);
            self.start(&mut out);
        }
        out
    }

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

    pub fn on_started(&mut self, device: Option<String>) -> Vec<MicAction> {
        if !self.running {
            return Vec::new();
        }
        vec![MicAction::Report(MicStatus::Active, device)]
    }

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

    pub fn on_device_changed(&mut self) -> Vec<MicAction> {
        let mut out = Vec::new();
        if self.running {
            self.stop(&mut out);
            self.start(&mut out);
        }
        out
    }

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
        assert_eq!(m.set_muted(true), vec![Report(MicStatus::Muted, None)]);
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
        let mut m = MicCtl::new(true);
        m.on_connected(false, true);
        assert_eq!(m.set_say(true), vec![Report(MicStatus::Muted, None)]);
        assert!(!m.running());
    }

    #[test]
    fn joined_paused_when_muted() {
        let mut m = MicCtl::new(true);
        assert_eq!(
            m.on_connected(true, true),
            vec![Report(MicStatus::Muted, None)]
        );
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
        assert_eq!(m.on_failed(MicStatus::Error, "x".into()).len(), 0);
        assert_eq!(m.on_started(None).len(), 0);
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
        m.set_muted(true);
        assert_eq!(
            m.on_connected(true, false),
            vec![Pause, Report(MicStatus::Muted, None)]
        );
        m.on_disconnected();
        assert_eq!(m.set_muted(false), vec![Report(MicStatus::Off, None)]);
        assert_eq!(
            m.on_connected(true, false),
            vec![Start, Report(MicStatus::Starting, None)]
        );
    }
}
