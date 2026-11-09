//! 加入流程与单次连接的资源释放。

use super::{
    Api, AppError, AppResult, Capture, Ctl, EndReason, Inbound, JoinParams, MEDIA_CLOSE_TIMEOUT,
    Media, MediaEvent, Outcome, Peer, Playback, Prefs, REMOTE_QUEUE, RemotePacket, Signal, Sink,
    Value, VoiceEvent, failed_join, json, mpsc, offline_ctl, prefs_get, run, sdp,
};

/// 单次连接持有的资源。
#[derive(Default)]
pub(super) struct Slots {
    pub(super) signal: Option<Signal>,
    pub(super) media: Option<Media>,
    pub(super) capture: Option<Capture>,
    pub(super) playback: Option<Playback>,
}

impl Slots {
    /// 释放全部资源。先结束信令，再停止设备与媒体连接，设备卡住时不会推迟 exit。
    /// `exit` 为 true 时发送 exit，否则只关闭连接。
    pub(super) async fn release(&mut self, exit: bool) {
        self.end_signal(exit);
        // 三者互不依赖，并发释放，总耗时不超过单项超时。
        let (capture, playback, media) =
            (self.capture.take(), self.playback.take(), self.media.take());
        tokio::join!(
            async {
                if let Some(c) = capture {
                    c.stop().await;
                }
            },
            async {
                if let Some(p) = playback {
                    p.stop().await;
                }
            },
            async {
                if let Some(m) = media
                    && tokio::time::timeout(MEDIA_CLOSE_TIMEOUT, m.close())
                        .await
                        .is_err()
                {
                    log::warn!("媒体连接关闭超时。");
                }
            },
        );
    }

    /// 结束信令。`exit` 为 true 时发送 exit，否则只关闭连接。
    pub(super) fn end_signal(&mut self, exit: bool) {
        if let Some(s) = self.signal.take() {
            if exit {
                s.exit();
            } else {
                s.close();
            }
        }
    }
}

pub(super) struct Live {
    pub(super) inbound: mpsc::UnboundedReceiver<Inbound>,
    pub(super) media_rx: mpsc::UnboundedReceiver<MediaEvent>,
    pub(super) params: JoinParams,
    pub(super) producer_id: String,
    /// 服务端是否允许发言。
    pub(super) say: bool,
    pub(super) me_id: String,
    /// 加入时服务端记录的麦克风是否已暂停。
    pub(super) paused: bool,
    pub(super) remote: mpsc::Receiver<RemotePacket>,
    /// 已告知服务端的耳机状态。
    pub(super) headset: bool,
}

/// 获取信令地址，完成 quickJoin 与 initSpeaking，建立推流连接。
pub(super) async fn join(
    api: &Api,
    channel_id: &str,
    slots: &mut Slots,
    sink: &Sink,
    muted: bool,
    deafened: bool,
    password: &str,
) -> AppResult<Live> {
    let url = api.voice_gateway(channel_id, password).await?;
    let (signal, inbound) = Signal::connect(&url).await?;
    let signal = slots.signal.insert(signal).clone();
    let joined = signal
        .request(
            "quickJoin",
            json!({ "transport": "webrtc", "forceTcp": false, "forceRelay": false }),
        )
        .await?;
    let params = JoinParams::parse(&joined)?;
    let (media_tx, media_rx) = mpsc::unbounded_channel();
    // 队列满时丢弃新包，不阻塞媒体接收。
    let (remote_tx, remote) = mpsc::channel(REMOTE_QUEUE);
    let media = slots
        .media
        .insert(Box::pin(Media::new(media_tx, Some(remote_tx))).await?);
    let dtls = sdp::dtls(media.fingerprint());
    let init = signal
        .request(
            "initSpeaking",
            json!({
                "pauseHeadSet": deafened, "pauseMic": muted,
                "startAiReduce": false, "startAccompaniment": false,
                "rtpParameters": sdp::rtp_parameters(media.offer(), &params.audio),
                "pDtls": dtls, "cDtls": dtls,
            }),
        )
        .await?;
    let me = init
        .get("peer")
        .and_then(Peer::parse)
        .ok_or_else(|| AppError::Format("initSpeaking 响应缺少 peer".into()))?;
    let say = me.permission_say;
    let me_id = me.id.clone();
    // 加入时的麦克风状态以服务端为准。
    let paused = me.mic_off;
    let producer_id = init
        .pointer("/producer/id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let peers = init
        .get("peers")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Peer::parse).collect())
        .unwrap_or_default();
    media
        .set_send_answer(sdp::send_answer(&params.send, &media.offer().mid)?)
        .await?;
    sink(VoiceEvent::Joined { me, peers });
    Ok(Live {
        inbound,
        media_rx,
        params,
        producer_id,
        say,
        me_id,
        paused,
        remote,
        headset: deafened,
    })
}

#[cfg(any(test, feature = "test-media"))]
use super::lost;

pub(super) async fn attempt(
    api: &Api,
    channel_id: &str,
    ctl: &mut mpsc::UnboundedReceiver<Ctl>,
    sink: &Sink,
    prefs: &Prefs,
) -> Outcome {
    #[cfg(any(test, feature = "test-media"))]
    if prefs_get(prefs).fail_join {
        return lost("已注入加入失败");
    }
    let mut slots = Slots::default();
    let started = std::time::Instant::now();
    log::info!("语音加入开始");
    // 加入期间仍处理控制命令，收到退出时中止加入。
    let joined = {
        let p = prefs_get(prefs);
        let password = p.password.clone().unwrap_or_default();
        let fut = join(
            api,
            channel_id,
            &mut slots,
            sink,
            p.muted,
            p.deafen.on(),
            &password,
        );
        tokio::pin!(fut);
        loop {
            tokio::select! {
                r = &mut fut => break Some(r),
                c = ctl.recv() => match c {
                    Some(Ctl::Leave) | None => break None,
                    Some(c) => offline_ctl(c, sink, prefs),
                }
            }
        }
    };
    let live = match joined {
        None => {
            slots.release(true).await;
            return Outcome::End(EndReason::Left, None);
        }
        Some(Err(e)) => {
            log::warn!(
                "语音加入失败：{e}，耗时 {}ms",
                started.elapsed().as_millis()
            );
            let (code, closed) = slots
                .signal
                .as_ref()
                .map_or((None, false), |s| (s.disconnect_code(), s.is_closed()));
            let (outcome, exit) = failed_join(&e, code, closed);
            slots.release(exit).await;
            return outcome;
        }
        Some(Ok(l)) => {
            log::info!("语音加入完成，耗时 {}ms", started.elapsed().as_millis());
            l
        }
    };
    let (outcome, server_ended) = run(live, &mut slots, ctl, sink, prefs).await;
    // 服务端已关闭连接时不再发送 exit。
    // 其余情况发送 exit 后释放。
    slots.release(!server_ended).await;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn release_sends_exit_before_waiting_for_stuck_devices() {
        use futures_util::StreamExt;
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = l.accept().await.unwrap();
            tokio_tungstenite::accept_async(tcp).await.unwrap()
        });
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        let (sig, _rx) = Signal::spawn(ws, Duration::from_millis(200));
        let mut server = server.await.unwrap();
        let mut slots = Slots {
            signal: Some(sig),
            playback: Some(crate::audio::output::Playback::stuck()),
            ..Slots::default()
        };
        let release = tokio::spawn(async move { slots.release(true).await });
        // 设备停止卡住时，exit 仍应立即发出。
        let exit = tokio::time::timeout(Duration::from_millis(1000), async {
            loop {
                if let Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) =
                    server.next().await
                {
                    return serde_json::from_str::<Value>(&t).unwrap();
                }
            }
        })
        .await
        .expect("exit 未及时发送");
        assert_eq!(exit["method"], "exit");
        release.await.unwrap();
    }
}
