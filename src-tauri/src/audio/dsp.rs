use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

pub const RATE: u32 = 48_000;
pub const FRAME_MS: u32 = 60;
pub const FRAME_SAMPLES: usize = (RATE / 1000 * FRAME_MS) as usize;

#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn mixdown(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|f| f.iter().sum::<f32>() / f.len() as f32)
        .collect()
}

const HALF: usize = 16;
const STEPS: usize = 512;

pub struct Resampler {
    step: f64,
    pos: f64,
    buf: Vec<f32>,
    table: Vec<f32>,
    passthrough: bool,
}

impl Resampler {
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    pub fn new(from: u32, to: u32) -> Self {
        let step = f64::from(from) / f64::from(to);
        let cutoff = (1.0 / step).min(1.0) * 0.95;
        let table = (0..=HALF * STEPS)
            .map(|i| {
                let d = i as f64 / STEPS as f64;
                let x = std::f64::consts::PI * d * cutoff;
                let sinc = if d == 0.0 { 1.0 } else { x.sin() / x };
                let w = 0.5 + d / (2.0 * HALF as f64);
                let win = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * w).cos()
                    + 0.08 * (4.0 * std::f64::consts::PI * w).cos();
                (cutoff * sinc * win) as f32
            })
            .collect();
        Self {
            step,
            pos: HALF as f64,
            buf: vec![0.0; HALF],
            table,
            passthrough: from == to,
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn kernel(&self, d: f64) -> f32 {
        let x = d.abs() * STEPS as f64;
        let i = x as usize;
        if i >= HALF * STEPS {
            return 0.0;
        }
        let t = (x - i as f64) as f32;
        self.table[i] + (self.table[i + 1] - self.table[i]) * t
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.passthrough {
            return input.to_vec();
        }
        self.buf.extend_from_slice(input);
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 2);
        while self.pos + (HALF as f64) < self.buf.len() as f64 {
            let center = self.pos.floor() as usize;
            let frac = self.pos - center as f64;
            let mut acc = 0.0f32;
            for k in (center + 1 - HALF)..=(center + HALF) {
                acc += self.buf[k] * self.kernel(k as f64 - center as f64 - frac);
            }
            out.push(acc);
            self.pos += self.step;
        }
        let keep_from = (self.pos.floor() as usize).saturating_sub(HALF);
        if keep_from > 0 {
            self.buf.drain(..keep_from);
            self.pos -= keep_from as f64;
        }
        out
    }
}

#[derive(Default)]
pub struct Framer {
    pending: Vec<i16>,
}

impl Framer {
    #[allow(clippy::cast_possible_truncation)]
    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<i16>> {
        self.pending.extend(
            samples
                .iter()
                .map(|x| (x.clamp(-1.0, 1.0) * 32767.0).round() as i16),
        );
        let mut frames = Vec::new();
        while self.pending.len() >= FRAME_SAMPLES {
            frames.push(self.pending.drain(..FRAME_SAMPLES).collect());
        }
        frames
    }
}

#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn audio_level(frame: &[i16]) -> u8 {
    if frame.is_empty() {
        return 127;
    }
    let sum: f64 = frame
        .iter()
        .map(|&s| {
            let v = f64::from(s) / 32768.0;
            v * v
        })
        .sum();
    let rms = (sum / frame.len() as f64).sqrt();
    if rms <= 0.0 {
        return 127;
    }
    (-20.0 * rms.log10()).round().clamp(0.0, 127.0) as u8
}

#[must_use]
pub fn meter(level: u8) -> u8 {
    let l = level.min(60);
    u8::try_from((u32::from(60 - l) * 100) / 60).unwrap_or(100)
}

pub struct ZeroProbe {
    window: usize,
    seen: usize,
    done: Option<bool>,
}

impl ZeroProbe {
    #[must_use]
    pub fn new(window: usize) -> Self {
        Self {
            window,
            seen: 0,
            done: None,
        }
    }

    pub fn feed(&mut self, samples: &[f32]) -> Option<bool> {
        if self.done.is_none() {
            if samples.iter().any(|&x| x != 0.0) {
                self.done = Some(false);
            } else {
                self.seen += samples.len();
                if self.seen >= self.window {
                    self.done = Some(true);
                }
            }
        }
        self.done
    }
}

#[derive(Clone, Debug)]
pub struct Gain(Arc<AtomicU16>);

pub const MAX_GAIN: u16 = 200;

impl Default for Gain {
    fn default() -> Self {
        Self(Arc::new(AtomicU16::new(100)))
    }
}

impl Gain {
    #[must_use]
    pub fn percent(&self) -> u16 {
        self.0.load(Ordering::Relaxed)
    }

    pub fn set(&self, percent: u16) {
        self.0.store(percent.min(MAX_GAIN), Ordering::Relaxed);
    }

    pub fn apply(&self, samples: &mut [f32]) {
        let p = self.percent();
        if p == 100 {
            return;
        }
        let g = f32::from(p) / 100.0;
        for x in samples.iter_mut() {
            *x *= g;
        }
        if p > 100 {
            for x in samples.iter_mut() {
                *x = soft_limit(*x);
            }
        }
    }
}

#[must_use]
pub fn soft_limit(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        let y = KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh();
        y.copysign(x)
    }
}

pub struct Bounded<T> {
    items: VecDeque<T>,
    cap: usize,
    dropped: u64,
}

impl<T> Bounded<T> {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(cap),
            cap: cap.max(1),
            dropped: 0,
        }
    }

    pub fn push(&mut self, item: T) -> bool {
        let full = self.items.len() >= self.cap;
        if full {
            self.items.pop_front();
            self.dropped += 1;
        }
        self.items.push_back(item);
        full
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::cast_precision_loss)]
    fn tone(freq: f64, rate: u32, n: usize, amp: f64) -> Vec<f32> {
        (0..n)
            .map(|i| {
                #[allow(clippy::cast_possible_truncation)]
                let v = (amp
                    * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(rate)).sin())
                    as f32;
                v
            })
            .collect()
    }

    #[allow(clippy::cast_precision_loss)]
    fn magnitude(x: &[f32], freq: f64, rate: u32) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / f64::from(rate);
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in x.iter().enumerate() {
            re += f64::from(v) * (w * i as f64).cos();
            im += f64::from(v) * (w * i as f64).sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f64
    }

    #[test]
    fn mixdown_averages_channels() {
        assert_eq!(mixdown(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(mixdown(&[0.1, 0.2], 1), vec![0.1, 0.2]);
    }

    #[test]
    fn resampler_keeps_rate_and_tone() {
        for from in [44_100, 16_000, 96_000] {
            let mut r = Resampler::new(from, RATE);
            let input = tone(440.0, from, from as usize, 0.5);
            let mut out = Vec::new();
            for chunk in input.chunks(473) {
                out.extend(r.process(chunk));
            }
            let expect = RATE as usize;
            assert!(
                out.len().abs_diff(expect) <= HALF * 4,
                "{from}: {}",
                out.len()
            );
            let seg = &out[4800..out.len() - 4800];
            let at = magnitude(seg, 440.0, RATE);
            assert!((at - 0.5).abs() < 0.03, "{from}: 440 Hz 幅度为 {at}");
            assert!(magnitude(seg, 1000.0, RATE) < 0.01);
        }
    }

    #[test]
    fn resampler_suppresses_aliasing_when_downsampling() {
        let mut r = Resampler::new(96_000, RATE);
        let out = r.process(&tone(30_000.0, 96_000, 96_000, 0.5));
        let seg = &out[2000..out.len() - 2000];
        assert!(magnitude(seg, 18_000.0, RATE) < 0.01);
    }

    #[test]
    fn resampler_passthrough_at_48k() {
        let mut r = Resampler::new(RATE, RATE);
        assert_eq!(r.process(&[0.1, 0.2]), vec![0.1, 0.2]);
    }

    #[test]
    fn framer_splits_60ms_frames_across_pushes() {
        let mut f = Framer::default();
        assert_eq!(f.push(&vec![0.0; FRAME_SAMPLES - 1]).len(), 0);
        let frames = f.push(&[1.0; 3]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), 2880);
        assert_eq!(frames[0][FRAME_SAMPLES - 1], 32767);
        assert_eq!(f.push(&vec![2.0; FRAME_SAMPLES - 3]).len(), 0);
        assert_eq!(f.push(&[2.0]).len(), 1, "跨块累积满一帧时输出");
    }

    #[test]
    fn level_follows_dbov() {
        assert_eq!(audio_level(&[0; 100]), 127);
        assert_eq!(audio_level(&[]), 127);
        let square: Vec<i16> = (0..960)
            .map(|i| if i % 2 == 0 { 32767 } else { -32767 })
            .collect();
        assert_eq!(audio_level(&square), 0);
        #[allow(clippy::cast_possible_truncation)]
        let sine: Vec<i16> = tone(440.0, RATE, 2880, 0.1)
            .iter()
            .map(|x| (x * 32768.0) as i16)
            .collect();
        assert_eq!(audio_level(&sine), 23);
        assert_eq!(meter(0), 100);
        assert_eq!(meter(127), 0);
        assert_eq!(meter(30), 50);
    }

    #[test]
    fn zero_probe_detects_denied_input() {
        let mut p = ZeroProbe::new(1000);
        assert_eq!(p.feed(&[0.0; 600]), None);
        assert_eq!(p.feed(&[0.0; 600]), Some(true));
        assert_eq!(p.feed(&[0.5]), Some(true));
        let mut p = ZeroProbe::new(1000);
        assert_eq!(p.feed(&[0.0, 1e-6]), Some(false));
        assert_eq!(p.feed(&vec![0.0; 5000]), Some(false));
    }

    #[test]
    fn bounded_queue_drops_oldest() {
        let mut q = Bounded::new(3);
        for i in 0..5 {
            q.push(i);
        }
        assert_eq!(q.len(), 3);
        assert_eq!(q.dropped(), 2);
        assert_eq!(q.pop(), Some(2));
        assert!(!q.push(9));
        assert_eq!(q.pop(), Some(3));
        assert_eq!(q.pop(), Some(4));
        assert_eq!(q.pop(), Some(9));
        assert_eq!(q.len(), 0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn gain_scales_samples_and_limits_above_unity() {
        let base = [0.0f32, 0.1, -0.2, 0.4, -0.6];
        let run = |p: u16| {
            let g = Gain::default();
            g.set(p);
            let mut v = base;
            g.apply(&mut v);
            v
        };
        assert_eq!(run(100), base);
        assert!(run(0).iter().all(|&x| x == 0.0));
        for (a, b) in run(50).iter().zip(base) {
            assert!((a - b * 0.5).abs() < 1e-6);
        }
        let r = run(200);
        assert!((r[1] - 0.2).abs() < 1e-6);
        assert!((r[2] + 0.4).abs() < 1e-6);
        assert!(r[3] > 0.79 && r[3] <= 0.8 + 1e-6);
        assert!(r[4] < -0.8 && r[4] > -1.0);
        let g = Gain::default();
        g.set(1000);
        assert_eq!(g.percent(), MAX_GAIN);
        let mut loud = [1.0f32, -1.0];
        g.apply(&mut loud);
        assert!(loud.iter().all(|x| x.abs() < 1.0));
    }

    #[test]
    fn gain_changes_frame_level() {
        #[allow(clippy::cast_precision_loss)]
        let tone: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|i| 0.25 * (i as f32 * 2.0 * std::f32::consts::PI / 48.0).sin())
            .collect();
        let level = |p: u16| {
            let g = Gain::default();
            g.set(p);
            let mut v = tone.clone();
            g.apply(&mut v);
            audio_level(&Framer::default().push(&v)[0])
        };
        assert_eq!(level(100), 15);
        assert_eq!(level(50), 21);
        assert_eq!(level(200), 9);
        assert_eq!(level(0), 127);
    }
}
