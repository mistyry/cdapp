import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { Clock, Lock, Search, User } from "lucide-solid";
import { UNICODE_GROUPS, gridMove, search, unavailable, type Pick, type Purpose } from "../../lib/emoji";
import { emojiUrl } from "../../lib/kmarkdown";
import { allCustom, emoji, ensureEmojis, noteRecent, shownGuilds } from "../../stores/emoji";
import { session } from "../../stores/session";
import GuildIcon from "../common/GuildIcon";

// 表情选择器。发送与回应共用。页签为最近、个人、各服务器；不显示预览，名称以悬停提示显示。
// purpose 决定哪些表情可选，选择器本身不决定发送目标。
// 自定义表情只在可视区域附近加载图片（loading=lazy），分区按需渲染。
type Section = { key: string; title: string; icon?: string; items: Pick[] };

const CELL = 36;

function Glyph(p: { pick: Pick; size: number }) {
  const [broken, setBroken] = createSignal(false);
  return (
    <Show when={p.pick.kind === "custom" && p.pick} fallback={<span class="ep-char" style={{ "font-size": `${p.size - 6}px` }}>{(p.pick as { char: string }).char}</span>}>
      {(c) => (
        <Show when={!broken()} fallback={<span class="ep-broken">{c().name.slice(0, 2)}</span>}>
          <img src={emojiUrl(c().id)} alt="" width={p.size} height={p.size} loading="lazy" referrerPolicy="no-referrer" draggable={false} onError={() => setBroken(true)} />
        </Show>
      )}
    </Show>
  );
}

export default function EmojiPicker(p: { purpose: Purpose; onPick: (x: Pick) => void; onClose: (refocus: boolean) => void; class?: string; style?: Record<string, string>; trigger?: () => HTMLElement | undefined; onReflow?: () => void }) {
  let root!: HTMLDivElement;
  let input!: HTMLInputElement;
  let body!: HTMLDivElement;
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  // 列数取表情区实际可用宽度（扣除滚动条与内边距），网格与键盘导航使用同一列数。
  const [cols, setCols] = createSignal(8);

  onMount(() => {
    void ensureEmojis();
    input.focus();
    const fit = () => {
      const cs = getComputedStyle(body);
      const w = body.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
      setCols(Math.max(1, Math.floor(w / CELL)));
    };
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(body);
    onCleanup(() => ro.disconnect());
  });
  // 触发按钮自己负责开关，不按外部点击处理。
  const onDown = (e: PointerEvent) => {
    const t = e.target as Node;
    if (!root.contains(t) && !p.trigger?.()?.contains(t)) p.onClose(false);
  };
  // 固定定位的弹层：外部滚动或窗口缩放时由调用方重新定位。
  const onScroll = (e: Event) => {
    if (!root.contains(e.target as Node)) p.onReflow?.();
  };
  const onResize = () => p.onReflow?.();
  document.addEventListener("pointerdown", onDown, true);
  if (p.onReflow) {
    document.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onResize);
  }
  onCleanup(() => {
    document.removeEventListener("pointerdown", onDown, true);
    document.removeEventListener("scroll", onScroll, true);
    window.removeEventListener("resize", onResize);
  });

  // 页签：最近、个人、各服务器。“最近”页上方为最近使用，下方为按分组排列的 Unicode 表情。
  // 各分区分别缓存，记录最近使用时只有该分区重建，其余格子保持不变。
  const recentSec = createMemo<Section | null>(() => (emoji.recent.length ? { key: "recent", title: "最近使用", items: [...emoji.recent] } : null));
  const personalSec = createMemo<Section>(() =>
    ({ key: "personal", title: "个人表情", items: emoji.personal.map((x): Pick => ({ kind: "custom", ...x, guildId: "" })) }));
  const guildSecs = createMemo<Section[]>(() => shownGuilds(p.purpose.guildId)
    .map((g) => ({ key: `g:${g.guildId}`, title: g.name || "服务器表情", icon: g.icon, items: g.items.map((x): Pick => ({ kind: "custom", ...x, guildId: g.guildId })) })));
  const unicodeSecs = createMemo<Section[]>(() => UNICODE_GROUPS.map((grp) => ({
    key: `u:${grp.id}`, title: grp.name,
    items: emoji.unicode.filter((u) => u.group === grp.id).map((u): Pick => ({ kind: "unicode", char: u.char, name: u.name })),
  })).filter((x) => x.items.length));
  // 服务器页签使用与侧边栏相同的服务器名称与图标；不在服务器列表中时取表情接口提供的名称与图标。
  const guildOf = (gid: string, name: string, icon?: string) => {
    const g = session.guilds.find((x) => x.id === gid);
    return { name: g?.name ?? name, icon: g ? g.icon : icon };
  };
  // 整组不可用（例如在其他服务器或私信中）时页签加锁。
  const groupLocked = (sec: Section) => sec.items.length > 0 && sec.items.every((x) => unavailable(x, p.purpose));
  type Tab = { key: string; title: string; guild?: { name: string; icon?: string }; locked?: boolean };
  const tabs = createMemo<Tab[]>(() => [
    { key: "recent", title: "最近" },
    { key: "personal", title: "个人表情", locked: groupLocked(personalSec()) },
    ...guildSecs().map((g) => {
      const guild = guildOf(g.key.slice(2), g.title, g.icon);
      return { key: g.key, title: guild.name, guild, locked: groupLocked(g) };
    }),
  ]);
  const [tab, setTab] = createSignal("recent");
  // 所选页签消失（例如列表刷新）时回到“最近”。
  const current = () => (tabs().some((t) => t.key === tab()) ? tab() : "recent");
  const sections = createMemo<Section[]>(() => {
    const k = current();
    if (k === "recent") return [recentSec(), ...unicodeSecs()].filter((x): x is Section => Boolean(x));
    if (k === "personal") return [personalSec()];
    return guildSecs().filter((g) => g.key === k);
  });
  const switchTab = (k: string) => {
    setTab(k);
    setActive(0);
    body.scrollTop = 0;
  };
  const results = createMemo(() => (query().trim() ? search(emoji.unicode, allCustom(), query()) : null));
  // 键盘导航用的扁平列表。
  const flat = createMemo(() => results() ?? sections().flatMap((s) => s.items));

  const why = (x: Pick) => unavailable(x, p.purpose);
  const choose = (x: Pick) => {
    if (why(x)) return;
    noteRecent(x);
    p.onPick(x);
  };

  const focusCell = (i: number) => {
    const list = flat();
    if (!list.length) return;
    const n = Math.max(0, Math.min(list.length - 1, i));
    setActive(n);
    body.querySelector<HTMLElement>(`[data-i="${n}"]`)?.scrollIntoView({ block: "nearest" });
  };
  // 每个分区各自从第一列起排。上下移动到相邻行的同一列，跨分区时取目标行，越界时取该行最后一格。
  const bounds = () => (results() ? [{ start: 0, len: results()!.length }] : sections().map((x, i) => ({ start: offsets()[i], len: x.items.length })));
  const move = (i: number, d: number) => gridMove(bounds(), i, d, cols());
  const onKey = (e: KeyboardEvent) => {
    if (e.isComposing) return;
    const step: Record<string, number> = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: cols(), ArrowUp: -cols() };
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      p.onClose(true);
    } else if (e.key in step) {
      // 搜索框内左右键用于移动光标。
      if (e.target === input && (e.key === "ArrowLeft" || e.key === "ArrowRight") && query()) return;
      e.preventDefault();
      focusCell(move(active(), step[e.key]));
    } else if (e.key === "Enter") {
      const x = flat()[active()];
      if (x) {
        e.preventDefault();
        choose(x);
      }
    }
  };

  // 渲染单个表情格子。index 用于键盘高亮，跨分区连续编号。
  const cell = (x: Pick, i: number) => {
    const reason = why(x);
    const label = x.name;
    return (
      <button type="button" class="ep-cell" classList={{ active: active() === i, off: Boolean(reason) }} data-i={i}
        data-tip={label} aria-label={label} aria-disabled={Boolean(reason)}
        onMouseEnter={() => setActive(i)} onClick={() => choose(x)}>
        <Glyph pick={x} size={26} />
      </button>
    );
  };
  const offsets = createMemo(() => {
    let n = 0;
    return sections().map((s) => {
      const o = n;
      n += s.items.length;
      return o;
    });
  });

  return (
    <div ref={root} class={`emoji-picker ${p.class ?? ""}`} style={p.style} role="dialog" aria-label="表情" onKeyDown={onKey}>
      <div class="ep-search">
        <Search size={16} />
        <input ref={input} value={query()} placeholder="搜索表情" aria-label="搜索表情" onInput={(e) => (setQuery(e.currentTarget.value), setActive(0), body.scrollTop = 0)} />
      </div>
      <div ref={body} class="ep-body" style={{ "--ep-cell": `${CELL}px`, "--ep-cols": String(cols()) }}>
        <Show when={emoji.status === "failed"}>
          <div class="ep-note">自定义表情加载失败 <button class="retry" onClick={() => void ensureEmojis(true)}>重试</button></div>
        </Show>
        <Show when={emoji.personalFailed}><div class="ep-note">个人表情加载失败</div></Show>
        <Show when={emoji.unicodeStatus === "failed"}>
          <div class="ep-note">表情数据加载失败 <button class="retry" onClick={() => void ensureEmojis(true)}>重试</button></div>
        </Show>
        <Show when={results()} fallback={
          <>
            <For each={sections()}>
              {(s, si) => (
                <section data-sec={s.key}>
                  <h4>{s.title}</h4>
                  <div class="ep-grid"><For each={s.items}>{(x, i) => cell(x, offsets()[si()] + i())}</For></div>
                </section>
              )}
            </For>
            <Show when={current() === "personal" && emoji.status === "ready" && !emoji.personal.length && !emoji.personalFailed}>
              <div class="ep-note">还没有个人表情</div>
            </Show>
            <Show when={emoji.status === "loading" || emoji.unicodeStatus === "loading"}><div class="ep-note">加载中</div></Show>
          </>
        }>
          {(r) => (
            <Show when={r().length} fallback={<div class="ep-note">没有找到相关表情</div>}>
              <div class="ep-grid"><For each={r()}>{(x, i) => cell(x, i())}</For></div>
            </Show>
          )}
        </Show>
      </div>
      <Show when={!results()}>
        <nav class="ep-tabs" role="tablist" aria-label="表情分类">
          <For each={tabs()}>
            {(t) => (
              <button type="button" role="tab" class="icon-btn" classList={{ on: current() === t.key }} aria-selected={current() === t.key}
                data-tip={t.title} aria-label={t.title} tabIndex={-1} onClick={() => switchTab(t.key)}>
                {t.key === "recent" ? <Clock size={18} /> : t.key === "personal" ? <User size={18} />
                  : <GuildIcon name={t.guild!.name} icon={t.guild!.icon} size={22} />}
                <Show when={t.locked}><span class="ep-lock"><Lock size={10} stroke-width={2.5} /></span></Show>
              </button>
            )}
          </For>
        </nav>
      </Show>
    </div>
  );
}

