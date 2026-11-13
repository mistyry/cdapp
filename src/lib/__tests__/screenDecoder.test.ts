import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { AV1, AV1_DEFAULT, ScreenDecoder, VP8, VP9, VP9_DEFAULT, av1Codec, av1ShowExisting, avcCodec, codecOf, hevcCodec, parseFrame } from "../screenDecoder";

function frame(key: boolean, w: number, h: number, ts: bigint, data: number[], codec = 1) {
  const buf = new ArrayBuffer(14 + data.length);
  const dv = new DataView(buf);
  dv.setUint8(0, key ? 1 : 0);
  dv.setUint8(1, codec);
  dv.setUint16(2, w, true);
  dv.setUint16(4, h, true);
  dv.setBigInt64(6, ts, true);
  new Uint8Array(buf, 14).set(data);
  return buf;
}

test("解析帧头与数据", () => {
  const f = parseFrame(frame(true, 1920, 1080, 123456789n, [0, 0, 0, 1, 0x67]))!;
  expect([f.key, f.codec, f.width, f.height, f.ts]).toEqual([true, 1, 1920, 1080, 123456789]);
  expect([...f.data]).toEqual([0, 0, 0, 1, 0x67]);
  expect(parseFrame(new ArrayBuffer(14))).toBeNull();
});

test("从 H.264 SPS 取 codec 字符串，支持 3 与 4 字节起始码", () => {
  expect(avcCodec(new Uint8Array([0, 0, 0, 1, 0x67, 0x64, 0x00, 0x2a, 0xac]))).toBe("avc1.64002a");
  expect(avcCodec(new Uint8Array([0, 0, 1, 0x06, 1, 2, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0]))).toBe("avc1.42e01f");
  expect(avcCodec(new Uint8Array([0, 0, 0, 1, 0x41, 0x9a, 0, 0]))).toBeNull();
});

function bits(fields: Array<[number, number]>) {
  const out: number[] = [];
  let cur = 0;
  let n = 0;
  for (const [v, w] of fields)
    for (let i = w - 1; i >= 0; i--) {
      cur = (cur << 1) | (Math.floor(v / 2 ** i) & 1);
      if (++n === 8) [out[out.length], cur, n] = [cur, 0, 0];
    }
  if (n) out.push(cur << (8 - n));
  return out;
}

function seqHeader(level: number, tier: number, high: number) {
  return bits([
    [0, 3], [0, 1], [0, 1], [0, 1], [0, 1], [0, 5],
    [0, 12], [level, 5], ...(level > 7 ? [[tier, 1] as [number, number]] : []),
    [10, 4], [10, 4], [1279, 11], [719, 11],
    [0, 1], [0, 1], [0, 1], [0, 1],
    [0, 4], [1, 1], [0, 2], [1, 1], [1, 1], [6, 3],
    [0, 3], [high, 1], [0, 1], [0, 1], [0, 1],
  ]);
}

function obu(type: number, payload: number[]) {
  return [(type << 3) | 2, payload.length, ...payload];
}

test("从 AV1 序列头取 codec 字符串", () => {
  const td = obu(2, []);
  expect(av1Codec(Uint8Array.from([...td, ...obu(1, seqHeader(8, 0, 0)), ...obu(6, [1, 2, 3])]))).toBe("av01.0.08M.08");
  expect(av1Codec(Uint8Array.from(obu(1, seqHeader(12, 1, 1))))).toBe("av01.0.12H.10");
  expect(av1Codec(Uint8Array.from(obu(1, seqHeader(5, 0, 0))))).toBe("av01.0.05M.08");
  expect(av1Codec(Uint8Array.from([...td, ...obu(6, [1, 2])]))).toBeNull();
  expect(av1Codec(Uint8Array.from(obu(1, [0x00])))).toBeNull();
  expect(av1Codec(Uint8Array.from([0x0a, 0x30, 1]))).toBeNull();
});

test("从 H.265 SPS 取 codec 字符串并去除防竞争字节", () => {
  const sps = [0, 0, 0, 1, 0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x5d, 0xa0];
  expect(hevcCodec(Uint8Array.from(sps))).toBe("hvc1.1.6.L93.90");
  expect(hevcCodec(Uint8Array.from([0, 0, 0, 1, 0x40, 0x01, 0x0c]))).toBeNull();
});

test("按帧头 codec 选择解析，AV1 无序列头时用默认值，未知编码不解析", () => {
  const av1 = parseFrame(frame(true, 1280, 720, 1n, obu(1, seqHeader(9, 0, 0)), AV1))!;
  expect(codecOf(av1)).toBe("av01.0.09M.08");
  const bare = parseFrame(frame(true, 1280, 720, 1n, obu(6, [1, 2]), AV1))!;
  expect(codecOf(bare)).toBe(AV1_DEFAULT);
  const h264 = parseFrame(frame(true, 1, 1, 1n, [0, 0, 0, 1, 0x67, 0x64, 0x00, 0x2a, 0xac]))!;
  expect(codecOf(h264)).toBe("avc1.64002a");
  expect(codecOf(parseFrame(frame(true, 1, 1, 1n, [1, 2, 3], 9))!)).toBeNull();
});

test("VP8 与 VP9 帧映射为 WebCodecs codec 字符串", () => {
  expect(codecOf(parseFrame(frame(true, 1, 1, 1n, [1], VP8))!)).toBe("vp8");
  expect(codecOf(parseFrame(frame(true, 1, 1, 1n, [1], VP9))!)).toBe(VP9_DEFAULT);
});

test("识别只含 show_existing_frame 的 AV1 帧", () => {
  expect(av1ShowExisting(Uint8Array.from([0x1a, 0x01, 0x88]))).toBe(true);
  expect(av1ShowExisting(Uint8Array.from([...obu(2, []), 0x1a, 0x01, 0x88]))).toBe(true);
  expect(av1ShowExisting(Uint8Array.from([0x1a, 0x01, 0x08]))).toBe(false);
  expect(av1ShowExisting(Uint8Array.from(obu(6, [0x32, 1])))).toBe(false);
  expect(av1ShowExisting(Uint8Array.from([0x1a, 0x01, 0x88, ...obu(4, [1])]))).toBe(false);
  expect(av1ShowExisting(Uint8Array.from([0x1a, 0x05, 0x88]))).toBe(false);
});

describe("ScreenDecoder", () => {
  type Chunk = { type: string; timestamp: number };
  const decoded: Chunk[] = [];
  const configs: Array<Record<string, unknown>> = [];
  const g = globalThis as Record<string, unknown>;
  const saved = { VideoDecoder: g.VideoDecoder, EncodedVideoChunk: g.EncodedVideoChunk };

  beforeEach(() => {
    decoded.length = 0;
    configs.length = 0;
    g.EncodedVideoChunk = class {
      type: string;
      timestamp: number;
      constructor(i: Chunk) {
        this.type = i.type;
        this.timestamp = i.timestamp;
      }
    };
    g.VideoDecoder = class {
      state = "unconfigured";
      static isConfigSupported() {
        return Promise.resolve({ supported: true });
      }
      configure(c: Record<string, unknown>) {
        configs.push(c);
        this.state = "configured";
      }
      decode(c: Chunk) {
        decoded.push(c);
      }
      close() {
        this.state = "closed";
      }
    };
  });

  afterEach(() => {
    g.VideoDecoder = saved.VideoDecoder;
    g.EncodedVideoChunk = saved.EncodedVideoChunk;
  });

  const key = (ts: bigint) => frame(true, 1280, 720, ts, [...obu(1, seqHeader(5, 0, 0)), ...obu(6, [0x10])], AV1);
  const delta = (ts: bigint) => frame(false, 1280, 720, ts, obu(6, [0x32]), AV1);
  const repeat = (ts: bigint) => frame(false, 1280, 720, ts, [0x1a, 0x01, 0x88], AV1);

  test("重进房间后关键帧之后的 show_existing_frame 不送解码", () => {
    const d = new ScreenDecoder(() => {}, () => {});
    d.push(key(1n));
    d.push(repeat(2n));
    d.push(delta(3n));
    d.push(repeat(4n));
    expect(decoded.map((c) => [c.type, c.timestamp])).toEqual([["key", 1], ["delta", 3], ["delta", 4]]);
    expect(d.stats.dropped).toBe(1);
  });

  test("关键帧之前的帧丢弃", () => {
    const d = new ScreenDecoder(() => {}, () => {});
    d.push(delta(1n));
    d.push(repeat(2n));
    d.push(key(3n));
    expect(decoded.map((c) => c.timestamp)).toEqual([3]);
    expect(d.stats.dropped).toBe(2);
  });

  test("VP8 关键帧配置解码器且不强制硬件偏好", () => {
    const d = new ScreenDecoder(() => {}, () => {});
    d.push(frame(true, 1280, 720, 1n, [1], VP8));
    d.push(frame(false, 1280, 720, 2n, [2], VP8));
    expect(decoded.map((c) => c.timestamp)).toEqual([1, 2]);
    expect(configs).toEqual([{ codec: "vp8", codedWidth: 1280, codedHeight: 720, optimizeForLatency: true }]);
  });
});
