//! 已加入连接的事件循环。

#[cfg(any(test, feature = "test-media"))]
use super::Fault;
use super::{
    CONNECT_TIMEOUT, CaptureEvent, Ctl, DisconnectKind, DownStats, EndReason, HEALTH_TICK, Health,
    Inbound, Instant, Leg, LinkMeter, Live, Lost, Media, MediaEvent, MicCtl, MicRun, Outcome,
    OutputEvent, PcState, Phase, Play, Prefs, ProducerDone, Room, SPEAK_EVERY, SPEAK_TICK,
    STATS_EVERY, Signal, Sink, Slots, SystemTime, Throttle, UpStats, Value, VoiceEvent,
    closed_outcome, deafen_prefs, health, json, lost, mpsc, mute_prefs, new_mixer, own_say,
    prefs_get, producer_queue, settle_lost,
};

/// 未关麦的音轨超过该时长无包时记录日志。
const QUIET_LOG_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

/// 运行一次已加入的连接，返回结果与服务端是否已关闭信令。
pub(super) async fn run(
    live: Live,
    slots: &mut Slots,
    ctl: &mut mpsc::UnboundedReceiver<Ctl>,
    sink: &Sink,
    prefs: &Prefs,
) -> (Outcome, bool) {
    let (Some(signal), Some(uplink)) = (
        slots.signal.clone(),
        slots.media.as_ref().map(Media::uplink),
    ) else {
        return (lost("语音连接未就绪"), false);
    };
    let (cap_tx, mut cap_rx) = mpsc::unbounded_channel();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let mut play = Play {
        mixer: new_mixer(prefs),
        prefs,
        sink,
        signal: signal.clone(),
        events: out_tx,
        epoch: 0,
        device: None,
        live: false,
        headset: live.headset,
        throttle: Throttle::new(SPEAK_EVERY),
        since: None,
        failed_at: None,
    };
    let mut mic = MicRun {
        mic: MicCtl::new(prefs_get(prefs).muted),
        prefs,
        sink,
        producer: producer_queue(signal, live.producer_id.clone(), done_tx),
        events: cap_tx,
        uplink,
        epoch: 0,
        device: None,
        failed_at: None,
    };
    let mut chans = Chans {
        cap: &mut cap_rx,
        done: &mut done_rx,
        out: &mut out_rx,
    };
    let mut r = run_loop(live, slots, ctl, sink, &mut mic, &mut play, &mut chans).await;
    if let Outcome::Lost(l) = &mut r.0 {
        l.stable = play
            .since
            .is_some_and(|t| t.elapsed() >= health::STABLE_AFTER);
    }
    // 断线与 disconnect 通知可能同时到达，以信令记录的错误码为准，避免被替换后重入。
    let code = slots.signal.as_ref().and_then(Signal::disconnect_code);
    r = settle_lost(r.0, r.1, code);
    // 先结束信令再停止设备，设备卡住时不推迟退出。
    slots.end_signal(!r.1);
    let actions = mic.mic.on_disconnected();
    mic.apply(actions, &mut slots.capture, None).await;
    play.live = false;
    if let Some(p) = slots.playback.take() {
        p.stop().await;
    }
    if let Ok(mut m) = play.mixer.lock() {
        m.reset();
    }
    if !play.throttle.is_idle() {
        sink(VoiceEvent::Speaking { ids: Vec::new() });
    }
    r
}

/// 运行期间的事件通道。
pub(super) struct Chans<'a> {
    pub(super) cap: &'a mut mpsc::UnboundedReceiver<CaptureEvent>,
    pub(super) done: &'a mut mpsc::UnboundedReceiver<ProducerDone>,
    pub(super) out: &'a mut mpsc::UnboundedReceiver<OutputEvent>,
}

#[allow(clippy::too_many_lines)]
pub(super) async fn run_loop(
    mut live: Live,
    slots: &mut Slots,
    ctl: &mut mpsc::UnboundedReceiver<Ctl>,
    sink: &Sink,
    mic: &mut MicRun<'_>,
    play: &mut Play<'_>,
    ch: &mut Chans<'_>,
) -> (Outcome, bool) {
    let Some(media) = slots.media.as_ref() else {
        return (lost("语音连接未就绪"), false);
    };
    let capture = &mut slots.capture;
    let playback = &mut slots.playback;
    let mut room = Room {
        mixer: Some(play.mixer.clone()),
        ..Room::default()
    };
    let mut speak = tokio::time::interval(SPEAK_TICK);
    speak.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut disconnect: Option<i64> = None;
    let mut connected = false;
    let deadline = tokio::time::sleep(CONNECT_TIMEOUT);
    tokio::pin!(deadline);
    let mut health = Health::default();
    let mut check = tokio::time::interval(HEALTH_TICK);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut stats =
        tokio::time::interval_at(tokio::time::Instant::now() + STATS_EVERY, STATS_EVERY);
    stats.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let (stats_tx, mut stats_rx) = mpsc::unbounded_channel::<(Option<UpStats>, Option<u32>)>();
    let mut link = LinkMeter::default();
    // 每条音轨最后收到包的时刻与累计包数，用于诊断无数据重建。
    let mut tracks: std::collections::HashMap<u32, (Instant, u64)> =
        std::collections::HashMap::new();
    // 已记录为长时间无包的音轨，恢复收包时再记录一次。
    let mut quiet: std::collections::HashSet<u32> = std::collections::HashSet::new();
    loop {
        tokio::select! {
            _ = check.tick() => {
                let now = Instant::now();
                // 只记录，不处理：对端未关麦时是否持续发包尚未实测。
                if let Some(since) = play.since {
                    for ssrc in room.quiet_tracks(&tracks, since, now, QUIET_LOG_AFTER) {
                        if quiet.insert(ssrc) {
                            log::info!("语音音轨长时间无包；{}", room.describe(&tracks, now));
                        }
                    }
                }
                let checked = health.check(now, SystemTime::now());
                if let Some((mono, wall)) = checked.gap {
                    log::warn!(
                        "语音检查间隔异常：单调 {}ms，系统 {}ms；媒体断开 {:?}；{}",
                        mono.as_millis(),
                        wall.as_millis(),
                        health.down_for(now),
                        room.describe(&tracks, now),
                    );
                }
                if let Some(p) = checked.problem {
                    log::warn!(
                        "语音连接异常 {p:?}，重建；已连接 {}，媒体断开 {:?}；{}",
                        play.since.map_or_else(|| "-".into(), |t| format!("{}s", t.elapsed().as_secs())),
                        health.down_for(now),
                        room.describe(&tracks, now),
                    );
                    return (Outcome::Lost(Lost { message: p.message().into(), urgent: p.urgent(), stable: false }), false);
                }
                if connected {
                    play.retry(playback, now).await;
                    if let Some(actions) = mic.retry_due(now) {
                        mic.apply(actions, capture, None).await;
                    }
                }
            }
            _ = stats.tick(), if connected => {
                let (signal, producer, tx) = (play.signal.clone(), live.producer_id.clone(), stats_tx.clone());
                tauri::async_runtime::spawn(async move {
                    let t0 = Instant::now();
                    let r = signal.request("getProducerStats", json!({ "producerId": producer })).await;
                    // 信令往返时间作为连接延迟。
                    let rtt = r.is_ok().then(|| u32::try_from(t0.elapsed().as_millis()).unwrap_or(u32::MAX));
                    let _ = tx.send((r.ok().as_ref().and_then(health::up_stats), rtt));
                });
            }
            Some((up, rtt)) = stats_rx.recv() => {
                let (s, depth) = play.mixer.lock().map(|m| m.stats()).unwrap_or_default();
                sink(link.event(up, rtt, DownStats::from_jitter(&s, depth)));
            }
            () = &mut deadline, if !connected => {
                log::warn!("语音媒体连接超时");
                return (lost("语音连接超时"), false);
            }
            c = ctl.recv() => match c {
                Some(Ctl::Mute(muted, tx)) => {
                    // 打开麦克风会同时打开耳机，先恢复播放。
                    // 关闭麦克风不影响耳机。
                    if mute_prefs(mic.prefs, muted) {
                        play.sync_headset();
                        play.sync(playback, false).await;
                    }
                    let actions = mic.mic.set_muted(muted);
                    // 推流请求完成后答复。
                    mic.apply(actions, capture, Some(tx)).await;
                }
                Some(Ctl::Deafen(on, tx)) => {
                    let change = deafen_prefs(mic.prefs, on);
                    play.sync_headset();
                    play.sync(playback, false).await;
                    match change {
                        Some(muted) => {
                            let actions = mic.mic.set_muted(muted);
                            mic.apply(actions, capture, Some(tx)).await;
                        }
                        None => {
                            let _ = tx.send(Ok(()));
                        }
                    }
                }
                Some(Ctl::Output(o)) => {
                    let changed = mic.prefs.lock().map(|mut p| {
                        let changed = p.output != o;
                        p.output = o;
                        changed
                    });
                    if changed.unwrap_or(false) && play.live {
                        play.sync(playback, true).await;
                    }
                }
                Some(Ctl::Volume(peer, v)) => {
                    let v = v.min(crate::audio::mixer::MAX_VOLUME);
                    if let Ok(mut p) = mic.prefs.lock() {
                        p.volumes.insert(peer.clone(), v);
                    }
                    if let Ok(mut m) = play.mixer.lock() {
                        m.set_volume(&peer, v);
                    }
                }
                Some(Ctl::PeerDenoise(peer, on)) => {
                    if let Ok(mut p) = mic.prefs.lock() {
                        if on {
                            p.denoised.insert(peer.clone());
                        } else {
                            p.denoised.remove(&peer);
                        }
                    }
                    if let Ok(mut m) = play.mixer.lock() {
                        m.set_peer_denoise(&peer, on);
                    }
                }
                Some(Ctl::Master(v)) => {
                    let v = v.min(crate::audio::mixer::MAX_VOLUME);
                    if let Ok(mut p) = mic.prefs.lock() {
                        p.master = v;
                    }
                    if let Ok(mut m) = play.mixer.lock() {
                        m.set_master(v);
                    }
                }
                Some(Ctl::Source(src)) => {
                    let changed = mic.prefs.lock().map(|mut p| {
                        let changed = p.source != src;
                        p.source = src;
                        changed
                    });
                    if changed.unwrap_or(false) {
                        let actions = mic.mic.on_device_changed();
                        mic.apply(actions, capture, None).await;
                    }
                }
                Some(Ctl::Leave) | None => return (Outcome::End(EndReason::Left, None), false),
                Some(Ctl::Wake) => {
                    let down = health.down_for(Instant::now());
                    log::info!("主网关或系统网络已恢复；语音媒体断开 {down:?}");
                    // 媒体连接已断开时立即重建。
                    if health.disconnected() {
                        return (Outcome::Lost(Lost { message: "网络恢复后重新连接".into(), urgent: true, stable: false }), false);
                    }
                }
                #[cfg(any(test, feature = "test-media"))]
                Some(Ctl::Fault(f)) => match f {
                    Fault::CloseSignal => play.signal.close(),
                    Fault::Sleep => health.fake_sleep(),
                    Fault::FailJoin => {
                        if let Ok(mut p) = mic.prefs.lock() {
                            p.fail_join = true;
                        }
                        play.signal.close();
                    }
                },
            },
            Some(pkt) = live.remote.recv() => {
                let now = Instant::now();
                if quiet.remove(&pkt.ssrc)
                    && let Some((last, _)) = tracks.get(&pkt.ssrc)
                {
                    log::info!("语音音轨恢复收包，间隔 {}ms", now.saturating_duration_since(*last).as_millis());
                }
                let t = tracks.entry(pkt.ssrc).or_insert((now, 0));
                *t = (now, t.1 + 1);
                if let Ok(mut m) = play.mixer.lock() {
                    m.push(pkt.ssrc, pkt.sequence, pkt.timestamp, pkt.payload);
                }
            }
            _ = speak.tick(), if play.live => play.tick(),
            Some(ev) = ch.out.recv() => play.on_output(ev, playback),
            Some((pause, result)) = ch.done.recv() => {
                let actions = mic.mic.on_producer(pause, result);
                mic.sync_prefs();
                mic.apply(actions, capture, None).await;
            }
            Some(ev) = ch.cap.recv() => {
                let actions = mic.on_capture(ev);
                mic.apply(actions, capture, None).await;
            }
            ev = live.media_rx.recv() => match ev {
                Some(MediaEvent::State(Leg::Send, PcState::Connected)) if !connected => {
                    log::info!("语音媒体连接已建立");
                    connected = true;
                    play.since = Some(Instant::now());
                    let _ = health.on_media(0, true, Instant::now());
                    sink(VoiceEvent::state(Phase::Connected, None, None));
                    // 按发言权限与加入时的状态开始采集或暂停推流。
                    let actions = mic.mic.on_connected(live.say, live.paused);
                    mic.apply(actions, capture, None).await;
                    // 媒体连接建立后才开始播放。
                    play.live = true;
                    // 加入前切换的耳机状态在此同步。
                    play.sync_headset();
                    play.sync(playback, false).await;
                }
                Some(MediaEvent::State(leg, PcState::Failed)) => {
                    log::warn!("语音媒体连接失败 {leg:?}");
                    return (lost(format!("{}连接失败", if leg == Leg::Send { "推流" } else { "拉流" })), false);
                }
                Some(MediaEvent::State(leg, st @ (PcState::Connected | PcState::Disconnected))) => {
                    log::info!("语音媒体连接 {leg:?} {st:?}");
                    // 短暂断开等待宽限期，超时后由健康检查重建。
                    if let Some(d) = health.on_media(usize::from(leg == Leg::Recv), st == PcState::Connected, Instant::now()) {
                        log::info!("语音媒体连接 {leg:?} 恢复，断开 {}ms", d.as_millis());
                    }
                }
                _ => {}
            },
            msg = live.inbound.recv() => match msg {
                None | Some(Inbound::Closed) => {
                    log::warn!("语音信令关闭，断开码 {disconnect:?}");
                    return (closed_outcome(disconnect), true);
                }
                Some(Inbound::Request { method, data }) => {
                    if let Some(offer) = room.on_request(&method, &data, &live.params) {
                        match offer {
                            Ok(sdp) => {
                                if let Err(e) = media.update_recv(sdp).await {
                                    log::warn!("更新拉流连接失败：{e}");
                                }
                            }
                            Err(e) => log::warn!("处理 newConsumer 失败：{e}"),
                        }
                    }
                }
                Some(Inbound::Notification { method, data }) => {
                    if method == "disconnect" {
                        let code = data.get("errorCode").and_then(Value::as_i64).unwrap_or(0);
                        log::warn!("语音服务端通知断开 {code}");
                        disconnect = Some(code);
                        // 替换、正常退出与加入失败不重建。
                        if !DisconnectKind::from_code(code).may_retry() {
                            return (closed_outcome(disconnect), true);
                        }
                    } else if let Some(say) = own_say(&method, &data, &live.me_id) {
                        live.say = say;
                        room.on_notification(&method, &data, &live.params, sink);
                        let actions = mic.mic.set_say(say);
                        mic.apply(actions, capture, None).await;
                    } else if let Some(offer) = room.on_notification(&method, &data, &live.params, sink)
                        && let Ok(sdp) = offer
                        && let Err(e) = media.update_recv(sdp).await
                    {
                        log::warn!("更新拉流连接失败：{e}");
                    }
                }
            },
        }
    }
}
