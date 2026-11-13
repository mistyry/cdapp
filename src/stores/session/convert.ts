import type { ChannelDto, LiveInfo, Raw } from "../../lib/api";
import type { Channel, Group, Message, Quote, Reaction, User, VoiceUser } from "../types";
import { parse as parseKmd, toPlain, type Names } from "../../lib/kmarkdown";
import { isText, label, parseCards, toAttachment, Kind } from "../../lib/content";
import { channelById, session, setSession } from "./state";

export function toUser(r: Raw | undefined): User {
  return {
    id: String(r?.id ?? ""),
    name: r?.nickname || r?.username || "未知用户",
    tag: r?.identify_num ? String(r.identify_num) : undefined,
    avatar: r?.avatar || undefined,
    online: r?.online ?? true,
  };
}

export function remember(u: User) {
  if (u.id && !session.users[u.id]) setSession("users", u.id, u);
}

export const pad = (n: number) => String(n).padStart(2, "0");
export function stamp(ms: number) {
  const d = new Date(ms);
  const today = new Date().toDateString() === d.toDateString();
  const short = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const day = today ? "今天" : `${d.getFullYear()}/${d.getMonth() + 1}/${d.getDate()}`;
  return { time: `${day} ${short}`, short };
}

export const names: Names = {
  user: (id) => session.users[id]?.name,
  role: (id) => {
    for (const a of Object.values(session.access)) {
      const r = a.data?.roles.find((x) => String(x.id) === id);
      if (r) return r.name;
    }
    return session.refs.roles[id];
  },
  channel: (id) => channelById(id)?.channel.name ?? session.refs.channels[id],
};

export function plainContent(type: number, content: string, attachment?: Message["attachment"]): string {
  if (type === Kind.CARD) {
    const parts = parseCards(content)?.flatMap((c) => c.parts) ?? [];
    const file = parts.find((p) => p.t === "file");
    if (file?.t === "file") return `[文件] ${file.name}`.trim().slice(0, 200);
    const first = parts.find((p) => p.t !== "image");
    return first?.t === "header" || first?.t === "text" ? `[卡片消息] ${first.kmd ? toPlain(parseKmd(first.v), names) : first.v}`.slice(0, 200) : "[卡片消息]";
  }
  const l = label(type, attachment);
  if (l) return l;
  return toPlain(parseKmd(content.slice(0, 2000)), names);
}

export function rememberRefs(k: Raw | undefined) {
  if (!k || typeof k !== "object") return;
  for (const u of Array.isArray(k.mention_part) ? k.mention_part : []) {
    if (u?.id && u.username) remember({ id: String(u.id), name: String(u.username), avatar: u.avatar || undefined, online: true });
  }
  for (const r of Array.isArray(k.mention_role_part) ? k.mention_role_part : []) {
    if (r?.role_id !== undefined && r.name && !session.refs.roles[String(r.role_id)]) setSession("refs", "roles", String(r.role_id), String(r.name));
  }
  for (const c of Array.isArray(k.channel_part) ? k.channel_part : []) {
    if (c?.id && c.name && !session.refs.channels[String(c.id)]) setSession("refs", "channels", String(c.id), String(c.name));
  }
}

export function toQuote(q: Raw | undefined | null): Quote | undefined {
  if (!q || typeof q !== "object") return undefined;
  const author = toUser(q.author);
  if (author.id) remember(author);
  rememberRefs(q.kmarkdown);
  const type = Number(q.type ?? Kind.KMARKDOWN);
  const content = String(q.content ?? "");
  const id = q.rong_id ?? q.msg_id;
  return { id: id ? String(id) : undefined, authorId: author.id || undefined, author: author.name, type, content, text: plainContent(type, content, toAttachment(q.attachments)) };
}

export function toMessage(r: Raw): Message {
  const author = toUser(r.author);
  remember(author);
  rememberRefs(r.kmarkdown);
  const createAt = Number(r.create_at ?? r.msg_timestamp ?? Date.now());
  const type = Number(r.type ?? Kind.TEXT);
  return {
    id: String(r.rong_id ?? r.msg_id ?? r.id),
    rongId: r.rong_id,
    author: author.id,
    name: author.name,
    avatar: author.avatar,
    system: Boolean(r.author?.is_sys) || undefined,
    createAt,
    ...stamp(createAt),
    type,
    content: String(r.content ?? ""),
    quote: toQuote(r.quote),
    attachment: isText(type) || type === Kind.IMAGE ? undefined : toAttachment(r.attachments),
    updatedAt: Number(r.updated_at) > 0 ? Number(r.updated_at) : undefined,
    reactions: toReactions(r.reactions),
  };
}

export function toReactions(list: unknown): Reaction[] | undefined {
  if (!Array.isArray(list)) return undefined;
  const out: Reaction[] = [];
  for (const r of list) {
    const id = r?.emoji?.id;
    const count = Number(r?.count ?? 0);
    if (!id || !(count > 0)) continue;
    out.push({ id: String(id), name: String(r.emoji.name ?? id), count, me: Boolean(r.me), users: {} });
  }
  return out.length ? out : undefined;
}

export function toChannel(c: ChannelDto): Channel {
  return {
    id: c.id,
    name: c.name || (c.kind === "unknown" || c.kind === "radio" ? `未知频道（类型 ${c.rawType}）` : "未命名频道"),
    type: c.kind === "text" || c.kind === "voice" ? c.kind : "unknown",
    radio: c.kind === "radio" || undefined,
    limit: c.limit || undefined,
    overwrites: c.overwrites ?? [],
    userOverwrites: c.userOverwrites ?? [],
    password: c.hasPassword || undefined,
    sync: c.permissionSync || undefined,
    users: c.kind === "voice" ? voiceUsers(c.users ?? []) : undefined,
    preview: c.kind === "text" ? c.preview || undefined : undefined,
  };
}

export function voiceUsers(list: Raw[]): VoiceUser[] {
  const out: VoiceUser[] = [];
  for (const u of list) {
    const id = String(u?.id ?? "");
    if (!id || out.some((x) => x.id === id)) continue;
    const user: VoiceUser = toUser({ id, nickname: u.nickname, username: u.username, identify_num: u.identifyNum ?? u.identify_num, avatar: u.avatar });
    const live = u.live ?? toLive(u.live_info);
    if (live) user.live = live;
    out.push(user);
  }
  return out;
}

export function toLive(r: Raw | null | undefined): LiveInfo | undefined {
  if (!r?.in_live) return undefined;
  const n = (v: unknown) => (Number.isFinite(Number(v)) ? Number(v) : 0);
  return {
    resolution: n(r.resolution), frameRate: n(r.frame_rate), startTime: n(r.live_start_time),
    audienceCount: n(r.audience_count), audienceLimit: n(r.audience_limit),
  };
}

export function toGroups(list: ChannelDto[]): Group[] {
  const byLevel = (a: ChannelDto, b: ChannelDto) => a.level - b.level;
  const cats = new Map(list.filter((c) => c.kind === "category").map((c) => [c.id, c]));
  const roots = list.filter((c) => c.kind === "category" || !c.parentId || !cats.has(c.parentId)).sort(byLevel);
  const groups: Group[] = [];
  for (const r of roots) {
    if (r.kind === "category") {
      const children = list.filter((c) => c.kind !== "category" && c.parentId === r.id).sort(byLevel);
      groups.push({ id: r.id, name: r.name, channels: children.map(toChannel) });
      continue;
    }
    const last = groups[groups.length - 1];
    if (last?.loose) last.channels.push(toChannel(r));
    else groups.push({ id: `_${groups.length}`, name: "", loose: true, channels: [toChannel(r)] });
  }
  return groups;
}

export function parse(s: unknown): Raw | null {
  if (typeof s !== "string") return (s as Raw) ?? null;
  try {
    return JSON.parse(s);
  } catch {
    return null;
  }
}
