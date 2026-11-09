//! 连接期间的播放控制与耳机同步。

use super::{
    Arc, DEVICE_RETRY, Instant, Mixer, Mutex, OutputEvent, Playback, Prefs, Signal, Sink, Throttle,
    VoiceEvent, json, mpsc, output_event, prefs_get,
};

/// 连接期间的播放控制。
pub(super) struct Play<'a> {
    pub(super) mixer: Arc<Mutex<Mixer>>,
    pub(super) prefs: &'a Prefs,
    pub(super) sink: &'a Sink,
    pub(super) signal: Signal,
    pub(super) events: mpsc::UnboundedSender<OutputEvent>,
    pub(super) epoch: u64,
    pub(super) device: Option<String>,
    /// 媒体连接已建立，可以播放。
    pub(super) live: bool,
    /// 已告知服务端的耳机状态。
    pub(super) headset: bool,
    pub(super) throttle: Throttle,
    /// 媒体连接建立的时间，用于判断连接是否稳定。
    pub(super) since: Option<Instant>,
    /// 播放设备失败的时间，用于定时重试。
    pub(super) failed_at: Option<Instant>,
}

/// 连接后需要告知服务端的耳机状态，没有变化时返回 None。
pub(super) fn headset_change(sent: &mut bool, live: bool, want: bool) -> Option<bool> {
    (live && *sent != want).then(|| {
        *sent = want;
        want
    })
}

impl Play<'_> {
    pub(super) fn deafened(&self) -> bool {
        self.prefs.lock().is_ok_and(|p| p.deafen.on())
    }

    pub(super) fn report(&self, active: bool, message: Option<String>) {
        (self.sink)(output_event(
            self.deafened(),
            active,
            self.device.clone(),
            message,
        ));
    }

    /// 按连接与耳机状态打开或关闭播放设备。`restart` 为 true 时重新打开。
    pub(super) async fn sync(&mut self, playback: &mut Option<Playback>, restart: bool) {
        let want = self.live && !self.deafened();
        if let Some(p) = playback.take_if(|_| !want || restart) {
            p.stop().await;
            self.device = None;
        }
        if !want {
            if let Ok(mut m) = self.mixer.lock() {
                m.clear_all();
            }
            self.report(false, None);
            return;
        }
        if playback.is_none() {
            // 打开设备前清空缓冲，不播放积压的音频。
            if let Ok(mut m) = self.mixer.lock() {
                m.clear_all();
            }
            self.epoch += 1;
            let target = prefs_get(self.prefs).output;
            *playback = Some(Playback::start(
                target,
                self.mixer.clone(),
                self.epoch,
                self.events.clone(),
            ));
        }
    }

    pub(super) fn on_output(&mut self, ev: OutputEvent, playback: &mut Option<Playback>) {
        match ev {
            OutputEvent::Started { epoch, device } if epoch == self.epoch => {
                self.failed_at = None;
                self.device = Some(device);
                self.report(true, None);
            }
            OutputEvent::Failed { epoch, message } if epoch == self.epoch => {
                // 线程已退出，丢弃句柄即可。
                playback.take();
                self.device = None;
                self.failed_at = Some(Instant::now());
                self.report(false, Some(message));
            }
            _ => {}
        }
    }

    /// 播放设备失败超过重试间隔后重新打开。
    pub(super) async fn retry(&mut self, playback: &mut Option<Playback>, now: Instant) {
        if playback.is_none()
            && self
                .failed_at
                .is_some_and(|t| now.saturating_duration_since(t) >= DEVICE_RETRY)
        {
            self.failed_at = None;
            self.sync(playback, false).await;
        }
    }

    /// 把耳机状态发给服务端，不等待结果。
    pub(super) fn signal_headset(&self, on: bool) {
        let signal = self.signal.clone();
        tauri::async_runtime::spawn(async move {
            let method = if on { "pauseHeadset" } else { "resumeHeadset" };
            if let Err(e) = signal.request(method, json!({})).await {
                log::warn!("{method} 失败：{e}");
            }
        });
    }

    /// 耳机状态与服务端不一致时同步。
    pub(super) fn sync_headset(&mut self) {
        let want = self.deafened();
        if let Some(on) = headset_change(&mut self.headset, self.live, want) {
            self.signal_headset(on);
        }
    }

    pub(super) fn tick(&mut self) {
        let ids = if self.deafened() {
            Vec::new()
        } else {
            self.mixer.lock().map(|m| m.speaking()).unwrap_or_default()
        };
        if let Some(ids) = self.throttle.update(Instant::now(), ids) {
            (self.sink)(VoiceEvent::Speaking { ids });
        }
    }
}

pub(super) fn new_mixer(prefs: &Prefs) -> Arc<Mutex<Mixer>> {
    let p = prefs_get(prefs);
    let mut m = Mixer::default();
    m.set_volumes(&p.volumes);
    m.set_denoise_base(p.denoise.clone());
    m.set_denoised(&p.denoised);
    m.set_master(p.master);
    #[cfg(any(test, feature = "test-media"))]
    {
        m.tap = p.tap;
    }
    Arc::new(Mutex::new(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headset_toggled_before_connect_is_sent_once_connected() {
        // 连接前切换耳机只记录，不发送。
        let mut sent = false;
        assert_eq!(headset_change(&mut sent, false, true), None);
        // 连接后发送一次。
        assert_eq!(headset_change(&mut sent, true, true), Some(true));
        assert_eq!(headset_change(&mut sent, true, true), None);
        // 再次切换时发送。
        assert_eq!(headset_change(&mut sent, true, false), Some(false));
        // 加入时已关闭耳机，状态一致时不发送。
        let mut sent = true;
        assert_eq!(headset_change(&mut sent, false, false), None);
        assert_eq!(headset_change(&mut sent, false, true), None);
        assert_eq!(headset_change(&mut sent, true, true), None);
    }
}
