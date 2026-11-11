// 屏幕分享画面解码：后端经 Channel 发来编码后的帧，用 WebCodecs 解码后绘制到 canvas。
// 帧格式：u8 关键帧标记、u8 编码（1 H.264）、u16 宽、u16 高、i64 时间戳微秒，小端，共 14 字节，之后为 Annex B 数据。
// 解码出错、尺寸或编码参数变化时重建解码器，并通过 needKey 请求关键帧；收到关键帧前丢弃其余帧。

export type ParsedFrame = { key: boolean; codec: number; width: number; height: number; ts: number; data: Uint8Array };

const HEAD = 14;
const H264 = 1;

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

// 从 Annex B 数据中取 SPS 的 profile、约束与 level，组成 WebCodecs 的 codec 字符串。没有 SPS 时为 null。
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

export type DecoderStats = { decoded: number; dropped: number; errors: number; hardware: boolean | null };

export class ScreenDecoder {
  private decoder: VideoDecoder | null = null;
  private codec = "";
  private width = 0;
  private height = 0;
  private waitingKey = true;
  private latest: VideoFrame | null = null;
  private raf = 0;
  private canvas: HTMLCanvasElement | null = null;
  private ctx: CanvasRenderingContext2D | null = null;
  private closed = false;
  readonly stats: DecoderStats = { decoded: 0, dropped: 0, errors: 0, hardware: null };

  // needKey：需要关键帧时调用，由调用方转给后端。onSize：画面尺寸变化，用于布局宽高比。
  constructor(private readonly needKey: () => void, private readonly onSize: (w: number, h: number) => void) {}

  // 绑定绘制目标。画面切换承载位置时重新绑定，解码器不受影响。
  attach(canvas: HTMLCanvasElement | null) {
    this.canvas = canvas;
    this.ctx = canvas?.getContext("2d", { alpha: false, desynchronized: true }) ?? null;
    if (canvas && this.width) {
      canvas.width = this.width;
      canvas.height = this.height;
    }
  }

  push(buf: ArrayBuffer) {
    if (this.closed) return;
    const f = parseFrame(buf);
    if (!f) return;
    if (f.codec !== H264) {
      // 只协商出过 H.264。其他编码不解码，等对方回落。
      this.stats.dropped++;
      return;
    }
    if (f.key) {
      const codec = avcCodec(f.data);
      if (codec && (codec !== this.codec || f.width !== this.width || f.height !== this.height || !this.decoder)) {
        this.configure(codec, f.width, f.height);
      }
      this.waitingKey = false;
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
        // 只保留最新一帧，在下一个绘制周期画出，跟不上时跳过旧帧。
        this.latest?.close();
        this.latest = vf;
        this.raf ||= requestAnimationFrame(() => this.draw());
      },
      error: (e) => {
        if (this.decoder === dec) this.fail(e);
      },
    });
    dec.configure({ codec, codedWidth: width, codedHeight: height, hardwareAcceleration: "prefer-hardware", optimizeForLatency: true });
    this.decoder = dec;
    if (this.stats.hardware === null) {
      void VideoDecoder.isConfigSupported({ codec, codedWidth: width, codedHeight: height, hardwareAcceleration: "prefer-hardware" })
        .then((s) => { this.stats.hardware = Boolean(s.supported); })
        .catch(() => {});
    }
  }

  private draw() {
    this.raf = 0;
    const vf = this.latest;
    this.latest = null;
    if (!vf) return;
    if (this.ctx && this.canvas) this.ctx.drawImage(vf, 0, 0, this.canvas.width, this.canvas.height);
    vf.close();
  }

  // 解码出错：丢弃解码器，等关键帧重建。
  private fail(e: unknown) {
    console.warn("屏幕分享解码失败", e);
    this.stats.errors++;
    try { this.decoder?.close(); } catch { /* 已关闭 */ }
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
    try { this.decoder?.close(); } catch { /* 已关闭 */ }
    this.decoder = null;
    this.canvas = null;
    this.ctx = null;
  }
}
