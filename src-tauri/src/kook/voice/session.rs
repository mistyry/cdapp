#[cfg(any(test, feature = "test-media"))]
use super::Fault;
use super::{
    Api, AppError, AppResult, Arc, Ctl, LEAVE_TIMEOUT, MicPrefs, Mutex, Phase, Prefs, Sink, Source,
    VoiceEvent, attempt, mpsc, oneshot, output, supervise,
};

pub struct VoiceSession {
    pub(super) ctl: mpsc::UnboundedSender<Ctl>,
    pub(super) done: tauri::async_runtime::JoinHandle<()>,
    pub(super) ended: tokio::sync::watch::Receiver<bool>,
    pub(super) rebuilt: tokio::sync::watch::Receiver<(u64, bool)>,
    pub channel_id: String,
    pub generation: u64,
}

#[derive(Clone)]
pub struct VoiceEnded(tokio::sync::watch::Receiver<bool>);

impl VoiceEnded {
    pub async fn wait(mut self) {
        let _ = self.0.wait_for(|e| *e).await;
    }
}

fn count_rebuilds(sink: Sink) -> (Sink, tokio::sync::watch::Receiver<(u64, bool)>) {
    let (tx, rx) = tokio::sync::watch::channel((0u64, false));
    let wrapped: Sink = Arc::new(move |e: VoiceEvent| {
        if let VoiceEvent::State { phase, .. } = &e {
            match phase {
                Phase::Reconnecting => tx.send_if_modified(|s| !std::mem::replace(&mut s.1, true)),
                Phase::Connected => tx.send_if_modified(|s| {
                    let was = std::mem::replace(&mut s.1, false);
                    s.0 += u64::from(was);
                    was
                }),
                Phase::Disconnected => tx.send_if_modified(|s| std::mem::replace(&mut s.1, false)),
                Phase::Connecting => false,
            };
        }
        sink(e);
    });
    (wrapped, rx)
}

impl VoiceSession {
    #[must_use]
    pub fn start(api: Api, channel_id: String, generation: u64, sink: Sink) -> Self {
        Self::start_with(api, channel_id, generation, sink, MicPrefs::default())
    }

    #[must_use]
    pub fn start_with(
        api: Api,
        channel_id: String,
        generation: u64,
        sink: Sink,
        mic: MicPrefs,
    ) -> Self {
        let (ctl, rx) = mpsc::unbounded_channel();
        let prefs = Arc::new(Mutex::new(mic));
        let (ended_tx, ended) = tokio::sync::watch::channel(false);
        let (sink, rebuilt) = count_rebuilds(sink);
        let task = drive(api, channel_id.clone(), rx, sink, prefs);
        let done = tauri::async_runtime::spawn(async move {
            task.await;
            let _ = ended_tx.send(true);
        });
        Self {
            ctl,
            done,
            ended,
            rebuilt,
            channel_id,
            generation,
        }
    }

    #[must_use]
    pub fn ended(&self) -> VoiceEnded {
        VoiceEnded(self.ended.clone())
    }

    #[must_use]
    pub fn rebuilt(&self) -> tokio::sync::watch::Receiver<(u64, bool)> {
        self.rebuilt.clone()
    }

    pub async fn leave(self) {
        let _ = self.ctl.send(Ctl::Leave);
        let abort = self.done.inner().abort_handle();
        if tokio::time::timeout(LEAVE_TIMEOUT, self.done)
            .await
            .is_err()
        {
            log::warn!("语音会话释放超时，已中止。");
            abort.abort();
        }
    }

    /// # Errors
    ///
    /// 会话已结束或服务端拒绝时返回错误。
    pub async fn pause(&self, paused: bool) -> AppResult<()> {
        self.control().pause(paused).await
    }

    #[must_use]
    pub fn control(&self) -> VoiceControl {
        VoiceControl(self.ctl.clone())
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.done.inner().is_finished()
    }
}

#[derive(Clone)]
pub struct VoiceControl(mpsc::UnboundedSender<Ctl>);

impl VoiceControl {
    /// # Errors
    ///
    /// 会话已结束或服务端拒绝时返回错误。
    pub async fn pause(&self, paused: bool) -> AppResult<()> {
        let (tx, rx) = oneshot::channel();
        self.0
            .send(Ctl::Mute(paused, tx))
            .map_err(|_| AppError::Other("语音已断开".into()))?;
        rx.await.map_err(|_| AppError::Other("语音已断开".into()))?
    }

    pub fn set_source(&self, source: Source) {
        let _ = self.0.send(Ctl::Source(source));
    }

    /// # Errors
    ///
    /// 会话已结束或服务端拒绝时返回错误。
    pub async fn deafen(&self, on: bool) -> AppResult<()> {
        let (tx, rx) = oneshot::channel();
        self.0
            .send(Ctl::Deafen(on, tx))
            .map_err(|_| AppError::Other("语音已断开".into()))?;
        rx.await.map_err(|_| AppError::Other("语音已断开".into()))?
    }

    pub fn set_output(&self, sink: output::Sink) {
        let _ = self.0.send(Ctl::Output(sink));
    }

    pub fn set_volume(&self, peer: String, percent: u16) {
        let _ = self.0.send(Ctl::Volume(peer, percent));
    }

    pub fn set_peer_denoise(&self, peer: String, on: bool) {
        let _ = self.0.send(Ctl::PeerDenoise(peer, on));
    }

    pub fn set_master(&self, percent: u16) {
        let _ = self.0.send(Ctl::Master(percent));
    }

    pub fn wake(&self) {
        let _ = self.0.send(Ctl::Wake);
    }

    #[cfg(any(test, feature = "test-media"))]
    pub fn inject(&self, fault: Fault) {
        let _ = self.0.send(Ctl::Fault(fault));
    }
}

pub(super) async fn drive(
    api: Api,
    channel_id: String,
    ctl: mpsc::UnboundedReceiver<Ctl>,
    sink: Sink,
    prefs: Prefs,
) {
    let (s, p) = (sink.clone(), prefs.clone());
    let step = move |mut ctl: mpsc::UnboundedReceiver<Ctl>| {
        let (api, channel_id, sink, prefs) =
            (api.clone(), channel_id.clone(), s.clone(), p.clone());
        async move {
            let outcome = Box::pin(attempt(&api, &channel_id, &mut ctl, &sink, &prefs)).await;
            (outcome, ctl)
        }
    };
    supervise(ctl, &sink, &prefs, step).await;
}
