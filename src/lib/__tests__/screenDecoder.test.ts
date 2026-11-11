import { expect, test } from "bun:test";
import { avcCodec, parseFrame } from "../screenDecoder";

function frame(key: boolean, w: number, h: number, ts: bigint, data: number[]) {
  const buf = new ArrayBuffer(14 + data.length);
  const dv = new DataView(buf);
  dv.setUint8(0, key ? 1 : 0);
  dv.setUint8(1, 1);
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

test("从 SPS 取 codec 字符串，支持 3 与 4 字节起始码", () => {
  // 实测 3.58 关键帧开头：00 00 00 01 67 64 00 2a。
  expect(avcCodec(new Uint8Array([0, 0, 0, 1, 0x67, 0x64, 0x00, 0x2a, 0xac]))).toBe("avc1.64002a");
  expect(avcCodec(new Uint8Array([0, 0, 1, 0x06, 1, 2, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0]))).toBe("avc1.42e01f");
  // 只有非关键帧切片时没有 SPS。
  expect(avcCodec(new Uint8Array([0, 0, 0, 1, 0x41, 0x9a, 0, 0]))).toBeNull();
});
