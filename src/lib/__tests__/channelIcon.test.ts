import { expect, test } from "bun:test";
import { channelIcon } from "../channelIcon";
import { ALL, Perm } from "../perms";
import type { Channel } from "../../stores/types";

const ch = (type: Channel["type"], extra: Partial<Channel> = {}): Channel => ({ id: "c", name: "c", type, overwrites: [], userOverwrites: [], ...extra });
const deny = (bits: number, roleId = 0) => [{ roleId, allow: 0, deny: bits }];
const full = Perm.VIEW | Perm.SEND | Perm.VOICE_CONNECT;

test("普通文字与语音", () => {
  expect(channelIcon(ch("text"), full)).toMatchObject({ base: "text", lock: false, restricted: false, label: "文字频道" });
  expect(channelIcon(ch("voice"), full)).toMatchObject({ base: "voice", lock: false, restricted: false, label: "语音频道" });
});

test("密码语音显示锁，已有连接权限时仍保留", () => {
  expect(channelIcon(ch("voice", { password: true }), ALL)).toMatchObject({ lock: true, label: "语音频道，需要密码" });
  // 文字频道没有密码概念。
  expect(channelIcon(ch("text", { password: true }), full).lock).toBe(false);
});

test("受限标识只看频道对 @全体成员 的限制，不看覆盖数组是否为空", () => {
  expect(channelIcon(ch("text", { overwrites: deny(Perm.VIEW) }), ALL).restricted).toBe(true);
  expect(channelIcon(ch("voice", { overwrites: deny(Perm.VOICE_CONNECT) }), ALL).restricted).toBe(true);
  // 仅限制发言、或只针对某个角色的覆盖，不算受限。
  expect(channelIcon(ch("text", { overwrites: deny(Perm.SEND) }), full).restricted).toBe(false);
  expect(channelIcon(ch("text", { overwrites: deny(Perm.VIEW, 5) }), full).restricted).toBe(false);
  expect(channelIcon(ch("voice", { password: true, overwrites: deny(Perm.VIEW) }), ALL)).toMatchObject({ lock: true, restricted: true });
});

test("当前账号的限制写入提示，依据未知时不推断", () => {
  expect(channelIcon(ch("text"), Perm.VIEW).label).toBe("文字频道，只读");
  expect(channelIcon(ch("voice"), Perm.VIEW).label).toBe("语音频道，没有连接权限");
  expect(channelIcon(ch("voice"), null).label).toBe("语音频道");
  expect(channelIcon(ch("unknown"), null).base).toBe("unknown");
});
