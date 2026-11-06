import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount, type JSX } from "solid-js";
import { ArrowDown } from "lucide-solid";
import type { Focus, History, Message } from "../../stores/types";
import { indexAt, prefix, visibleRange } from "../../lib/virtual";
import MessageItem from "./MessageItem";
import { setUi } from "../../stores/ui";
import { readCurrent } from "../../stores/session";

// 窗口化消息列表：只渲染可视区附近的消息，高度按实测缓存（图片加载等变化由 ResizeObserver 更新）。
// 位于底部时跟随新消息；阅读旧消息时保持位置并提示新消息；上翻插入旧消息后保持阅读位置。
type Props = {
  messages: Message[];
  history: History | undefined;
  onOlder: () => void;
  onRetry: () => void;
  // 跳转片段之后还有消息时，滚到底部附近加载更新一页；onLatest 放弃片段回到最新。
  onNewer: () => void;
  onLatest: () => void;
  focus: Focus | null;
  // 列表之后的非虚拟内容（待发消息）。
  footer?: JSX.Element;
  // footer 内容变化的标记，例如待发条数。
  footerKey?: unknown;
};

const HEAD = 40; // 顶部状态行固定高度，避免状态切换影响锚点。
const OVERSCAN = 600;
const NEAR_TOP = 400;
const BOTTOM_SLACK = 8;
const estimate = (first: boolean) => (first ? 61 : 28);

export default function MessageList(p: Props) {
  let box!: HTMLDivElement;
  const heights = new Map<string, number>();
  const [version, setVersion] = createSignal(0);
  const [top, setTop] = createSignal(0);
  const [viewH, setViewH] = createSignal(0);
  const [atBottom, setAtBottom] = createSignal(true);
  // 用户离开底部时看到的最后一条消息，之后到达的计为新消息。
  const [seen, setSeen] = createSignal<string | undefined>();

  // 同作者且间隔五分钟内的连续消息合并。
  const isFirst = (i: number) => {
    const l = p.messages;
    return i === 0 || l[i - 1].author !== l[i].author || l[i].createAt - l[i - 1].createAt > 300_000;
  };
  const offsets = createMemo(() => {
    version();
    return prefix(p.messages.map((m, i) => heights.get(m.id) ?? estimate(isFirst(i))));
  });
  const range = createMemo(() => visibleRange(offsets(), top() - HEAD, viewH(), OVERSCAN));
  // 以消息对象为键，滚动时只增删进出窗口的行。
  const rows = createMemo(() => {
    const [s, e] = range();
    return p.messages.slice(s, e);
  });
  const unseen = createMemo(() => {
    if (atBottom()) return 0;
    const id = seen();
    const i = id ? p.messages.findIndex((m) => m.id === id) : -1;
    return i < 0 ? 0 : p.messages.length - 1 - i;
  });

  // 阅读锚点：视口顶部所在消息及其相对偏移。布局变化后据此还原位置。
  let anchor: { id: string; delta: number } | undefined;
  const capture = () => {
    const o = offsets();
    if (!p.messages.length) return (anchor = undefined);
    const y = box.scrollTop - HEAD;
    const i = indexAt(o, Math.max(0, y));
    anchor = { id: p.messages[i].id, delta: y - o[i] };
  };
  const bottomGap = () => box.scrollHeight - box.scrollTop - box.clientHeight;
  // 跳转片段的底部不是最新消息：不贴底、不标记已读。
  const fragment = () => Boolean(p.history?.newer);
  const [flash, setFlash] = createSignal<string | undefined>();
  // 程序设定的滚动位置。滚动事件异步派发，期间内容高度可能已变化，
  // 位置未变的滚动事件来自程序设定，不据此改变贴底状态。
  let expected = -1;
  const setScroll = (y: number) => {
    box.scrollTop = y;
    expected = box.scrollTop;
    setTop(expected);
  };
  const toBottom = () => setScroll(box.scrollHeight);

  const onScroll = () => {
    setTop(box.scrollTop);
    if (box.scrollTop === expected) return capture();
    expected = -1;
    const bottom = bottomGap() <= BOTTOM_SLACK && !fragment();
    if (bottom !== atBottom()) {
      setAtBottom(bottom);
      setUi("atBottom", bottom);
      // 滚动到底部即读到最新消息。
      if (bottom) readCurrent();
      if (!bottom) setSeen(p.messages[p.messages.length - 1]?.id);
    }
    capture();
    maybeOlder();
    maybeNewer();
  };

  const maybeNewer = () => {
    if (fragment() && p.messages.length && bottomGap() < NEAR_TOP) p.onNewer();
  };

  // 定位到指定消息并短暂高亮。未测高度的行先按估计值滚动，测量后锚点还原会修正位置。
  const scrollTo = (id: string) => {
    const i = p.messages.findIndex((m) => m.id === id);
    if (i < 0) return;
    setAtBottom(false);
    setUi("atBottom", false);
    setSeen(p.messages[p.messages.length - 1]?.id);
    const o = offsets();
    const h = o[i + 1] - o[i];
    setScroll(Math.max(0, o[i] + HEAD - Math.max(0, (box.clientHeight - h) / 2)));
    capture();
    setFlash(id);
    setTimeout(() => flash() === id && setFlash(undefined), 2000);
  };
  createEffect(on(() => p.focus, (f) => f && queueMicrotask(() => scrollTo(f.id)), { defer: true }));

  const maybeOlder = () => {
    const h = p.history;
    if (box.scrollTop < NEAR_TOP && h && !h.end && !h.loading && !h.failed && p.messages.length) p.onOlder();
  };

  // 布局变化（新消息、旧消息前插、高度更新）后：在底部则继续贴底，否则按锚点还原。
  const restore = () => {
    if (!anchor) return;
    const i = p.messages.findIndex((m) => m.id === anchor!.id);
    if (i < 0) return capture();
    const want = offsets()[i] + anchor.delta + HEAD;
    if (Math.abs(box.scrollTop - want) > 0.5) setScroll(want);
    else setTop(box.scrollTop);
  };
  createEffect(on(offsets, () => (atBottom() && !fragment() ? toBottom() : restore()), { defer: true }));

  // 待发消息等非虚拟内容变化时，在底部则继续贴底。
  createEffect(on(() => p.footerKey, () => atBottom() && queueMicrotask(toBottom), { defer: true }));

  // 内容不足一屏时无法滚动触发上翻，加载完成后再检查一次。
  createEffect(on(() => [p.history?.loading, p.history?.end, p.history?.newer, p.messages.length], () => queueMicrotask(() => (maybeOlder(), maybeNewer())), { defer: true }));

  const observer = new ResizeObserver((entries) => {
    let changed = false;
    for (const e of entries) {
      const el = e.target as HTMLElement;
      if (el === box) {
        // 视口变化（窗口缩放、输入框增高、回复栏出现）不触发滚动事件：贴底时保持贴底，否则按锚点还原。
        setViewH(box.clientHeight);
        if (atBottom()) toBottom();
        else restore();
        continue;
      }
      const id = el.dataset.id;
      if (!id || !el.isConnected) continue;
      const h = el.getBoundingClientRect().height;
      if (h > 0 && heights.get(id) !== h) {
        heights.set(id, h);
        changed = true;
      }
    }
    if (changed) setVersion((v) => v + 1);
  });
  onMount(() => {
    setUi("atBottom", true);
    setViewH(box.clientHeight);
    observer.observe(box);
    toBottom();
  });
  onCleanup(() => observer.disconnect());

  const measure = (el: HTMLDivElement) => {
    observer.observe(el);
    onCleanup(() => observer.unobserve(el));
  };

  const head = () => {
    const h = p.history;
    if (h?.failed) return <>历史消息加载失败 <button class="retry" onClick={() => (p.messages.length ? p.onOlder() : p.onRetry())}>重试</button></>;
    if (!h || h.loading) return "加载中";
    if (h.end) return p.messages.length ? "已到达频道开头" : "还没有消息，发送第一条吧";
    return "";
  };

  return (
    <div class="message-wrap">
      <div class="message-box" ref={box} onScroll={onScroll}>
        <div class="history-head" style={{ height: `${HEAD}px` }}>{head()}</div>
        <div class="vlist" style={{ height: `${offsets()[offsets().length - 1]}px` }}>
          <For each={rows()}>
            {(m, k) => {
              const i = () => range()[0] + k();
              return (
                <div class={`vrow ${flash() === m.id ? "flash" : ""}`} data-id={m.id} ref={measure} style={{ transform: `translateY(${offsets()[i()]}px)` }}>
                  <MessageItem msg={m} first={isFirst(i())} />
                </div>
              );
            }}
          </For>
        </div>
        {p.footer}
      </div>
      <Show when={fragment()}>
        <button class="new-tip" onClick={p.onLatest}>
          <ArrowDown size={14} /> 回到最新消息
        </button>
      </Show>
      <Show when={!fragment() && unseen() > 0}>
        <button class="new-tip" onClick={toBottom}>
          <ArrowDown size={14} /> {unseen()} 条新消息
        </button>
      </Show>
    </div>
  );
}
