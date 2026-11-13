import { emojiMarkup, tokenize } from "./emoji";
import { emojiUrl } from "./kmarkdown";

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
  img.dataset.id = id;
  img.dataset.name = name;
  img.onerror = () => {
    img.onerror = null;
    img.src = nameImage(name);
    img.classList.add("broken");
  };
  return img;
}

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
  const last = el.lastChild;
  if (last instanceof HTMLBRElement && out.endsWith("\n")) out = out.slice(0, -1);
  return out;
}

function nameImage(name: string): string {
  const w = Math.max(20, [...name].length * 14 + 4);
  const esc = name.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="20"><text x="2" y="15" font-size="14" fill="#8f959e" font-family="sans-serif">${esc}</text></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}
