//! 降噪。可选 KOOK AI 降噪 2.0 与 `RNNoise`。
//! KOOK AI 降噪 2.0 在本机运行 KOOK 网页端使用的声网降噪模块。
//! 模块是 Emscripten 编译的 wasm，由 wasmtime 加载，宿主导入在此实现。
//! 没有可用的 Rust 实现或 C 接口可替代，只能按模块的导入表补齐宿主函数。
//! 采集回调按 128 样本块处理 48 kHz 单声道音频。实例创建较慢，在采集线程中完成后交给回调。
//! KOOK AI 降噪实例出错或无法创建时，关闭降噪并通知界面，不中断采集。输出持续为零只记录日志。
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use wasmtime::error::Context;
use wasmtime::{
    Caller, Engine, Extern, Func, Instance, Linker, Memory, Module, Result, Store, Table, Val,
    bail, format_err,
};

/// 每次处理的样本数，与模块创建参数一致。
pub const BLOCK: usize = 128;
/// 模块要求的版本。
const VERSION: &str = "2.0.0-beta4-license";
/// `RNNoise` 的帧长。
const RNN_FRAME: usize = nnnoiseless::FRAME_SIZE;
/// 输入有声而输出持续为零的样本数，约 2 秒，超过时记录日志。
/// 已知的许可失效在创建时返回错误码或运行中陷入，通话中途到期的表现尚未实测。
const MUTED_LOG: usize = 48_000 * 2;

/// 已编译的模块。编译耗时较长，全进程只做一次。
static MODULE: OnceLock<(Engine, Module)> = OnceLock::new();
/// 当前许可证书，只保存在内存中。
static LICENSE: Mutex<Option<String>> = Mutex::new(None);

/// 设置许可证书。
pub fn set_license(cert: Option<String>) {
    if let Ok(mut l) = LICENSE.lock() {
        *l = cert;
    }
}

/// 是否已有许可证书。
#[must_use]
pub fn has_license() -> bool {
    LICENSE.lock().is_ok_and(|l| l.is_some())
}

/// 编译模块并缓存。已编译时直接返回。
///
/// # Errors
///
/// 模块无法编译时返回错误。
pub fn prepare(wasm: &[u8]) -> Result<()> {
    if MODULE.get().is_some() {
        return Ok(());
    }
    let engine = Engine::default();
    let module = Module::new(&engine, wasm)?;
    let _ = MODULE.set((engine, module));
    Ok(())
}

/// 模块是否已编译。
#[must_use]
pub fn ready() -> bool {
    MODULE.get().is_some()
}

#[derive(Default)]
struct Host {
    memory: Option<Memory>,
    table: Option<Table>,
}

fn mem<'a>(c: &'a mut Caller<'_, Host>) -> Result<&'a mut [u8]> {
    let m = c.data().memory.context("memory")?;
    Ok(m.data_mut(c))
}

fn cstr(data: &[u8], ptr: u32) -> String {
    let s = data.get(ptr as usize..).unwrap_or_default();
    let n = s.iter().position(|&b| b == 0).unwrap_or(0);
    String::from_utf8_lossy(&s[..n]).into_owned()
}

/// 墙上时间毫秒数，许可按它计时。
fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// Emscripten 的 invoke 系列：从间接函数表取函数调用。
fn invoke(c: &mut Caller<'_, Host>, idx: i32, args: &[Val], ret: bool) -> Result<i32> {
    let table = c.data().table.context("table")?;
    let f = table
        .get(&mut *c, u64::try_from(idx)?)
        .and_then(|r| r.as_func().flatten().copied())
        .ok_or_else(|| format_err!("空函数表项 {idx}"))?;
    let mut out = [Val::I32(0)];
    let res = if ret { &mut out[..] } else { &mut out[..0] };
    f.call(&mut *c, args, res)?;
    Ok(if ret { out[0].unwrap_i32() } else { 0 })
}

fn write_i32s(d: &mut [u8], at: i32, values: &[i32]) -> Result<()> {
    for (k, v) in values.iter().enumerate() {
        let p = at as usize + k * 4;
        d.get_mut(p..p + 4)
            .context("越界写入")?
            .copy_from_slice(&v.to_le_bytes());
    }
    Ok(())
}

/// 由 1970 年起的天数得到年月日。
fn civil(days: i64) -> (i32, i32, i32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as i32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as i32;
    let y = (yoe + era * 400) as i32 + i32::from(m <= 2);
    (y, m, d)
}

#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn link(linker: &mut Linker<Host>) -> Result<()> {
    let m = "a";
    linker.func_wrap(
        m,
        "a",
        |_: Caller<'_, Host>, _: i32, _: i32, _: i32| -> Result<()> { bail!("cxa_throw") },
    )?;
    linker.func_wrap(m, "b", |_: Caller<'_, Host>| -> Result<()> {
        bail!("abort")
    })?;
    linker.func_wrap(m, "c", |_: Caller<'_, Host>, _: i32| -> i32 { 0 })?;
    linker.func_wrap(m, "d", |_: Caller<'_, Host>| -> i32 { 0 })?;
    linker.func_wrap(m, "e", |_: Caller<'_, Host>, _: i32| -> Result<()> {
        bail!("resume_exception")
    })?;
    linker.func_wrap(
        m,
        "f",
        |mut c: Caller<'_, Host>, f: i32, a: i32, b: i32| -> Result<i32> {
            invoke(&mut c, f, &[Val::I32(a), Val::I32(b)], true)
        },
    )?;
    // fd_write：丢弃输出，只回报写入长度。
    linker.func_wrap(
        m,
        "g",
        |mut c: Caller<'_, Host>, _fd: i32, iov: i32, cnt: i32, out: i32| -> Result<i32> {
            let d = mem(&mut c)?;
            let mut total = 0u32;
            for k in 0..cnt.max(0) as usize {
                let p = iov as usize + k * 8 + 4;
                let len = d.get(p..p + 4).context("越界读取")?;
                total += u32::from_le_bytes(len.try_into()?);
            }
            write_i32s(d, out, &[total as i32])?;
            Ok(0)
        },
    )?;
    linker.func_wrap(m, "h", |_: Caller<'_, Host>| -> f64 { now_ms() })?;
    linker.func_wrap(
        m,
        "i",
        |mut c: Caller<'_, Host>, f: i32, a: i32, b: i32| -> Result<()> {
            invoke(&mut c, f, &[Val::I32(a), Val::I32(b)], false).map(drop)
        },
    )?;
    linker.func_wrap(
        m,
        "j",
        |mut c: Caller<'_, Host>, f: i32, a: i32, b: i32, d: i32| -> Result<()> {
            invoke(&mut c, f, &[Val::I32(a), Val::I32(b), Val::I32(d)], false).map(drop)
        },
    )?;
    linker.func_wrap(
        m,
        "k",
        |mut c: Caller<'_, Host>, f: i32, a: i32, b: i32, d: i32| -> Result<i32> {
            invoke(&mut c, f, &[Val::I32(a), Val::I32(b), Val::I32(d)], true)
        },
    )?;
    linker.func_wrap(m, "l", |_: Caller<'_, Host>| -> i32 { 1 })?;
    linker.func_wrap(
        m,
        "m",
        |mut c: Caller<'_, Host>, dst: i32, src: i32, n: i32| -> Result<()> {
            let d = mem(&mut c)?;
            let (s, n) = (src as usize, n as usize);
            if s + n > d.len() || dst as usize + n > d.len() {
                bail!("越界复制");
            }
            d.copy_within(s..s + n, dst as usize);
            Ok(())
        },
    )?;
    linker.func_wrap(
        m,
        "n",
        |mut c: Caller<'_, Host>, level: i32, p: i32| -> Result<()> {
            if p != 0 && level <= 1 {
                log::debug!("降噪模块：{}", cstr(mem(&mut c)?, p as u32));
            }
            Ok(())
        },
    )?;
    linker.func_wrap(m, "o", |_: Caller<'_, Host>, _: i32, _: i32| {})?;
    linker.func_wrap(m, "p", |mut c: Caller<'_, Host>, a: i32| -> Result<i32> {
        let f = c
            .get_export("na")
            .and_then(Extern::into_func)
            .context("na")?;
        f.typed::<i32, i32>(&c)?.call(&mut c, a)
    })?;
    // localtime：按 UTC 填写 tm 结构。
    linker.func_wrap(
        m,
        "q",
        |mut c: Caller<'_, Host>, lo: i32, hi: i32, tm: i32| -> Result<()> {
            let t = (i64::from(hi) << 32) | i64::from(lo as u32);
            let days = t.div_euclid(86400);
            let secs = t.rem_euclid(86400) as i32;
            let (y, mo, d) = civil(days);
            let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
            let cum = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
            let yday = cum[(mo - 1) as usize] + d - 1 + i32::from(leap && mo > 2);
            let wday = (days + 4).rem_euclid(7) as i32;
            let f = [
                secs % 60,
                secs / 60 % 60,
                secs / 3600,
                d,
                mo - 1,
                y - 1900,
                wday,
                yday,
                0,
                0,
            ];
            write_i32s(mem(&mut c)?, tm, &f)
        },
    )?;
    linker.func_wrap(
        m,
        "r",
        |_: Caller<'_, Host>, _: i32, _: i32, _: i32, _: i32| {},
    )?;
    linker.func_wrap(m, "s", |mut c: Caller<'_, Host>, f: i32| -> Result<()> {
        invoke(&mut c, f, &[], false).map(drop)
    })?;
    linker.func_wrap(m, "t", |_: Caller<'_, Host>, _: i32| -> Result<i32> {
        bail!("cxa_begin_catch")
    })?;
    linker.func_wrap(
        m,
        "u",
        |mut c: Caller<'_, Host>, f: i32, a: i32| -> Result<i32> {
            invoke(&mut c, f, &[Val::I32(a)], true)
        },
    )?;
    linker.func_wrap(m, "v", |mut c: Caller<'_, Host>, want: i32| -> i32 {
        let Some(memory) = c.data().memory else {
            return 0;
        };
        let cur = memory.data_size(&c) as u64;
        let want = u64::from(want as u32);
        if want <= cur {
            return 1;
        }
        i32::from(memory.grow(&mut c, (want - cur).div_ceil(65536)).is_ok())
    })?;
    linker.func_wrap(
        m,
        "w",
        |mut c: Caller<'_, Host>, tz: i32, dst: i32, names: i32| -> Result<()> {
            let d = mem(&mut c)?;
            for p in [tz, dst, names, names + 4] {
                write_i32s(d, p, &[0])?;
            }
            Ok(())
        },
    )?;
    linker.func_wrap(m, "x", |_: Caller<'_, Host>, code: i32| -> Result<()> {
        bail!("exit({code})")
    })?;
    Ok(())
}

/// 一个降噪实例。
pub struct Denoiser {
    store: Store<Host>,
    memory: Memory,
    inst: Instance,
    handle: i32,
    buf: i32,
    process: wasmtime::TypedFunc<(i32, i32, i32), ()>,
    ptr: usize,
}

impl Denoiser {
    fn func<P: wasmtime::WasmParams, R: wasmtime::WasmResults>(
        &mut self,
        name: &str,
    ) -> Result<wasmtime::TypedFunc<P, R>> {
        let f: Func = self
            .inst
            .get_func(&mut self.store, name)
            .with_context(|| format!("导出 {name}"))?;
        f.typed(&self.store)
    }

    /// 由已编译的模块创建实例，载入许可并设为低延迟。
    ///
    /// # Errors
    ///
    /// 模块未编译、没有许可、版本不符或初始化失败时返回错误。
    pub fn new() -> Result<Self> {
        let (engine, module) = MODULE.get().context("降噪模块未加载")?;
        let cert = LICENSE
            .lock()
            .ok()
            .and_then(|l| l.clone())
            .context("没有降噪许可")?;
        let mut linker = Linker::new(engine);
        link(&mut linker)?;
        let mut store = Store::new(engine, Host::default());
        let inst = linker.instantiate(&mut store, module)?;
        let memory = inst.get_memory(&mut store, "y").context("memory")?;
        let table = inst.get_table(&mut store, "da").context("table")?;
        store.data_mut().memory = Some(memory);
        store.data_mut().table = Some(table);
        let process = inst
            .get_func(&mut store, "_")
            .context("导出 _")?
            .typed(&store)?;
        let mut d = Self {
            store,
            memory,
            inst,
            handle: 0,
            buf: 0,
            process,
            ptr: 0,
        };
        d.func::<(), ()>("z")?.call(&mut d.store, ())?;
        let v = d.func::<(), i32>("A")?.call(&mut d.store, ())?;
        let version = cstr(d.memory.data(&d.store), v as u32);
        if version != VERSION {
            bail!("降噪模块版本不符：{version}");
        }
        d.func::<i32, ()>("C")?.call(&mut d.store, 3)?;
        d.func::<(), ()>("D")?.call(&mut d.store, ())?;
        let params = d.func::<(), i32>("O")?.call(&mut d.store, ())?;
        d.handle = d
            .func::<(i32, i32, i32, i32), i32>("T")?
            .call(&mut d.store, (48_000, 1, BLOCK as i32, params))?;
        d.func::<i32, ()>("S")?.call(&mut d.store, params)?;
        d.buf = d
            .func::<(i32, i32), i32>("H")?
            .call(&mut d.store, (1, BLOCK as i32))?;
        let status = d.set_license(cert.trim().as_bytes())?;
        if status != 0 {
            bail!("降噪许可无效：{status}");
        }
        let h = d.handle;
        // 1 为低延迟模式。
        d.func::<(i32, i32), ()>("Z")?.call(&mut d.store, (h, 1))?;
        let b = d.buf;
        d.ptr = d.func::<(i32, i32), i32>("M")?.call(&mut d.store, (b, 0))? as usize;
        Ok(d)
    }

    fn set_license(&mut self, text: &[u8]) -> Result<i32> {
        let p = self
            .func::<i32, i32>("ea")?
            .call(&mut self.store, text.len() as i32 + 1)?;
        let d = self.memory.data_mut(&mut self.store);
        let at = p as usize;
        let dst = d.get_mut(at..=at + text.len()).context("越界写入")?;
        dst[..text.len()].copy_from_slice(text);
        dst[text.len()] = 0;
        let s = self.func::<i32, i32>("G")?.call(&mut self.store, p)?;
        self.func::<i32, ()>("fa")?.call(&mut self.store, p)?;
        Ok(s)
    }

    /// 原地处理一块样本。
    ///
    /// # Errors
    ///
    /// 模块陷入时返回错误，实例随后不可再用。
    pub fn process(&mut self, block: &mut [f32; BLOCK]) -> Result<()> {
        let ptr = self.ptr;
        let d = self.memory.data_mut(&mut self.store);
        let area = d.get_mut(ptr..ptr + BLOCK * 4).context("越界写入")?;
        for (dst, s) in area.as_chunks_mut::<4>().0.iter_mut().zip(block.iter()) {
            dst.copy_from_slice(&s.to_le_bytes());
        }
        self.process
            .call(&mut self.store, (self.handle, self.buf, self.buf))?;
        let d = self.memory.data(&self.store);
        let area = d.get(ptr..ptr + BLOCK * 4).context("越界读取")?;
        for (src, s) in area.as_chunks::<4>().0.iter().zip(block.iter_mut()) {
            *s = f32::from_le_bytes(*src);
        }
        Ok(())
    }
}

/// 降噪方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// `RNNoise`，本地开源实现，不需要下载与许可。
    Rnnoise,
    /// KOOK AI 降噪 2.0。无法识别的值也按此处理。
    #[default]
    #[serde(other)]
    Kook,
}

impl Mode {
    fn from_u8(v: u8) -> Self {
        if v == 1 { Self::Rnnoise } else { Self::Kook }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Kook => 0,
            Self::Rnnoise => 1,
        }
    }
}

struct Inner {
    on: AtomicBool,
    mode: AtomicU8,
    /// 采集回调正持有可用实例。
    armed: AtomicBool,
    /// 采集线程创建好、等待回调取走的实例。
    slot: Mutex<Option<Denoiser>>,
    /// 降噪因故障关闭时的通知，参数为原因。
    on_fail: Mutex<Option<FailHook>>,
}

/// 降噪因故障关闭时的回调。
pub type FailHook = Box<dyn Fn(String) + Send + Sync>;

/// 降噪开关，与命令层共享，修改立即作用于采集。
#[derive(Clone)]
pub struct Denoise(Arc<Inner>);

impl Default for Denoise {
    fn default() -> Self {
        Self(Arc::new(Inner {
            on: AtomicBool::new(false),
            mode: AtomicU8::new(0),
            armed: AtomicBool::new(false),
            slot: Mutex::new(None),
            on_fail: Mutex::new(None),
        }))
    }
}

impl Denoise {
    #[must_use]
    pub fn on(&self) -> bool {
        self.0.on.load(Ordering::Relaxed)
    }

    pub fn set(&self, on: bool) {
        self.0.on.store(on, Ordering::Relaxed);
    }

    /// 设置降噪因故障关闭时的通知。
    pub fn on_fail(&self, hook: FailHook) {
        if let Ok(mut h) = self.0.on_fail.lock() {
            *h = Some(hook);
        }
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
        Mode::from_u8(self.0.mode.load(Ordering::Relaxed))
    }

    pub fn set_mode(&self, mode: Mode) {
        self.0.mode.store(mode.as_u8(), Ordering::Relaxed);
    }

    /// 关闭降噪并通知界面。已关闭时不重复通知。
    fn fail(&self, e: &wasmtime::Error) {
        if !self.0.on.swap(false, Ordering::SeqCst) {
            return;
        }
        log::warn!("KOOK AI 降噪故障，已关闭降噪：{e:#}");
        if let Ok(h) = self.0.on_fail.lock()
            && let Some(h) = h.as_ref()
        {
            h(format!("{e:#}"));
        }
    }

    /// 在采集线程中定期调用：需要时创建实例，交给采集回调。
    pub fn maintain(&self) {
        if !self.on()
            || self.mode() != Mode::Kook
            || self.0.armed.load(Ordering::SeqCst)
            || !ready()
            || !has_license()
        {
            return;
        }
        if self.0.slot.lock().is_ok_and(|s| s.is_some()) {
            return;
        }
        match Denoiser::new() {
            Ok(d) => {
                log::info!("KOOK AI 降噪实例已就绪");
                if let Ok(mut s) = self.0.slot.lock() {
                    *s = Some(d);
                }
            }
            Err(e) => self.fail(&e),
        }
    }

    /// 采集回调中的处理阶段。
    #[must_use]
    pub fn stage(&self) -> Stage {
        Stage {
            shared: self.clone(),
            inst: None,
            rnn: None,
            active: None,
            pending: Vec::with_capacity(RNN_FRAME * 2),
            muted: 0,
            muted_logged: false,
        }
    }
}

/// 采集回调持有的降噪阶段。样本按块处理，不足一块的部分留到下次处理。
pub struct Stage {
    shared: Denoise,
    inst: Option<Denoiser>,
    rnn: Option<Box<nnnoiseless::DenoiseState<'static>>>,
    /// 正在使用的方式，切换时先送出缓冲中的样本。
    active: Option<Mode>,
    pending: Vec<f32>,
    /// 输入有声而输出为零的连续样本数。
    muted: usize,
    /// 本段静音是否已记录。
    muted_logged: bool,
}

impl Stage {
    fn drop_inst(&mut self) {
        self.inst = None;
        self.muted = 0;
        self.muted_logged = false;
        self.shared.0.armed.store(false, Ordering::SeqCst);
    }

    /// 送出缓冲中尚未处理的样本，放在 `samples` 之前。
    fn flush(&mut self, samples: &mut Vec<f32>) {
        if !self.pending.is_empty() {
            let mut out = std::mem::take(&mut self.pending);
            out.append(samples);
            *samples = out;
        }
    }

    /// 处理 48 kHz 单声道样本，结果写回 `samples`。
    pub fn process(&mut self, samples: &mut Vec<f32>) {
        let want = self.shared.on().then(|| self.shared.mode());
        if want != self.active {
            self.flush(samples);
            if self.inst.is_some() {
                self.drop_inst();
            }
            self.rnn = None;
            self.active = want;
        }
        match want {
            None => {}
            Some(Mode::Kook) => self.kook(samples),
            Some(Mode::Rnnoise) => self.rnnoise(samples),
        }
    }

    fn rnnoise(&mut self, samples: &mut Vec<f32>) {
        let rnn = self.rnn.get_or_insert_with(nnnoiseless::DenoiseState::new);
        self.pending.append(samples);
        let full = self.pending.len() / RNN_FRAME * RNN_FRAME;
        let mut out = [0.0f32; RNN_FRAME];
        let mut input = [0.0f32; RNN_FRAME];
        for frame in self.pending[..full].as_chunks_mut::<RNN_FRAME>().0 {
            // nnnoiseless 按 16 位整数幅度处理。
            for (i, s) in input.iter_mut().zip(frame.iter()) {
                *i = s * 32768.0;
            }
            rnn.process_frame(&mut out, &input);
            for (s, o) in frame.iter_mut().zip(out) {
                *s = o / 32768.0;
            }
        }
        samples.extend(self.pending.drain(..full));
    }

    fn kook(&mut self, samples: &mut Vec<f32>) {
        if self.inst.is_none()
            && let Some(d) = self
                .shared
                .0
                .slot
                .try_lock()
                .ok()
                .and_then(|mut s| s.take())
        {
            self.inst = Some(d);
            self.shared.0.armed.store(true, Ordering::SeqCst);
        }
        let Some(inst) = self.inst.as_mut() else {
            return;
        };
        self.pending.append(samples);
        let full = self.pending.len() / BLOCK * BLOCK;
        let mut error = None;
        for block in self.pending[..full].as_chunks_mut::<BLOCK>().0 {
            let loud = block.iter().any(|s| s.abs() > 1e-3);
            if let Err(e) = inst.process(block) {
                error = Some(e);
                break;
            }
            if loud && block.iter().all(|s| *s == 0.0) {
                self.muted += BLOCK;
            } else if loud {
                if self.muted_logged {
                    log::info!("KOOK AI 降噪输出恢复，此前全零 {}ms", self.muted / 48);
                }
                self.muted = 0;
                self.muted_logged = false;
            }
        }
        samples.extend(self.pending.drain(..full));
        if let Some(e) = error {
            self.drop_inst();
            self.shared.fail(&e);
        } else if self.muted >= MUTED_LOG && !self.muted_logged {
            self.muted_logged = true;
            log::warn!("KOOK AI 降噪输入有声而输出持续为零 {}ms", self.muted / 48);
        }
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        if self.inst.is_some() {
            self.shared.0.armed.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(19_782), (2024, 2, 29));
    }

    #[test]
    fn stage_passes_through_without_instance() {
        let d = Denoise::default();
        let mut s = d.stage();
        let mut x = vec![0.5f32; 300];
        s.process(&mut x);
        assert_eq!(x.len(), 300);
        d.set(true);
        let mut x = vec![0.5f32; 300];
        s.process(&mut x);
        // 没有实例时原样通过。
        assert_eq!(x, vec![0.5f32; 300]);
    }

    #[test]
    fn rnnoise_keeps_sample_count_and_mode_switch_flushes() {
        let d = Denoise::default();
        d.set(true);
        d.set_mode(Mode::Rnnoise);
        let mut s = d.stage();
        let mut total = 0;
        for _ in 0..10 {
            let mut x = vec![0.01f32; 300];
            s.process(&mut x);
            total += x.len();
        }
        assert_eq!(total, 3000 / RNN_FRAME * RNN_FRAME);
        // 关闭时送出剩余样本。
        d.set(false);
        let mut x = vec![0.0f32; 10];
        s.process(&mut x);
        assert_eq!(total + x.len(), 3010);
    }

    #[test]
    fn failure_turns_denoise_off_and_notifies_once() {
        let d = Denoise::default();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let h = hits.clone();
        d.on_fail(Box::new(move |r| h.lock().unwrap().push(r)));
        d.set(true);
        d.fail(&format_err!("陷入"));
        assert!(!d.on());
        // 已关闭时不重复通知。
        d.fail(&format_err!("再次"));
        assert_eq!(*hits.lock().unwrap(), vec!["陷入".to_owned()]);
        // 用户重新开启后，再次故障仍会关闭并通知。
        d.set(true);
        d.fail(&format_err!("第三次"));
        assert!(!d.on());
        assert_eq!(hits.lock().unwrap().len(), 2);
    }

    #[test]
    fn unknown_mode_falls_back_to_kook() {
        let m: Mode = serde_json::from_str("\"x\"").unwrap();
        assert_eq!(m, Mode::Kook);
        assert_eq!(
            serde_json::to_string(&Mode::Rnnoise).unwrap(),
            "\"rnnoise\""
        );
    }
}

/// 用真实模块与证书验证处理链路。资源不随仓库分发，按环境变量指定路径后手动运行：
/// `DENOISE_WASM=… DENOISE_CERT=… cargo test --lib denoise::real -- --ignored`
#[cfg(test)]
mod real {
    use super::*;

    #[test]
    #[ignore = "需要本地降噪模块与证书"]
    fn real_module_reduces_noise() {
        let wasm = std::fs::read(std::env::var("DENOISE_WASM").unwrap()).unwrap();
        let cert = std::fs::read_to_string(std::env::var("DENOISE_CERT").unwrap()).unwrap();
        prepare(&wasm).unwrap();
        set_license(Some(cert));
        let d = Denoise::default();
        d.set(true);
        d.maintain();
        let mut stage = d.stage();
        let mut seed = 1u32;
        let (mut rms_in, mut rms_out) = (0.0f64, 0.0f64);
        // 3 秒白噪声，按 480 样本一段送入。
        for k in 0..300 {
            let mut x: Vec<f32> = (0..480)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (f64::from(seed) / f64::from(u32::MAX) - 0.5) as f32 * 0.2
                })
                .collect();
            if k >= 200 {
                rms_in += x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            }
            stage.process(&mut x);
            if k >= 200 {
                rms_out += x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            }
        }
        assert!(stage.inst.is_some(), "实例未启用");
        eprintln!("输入能量 {rms_in:.3}，输出能量 {rms_out:.3}");
        assert!(rms_out < rms_in / 4.0);
    }
}
