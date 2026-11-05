// KMarkdown 解析为语法树。只产出数据，不生成 HTML；渲染端用文本节点输出，原始 HTML 一律按文字显示。
// 服务端按原样保存内容（含 HTML、实体与 javascript: 链接），安全处理全部在客户端完成。
// 解析对任意输入不抛异常，复杂度近似线性：未闭合的标记按字面输出，并缓存“其后不存在闭合”的结论。

export type Inline =
  | { t: "text"; v: string }
  | { t: "br" }
  | { t: "b" | "i" | "s" | "u" | "spl"; c: Inline[] }
  | { t: "code"; v: string }
  | { t: "link"; href: string; c: Inline[] }
  | { t: "met"; id: string }
  | { t: "rol"; id: string }
  | { t: "chn"; id: string }
  | { t: "emj"; name: string; id: string };

export type Block =
  | { t: "p"; c: Inline[] }
  | { t: "pre"; lang: string; v: string }
  | { t: "quote"; c: Block[] };

// 超长内容只解析前段，其余按纯文本附加，避免极端输入拖慢渲染。
export const MAX_PARSE = 20_000;
const MAX_DEPTH = 6;
const ID = /^[A-Za-z0-9_-]{1,64}$/;
const EMOJI_ID = /^[A-Za-z0-9_-]{1,64}\/[A-Za-z0-9_-]{1,64}$/;
const ESCAPABLE = "\\`*_~()[]>-#+.!|{}<";

// 只允许 http / https 链接，其余（javascript:、data:、相对路径等）返回 null。
export function safeUrl(raw: string): string | null {
  const s = raw.trim();
  if (!/^https?:\/\//i.test(s)) return null;
  try {
    const u = new URL(s);
    return u.protocol === "http:" || u.protocol === "https:" ? u.href : null;
  } catch {
    return null;
  }
}

export const emojiUrl = (id: string) => `https://img.kookapp.cn/emojis/${id}.png`;

export function parse(src: string): Block[] {
  let text = String(src ?? "").replace(/\r\n?/g, "\n");
  let tail = "";
  if (text.length > MAX_PARSE) {
    tail = text.slice(MAX_PARSE);
    text = text.slice(0, MAX_PARSE);
  }
  const blocks = parseBlocks(text.split("\n"), 0);
  if (tail) {
    const last = blocks[blocks.length - 1];
    if (last?.t === "p") last.c.push({ t: "text", v: tail });
    else blocks.push({ t: "p", c: [{ t: "text", v: tail }] });
  }
  return blocks;
}

function parseBlocks(lines: string[], depth: number): Block[] {
  const out: Block[] = [];
  let para: string[] = [];
  const flush = () => {
    if (para.length) out.push({ t: "p", c: inline(para.join("\n")) });
    para = [];
  };
  // 可作为结束围栏的行号（升序），查找下一个时不重复扫描。
  const fences: number[] = [];
  lines.forEach((l, j) => /^\s*```\s*$/.test(l) && fences.push(j));
  let fi = 0;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const fence = /^\s*```(.*)$/.exec(line);
    if (fence) {
      const after = fence[1];
      // 单行代码块 ```code```。
      if (after.length > 3 && after.endsWith("```")) {
        flush();
        out.push({ t: "pre", lang: "", v: after.slice(0, -3) });
        continue;
      }
      // 多行代码块：寻找结束围栏，没有则按普通文字处理。围栏后若不是语言名，视为第一行代码。
      while (fi < fences.length && fences[fi] <= i) fi++;
      const close = fi < fences.length ? fences[fi] : -1;
      if (close > i) {
        flush();
        const lang = /^[\w+#.-]*$/.test(after.trim()) ? after.trim() : "";
        const body = lines.slice(i + 1, close);
        if (!lang && after.trim()) body.unshift(after);
        out.push({ t: "pre", lang, v: body.join("\n") });
        i = close;
        continue;
      }
    }
    if (depth < MAX_DEPTH && /^>\s?/.test(line)) {
      flush();
      const q: string[] = [];
      // 引用从 "> " 开始，跨单个换行延续，遇到空行结束。
      while (i < lines.length && lines[i].trim() !== "") q.push(lines[i++].replace(/^>\s?/, ""));
      i--;
      out.push({ t: "quote", c: parseBlocks(q, depth + 1) });
      continue;
    }
    para.push(line);
  }
  flush();
  return out;
}

const MAX_URL = 2048;

type Wrap = "b" | "i" | "s" | "u" | "spl";
const PAIRS: Array<[string, Wrap]> = [["**", "b"], ["~~", "s"], ["(ins)", "u"], ["(spl)", "spl"], ["*", "i"]];
const REFS = ["met", "rol", "chn"] as const;

export function inline(s: string, depth = 0): Inline[] {
  const out: Inline[] = [];
  let buf = "";
  const push = (n: Inline) => {
    if (buf) out.push({ t: "text", v: buf });
    buf = "";
    out.push(n);
  };
  // 记录某个闭合标记从哪里开始不再出现，未闭合的标记不重复向后搜索。
  const none = new Map<string, number>();
  const find = (tok: string, from: number) => {
    const miss = none.get(tok);
    if (miss !== undefined && from >= miss) return -1;
    const j = closeAt(s, tok, from);
    if (j < 0) none.set(tok, Math.min(miss ?? Infinity, from));
    return j;
  };

  let i = 0;
  while (i < s.length) {
    const ch = s[i];
    if (ch === "\\" && i + 1 < s.length && ESCAPABLE.includes(s[i + 1])) {
      buf += s[i + 1];
      i += 2;
      continue;
    }
    if (ch === "\n") {
      push({ t: "br" });
      i++;
      continue;
    }
    if (ch === "`") {
      const j = s.indexOf("`", i + 1);
      if (j > i + 1 && !s.slice(i + 1, j).includes("\n")) {
        push({ t: "code", v: s.slice(i + 1, j) });
        i = j + 1;
        continue;
      }
    }
    if (ch === "(") {
      let done = false;
      for (const k of REFS) {
        const tag = `(${k})`;
        if (!s.startsWith(tag, i)) continue;
        const j = find(tag, i + tag.length);
        const id = j < 0 ? "" : s.slice(i + tag.length, j);
        if (j >= 0 && ID.test(id)) {
          push({ t: k, id });
          i = j + tag.length;
          done = true;
        }
        break;
      }
      if (done) continue;
      if (s.startsWith("(emj)", i)) {
        const j = find("(emj)", i + 5);
        const m = j < 0 ? null : /^\[([^\]\s]{1,140})\]/.exec(s.slice(j + 5, j + 5 + 142));
        const name = j < 0 ? "" : s.slice(i + 5, j);
        if (m && name && name.length <= 64 && !name.includes("\n")) {
          if (EMOJI_ID.test(m[1])) push({ t: "emj", name, id: m[1] });
          else push({ t: "text", v: `:${name}:` });
          i = j + 5 + m[0].length;
          continue;
        }
      }
    }
    if (ch === "[") {
      const link = parseLink(s, i, find);
      if (link) {
        const inner = depth < MAX_DEPTH ? inline(link.text, depth + 1) : [{ t: "text" as const, v: link.text }];
        const href = safeUrl(link.url);
        if (href) push({ t: "link", href, c: inner.length ? inner : [{ t: "text", v: href }] });
        else {
          // 不安全或无效的链接只显示文字与原地址，不可点击。
          for (const n of inner) push(n);
          buf += ` (${link.url})`;
        }
        i = link.end;
        continue;
      }
    }
    if ((ch === "h" || ch === "H") && /^https?:\/\//i.test(s.slice(i, i + 8)) && !/[\w/]/.test(s[i - 1] ?? "")) {
      let j = i;
      // 限制裸链接长度，避免长串无效地址反复扫描。
      while (j < s.length && j - i < MAX_URL && !/[\s<>"'`\]()（）【】，。！？；：、]/.test(s[j])) j++;
      const scanned = j;
      while (j > i && /[.,;:!?*~]/.test(s[j - 1])) j--;
      const raw = s.slice(i, j);
      const href = safeUrl(raw);
      if (href) {
        push({ t: "link", href, c: [{ t: "text", v: raw }] });
        i = j;
        continue;
      }
      // 无效地址整体按文字处理，不逐字符重试。
      buf += s.slice(i, scanned);
      i = scanned;
      continue;
    }
    let wrapped = false;
    for (const [tok, t] of PAIRS) {
      if (!s.startsWith(tok, i)) continue;
      const from = i + tok.length;
      // 强调内容不能为空，也不能以空白开头（例如 "a * b * c" 中的乘号）。
      if (from >= s.length || /\s/.test(s[from]) || s.startsWith(tok, from)) break;
      const j = find(tok, from);
      if (j > from && !/\s/.test(s[j - 1])) {
        const body = s.slice(from, j);
        push({ t, c: depth < MAX_DEPTH ? inline(body, depth + 1) : [{ t: "text", v: body }] });
        i = j + tok.length;
        wrapped = true;
      }
      break;
    }
    if (wrapped) continue;
    buf += ch;
    i++;
  }
  if (buf) out.push({ t: "text", v: buf });
  return out;
}

// 查找未被反斜杠转义的闭合标记。
function closeAt(s: string, tok: string, from: number): number {
  let j = s.indexOf(tok, from);
  while (j >= 0) {
    let k = j - 1;
    let slashes = 0;
    while (k >= from && s[k] === "\\") (slashes++, k--);
    if (slashes % 2 === 0) return j;
    j = s.indexOf(tok, j + 1);
  }
  return -1;
}

function parseLink(s: string, i: number, find: (tok: string, from: number) => number) {
  const mid = find("](", i + 1);
  if (mid < 0 || mid - i > 2000 || s.slice(i + 1, mid).includes("\n")) return null;
  // 结束括号须未被转义，并与地址中的括号配对。
  let close = -1;
  let open = 0;
  for (let k = mid + 2; k < s.length && k - mid <= 4000; k++) {
    const c = s[k];
    if (c === "\\") { k++; continue; }
    if (c === "\n") break;
    if (c === "(") open++;
    else if (c === ")") {
      if (open === 0) { close = k; break; }
      open--;
    }
  }
  if (close < 0) return null;
  const url = s.slice(mid + 2, close).replace(/\\(.)/g, (m, c: string) => (ESCAPABLE.includes(c) ? c : m));
  if (/\s/.test(url.trim()) || !url.trim()) return null;
  return { text: s.slice(i + 1, mid), url, end: close + 1 };
}

// 名称解析：按 ID 查找，查不到时显示 ID 的占位，不按显示名称匹配。
export type Names = {
  user: (id: string) => string | undefined;
  role: (id: string) => string | undefined;
  channel: (id: string) => string | undefined;
};

export function mentionLabel(n: Inline & { t: "met" | "rol" | "chn" }, names: Names): string {
  if (n.t === "met") {
    if (n.id === "all") return "@全体成员";
    if (n.id === "here") return "@在线成员";
    return `@${names.user(n.id) ?? "未知用户"}`;
  }
  if (n.t === "rol") return `@${names.role(n.id) ?? "未知角色"}`;
  return `#${names.channel(n.id) ?? "未知频道"}`;
}

// 转为单行纯文本，用于引用摘要与回复预览。
export function toPlain(blocks: Block[], names: Names): string {
  const parts: string[] = [];
  const walk = (ns: Inline[]) => {
    for (const n of ns) {
      switch (n.t) {
        case "text": parts.push(n.v); break;
        case "br": parts.push(" "); break;
        case "code": parts.push(n.v); break;
        case "spl": parts.push("[隐藏内容]"); break;
        case "link": walk(n.c); break;
        case "emj": parts.push(`:${n.name}:`); break;
        case "met": case "rol": case "chn": parts.push(mentionLabel(n, names)); break;
        default: walk(n.c);
      }
    }
  };
  const block = (bs: Block[]) => {
    for (const b of bs) {
      if (parts.length) parts.push(" ");
      if (b.t === "p") walk(b.c);
      else if (b.t === "pre") parts.push("[代码块]");
      else block(b.c);
    }
  };
  block(blocks);
  return parts.join("").replace(/\s+/g, " ").trim();
}
