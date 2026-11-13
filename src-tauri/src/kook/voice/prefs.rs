use super::{
    AppResult, Arc, Deafen, HashMap, MicStatus, Mutex, Source, VoiceEvent, oneshot, output,
};

#[cfg(any(test, feature = "test-media"))]
pub type Tap = Arc<Mutex<Vec<f32>>>;

#[derive(Clone)]
pub struct MicPrefs {
    pub muted: bool,
    pub source: Source,
    pub gain: crate::audio::dsp::Gain,
    pub denoise: crate::audio::denoise::Denoise,
    pub deafen: Deafen,
    pub output: output::Sink,
    pub volumes: HashMap<String, u16>,
    pub denoised: std::collections::HashSet<String>,
    pub master: u16,
    pub password: Option<String>,
    #[cfg(any(test, feature = "test-media"))]
    pub tap: Option<Tap>,
    #[cfg(any(test, feature = "test-media"))]
    pub fail_join: bool,
}

impl Default for MicPrefs {
    fn default() -> Self {
        Self {
            muted: false,
            source: Source::Device(None),
            gain: crate::audio::dsp::Gain::default(),
            denoise: crate::audio::denoise::Denoise::default(),
            deafen: Deafen::default(),
            output: output::Sink::Device(None),
            volumes: HashMap::new(),
            denoised: std::collections::HashSet::new(),
            master: 100,
            password: None,
            #[cfg(any(test, feature = "test-media"))]
            tap: None,
            #[cfg(any(test, feature = "test-media"))]
            fail_join: false,
        }
    }
}

pub(super) type Prefs = Arc<Mutex<MicPrefs>>;

pub(super) fn prefs_get(p: &Prefs) -> MicPrefs {
    p.lock().map(|g| g.clone()).unwrap_or_default()
}

pub type Sink = Arc<dyn Fn(VoiceEvent) + Send + Sync>;

pub(super) enum Ctl {
    Leave,
    Mute(bool, oneshot::Sender<AppResult<()>>),
    Source(Source),
    Deafen(bool, oneshot::Sender<AppResult<()>>),
    Output(output::Sink),
    Volume(String, u16),
    PeerDenoise(String, bool),
    Master(u16),
    Wake,
    #[cfg(any(test, feature = "test-media"))]
    Fault(Fault),
}

#[cfg(any(test, feature = "test-media"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    CloseSignal,
    Sleep,
    FailJoin,
}

pub(super) fn output_event(
    deafened: bool,
    active: bool,
    device: Option<String>,
    message: Option<String>,
) -> VoiceEvent {
    VoiceEvent::Output {
        deafened,
        active,
        device,
        message,
    }
}

pub(super) fn deafen_prefs(prefs: &Prefs, on: bool) -> Option<bool> {
    let mut p = prefs.lock().ok()?;
    let muted = p.muted;
    let change = p.deafen.set(on, muted);
    if let Some(m) = change {
        p.muted = m;
    }
    change
}

pub(super) fn mute_prefs(prefs: &Prefs, muted: bool) -> bool {
    prefs.lock().is_ok_and(|mut p| {
        p.muted = muted;
        p.deafen.on_mic(muted)
    })
}

pub(super) fn offline_ctl(c: Ctl, sink: &Sink, prefs: &Prefs) {
    match c {
        Ctl::Mute(muted, tx) => {
            if mute_prefs(prefs, muted) {
                sink(output_event(false, false, None, None));
            }
            sink(VoiceEvent::Mic {
                status: if muted {
                    MicStatus::Muted
                } else {
                    MicStatus::Off
                },
                muted,
                device: None,
                message: None,
            });
            let _ = tx.send(Ok(()));
        }
        Ctl::Source(src) => {
            if let Ok(mut p) = prefs.lock() {
                p.source = src;
            }
        }
        Ctl::Deafen(on, tx) => {
            let change = deafen_prefs(prefs, on);
            sink(output_event(on, false, None, None));
            if let Some(muted) = change {
                sink(VoiceEvent::Mic {
                    status: if muted {
                        MicStatus::Muted
                    } else {
                        MicStatus::Off
                    },
                    muted,
                    device: None,
                    message: None,
                });
            }
            let _ = tx.send(Ok(()));
        }
        Ctl::Output(o) => {
            if let Ok(mut p) = prefs.lock() {
                p.output = o;
            }
        }
        Ctl::Volume(peer, v) => {
            if let Ok(mut p) = prefs.lock() {
                p.volumes
                    .insert(peer, v.min(crate::audio::mixer::MAX_VOLUME));
            }
        }
        Ctl::PeerDenoise(peer, on) => {
            if let Ok(mut p) = prefs.lock() {
                if on {
                    p.denoised.insert(peer);
                } else {
                    p.denoised.remove(&peer);
                }
            }
        }
        Ctl::Master(v) => {
            if let Ok(mut p) = prefs.lock() {
                p.master = v.min(crate::audio::mixer::MAX_VOLUME);
            }
        }
        Ctl::Leave | Ctl::Wake => {}
        #[cfg(any(test, feature = "test-media"))]
        Ctl::Fault(_) => {}
    }
}
