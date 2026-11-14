export type ParsedFrame = { key: boolean; codec: number; width: number; height: number; ts: number; data: Uint8Array };

const HEAD = 14;
export const H264 = 1;
export const H265 = 2;
export const AV1 = 3;
export const VP8 = 4;
export const VP9 = 5;
export const I420 = 6;
export const AV1_DEFAULT = "av01.0.08M.08";
export const VP9_DEFAULT = "vp09.00.40.08";

export function parseFrame(buf: ArrayBuffer): ParsedFrame | null {
  if (buf.byteLength <= HEAD) return null;
  const dv = new DataView(buf);
  return {
    key: dv.getUint8(0) === 1,
    codec: dv.getUint8(1),
    width: dv.getUint16(2, true),
    height: dv.getUint16(4, true),
    ts: Number(dv.getBigInt64(6, true)),
    data: new Uint8Array(buf, HEAD),
  };
}

export function avcCodec(data: Uint8Array): string | null {
  for (let i = 0; i + 6 < data.length; i++) {
    if (data[i] !== 0 || data[i + 1] !== 0) continue;
    const at = data[i + 2] === 1 ? i + 3 : data[i + 2] === 0 && data[i + 3] === 1 ? i + 4 : -1;
    if (at < 0 || at + 3 >= data.length) continue;
    if ((data[at] & 0x1f) !== 7) continue;
    const hex = (x: number) => x.toString(16).padStart(2, "0");
    return `avc1.${hex(data[at + 1])}${hex(data[at + 2])}${hex(data[at + 3])}`;
  }
  return null;
}

class Bits {
  private pos = 0;
  constructor(private readonly d: Uint8Array, private readonly end = d.length * 8) {}
  get left() {
    return this.end - this.pos;
  }
  u(n: number): number {
    if (n > this.left) throw new RangeError("eof");
    let v = 0;
    for (let i = 0; i < n; i++, this.pos++) v = v * 2 + ((this.d[this.pos >> 3] >> (7 - (this.pos & 7))) & 1);
    return v;
  }
  uvlc(): number {
    let z = 0;
    while (this.u(1) === 0) if (++z >= 32) throw new RangeError("uvlc");
    return z ? this.u(z) + (2 ** z - 1) : 0;
  }
}

function leb128(d: Uint8Array, at: number): [number, number] | null {
  let v = 0;
  for (let i = 0; i < 8 && at + i < d.length; i++) {
    v += (d[at + i] & 0x7f) * 2 ** (7 * i);
    if (!(d[at + i] & 0x80)) return [v, i + 1];
  }
  return null;
}

function av1SeqHeader(b: Bits): string {
  const profile = b.u(3);
  b.u(1);
  const reduced = b.u(1);
  let level = 0;
  let tier = 0;
  let decoderModel = 0;
  let delayLen = 0;
  if (reduced) level = b.u(5);
  else {
    if (b.u(1)) {
      b.u(32);
      b.u(32);
      if (b.u(1)) b.uvlc();
      decoderModel = b.u(1);
      if (decoderModel) {
        delayLen = b.u(5) + 1;
        b.u(32);
        b.u(5);
        b.u(5);
      }
    }
    const initialDelay = b.u(1);
    const ops = b.u(5) + 1;
    for (let i = 0; i < ops; i++) {
      b.u(12);
      const lv = b.u(5);
      const tr = lv > 7 ? b.u(1) : 0;
      if (i === 0) [level, tier] = [lv, tr];
      if (decoderModel && b.u(1)) {
        b.u(delayLen);
        b.u(delayLen);
        b.u(1);
      }
      if (initialDelay && b.u(1)) b.u(4);
    }
  }
  const wBits = b.u(4) + 1;
  const hBits = b.u(4) + 1;
  b.u(wBits);
  b.u(hBits);
  if (!reduced && b.u(1)) {
    b.u(4);
    b.u(3);
  }
  b.u(3);
  if (!reduced) {
    b.u(4);
    const orderHint = b.u(1);
    if (orderHint) b.u(2);
    const force = b.u(1) ? 2 : b.u(1);
    if (force > 0 && !b.u(1)) b.u(1);
    if (orderHint) b.u(3);
  }
  b.u(3);
  const high = b.u(1);
  const depth = profile === 2 && high ? (b.u(1) ? 12 : 10) : high ? 10 : 8;
  const two = (x: number) => String(x).padStart(2, "0");
  return `av01.${profile}.${two(level)}${tier ? "H" : "M"}.${two(depth)}`;
}

export function av1Codec(data: Uint8Array): string | null {
  let at = 0;
  while (at < data.length) {
    const h = data[at];
    const type = (h >> 3) & 0xf;
    const ext = (h >> 2) & 1;
    const hasSize = (h >> 1) & 1;
    let p = at + 1 + ext;
    let size = data.length - p;
    if (hasSize) {
      const s = leb128(data, p);
      if (!s) return null;
      [size, p] = [s[0], p + s[1]];
    }
    if (p + size > data.length) return null;
    if (type === 1) {
      try {
        return av1SeqHeader(new Bits(data.subarray(p, p + size)));
      } catch {
        return null;
      }
    }
    if (!hasSize) return null;
    at = p + size;
  }
  return null;
}

function unescape(d: Uint8Array): Uint8Array {
  const out: number[] = [];
  let zeros = 0;
  for (let i = 0; i < d.length; i++) {
    if (d[i] === 3 && zeros >= 2) {
      zeros = 0;
      continue;
    }
    zeros = d[i] === 0 ? zeros + 1 : 0;
    out.push(d[i]);
  }
  return Uint8Array.from(out);
}

export function hevcCodec(data: Uint8Array): string | null {
  for (let i = 0; i + 4 < data.length; i++) {
    if (data[i] !== 0 || data[i + 1] !== 0) continue;
    const at = data[i + 2] === 1 ? i + 3 : data[i + 2] === 0 && data[i + 3] === 1 ? i + 4 : -1;
    if (at < 0 || ((data[at] >> 1) & 0x3f) !== 33) continue;
    let end = data.length;
    for (let k = at + 2; k + 2 < data.length; k++) {
      if (data[k] === 0 && data[k + 1] === 0 && (data[k + 2] === 1 || (data[k + 2] === 0 && data[k + 3] === 1))) {
        end = k;
        break;
      }
    }
    const sps = unescape(data.subarray(at + 2, end));
    if (sps.length < 13) return null;
    const space = sps[1] >> 6;
    const tier = (sps[1] >> 5) & 1;
    const profile = sps[1] & 0x1f;
    let compat = ((sps[2] << 24) | (sps[3] << 16) | (sps[4] << 8) | sps[5]) >>> 0;
    let rev = 0;
    for (let k = 0; k < 32; k++, compat >>>= 1) rev = (rev << 1) | (compat & 1);
    const cons = Array.from(sps.subarray(6, 12));
    while (cons.length && cons[cons.length - 1] === 0) cons.pop();
    const level = sps[12];
    const sp = ["", "A", "B", "C"][space];
    const tail = cons.map((c) => `.${c.toString(16).toUpperCase()}`).join("");
    return `hvc1.${sp}${profile}.${(rev >>> 0).toString(16).toUpperCase()}.${tier ? "H" : "L"}${level}${tail}`;
  }
  return null;
}

export function av1ShowExisting(data: Uint8Array): boolean {
  let at = 0;
  let found = false;
  while (at < data.length) {
    const h = data[at];
    const type = (h >> 3) & 0xf;
    const ext = (h >> 2) & 1;
    const hasSize = (h >> 1) & 1;
    let p = at + 1 + ext;
    let size = data.length - p;
    if (hasSize) {
      const s = leb128(data, p);
      if (!s) return false;
      [size, p] = [s[0], p + s[1]];
    }
    if (p + size > data.length) return false;
    if (type === 3) {
      if (found || size < 1 || !(data[p] & 0x80)) return false;
      found = true;
    } else if (type !== 2 && type !== 15) {
      return false;
    }
    if (!hasSize) break;
    at = p + size;
  }
  return found;
}

export function codecOf(f: ParsedFrame): string | null {
  if (f.codec === H264) return avcCodec(f.data);
  if (f.codec === H265) return hevcCodec(f.data);
  if (f.codec === AV1) return av1Codec(f.data) ?? AV1_DEFAULT;
  if (f.codec === VP8) return "vp8";
  if (f.codec === VP9) return VP9_DEFAULT;
  return null;
}

export type DecoderStats = { decoded: number; dropped: number; errors: number };

export class ScreenDecoder {
  private decoder: VideoDecoder | null = null;
  private codec = "";
  private width = 0;
  private height = 0;
  private waitingKey = true;
  private afterKey = false;
  private latest: VideoFrame | null = null;
  private shown: VideoFrame | null = null;
  private raf = 0;
  private canvas: HTMLCanvasElement | null = null;
  private ctx: CanvasRenderingContext2D | null = null;
  private closed = false;
  private readonly unsupported = new Set<string>();
  readonly stats: DecoderStats = { decoded: 0, dropped: 0, errors: 0 };

  constructor(private readonly needKey: () => void, private readonly onSize: (w: number, h: number) => void) {}

  attach(canvas: HTMLCanvasElement | null) {
    this.canvas = canvas;
    this.ctx = canvas?.getContext("2d", { alpha: false, desynchronized: true }) ?? null;
    if (canvas && this.width) {
      canvas.width = this.width;
      canvas.height = this.height;
    }
    if (this.ctx && this.canvas && this.shown) this.ctx.drawImage(this.shown, 0, 0, this.canvas.width, this.canvas.height);
  }

  push(buf: ArrayBuffer) {
    if (this.closed) return;
    const f = parseFrame(buf);
    if (!f) return;
    if (f.codec === I420) {
      this.raw(f);
      return;
    }
    if (f.codec < H264 || f.codec > VP9) {
      this.stats.dropped++;
      return;
    }
    if (f.key) {
      const codec = codecOf(f);
      if (codec && (codec !== this.codec || f.width !== this.width || f.height !== this.height || !this.decoder)) {
        if (this.unsupported.has(codec)) {
          this.stats.dropped++;
          return;
        }
        this.configure(codec, f.width, f.height);
      }
      this.waitingKey = false;
      this.afterKey = true;
    } else if (this.afterKey && f.codec === AV1 && av1ShowExisting(f.data)) {
      this.stats.dropped++;
      return;
    } else {
      this.afterKey = false;
    }
    if (this.waitingKey || !this.decoder || this.decoder.state !== "configured") {
      this.stats.dropped++;
      return;
    }
    try {
      this.decoder.decode(new EncodedVideoChunk({ type: f.key ? "key" : "delta", timestamp: f.ts, data: f.data }));
    } catch (e) {
      this.fail(e);
    }
  }

  private raw(f: ParsedFrame) {
    const { width, height } = f;
    if (!width || !height || width % 2 || height % 2 || f.data.length !== (width * height * 3) / 2) {
      this.stats.dropped++;
      return;
    }
    if (width !== this.width || height !== this.height) {
      this.width = width;
      this.height = height;
      this.onSize(width, height);
      if (this.canvas) {
        this.canvas.width = width;
        this.canvas.height = height;
      }
    }
    let vf: VideoFrame;
    try {
      vf = new VideoFrame(f.data, { format: "I420", codedWidth: width, codedHeight: height, timestamp: f.ts });
    } catch (e) {
      this.stats.errors++;
      console.warn("屏幕共享画面无法显示", e);
      return;
    }
    this.stats.decoded++;
    this.latest?.close();
    this.latest = vf;
    this.raf ||= requestAnimationFrame(() => this.draw());
  }

  private configure(codec: string, width: number, height: number) {
    this.decoder?.close();
    this.codec = codec;
    this.width = width;
    this.height = height;
    this.onSize(width, height);
    if (this.canvas) {
      this.canvas.width = width;
      this.canvas.height = height;
    }
    const dec = new VideoDecoder({
      output: (vf) => {
        if (this.decoder !== dec) return vf.close();
        this.stats.decoded++;
        this.latest?.close();
        this.latest = vf;
        this.raf ||= requestAnimationFrame(() => this.draw());
      },
      error: (e) => {
        if (this.decoder === dec) this.fail(e);
      },
    });
    dec.configure({ codec, codedWidth: width, codedHeight: height, optimizeForLatency: true });
    this.decoder = dec;
    void VideoDecoder.isConfigSupported({ codec, codedWidth: width, codedHeight: height })
      .then((s) => {
        if (s.supported || this.decoder !== dec) return;
        this.unsupported.add(codec);
        this.fail(new Error(`不支持的编码 ${codec}`));
      })
      .catch(() => {});
  }

  private draw() {
    this.raf = 0;
    const vf = this.latest;
    this.latest = null;
    if (!vf) return;
    if (this.ctx && this.canvas) this.ctx.drawImage(vf, 0, 0, this.canvas.width, this.canvas.height);
    this.shown?.close();
    this.shown = vf;
  }

  private fail(e: unknown) {
    console.warn("屏幕共享解码失败", e);
    this.stats.errors++;
    try { this.decoder?.close(); } catch {}
    this.decoder = null;
    this.codec = "";
    this.waitingKey = true;
    this.needKey();
  }

  close() {
    this.closed = true;
    cancelAnimationFrame(this.raf);
    this.raf = 0;
    this.latest?.close();
    this.latest = null;
    this.shown?.close();
    this.shown = null;
    try { this.decoder?.close(); } catch {}
    this.decoder = null;
    this.canvas = null;
    this.ctx = null;
  }
}
