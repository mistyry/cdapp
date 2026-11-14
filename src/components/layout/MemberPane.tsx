import { createEffect, For, on, onCleanup, Show } from "solid-js";
import type { MemberDto } from "../../lib/api";
import { closeMembers, ensureMembers, openDmWith, loadMoreMembers, refreshMembers, session } from "../../stores/session";
import { ui } from "../../stores/ui";
import Avatar from "../common/Avatar";
import { MemberSkeleton } from "../common/Skeleton";

const hex = (c: number) => (c ? `#${c.toString(16).padStart(6, "0")}` : undefined);

function Row(p: { m: MemberDto }) {
  const user = () => ({ id: p.m.id, name: p.m.nickname || p.m.username || "未知用户", avatar: p.m.avatar || undefined, online: p.m.online });
  return (
    <div class={`member ${p.m.online ? "" : "offline"}`} data-tip={`${user().name}#${p.m.identifyNum}`}
      onDblClick={() => !p.m.bot && session.me?.id !== p.m.id && void openDmWith(p.m.id)}>
      <Avatar user={user()} size={32} status />
      <div class="info">
        <div class="nm" style={{ color: hex(p.m.color) }}>{user().name}<Show when={p.m.bot}><span class="bot-tag">机器人</span></Show></div>
      </div>
    </div>
  );
}

export default function MemberPane() {
  createEffect(on(() => ui.target, () => void ensureMembers()));
  onCleanup(closeMembers);
  const list = () => session.members;
  const online = () => list()?.items.filter((m) => m.online) ?? [];
  const offline = () => list()?.items.filter((m) => !m.online) ?? [];
  const onScroll = (e: Event) => {
    const el = e.currentTarget as HTMLElement;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 200) void loadMoreMembers();
  };
  const pending = () => !list() || (!list()!.items.length && !list()!.failed && (list()!.loading || list()!.page === 0));
  return (
    <aside class="members" onScroll={onScroll}>
      <Show when={!pending() && list()} fallback={<MemberSkeleton />}>
        {(l) => (
          <>
            <Show when={online().length}>
              <div class="member-title">在线 - {l().online}</div>
              <For each={online()}>{(m) => <Row m={m} />}</For>
            </Show>
            <Show when={offline().length}>
              <div class="member-title">离线 - {l().offline}</div>
              <For each={offline()}>{(m) => <Row m={m} />}</For>
            </Show>
            <Show when={l().loading}><div class="member-note">加载中</div></Show>
            <Show when={l().failed}>
              <div class="member-note">成员加载失败 <button class="retry" onClick={() => refreshMembers()}>重试</button></div>
            </Show>
            <Show when={!l().loading && !l().failed && !l().items.length && l().page > 0}><div class="member-note">没有成员</div></Show>
          </>
        )}
      </Show>
    </aside>
  );
}
