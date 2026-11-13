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

pub const MAX_REBUILDS: u32 = 3;

const LEAVE_TIMEOUT: Duration = Duration::from_secs(3);
const HEALTH_TICK: Duration = Duration::from_secs(2);
const STATS_EVERY: Duration = Duration::from_secs(10);
const DEVICE_RETRY: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MEDIA_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const SPEAK_TICK: Duration = Duration::from_millis(100);
const SPEAK_EVERY: Duration = Duration::from_millis(200);
const REMOTE_QUEUE: usize = 256;

pub use self::event::{EndReason, Peer, PeerPatch, Phase, VoiceEvent};
#[cfg(any(test, feature = "test-media"))]
pub use self::prefs::{Fault, Tap};
pub use self::prefs::{MicPrefs, Sink};
pub use self::session::{VoiceControl, VoiceEnded, VoiceSession};

use self::connect::{Live, Slots, attempt};
use self::event::id_of;
use self::mic::{MicRun, ProducerDone, own_say, producer_queue};
use self::outcome::{Lost, Outcome, closed_outcome, failed_join, lost, settle_lost};
use self::play::{Play, new_mixer};
use self::prefs::{Ctl, Prefs, deafen_prefs, mute_prefs, offline_ctl, output_event, prefs_get};
use self::room::{LinkMeter, Room};
use self::run::run;
use self::supervise::supervise;
