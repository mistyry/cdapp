import { For, Show } from "solid-js";
import { MessageCircleMore, Plus } from "lucide-solid";
import { DM_VIEW, ui, setUi } from "../../stores/ui";
import { dmUnreadTotal, guildUnread, openDm, selectGuild, session } from "../../stores/session";
import Avatar from "../common/Avatar";

export default function GuildBar() {
  return (
    <nav class="guildbar">
      <div class="guild-scroll">
        <div class={`guild-item ${ui.view === DM_VIEW ? "active" : ""}`} title="私信">
          <div class="guild-btn" onClick={openDm}><MessageCircleMore size={28} /></div>
          <span class="ring" />
          <Show when={dmUnreadTotal()}>{(n) => <span class="badge">{n() > 99 ? "99+" : n()}</span>}</Show>
        </div>
        <div class="guild-line" />
        <For each={session.guilds}>
          {(g) => {
            const u = () => guildUnread(g.id);
            return (
            <div class={`guild-item ${ui.view === g.id ? "active" : ""} ${u().unread ? "unread" : ""}`} title={g.name}>
              <div class="guild-btn" onClick={() => selectGuild(g.id)}>
                <Show when={g.icon} fallback={g.name.slice(0, 2)}>{(src) => <img src={src()} alt="" draggable={false} />}</Show>
              </div>
              <span class="ring" />
              <Show when={u().mentions}><span class="badge">{u().mentions > 99 ? "99+" : u().mentions}</span></Show>
            </div>
            );
          }}
        </For>
        <div class="guild-item" title="创建或加入服务器">
          <div class="guild-btn"><Plus size={28} /></div>
        </div>
      </div>
      <div class="guildbar-bottom">
        <div class="guild-line" />
        <Show when={session.me}>{(me) => <div class="me-entry" title="设置" onClick={() => setUi("settings", "account")}><Avatar user={me()} size={48} status /></div>}</Show>
      </div>
    </nav>
  );
}
