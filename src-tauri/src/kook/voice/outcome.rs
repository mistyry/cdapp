//! 单次连接结果与是否重建的判定。

use super::{AppError, DisconnectKind, EndReason};

/// 单次连接的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Outcome {
    /// 结束会话，不再重建。
    End(EndReason, Option<String>),
    /// 连接中断，可以重建。
    Lost(Lost),
}

/// 连接中断的详情。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Lost {
    pub(super) message: String,
    /// 立即重建，不等待退避。
    pub(super) urgent: bool,
    /// 中断前已稳定连接，重建次数重新计数。
    pub(super) stable: bool,
}

pub(super) fn lost(message: impl Into<String>) -> Outcome {
    Outcome::Lost(Lost {
        message: message.into(),
        urgent: false,
        stable: false,
    })
}

/// 加入失败的处理。凭据失效与服务端拒绝结束会话，网络错误可以重建。
pub(super) fn join_error(e: &AppError) -> Outcome {
    match e {
        AppError::Unauthorized(m) => Outcome::End(EndReason::Unauthorized, Some(m.clone())),
        AppError::Api { code: 40310, .. } => Outcome::End(EndReason::Password, None),
        AppError::Api { .. } | AppError::Format(_) | AppError::NotLoggedIn => {
            Outcome::End(EndReason::JoinFailed, Some(e.to_string()))
        }
        _ => lost(e.to_string()),
    }
}

/// 加入失败的处理与是否需要发送 exit。
/// 已收到 disconnect 时按错误码处理，连接已关闭时不再发送 exit。
pub(super) fn failed_join(e: &AppError, disconnect: Option<i64>, closed: bool) -> (Outcome, bool) {
    let outcome = match disconnect {
        Some(_) => closed_outcome(disconnect),
        None => join_error(e),
    };
    (outcome, disconnect.is_none() && !closed)
}

/// 信令关闭后的处理，依据最后收到的 disconnect 错误码。
pub(super) fn closed_outcome(disconnect: Option<i64>) -> Outcome {
    match disconnect.map(DisconnectKind::from_code) {
        Some(DisconnectKind::Replaced) => Outcome::End(EndReason::Replaced, None),
        Some(DisconnectKind::Exited) => Outcome::End(EndReason::Exited, None),
        Some(DisconnectKind::JoinFailed) => Outcome::End(EndReason::JoinFailed, None),
        Some(DisconnectKind::Other(c)) => lost(format!("语音连接已断开，错误码 {c}。")),
        None => lost("语音信令连接已断开。"),
    }
}

/// 运行结果为断线且已收到不可重连的 disconnect 时，改用错误码对应的结束原因。
pub(super) fn settle_lost(
    outcome: Outcome,
    server_ended: bool,
    disconnect: Option<i64>,
) -> (Outcome, bool) {
    match (&outcome, disconnect.map(DisconnectKind::from_code)) {
        (Outcome::Lost(_), Some(k)) if !matches!(k, DisconnectKind::Other(_)) => {
            (closed_outcome(disconnect), true)
        }
        _ => (outcome, server_ended),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_password_ends_without_retry() {
        let e = AppError::Api {
            code: 40310,
            message: String::new(),
        };
        assert!(matches!(
            failed_join(&e, None, false).0,
            Outcome::End(EndReason::Password, None)
        ));
    }
    use crate::kook::voice::*;

    #[test]
    fn lost_with_recorded_disconnect_does_not_rebuild() {
        for (code, reason) in [
            (41101, EndReason::Replaced),
            (41110, EndReason::Exited),
            (41100, EndReason::JoinFailed),
        ] {
            let (o, ended) = settle_lost(lost("推流连接失败"), false, Some(code));
            assert!(matches!(o, Outcome::End(r, None) if r == reason));
            assert!(ended, "服务端已断开，不再发送 exit");
        }
        // 其他错误码与未收到 disconnect 时仍可重建。
        assert!(matches!(
            settle_lost(lost("x"), false, Some(1)).0,
            Outcome::Lost(_)
        ));
        assert!(matches!(
            settle_lost(lost("x"), false, None),
            (Outcome::Lost(_), false)
        ));
        // 主动退出不受影响。
        let (o, ended) = settle_lost(Outcome::End(EndReason::Left, None), false, Some(41101));
        assert!(matches!(o, Outcome::End(EndReason::Left, None)) && !ended);
    }

    #[test]
    fn disconnect_outcomes_never_retry_replaced_or_exited() {
        assert_eq!(
            closed_outcome(Some(41101)),
            Outcome::End(EndReason::Replaced, None)
        );
        assert_eq!(
            closed_outcome(Some(41110)),
            Outcome::End(EndReason::Exited, None)
        );
        assert_eq!(
            closed_outcome(Some(41100)),
            Outcome::End(EndReason::JoinFailed, None)
        );
        assert!(matches!(closed_outcome(Some(5)), Outcome::Lost(_)));
        assert!(matches!(closed_outcome(None), Outcome::Lost(_)));
        assert_eq!(
            join_error(&AppError::Unauthorized("x".into())),
            Outcome::End(EndReason::Unauthorized, Some("x".into()))
        );
        assert!(matches!(
            join_error(&AppError::Api {
                code: 403,
                message: "拒绝加入".into()
            }),
            Outcome::End(EndReason::JoinFailed, _)
        ));
        assert!(matches!(
            join_error(&AppError::Other("网络错误".into())),
            Outcome::Lost(_)
        ));
    }

    #[test]
    fn join_failure_after_disconnect_does_not_rebuild_or_exit() {
        let closed = AppError::Other("连接已关闭".into());
        assert_eq!(
            failed_join(&closed, Some(41101), true),
            (Outcome::End(EndReason::Replaced, None), false)
        );
        assert_eq!(
            failed_join(&closed, Some(41100), true),
            (Outcome::End(EndReason::JoinFailed, None), false)
        );
        // 连接已关闭但没有错误码，可重建，不发送 exit。
        assert!(matches!(
            failed_join(&closed, None, true),
            (Outcome::Lost(_), false)
        ));
        // 服务端拒绝时结束会话，并发送 exit。
        let rejected = AppError::Api {
            code: 0,
            message: "拒绝加入".into(),
        };
        assert!(matches!(
            failed_join(&rejected, None, false),
            (Outcome::End(EndReason::JoinFailed, _), true)
        ));
        assert!(matches!(
            failed_join(&AppError::Other("网络错误".into()), None, false),
            (Outcome::Lost(_), true)
        ));
    }
}
