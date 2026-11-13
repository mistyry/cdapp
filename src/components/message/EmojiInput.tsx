import { createEffect, on, onCleanup, onMount } from "solid-js";
import { insertText, type Pick } from "../../lib/emoji";
import { emojiNode, read, render } from "../../lib/draftDom";

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
  let own: string | null = null;
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
    if (last instanceof HTMLBRElement && el.lastChild === last) el.append(document.createElement("br"));
    if (last) {
      r.setStartAfter(last);
      r.collapse(true);
      s.removeAllRanges();
      s.addRange(r);
    }
    sync();
  };

  const onCopy = (e: ClipboardEvent, cut: boolean) => {
    const s = getSelection();
    if (!s?.rangeCount || s.isCollapsed || !el.contains(s.anchorNode)) return;
    if (cut && p.disabled) return void e.preventDefault();
    const box = document.createElement("div");
    box.append(s.getRangeAt(0).cloneContents());
    const text = read(box);
    e.clipboardData?.setData("text/plain", text);
    e.clipboardData?.setData("text/html", box.innerHTML);
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
