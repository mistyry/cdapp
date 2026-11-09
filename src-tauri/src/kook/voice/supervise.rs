//! 断线重建与退避。

use super::{
    Ctl, Duration, EndReason, MAX_REBUILDS, Outcome, Phase, Prefs, Sink, VoiceEvent, health, mpsc,
    offline_ctl,
};

/// 运行会话并在连接中断时重建。
/// `step` 完成一次连接并返回结果，控制通道在两次连接之间交回。
pub(super) async fn supervise<F, Fut>(
    mut ctl: mpsc::UnboundedReceiver<Ctl>,
    sink: &Sink,
    prefs: &Prefs,
    mut step: F,
) where
    F: FnMut(mpsc::UnboundedReceiver<Ctl>) -> Fut,
    Fut: Future<Output = (Outcome, mpsc::UnboundedReceiver<Ctl>)>,
{
    sink(VoiceEvent::state(Phase::Connecting, None, None));
    let mut rebuilds = 0;
    loop {
        let (outcome, back) = step(ctl).await;
        ctl = back;
        let l = match outcome {
            Outcome::End(reason, message) => {
                log::info!("语音会话结束 {reason:?} {message:?}");
                sink(VoiceEvent::state(
                    Phase::Disconnected,
                    Some(reason),
                    message,
                ));
                return;
            }
            Outcome::Lost(l) => l,
        };
        if l.stable {
            rebuilds = 0;
        }
        if rebuilds >= MAX_REBUILDS {
            log::warn!("语音重建次数用尽：{}", l.message);
            sink(VoiceEvent::state(
                Phase::Disconnected,
                Some(EndReason::Lost),
                Some(l.message),
            ));
            return;
        }
        rebuilds += 1;
        log::warn!(
            "语音重建 {rebuilds}/{MAX_REBUILDS}：{}；稳定 {}，紧急 {}",
            l.message,
            l.stable,
            l.urgent
        );
        let wait = if l.urgent {
            Duration::ZERO
        } else {
            health::backoff(rebuilds)
        };
        sink(VoiceEvent::State {
            phase: Phase::Reconnecting,
            reason: None,
            message: Some(l.message),
            attempt: Some(rebuilds),
            max: Some(MAX_REBUILDS),
            retry_in_ms: Some(u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)),
        });
        let sleep = tokio::time::sleep(wait);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => break,
                c = ctl.recv() => match c {
                    Some(Ctl::Leave) | None => {
                        sink(VoiceEvent::state(Phase::Disconnected, Some(EndReason::Left), None));
                        return;
                    }
                    Some(Ctl::Wake) => break,
                    Some(c) => offline_ctl(c, sink, prefs),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kook::voice::testing::*;
    use crate::kook::voice::*;

    fn phases(log: &Mutex<Vec<VoiceEvent>>) -> Vec<(Phase, Option<EndReason>)> {
        log.lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                VoiceEvent::State { phase, reason, .. } => Some((*phase, *reason)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn rebuilds_lost_sessions_a_bounded_number_of_times() {
        let (sink, log) = recorder();
        let (_tx, rx) = mpsc::unbounded_channel();
        let tries = Arc::new(Mutex::new(0));
        let t = tries.clone();
        supervise(rx, &sink, &Prefs::default(), move |ctl| {
            *t.lock().unwrap() += 1;
            async move { (lost("断开"), ctl) }
        })
        .await;
        assert_eq!(*tries.lock().unwrap(), MAX_REBUILDS + 1);
        let p = phases(&log);
        assert_eq!(p.first(), Some(&(Phase::Connecting, None)));
        assert_eq!(
            p.iter().filter(|x| x.0 == Phase::Reconnecting).count(),
            MAX_REBUILDS as usize
        );
        assert_eq!(
            p.last(),
            Some(&(Phase::Disconnected, Some(EndReason::Lost)))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn replaced_or_exited_never_rebuilds() {
        for reason in [EndReason::Replaced, EndReason::Exited, EndReason::Left] {
            let (sink, log) = recorder();
            let (_tx, rx) = mpsc::unbounded_channel();
            let tries = Arc::new(Mutex::new(0));
            let t = tries.clone();
            // 首次断开后重建，第二次以不可重建的原因结束。
            supervise(rx, &sink, &Prefs::default(), move |ctl| {
                let n = {
                    let mut g = t.lock().unwrap();
                    *g += 1;
                    *g
                };
                async move {
                    let o = if n == 1 {
                        lost("断开")
                    } else {
                        Outcome::End(reason, None)
                    };
                    (o, ctl)
                }
            })
            .await;
            assert_eq!(*tries.lock().unwrap(), 2);
            assert_eq!(
                phases(&log).last(),
                Some(&(Phase::Disconnected, Some(reason)))
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn leave_during_backoff_stops_rebuilding() {
        let (sink, log) = recorder();
        let (tx, rx) = mpsc::unbounded_channel();
        let tries = Arc::new(Mutex::new(0));
        let t = tries.clone();
        tx.send(Ctl::Leave).unwrap();
        supervise(rx, &sink, &Prefs::default(), move |ctl| {
            *t.lock().unwrap() += 1;
            async move { (lost("断开"), ctl) }
        })
        .await;
        assert_eq!(*tries.lock().unwrap(), 1);
        assert_eq!(
            phases(&log).last(),
            Some(&(Phase::Disconnected, Some(EndReason::Left)))
        );
    }

    fn reconnects(log: &Mutex<Vec<VoiceEvent>>) -> Vec<(u32, u64)> {
        log.lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                VoiceEvent::State {
                    phase: Phase::Reconnecting,
                    attempt: Some(a),
                    retry_in_ms: Some(w),
                    ..
                } => Some((*a, *w)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn rebuild_backoff_reports_attempts_and_grows() {
        let (sink, log) = recorder();
        let (_tx, rx) = mpsc::unbounded_channel();
        let t0 = tokio::time::Instant::now();
        supervise(rx, &sink, &Prefs::default(), |ctl| async move {
            (lost("断开"), ctl)
        })
        .await;
        assert_eq!(reconnects(&log), vec![(1, 1000), (2, 2000), (3, 4000)]);
        // 三次退避共 1 + 2 + 4 秒。
        assert_eq!(t0.elapsed(), Duration::from_secs(7));
    }

    #[tokio::test(start_paused = true)]
    async fn wake_skips_backoff_and_urgent_loss_rebuilds_immediately() {
        let (sink, log) = recorder();
        let (tx, rx) = mpsc::unbounded_channel();
        let tries = Arc::new(Mutex::new(0u32));
        let t = tries.clone();
        let t0 = tokio::time::Instant::now();
        // 第一次退避期间收到唤醒，第二次为紧急重建。
        tx.send(Ctl::Wake).unwrap();
        supervise(rx, &sink, &Prefs::default(), move |ctl| {
            let n = {
                let mut g = t.lock().unwrap();
                *g += 1;
                *g
            };
            async move {
                let o = match n {
                    1 => lost("断开"),
                    2 => Outcome::Lost(Lost {
                        message: "系统唤醒".into(),
                        urgent: true,
                        stable: false,
                    }),
                    _ => Outcome::End(EndReason::Replaced, None),
                };
                (o, ctl)
            }
        })
        .await;
        assert_eq!(*tries.lock().unwrap(), 3);
        assert_eq!(t0.elapsed(), Duration::ZERO, "唤醒与紧急重建都不等待");
        assert_eq!(reconnects(&log), vec![(1, 1000), (2, 0)]);
        assert_eq!(
            phases(&log).last(),
            Some(&(Phase::Disconnected, Some(EndReason::Replaced)))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stable_connection_resets_rebuild_count_but_never_loops_forever() {
        let (sink, log) = recorder();
        let (_tx, rx) = mpsc::unbounded_channel();
        let tries = Arc::new(Mutex::new(0u32));
        let t = tries.clone();
        // 前 5 次断开前已稳定，不计入重建次数，之后按上限结束。
        supervise(rx, &sink, &Prefs::default(), move |ctl| {
            let n = {
                let mut g = t.lock().unwrap();
                *g += 1;
                *g
            };
            async move {
                (
                    Outcome::Lost(Lost {
                        message: "断开".into(),
                        urgent: false,
                        stable: n <= 5,
                    }),
                    ctl,
                )
            }
        })
        .await;
        assert_eq!(*tries.lock().unwrap(), 5 + MAX_REBUILDS);
        assert_eq!(
            phases(&log).last(),
            Some(&(Phase::Disconnected, Some(EndReason::Lost)))
        );
    }
}
