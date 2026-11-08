import { expect, test } from "bun:test";
import { Window } from "happy-dom";

const w = new Window();
Object.assign(globalThis, { window: w, document: w.document, Node: w.Node, HTMLElement: w.HTMLElement, HTMLImageElement: w.HTMLImageElement, HTMLBRElement: w.HTMLBRElement });
const { emojiNode, read, render } = await import("../draftDom");

const box = () => document.createElement("div") as unknown as HTMLElement;
const E = "(emj)笑(emj)[100/abc]";

test("渲染与读取往返，含换行、末尾空行与表情", () => {
  for (const s of ["", "a", "a\nb", "a\n", "a\n\n", `${E}`, `x${E}y\n${E}`, `\n${E}\n`]) {
    const el = box();
    render(el, s);
    expect(read(el)).toBe(s);
  }
});

test("表情渲染为带 ID 的图片单元", () => {
  const el = box();
  render(el, `a${E}`);
  const img = el.querySelector("img")!;
  expect(img.dataset.id).toBe("100/abc");
  expect(img.getAttribute("referrerpolicy")).toBe("no-referrer");
});

test("粘贴片段末尾换行不额外加空行", () => {
  const el = box();
  render(el, "xy");
  const frag = box();
  render(frag, "foo\n", false);
  const y = el.lastChild!;
  (y as Text).splitText(1);
  el.insertBefore(frag.childNodes[1], el.lastChild);
  el.insertBefore(frag.childNodes[0], el.childNodes[1]);
  expect(read(el)).toBe("xfoo\ny");
});

test("浏览器生成的 div 块按换行读取", () => {
  const el = box();
  el.innerHTML = "a<div>b</div><div>c</div>";
  expect(read(el)).toBe("a\nb\nc");
});

test("末尾粘贴以换行结尾的文本，补占位后换行不丢失", () => {
  const el = box();
  render(el, "abc");
  const frag = box();
  render(frag, "x\n", false);
  const last = frag.lastChild!;
  el.append(...frag.childNodes);
  if (el.lastChild === last) el.append(document.createElement("br"));
  expect(read(el)).toBe("abcx\n");
});

test("表情单元可编辑，部分选区与删除按单个字符处理", () => {
  const el = box();
  render(el, `a${E}(emj)笑(emj)[100/def]b`);
  const imgs = el.querySelectorAll("img");
  expect(imgs[0].getAttribute("contenteditable")).toBeNull();
  const r = document.createRange();
  r.setStartBefore(imgs[0]);
  r.setEndAfter(imgs[0]);
  const part = box();
  part.append(r.cloneContents());
  expect(read(part)).toBe(E);
  r.deleteContents();
  expect(read(el)).toBe("a(emj)笑(emj)[100/def]b");
});

test("资源失效后仍按原表情读取", () => {
  const el = box();
  const img = emojiNode("笑", "100/abc");
  el.append(img);
  img.onerror?.(new Event("error") as never);
  expect(img.classList.contains("broken")).toBe(true);
  expect(read(el)).toBe(E);
});
