//! 连接期间的麦克风控制与推流请求队列。

use super::{
    AppResult, Capture, CaptureEvent, DEVICE_RETRY, Instant, MicAction, MicCtl, MicStatus,
    PeerPatch, Prefs, Signal, Sink, Value, VoiceEvent, json, mpsc, oneshot, peer, prefs_get,
};

/// 暂停或恢复推流的请求，附带可选的结果通道。
pub(super) type ProducerOp = (bool, Option<oneshot::Sender<AppResult<()>>>);

/// 推流请求的完成通知：是否为暂停请求与结果。
pub(super) type ProducerDone = (bool, Result<(), String>);

pub(super) fn producer_queue(
    signal: Signal,
    producer_id: String,
    done: mpsc::UnboundedSender<ProducerDone>,
) -> mpsc::UnboundedSender<ProducerOp> {
    let (tx, mut rx) = mpsc::unbounded_channel::<ProducerOp>();
    tauri::async_runtime::spawn(async move {
        while let Some((paused, reply)) = rx.recv().await {
            let method = if paused {
                "pauseProducer"
            } else {
                "resumeProducer"
            };
            let r = signal
                .request(method, json!({ "producerId": producer_id }))
                .await
                .map(|_| ());
            if let Err(e) = &r {
                log::warn!("{method} 失败：{e}");
            }
            let _ = done.send((paused, r.as_ref().copied().map_err(ToString::to_string)));
            if let Some(reply) = reply {
                let _ = reply.send(r);
            }
        }
    });
    tx
}

/// 从成员信息变化中读取本人的发言权限。
pub(super) fn own_say(method: &str, data: &Value, me: &str) -> Option<bool> {
    if method != "peerInfoChanged" && method != "peerPermissionChanged" {
        return None;
    }
    PeerPatch::parse(data)
        .filter(|p| p.id == me)
        .and_then(|p| p.permission_say)
}

/// 连接期间的麦克风控制。
pub(super) struct MicRun<'a> {
    pub(super) mic: MicCtl,
    pub(super) prefs: &'a Prefs,
    pub(super) sink: &'a Sink,
    pub(super) producer: mpsc::UnboundedSender<ProducerOp>,
    pub(super) events: mpsc::UnboundedSender<CaptureEvent>,
    pub(super) uplink: peer::Uplink,
    pub(super) epoch: u64,
    pub(super) device: Option<String>,
    /// 设备失败的时间，用于定时重试。
    pub(super) failed_at: Option<Instant>,
}

impl MicRun<'_> {
    /// 设备失败超过重试间隔且麦克风未静音时，返回重新开始采集的动作。
    pub(super) fn retry_due(&mut self, now: Instant) -> Option<Vec<MicAction>> {
        let due = self
            .failed_at
            .is_some_and(|t| now.saturating_duration_since(t) >= DEVICE_RETRY);
        if !due {
            return None;
        }
        self.failed_at = None;
        (!self.mic.muted() && !self.mic.running()).then(|| self.mic.set_muted(false))
    }

    /// 执行麦克风动作。`reply` 在推流请求完成或全部动作执行后答复。
    pub(super) async fn apply(
        &mut self,
        actions: Vec<MicAction>,
        capture: &mut Option<Capture>,
        mut reply: Option<oneshot::Sender<AppResult<()>>>,
    ) {
        for a in actions {
            match a {
                MicAction::Start => {
                    if let Some(c) = capture.take() {
                        c.stop().await;
                    }
                    self.epoch += 1;
                    self.device = None;
                    let p = prefs_get(self.prefs);
                    *capture = Some(Capture::start(
                        p.source,
                        p.gain,
                        p.denoise,
                        self.uplink.clone(),
                        self.epoch,
                        self.events.clone(),
                    ));
                }
                MicAction::Stop => {
                    if let Some(c) = capture.take() {
                        c.stop().await;
                    }
                    self.device = None;
                }
                MicAction::Pause | MicAction::Resume => {
                    let _ = self.producer.send((a == MicAction::Pause, reply.take()));
                }
                MicAction::Report(status, message) => {
                    let (device, message) = if status == MicStatus::Active {
                        self.device.clone_from(&message);
                        (message, None)
                    } else {
                        (self.device.clone(), message)
                    };
                    (self.sink)(VoiceEvent::Mic {
                        status,
                        muted: self.mic.muted(),
                        device,
                        message,
                    });
                }
            }
        }
        if let Some(r) = reply {
            let _ = r.send(Ok(()));
        }
    }

    /// 推流恢复失败导致静音时写回偏好。
    pub(super) fn sync_prefs(&self) {
        if self.mic.muted()
            && let Ok(mut p) = self.prefs.lock()
        {
            p.muted = true;
        }
    }

    /// 处理采集事件，忽略旧采集轮次的事件。
    pub(super) fn on_capture(&mut self, ev: CaptureEvent) -> Vec<MicAction> {
        match ev {
            CaptureEvent::Started { epoch, device } if epoch == self.epoch => {
                self.failed_at = None;
                self.mic.on_started(Some(device))
            }
            CaptureEvent::Failed {
                epoch,
                status,
                message,
            } if epoch == self.epoch => {
                self.failed_at = matches!(status, MicStatus::NoDevice | MicStatus::DeviceLost)
                    .then(Instant::now);
                self.mic.on_failed(status, message)
            }
            CaptureEvent::Level { epoch, level } if epoch == self.epoch && self.mic.running() => {
                (self.sink)(VoiceEvent::MicLevel { level });
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kook::voice::*;

    #[test]
    fn own_say_change_is_detected_only_for_me() {
        let d = json!({"peerId": "me", "permissionSay": false});
        assert_eq!(own_say("peerInfoChanged", &d, "me"), Some(false));
        assert_eq!(own_say("peerInfoChanged", &d, "u1"), None);
        assert_eq!(own_say("newPeer", &d, "me"), None);
        assert_eq!(
            own_say(
                "peerInfoChanged",
                &json!({"peerId": "me", "displayName": "x"}),
                "me"
            ),
            None
        );
    }
}
