use super::{
    Arc, DEVICE_RETRY, Instant, Mixer, Mutex, OutputEvent, Playback, Prefs, Signal, Sink, Throttle,
    VoiceEvent, json, mpsc, output_event, prefs_get,
};

pub(super) struct Play<'a> {
    pub(super) mixer: Arc<Mutex<Mixer>>,
    pub(super) prefs: &'a Prefs,
    pub(super) sink: &'a Sink,
    pub(super) signal: Signal,
    pub(super) events: mpsc::UnboundedSender<OutputEvent>,
    pub(super) epoch: u64,
    pub(super) device: Option<String>,
    pub(super) live: bool,
    pub(super) headset: bool,
    pub(super) throttle: Throttle,
    pub(super) since: Option<Instant>,
    pub(super) failed_at: Option<Instant>,
}

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
                playback.take();
                self.device = None;
                self.failed_at = Some(Instant::now());
                self.report(false, Some(message));
            }
            _ => {}
        }
    }

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

    pub(super) fn signal_headset(&self, on: bool) {
        let signal = self.signal.clone();
        tauri::async_runtime::spawn(async move {
            let method = if on { "pauseHeadset" } else { "resumeHeadset" };
            if let Err(e) = signal.request(method, json!({})).await {
                log::warn!("{method} 失败：{e}");
            }
        });
    }

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
        let mut sent = false;
        assert_eq!(headset_change(&mut sent, false, true), None);
        assert_eq!(headset_change(&mut sent, true, true), Some(true));
        assert_eq!(headset_change(&mut sent, true, true), None);
        assert_eq!(headset_change(&mut sent, true, false), Some(false));
        let mut sent = true;
        assert_eq!(headset_change(&mut sent, false, false), None);
        assert_eq!(headset_change(&mut sent, false, true), None);
        assert_eq!(headset_change(&mut sent, true, true), None);
    }
}
