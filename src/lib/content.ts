// 消息类型与非文本内容的可读降级。只读取已验证的字段，结构异常时退回占位文字，不抛异常。
import { safeUrl } from "./kmarkdown";

export const Kind = { TEXT: 1, IMAGE: 2, VIDEO: 3, FILE: 4, AUDIO: 8, KMARKDOWN: 9, CARD: 10, ITEM: 12 } as const;

// 视频、文件、音频的附件。服务端给出单个对象（不是数组）。
export type Attachment = { name: string; url: string | null; size: number };

export function toAttachment(a: unknown): Attachment | undefined {
  const o = Array.isArray(a) ? a[0] : a;
  if (!o || typeof o !== "object") return undefined;
  const r = o as Record<string, unknown>;
  return { name: typeof r.name === "string" ? r.name : "", url: typeof r.url === "string" ? safeUrl(r.url) : null, size: Number(r.size) || 0 };
}

// 卡片中可显示的部分：标题与正文（KMarkdown 或纯文本）、图片。其余模块计入 other，用占位提示。
export type CardPart = { t: "header" | "text"; v: string; kmd: boolean } | { t: "image"; src: string; alt: string } | { t: "file"; name: string; url: string | null; size: number };
export type Card = { parts: CardPart[]; other: boolean };

const MAX_PARTS = 50;

export function parseCards(content: string): Card[] | null {
  let data: unknown;
  try {
    data = JSON.parse(content);
  } catch {
    return null;
  }
  if (!Array.isArray(data)) return null;
  return data.filter((c) => c && typeof c === "object").slice(0, 10).map((c) => {
    const card: Card = { parts: [], other: false };
    const add = (p: CardPart) => card.parts.length < MAX_PARTS && card.parts.push(p);
    const text = (el: any, t: "header" | "text"): boolean => {
      if (!el || typeof el !== "object") return false;
      if ((el.type === "kmarkdown" || el.type === "plain-text") && typeof el.content === "string") {
        add({ t, v: el.content, kmd: el.type === "kmarkdown" });
        return true;
      }
      if (el.type === "paragraph" && Array.isArray(el.fields)) {
        let ok = false;
        for (const f of el.fields) ok = text(f, t) || ok;
        return ok;
      }
      if (el.type === "image") return image(el);
      return false;
    };
    const image = (el: any): boolean => {
      const src = typeof el?.src === "string" ? safeUrl(el.src) : null;
      if (src) add({ t: "image", src, alt: typeof el.alt === "string" ? el.alt : "" });
      return Boolean(src);
    };
    for (const m of Array.isArray((c as any).modules) ? (c as any).modules : []) {
      if (!m || typeof m !== "object") continue;
      let ok = false;
      if (m.type === "header") ok = text(m.text, "header");
      else if (m.type === "section") ok = text(m.text, "text");
      else if (m.type === "context" && Array.isArray(m.elements)) for (const e of m.elements) ok = text(e, "text") || ok;
      else if ((m.type === "container" || m.type === "image-group") && Array.isArray(m.elements)) for (const e of m.elements) ok = image(e) || ok;
      else if (m.type === "divider") ok = true;
      // 发送文件（类型 4）后服务端转成的卡片：{type:"file", title, src, size}。
      else if (m.type === "file" && typeof m.src === "string") {
        add({ t: "file", name: typeof m.title === "string" ? m.title : "", url: safeUrl(m.src), size: Number(m.size) || 0 });
        ok = true;
      }
      if (!ok) card.other = true;
    }
    return card;
  });
}

const LABEL: Record<number, string> = { [Kind.IMAGE]: "[图片]", [Kind.VIDEO]: "[视频]", [Kind.FILE]: "[文件]", [Kind.AUDIO]: "[语音]", [Kind.CARD]: "[卡片消息]" };

export const isText = (type: number) => type === Kind.TEXT || type === Kind.KMARKDOWN || type === Kind.ITEM;

// 非文本消息的摘要，文本类型返回 null。
export function label(type: number, attachment?: Attachment): string | null {
  if (isText(type)) return null;
  const base = LABEL[type] ?? `[不支持的消息类型 ${type}]`;
  return attachment?.name && type !== Kind.IMAGE ? `${base} ${attachment.name}` : base;
}

export function sizeText(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
