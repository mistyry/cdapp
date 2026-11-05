import { createEffect, on, onCleanup, onMount } from "solid-js";
import { insertText, type Pick } from "../../lib/emoji";
import { emojiNode, read, render } from "../../lib/draftDom";

// 图形化输入框。草稿以 KMarkdown 文本为唯一来源：自定义表情显示为不可编辑的图片单元，Unicode 与其他文字为普通文本。
// 只读取 DOM 中的文本、换行与表情单元，粘贴一律按纯文本插入，不发送编辑器 DOM 或 HTML。

export type EmojiInputApi = { insert: (p: Pick) => void; focus: () => void };

export default function EmojiInput(p: {
  value: string;
  disabled: boolean;
  placeholder: string;
  onInput: (v: string) => void;
  onKeyDown: (e: KeyboardEvent) => void;
  onPaste: (e: ClipboardEvent) => void;
  ref?: (api: EmojiInputApi) => void;
}) {
  let el!: HTMLDivElement;
  // 自身输入产生的值，不再回写 DOM，避免打断光标与输入法。
  let own: string | null = null;
  // 最近一次光标位置。点击选择器后输入框失焦，插入时恢复到原位置。
  let saved: Range | null = null;

  const sync = () => {
    const v = read(el);
    own = v;
    p.onInput(v);
  };
  createEffect(on(() => p.value, (v) => {
    if (v === own) return;
    own = v;
    render(el, v);
    saved = null;
  }));

  const remember = () => {
    const s = getSelection();
    if (s?.rangeCount && el.contains(s.anchorNode)) saved = s.getRangeAt(0).cloneRange();
  };
  onMount(() => {
    document.addEventListener("selectionchange", remember);
    p.ref?.({
      focus: () => el.focus(),
      insert: (pick) => {
        if (p.disabled) return;
        el.focus();
        const s = getSelection()!;
        let r = saved && el.contains(saved.startContainer) ? saved : null;
        if (!r) {
          r = document.createRange();
          r.selectNodeContents(el);
          r.collapse(false);
          // 末尾 br 是显示空行的占位，插入到它之前。
          if (el.lastChild instanceof HTMLBRElement) {
            r.setStartBefore(el.lastChild);
            r.collapse(true);
          }
        }
        r.deleteContents();
        const node = pick.kind === "unicode" ? document.createTextNode(insertText(pick)) : emojiNode(pick.name, pick.id);
        r.insertNode(node);
        r.setStartAfter(node);
        r.collapse(true);
        s.removeAllRanges();
        s.addRange(r);
        saved = r.cloneRange();
        sync();
      },
    });
  });
  onCleanup(() => document.removeEventListener("selectionchange", remember));

  const onPaste = (e: ClipboardEvent) => {
    p.onPaste(e);
    if (e.defaultPrevented) return;
    e.preventDefault();
    const text = e.clipboardData?.getData("text/plain") ?? "";
    if (!text) return;
    // 粘贴文本中的表情标记同样显示为图形。
    const s = getSelection();
    if (!s?.rangeCount) return;
    const r = s.getRangeAt(0);
    r.deleteContents();
    const frag = document.createElement("div");
    render(frag, text, false);
    const last = frag.lastChild;
    const nodes = document.createDocumentFragment();
    nodes.append(...frag.childNodes);
    r.insertNode(nodes);
    // 粘贴内容以换行结尾且位于末尾时，补上显示空行的占位 br，否则读取时会把这个换行当作占位去掉。
    if (last instanceof HTMLBRElement && el.lastChild === last) el.append(document.createElement("br"));
    if (last) {
      r.setStartAfter(last);
      r.collapse(true);
      s.removeAllRanges();
      s.addRange(r);
    }
    sync();
  };

  // 复制时输出标记文本，便于粘贴回输入框后保持表情。
  const onCopy = (e: ClipboardEvent, cut: boolean) => {
    const s = getSelection();
    if (!s?.rangeCount || s.isCollapsed || !el.contains(s.anchorNode)) return;
    if (cut && p.disabled) return void e.preventDefault();
    const box = document.createElement("div");
    box.append(s.getRangeAt(0).cloneContents());
    e.clipboardData?.setData("text/plain", read(box));
    e.preventDefault();
    if (cut) {
      s.getRangeAt(0).deleteContents();
      sync();
    }
  };

  return (
    <div
      ref={el}
      class="composer-input"
      classList={{ empty: !p.value, disabled: p.disabled }}
      contentEditable={!p.disabled}
      role="textbox"
      aria-multiline="true"
      aria-disabled={p.disabled}
      aria-placeholder={p.placeholder}
      data-placeholder={p.placeholder}
      spellcheck={false}
      onInput={(e) => { if (!(e as InputEvent).isComposing) sync(); }}
      onCompositionEnd={sync}
      onKeyDown={p.onKeyDown}
      onPaste={onPaste}
      onCopy={(e) => onCopy(e, false)}
      onCut={(e) => onCopy(e, true)}
      onDrop={(e) => { if ([...(e.dataTransfer?.types ?? [])].includes("text/html")) e.preventDefault(); }}
    />
  );
}
