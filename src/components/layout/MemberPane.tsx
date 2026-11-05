import { createEffect, For, on, onCleanup, Show } from "solid-js";
import type { MemberDto } from "../../lib/api";
import { closeMembers, ensureMembers, openDmWith, loadMoreMembers, refreshMembers, session } from "../../stores/session";
import { ui } from "../../stores/ui";
import Avatar from "../common/Avatar";

const hex = (c: number) => (c ? `#${c.toString(16).padStart(6, "0")}` : undefined);

function Row(p: { m: MemberDto }) {
  // 显示服务器昵称；颜色取服务端计算的角色颜色。
  const user = () => ({ id: p.m.id, name: p.m.nickname || p.m.username || "未知用户", avatar: p.m.avatar || undefined, online: p.m.online });
  return (
    <div class={`member ${p.m.online ? "" : "offline"}`} title={`${user().name}#${p.m.identifyNum}（双击发私信）`}
      onDblClick={() => !p.m.bot && session.me?.id !== p.m.id && void openDmWith(p.m.id)}>
      <Avatar user={user()} size={32} status />
      <div class="info">
        <div class="nm" style={{ color: hex(p.m.color) }}>{user().name}<Show when={p.m.bot}><span class="bot-tag">机器人</span></Show></div>
      </div>
    </div>
  );
}

export default function MemberPane() {
  // 当前频道变化或面板打开时拉取；旧频道结果按 key 丢弃。
  // 只跟踪当前会话；ensureMembers 读取的成员状态不作为依赖，失败不会触发自动重试循环。
  createEffect(on(() => ui.target, () => void ensureMembers()));
  // 关闭成员栏时取消在线状态订阅。
  onCleanup(closeMembers);
  const list = () => session.members;
  const online = () => list()?.items.filter((m) => m.online) ?? [];
  const offline = () => list()?.items.filter((m) => !m.online) ?? [];
  const onScroll = (e: Event) => {
    const el = e.currentTarget as HTMLElement;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 200) void loadMoreMembers();
  };
  return (
    <aside class="members" onScroll={onScroll}>
      <Show when={list()}>
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
