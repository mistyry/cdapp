import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount, type JSX } from "solid-js";
import { ArrowDown } from "lucide-solid";
import type { Focus, History, Message } from "../../stores/types";
import { indexAt, prefix, visibleRange } from "../../lib/virtual";
import MessageItem from "./MessageItem";
import { setUi } from "../../stores/ui";
import { readCurrent } from "../../stores/session";

type Props = {
  messages: Message[];
  history: History | undefined;
  onOlder: () => void;
  onRetry: () => void;
  onNewer: () => void;
  onLatest: () => void;
  focus: Focus | null;
  footer?: JSX.Element;
  footerKey?: unknown;
  dm?: boolean;
};

const HEAD = 40;
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
  const [seen, setSeen] = createSignal<string | undefined>();

  const isFirst = (i: number) => {
    const l = p.messages;
    const sys = (m: Message) => Boolean(m.system) && !p.dm;
    if (sys(l[i])) return false;
    return i === 0 || sys(l[i - 1]) || l[i - 1].author !== l[i].author || l[i].createAt - l[i - 1].createAt > 300_000;
  };
  const offsets = createMemo(() => {
    version();
    return prefix(p.messages.map((m, i) => heights.get(m.id) ?? estimate(isFirst(i))));
  });
  const range = createMemo(() => visibleRange(offsets(), top() - HEAD, viewH(), OVERSCAN));
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

  let anchor: { id: string; delta: number } | undefined;
  const capture = () => {
    const o = offsets();
    if (!p.messages.length) return (anchor = undefined);
    const y = box.scrollTop - HEAD;
    const i = indexAt(o, Math.max(0, y));
    anchor = { id: p.messages[i].id, delta: y - o[i] };
  };
  const bottomGap = () => box.scrollHeight - box.scrollTop - box.clientHeight;
  const fragment = () => Boolean(p.history?.newer);
  const [flash, setFlash] = createSignal<string | undefined>();
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

  const restore = () => {
    if (!anchor) return;
    const i = p.messages.findIndex((m) => m.id === anchor!.id);
    if (i < 0) return capture();
    const want = offsets()[i] + anchor.delta + HEAD;
    if (Math.abs(box.scrollTop - want) > 0.5) setScroll(want);
    else setTop(box.scrollTop);
  };
  createEffect(on(offsets, () => (atBottom() && !fragment() ? toBottom() : restore()), { defer: true }));

  createEffect(on(() => p.footerKey, () => atBottom() && queueMicrotask(toBottom), { defer: true }));

  createEffect(on(() => [p.history?.loading, p.history?.end, p.history?.newer, p.messages.length], () => queueMicrotask(() => (maybeOlder(), maybeNewer())), { defer: true }));

  const observer = new ResizeObserver((entries) => {
    let changed = false;
    for (const e of entries) {
      const el = e.target as HTMLElement;
      if (el === box) {
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
