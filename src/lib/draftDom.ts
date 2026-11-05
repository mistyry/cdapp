// 输入框 DOM 与草稿文本的互相转换。草稿是 KMarkdown 文本，自定义表情显示为带 ID 的图片单元。
import { emojiMarkup, tokenize } from "./emoji";
import { emojiUrl } from "./kmarkdown";

// 把草稿渲染为 DOM 节点。
export function render(el: HTMLElement, src: string, root = true) {
  el.replaceChildren();
  for (const t of tokenize(src)) {
    if (t.t === "emj") el.append(emojiNode(t.name, t.id));
    else {
      const lines = t.v.split("\n");
      lines.forEach((l, i) => {
        if (i) el.append(document.createElement("br"));
        if (l) el.append(document.createTextNode(l));
      });
    }
  }
  // 末尾换行需要额外的 br 才能显示空行。
  if (root && src.endsWith("\n")) el.append(document.createElement("br"));
}

export function emojiNode(name: string, id: string): HTMLImageElement {
  const img = document.createElement("img");
  img.className = "emoji";
  img.src = emojiUrl(id);
  img.alt = name;
  img.dataset.tip = name;
  img.draggable = false;
  img.referrerPolicy = "no-referrer";
  img.contentEditable = "false";
  img.dataset.id = id;
  img.dataset.name = name;
  // 失效资源显示名称，单元仍保留 ID。
  img.onerror = () => {
    img.onerror = null;
    img.removeAttribute("src");
    img.classList.add("broken");
  };
  return img;
}

// 从 DOM 读出草稿。块级元素（浏览器回车可能生成 div/p）之间补换行。
export function read(el: Node): string {
  let out = "";
  const walk = (n: Node) => {
    for (const c of n.childNodes) {
      if (c.nodeType === Node.TEXT_NODE) out += (c.textContent ?? "").replace(/​/g, "");
      else if (c instanceof HTMLImageElement && c.dataset.id) out += emojiMarkup(c.dataset.name ?? "", c.dataset.id);
      else if (c instanceof HTMLBRElement) out += "\n";
      else if (c instanceof HTMLElement) {
        const block = c.tagName === "DIV" || c.tagName === "P";
        if (block && out && !out.endsWith("\n")) out += "\n";
        walk(c);
      }
    }
  };
  walk(el);
  // 渲染时为显示空行追加的末尾 br 不计入内容。
  const last = el.lastChild;
  if (last instanceof HTMLBRElement && out.endsWith("\n")) out = out.slice(0, -1);
  return out;
}
