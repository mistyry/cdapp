import { Show, createSignal, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";

// 悬停提示。全应用以 data-tip 声明，由此统一显示，不使用浏览器的 title 与 alt 弹出。
// 事件委托：指针悬停或键盘聚焦（:focus-visible）时显示，全局只有一个提示层。
// 提示只补充视觉说明，语义名称由元素自身的 aria-label 等提供。
// data-focus-tip 在元素获得焦点时显示，单击聚焦也会显示，用于提示下一步操作。
// 显示后不随指针移开消失，焦点离开或点击其他位置时隐藏。
// side 为 right 时显示在目标右侧并垂直居中（服务器栏），否则显示在上方，空间不足时在下方。
type Tip = { text: string; x: number; y: number; side: "top" | "bottom" | "right" };
const [tip, setTip] = createSignal<Tip | null>(null);
const MARGIN = 8;

export function TipLayer() {
  let anchor: HTMLElement | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let box: HTMLDivElement | undefined;
  // 当前提示由焦点触发，指针移动不影响。
  let sticky = false;

  const hide = () => {
    clearTimeout(timer);
    anchor = null;
    sticky = false;
    setTip(null);
  };
  const show = (el: HTMLElement, key: "tip" | "focusTip" = "tip") => {
    if (el === anchor) return;
    clearTimeout(timer);
    anchor = el;
    sticky = key === "focusTip";
    setTip(null);
    timer = setTimeout(() => {
      // 文本在显示时读取，延迟期间的变化不会显示旧内容。
      const text = el.dataset[key];
      if (anchor !== el || !el.isConnected || !text) return;
      const r = el.getBoundingClientRect();
      if (el.dataset.tipSide === "right") {
        setTip({ text, x: r.right + 8, y: r.top + r.height / 2, side: "right" });
        return;
      }
      const below = r.top < 40;
      setTip({ text, x: r.left + r.width / 2, y: below ? r.bottom + 6 : r.top - 6, side: below ? "bottom" : "top" });
      // 渲染后按实际宽度限制在窗口内。
      queueMicrotask(() => {
        if (!box) return;
        const w = box.offsetWidth;
        const x = Math.min(Math.max(r.left + r.width / 2, MARGIN + w / 2), window.innerWidth - MARGIN - w / 2);
        box.style.left = `${x}px`;
      });
    }, 250);
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
  // data-tip 只响应键盘聚焦。程序调用 focus() 归还焦点时不弹出提示。
  const focusIn = (e: FocusEvent) => {
    const f = e.target instanceof HTMLElement && e.target.dataset.focusTip ? e.target : null;
    if (f) return show(f, "focusTip");
    const el = tipOf(e.target);
    if (el && el.matches(":focus-visible, :has(:focus-visible)")) show(el);
  };
  // 禁用的按钮不派发指针事件。指针停在其父元素上时检查一次命中目标，按帧节流。
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
  // 只有包含提示目标的区域滚动时才隐藏，其他列表自动滚动不影响。
  const scroll = (e: Event) => {
    if (anchor && (e.target === document || (e.target instanceof Node && e.target.contains(anchor)))) hide();
  };
  // 目标被移除（弹层关闭、列表重绘）时不会触发 pointerout。
  const mo = new MutationObserver(() => {
    if (anchor && !anchor.isConnected) hide();
  });
  mo.observe(document.body, { childList: true, subtree: true });

  document.addEventListener("pointerover", over);
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
    clearTimeout(timer);
    document.removeEventListener("pointerover", over);
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
