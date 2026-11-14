import { Show, createSignal, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";

type Tip = { text: string; x: number; y: number; side: "top" | "bottom" | "right" };
const [tip, setTip] = createSignal<Tip | null>(null);

let keyboard = false;
const NAV_KEYS = new Set(["Tab", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown"]);
const setKeyboard = (on: boolean) => {
  keyboard = on;
  document.documentElement.toggleAttribute("data-keyboard", on);
};
const onKey = (e: KeyboardEvent) => { if (NAV_KEYS.has(e.key)) setKeyboard(true); };
const onPointer = () => setKeyboard(false);
document.addEventListener("keydown", onKey, true);
document.addEventListener("pointerdown", onPointer, true);
window.addEventListener("blur", onPointer);
export const keyboardFocus = (el: Element) => keyboard && el.matches(":focus-visible, :has(:focus-visible)");
const MARGIN = 8;
const GAP = 8;
const menuOpen = (el: Element) => el.closest('[aria-haspopup][aria-expanded="true"]') !== null;

export function TipLayer() {
  let anchor: HTMLElement | null = null;
  let box: HTMLDivElement | undefined;
  let sticky = false;
  let byFocus = false;

  const hide = () => {
    anchor = null;
    sticky = false;
    byFocus = false;
    setTip(null);
  };
  const show = (el: HTMLElement, key: "tip" | "focusTip" = "tip") => {
    if (el === anchor) return;
    anchor = el;
    sticky = key === "focusTip";
    byFocus = false;
    setTip(null);
    const text = el.dataset[key];
    if (!el.isConnected || !text || menuOpen(el)) return;
    const r = el.getBoundingClientRect();
    if (el.dataset.tipSide === "right") {
      setTip({ text, x: r.right + GAP, y: r.top + r.height / 2, side: "right" });
      return;
    }
    const below = r.top < 40;
    const cx = r.left + r.width / 2;
    setTip({ text, x: cx, y: below ? r.bottom + GAP : r.top - GAP, side: below ? "bottom" : "top" });
    queueMicrotask(() => {
      if (!box) return;
      const w = box.offsetWidth;
      const x = Math.min(Math.max(cx, MARGIN + w / 2), window.innerWidth - MARGIN - w / 2);
      box.style.left = `${x}px`;
      box.style.setProperty("--arrow", `${Math.min(Math.max(cx - x + w / 2, 8), w - 8)}px`);
    });
  };

  const tipOf = (n: EventTarget | null) => (n instanceof Element ? n.closest<HTMLElement>("[data-tip]") : null);
  const over = (e: PointerEvent) => {
    if (sticky) return;
    const el = tipOf(e.target);
    if (el) show(el);
    else if (anchor) hide();
  };
  const out = (e: Event) => {
    if (sticky && e.type !== "focusout") return;
    const to = (e as PointerEvent | FocusEvent).relatedTarget;
    if (anchor && !(to instanceof Node && anchor.contains(to))) hide();
  };
  const focusIn = (e: FocusEvent) => {
    const f = e.target instanceof HTMLElement && e.target.dataset.focusTip ? e.target : null;
    if (f) return show(f, "focusTip");
    const el = tipOf(e.target);
    if (el && keyboardFocus(el)) {
      show(el);
      byFocus = true;
    }
  };
  const pointerMove = (e: PointerEvent) => {
    if (byFocus && anchor && !(e.target instanceof Node && anchor.contains(e.target))) hide();
  };
  let frame = 0;
  const move = (e: MouseEvent) => {
    if (frame || !(e.target instanceof Element) || !e.target.querySelector("button:disabled[data-tip]")) return;
    const { clientX, clientY } = e;
    frame = requestAnimationFrame(() => {
      frame = 0;
      const el = tipOf(document.elementFromPoint(clientX, clientY));
      if (el && (el as HTMLButtonElement).disabled) show(el);
    });
  };
  const scroll = (e: Event) => {
    if (anchor && (e.target === document || (e.target instanceof Node && e.target.contains(anchor)))) hide();
  };
  const mo = new MutationObserver(() => {
    if (anchor && (!anchor.isConnected || !anchor.checkVisibility() || menuOpen(anchor))) hide();
  });
  mo.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["aria-expanded"] });

  document.addEventListener("pointerover", over);
  document.addEventListener("pointermove", pointerMove, { passive: true });
  window.addEventListener("blur", hide);
  document.addEventListener("pointerout", out);
  document.addEventListener("focusin", focusIn);
  document.addEventListener("focusout", out);
  document.addEventListener("mousemove", move, { passive: true });
  document.addEventListener("scroll", scroll, true);
  document.addEventListener("pointerdown", hide, true);
  document.addEventListener("keydown", hide, true);
  onCleanup(() => {
    mo.disconnect();
    cancelAnimationFrame(frame);
    document.removeEventListener("pointerover", over);
    document.removeEventListener("pointermove", pointerMove);
    window.removeEventListener("blur", hide);
    document.removeEventListener("pointerout", out);
    document.removeEventListener("focusin", focusIn);
    document.removeEventListener("focusout", out);
    document.removeEventListener("mousemove", move);
    document.removeEventListener("scroll", scroll, true);
    document.removeEventListener("pointerdown", hide, true);
    document.removeEventListener("keydown", hide, true);
  });
  return (
    <Show when={tip()}>
      {(t) => (
        <Portal>
          <div ref={box} class="tip" classList={{ below: t().side === "bottom", right: t().side === "right" }} aria-hidden="true" style={{ left: `${t().x}px`, top: `${t().y}px` }}>{t().text}</div>
        </Portal>
      )}
    </Show>
  );
}
