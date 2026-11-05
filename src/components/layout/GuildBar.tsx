import { For, Show } from "solid-js";
import { MessageCircleMore, Plus } from "lucide-solid";
import { DM_VIEW, ui, setUi } from "../../stores/ui";
import { dmUnreadTotal, guildUnread, openDm, selectGuild, session } from "../../stores/session";
import Avatar from "../common/Avatar";
import GuildIcon from "../common/GuildIcon";

// 可点击的导航项以按钮语义暴露给键盘与读屏，Enter 与空格触发。
const press = (f: () => void) => ({
  role: "button" as const, tabIndex: 0,
  // 点击仍只在图标上生效，与原来一致；键盘在整项上触发。
  onClick: (e: MouseEvent) => (e.target as Element).closest(".guild-btn, .me-entry") && f(),
  onKeyDown: (e: KeyboardEvent) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), f()),
});

export default function GuildBar() {
  return (
    <nav class="guildbar">
      <div class="guild-scroll">
        <div class={`guild-item ${ui.view === DM_VIEW ? "active" : ""}`} data-tip-side="right" data-tip="私信" aria-label="私信" aria-current={ui.view === DM_VIEW} {...press(openDm)}>
          <div class="guild-btn"><MessageCircleMore size={28} /></div>
          <span class="ring" />
          <Show when={dmUnreadTotal()}>{(n) => <span class="badge">{n() > 99 ? "99+" : n()}</span>}</Show>
        </div>
        <div class="guild-line" />
        <For each={session.guilds}>
          {(g) => {
            const u = () => guildUnread(g.id);
            return (
            <div class={`guild-item ${ui.view === g.id ? "active" : ""} ${u().unread ? "unread" : ""}`} data-tip-side="right" data-tip={g.name} aria-label={u().mentions ? `${g.name}，${u().mentions} 条提及` : g.name} aria-current={ui.view === g.id} {...press(() => void selectGuild(g.id))}>
              <GuildIcon name={g.name} icon={g.icon} />
              <span class="ring" />
              <Show when={u().mentions}><span class="badge">{u().mentions > 99 ? "99+" : u().mentions}</span></Show>
            </div>
            );
          }}
        </For>
        <div class="guild-item" data-tip-side="right" data-tip="创建或加入服务器" role="img" aria-label="创建或加入服务器">
          <div class="guild-btn"><Plus size={28} /></div>
        </div>
      </div>
      <div class="guildbar-bottom">
        <div class="guild-line" />
        <Show when={session.me}>{(me) => <div class="me-entry" data-tip-side="right" data-tip="设置" aria-label="设置" {...press(() => setUi("settings", "account"))}><Avatar user={me()} size={48} status /></div>}</Show>
      </div>
    </nav>
  );
}
