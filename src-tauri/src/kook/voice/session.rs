//! 会话句柄与控制句柄。

#[cfg(any(test, feature = "test-media"))]
use super::Fault;
use super::{
    Api, AppError, AppResult, Arc, Ctl, LEAVE_TIMEOUT, MicPrefs, Mutex, Prefs, Sink, Source,
    attempt, mpsc, oneshot, output, supervise,
};

/// 语音会话句柄。
pub struct VoiceSession {
    pub(super) ctl: mpsc::UnboundedSender<Ctl>,
    pub(super) done: tauri::async_runtime::JoinHandle<()>,
    pub channel_id: String,
    pub generation: u64,
}

impl VoiceSession {
    /// 以默认音频偏好在后台加入语音频道。
    #[must_use]
    pub fn start(api: Api, channel_id: String, generation: u64, sink: Sink) -> Self {
        Self::start_with(api, channel_id, generation, sink, MicPrefs::default())
    }

    /// 以指定音频偏好在后台加入语音频道。
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
        let done = tauri::async_runtime::spawn(drive(api, channel_id.clone(), rx, sink, prefs));
        Self {
            ctl,
            done,
            channel_id,
            generation,
        }
    }

    /// 退出并等待释放，最多 3 秒。超时后中止会话任务，不让旧会话继续运行。
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

    /// 设置麦克风静音。
    ///
    /// # Errors
    ///
    /// 会话已结束或服务端拒绝时返回错误。
    pub async fn pause(&self, paused: bool) -> AppResult<()> {
        self.control().pause(paused).await
    }

    /// 获取控制句柄。不持有会话即可发送命令。
    #[must_use]
    pub fn control(&self) -> VoiceControl {
        VoiceControl(self.ctl.clone())
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.done.inner().is_finished()
    }
}

/// 会话控制句柄。
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

    /// 切换采集来源，正在采集时立即重新打开设备。
    pub fn set_source(&self, source: Source) {
        let _ = self.0.send(Ctl::Source(source));
    }

    /// 设置耳机关闭。关闭耳机时同时静音麦克风，重新打开时恢复之前的麦克风状态。
    ///
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

    /// 切换播放设备。
    pub fn set_output(&self, sink: output::Sink) {
        let _ = self.0.send(Ctl::Output(sink));
    }

    /// 设置成员音量，上限为 `MAX_VOLUME`。
    pub fn set_volume(&self, peer: String, percent: u16) {
        let _ = self.0.send(Ctl::Volume(peer, percent));
    }

    /// 开启或关闭成员降噪。
    pub fn set_peer_denoise(&self, peer: String, on: bool) {
        let _ = self.0.send(Ctl::PeerDenoise(peer, on));
    }

    /// 设置总音量，上限为 `MAX_VOLUME`。
    pub fn set_master(&self, percent: u16) {
        let _ = self.0.send(Ctl::Master(percent));
    }

    /// 通知系统已唤醒。
    pub fn wake(&self) {
        let _ = self.0.send(Ctl::Wake);
    }

    /// 注入故障。
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
