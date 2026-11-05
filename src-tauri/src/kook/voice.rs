//! 语音会话。
//! 一个会话对应一次加入语音频道，在后台任务中完成信令、媒体连接、采集与播放，事件经 `Sink` 送往前端。
//!
//! 信令或媒体意外断开时按退避重建，次数有上限。连接稳定一段时间后重新计数。
//! 主动退出、被同账号新会话替换、服务端正常退出与加入被拒都不重建。
//! 每次结束都会释放信令、媒体连接、采集与播放设备。
//! 会话命令通过控制通道发送，会话结束后不再生效。

pub mod health;
pub mod peer;
pub mod sdp;
pub mod signal;

mod connect;
mod event;
mod mic;
mod outcome;
mod play;
mod prefs;
mod room;
mod run;
mod session;
mod supervise;
#[cfg(test)]
mod testing;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use self::health::{DownStats, Health, Quality, UpStats};
use self::peer::{Leg, Media, MediaEvent, PcState, RemotePacket};
use self::sdp::{Consumer, JoinParams};
use self::signal::{DisconnectKind, Inbound, Signal};
use super::api::Api;
use crate::audio::capture::{Capture, CaptureEvent, Source};
use crate::audio::mic::{MicAction, MicCtl, MicStatus};
use crate::audio::mixer::{Mixer, Throttle};
use crate::audio::output::{self, Deafen, OutputEvent, Playback};
use crate::error::{AppError, AppResult};

/// 连续重建的最大次数。
pub const MAX_REBUILDS: u32 = 3;

/// 退出时等待会话释放的上限，须大于单项释放超时。
const LEAVE_TIMEOUT: Duration = Duration::from_secs(3);
/// 连接健康检查间隔。
const HEALTH_TICK: Duration = Duration::from_secs(2);
/// 推流统计查询间隔。
const STATS_EVERY: Duration = Duration::from_secs(10);
/// 采集或播放设备失败后的重试间隔。
/// 设备拔出后重新插入时据此恢复。
const DEVICE_RETRY: Duration = Duration::from_secs(10);
/// 等待媒体连接建立的时限。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// 关闭媒体连接的时限，超时后不再等待。
const MEDIA_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// 说话状态的检查间隔与上报的最小间隔。
const SPEAK_TICK: Duration = Duration::from_millis(100);
const SPEAK_EVERY: Duration = Duration::from_millis(200);
/// 远端 RTP 包队列长度，满时丢弃新包。
const REMOTE_QUEUE: usize = 256;

pub use self::event::{EndReason, Peer, PeerPatch, Phase, VoiceEvent};
#[cfg(any(test, feature = "test-media"))]
pub use self::prefs::{Fault, Tap};
pub use self::prefs::{MicPrefs, Sink};
pub use self::session::{VoiceControl, VoiceSession};

use self::connect::{Live, Slots, attempt};
use self::event::id_of;
use self::mic::{MicRun, ProducerDone, own_say, producer_queue};
use self::outcome::{Lost, Outcome, closed_outcome, failed_join, lost, settle_lost};
use self::play::{Play, new_mixer};
use self::prefs::{Ctl, Prefs, deafen_prefs, mute_prefs, offline_ctl, output_event, prefs_get};
use self::room::{LinkMeter, Room};
use self::run::run;
use self::supervise::supervise;
