import { For, Match, Show, Switch, createEffect, createMemo, createSignal } from "solid-js";
import { Folder, MessageCircleMore, Plus, RotateCw } from "lucide-solid";
import { DM_VIEW, ui, setUi } from "../../stores/ui";
import { dmUnreadTotal, guildOfAny, guildUnread, loadFolders, openDm, selectGuild, session } from "../../stores/session";
import { voice, voiceActive } from "../../stores/voice";
import { sharing } from "../../stores/share";
import { buildRail, folderTint, readOpen, writeOpen } from "../../lib/guildFolders";
import { GuildScreenMark, GuildVoiceMark } from "../common/icons";
import Avatar from "../common/Avatar";
import GuildIcon from "../common/GuildIcon";

// 可点击的导航项以按钮语义暴露给键盘与读屏，Enter 与空格触发。
const press = (f: () => void, hit = ".guild-btn, .me-entry, .folder-btn") => ({
  role: "button" as const, tabIndex: 0,
  // 点击只在图标上生效；键盘在整项上触发。
  onClick: (e: MouseEvent) => (e.target as Element).closest(hit) && f(),
  onKeyDown: (e: KeyboardEvent) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), f()),
});

const count = (n: number) => (n > 99 ? "99+" : n);

// 右下角状态，与官方优先级一致：本人分享、本人语音、他人分享。
type Mark = "self-share" | "voice" | "screen" | null;
function guildMark(ids: string[]): Mark {
  const vg = voiceActive() ? guildOfAny(voice.channelId)?.id ?? "" : "";
  if (vg && ids.includes(vg)) return sharing() ? "self-share" : "voice";
  return ids.some((id) => (session.guilds.find((g) => g.id === id)?.liveCount ?? 0) > 0) ? "screen" : null;
}
const MARK_NAME = { "self-share": "正在屏幕分享", voice: "正在语音", screen: "有人在屏幕分享" } as const;

function StatusMark(p: { mark: Mark }) {
  return (
    <Show when={p.mark}>
      {(m) => <span class={`guild-mark ${m()}`} role="img" aria-label={MARK_NAME[m()]}>{m() === "voice" ? <GuildVoiceMark /> : <GuildScreenMark />}</span>}
    </Show>
  );
}

function GuildEntry(p: { id: string }) {
  const g = () => session.guilds.find((x) => x.id === p.id);
  const u = () => guildUnread(p.id);
  return (
    <Show when={g()}>
      {(g) => (
        <div class={`guild-item ${ui.view === p.id ? "active" : ""} ${u().unread ? "unread" : ""}`} data-tip-side="right" data-tip={g().name} aria-label={u().mentions ? `${g().name}，${u().mentions} 条提及` : g().name} aria-current={ui.view === p.id} {...press(() => void selectGuild(p.id))}>
          <GuildIcon name={g().name} icon={g().icon} />
          <span class="ring" />
          <StatusMark mark={guildMark([p.id])} />
          <Show when={u().mentions}><span class="badge">{count(u().mentions)}</span></Show>
        </div>
      )}
    </Show>
  );
}

export default function GuildBar() {
  const rail = createMemo(() => buildRail(session.guilds.map((g) => g.id), session.folders));
  // 展开的文件夹按账号保存；切换账号时重新读取。
  const [open, setOpen] = createSignal<string[]>([]);
  let owner = "";
  createEffect(() => {
    const me = session.me?.id ?? "";
    if (me === owner) return;
    owner = me;
    setOpen([]);
    if (me) void readOpen(me).then((ids) => owner === me && setOpen(ids));
  });
  const toggle = (id: string) => {
    const next = open().includes(id) ? open().filter((x) => x !== id) : [...open(), id];
    setOpen(next);
    if (owner) writeOpen(owner, next);
  };

  return (
    <nav class="guildbar">
      <div class="guild-scroll">
        <div class={`guild-item ${ui.view === DM_VIEW ? "active" : ""}`} data-tip-side="right" data-tip="私信" aria-label="私信" aria-current={ui.view === DM_VIEW} {...press(openDm)}>
          <div class="guild-btn"><MessageCircleMore size={28} /></div>
          <span class="ring" />
          <Show when={dmUnreadTotal()}>{(n) => <span class="badge">{count(n())}</span>}</Show>
        </div>
        <div class="guild-line" />
        <For each={rail()}>
          {(item) => (
            <Switch>
              <Match when={item.kind === "guild" && item}>{(g) => <GuildEntry id={g().id} />}</Match>
              <Match when={item.kind === "folder" && item}>
                {(f) => {
                  const expanded = () => open().includes(f().id);
                  const name = () => f().name || "未命名文件夹";
                  const sum = () => f().guildIds.reduce((a, id) => { const u = guildUnread(id); return { unread: a.unread || u.unread, mentions: a.mentions + u.mentions }; }, { unread: false, mentions: 0 });
                  const holds = () => f().guildIds.includes(ui.view);
                  const label = () => [name(), `${f().guildIds.length} 个服务器`, sum().mentions ? `${sum().mentions} 条提及` : ""].filter(Boolean).join("，");
                  return (
                    <div class={`folder ${expanded() ? "open" : ""}`} style={{ "--folder-bg": folderTint(f().color, 0.22) }}>
                      <div class={`guild-item folder-head ${!expanded() && holds() ? "active" : ""} ${!expanded() && sum().unread ? "unread" : ""}`} data-tip-side="right" data-tip={name()} aria-label={label()} aria-expanded={expanded()} {...press(() => toggle(f().id))}>
                        <Show when={expanded()} fallback={
                          <div class="folder-btn">
                            <For each={f().guildIds.slice(0, 4)}>
                              {(id) => { const g = () => session.guilds.find((x) => x.id === id); return <Show when={g()}>{(g) => <GuildIcon name={g().name} icon={g().icon} size={18} />}</Show>; }}
                            </For>
                          </div>
                        }>
                          <div class="folder-btn folder-glyph" style={{ color: folderTint(f().color, 1) }}><Folder size={24} fill="currentColor" /></div>
                        </Show>
                        <span class="ring" />
                        <Show when={!expanded()}><StatusMark mark={guildMark(f().guildIds)} /></Show>
                        <Show when={!expanded() && sum().mentions}><span class="badge">{count(sum().mentions)}</span></Show>
                      </div>
                      <Show when={expanded()}>
                        <div role="group" aria-label={name()}>
                          <For each={f().guildIds}>{(id) => <GuildEntry id={id} />}</For>
                        </div>
                      </Show>
                    </div>
                  );
                }}
              </Match>
            </Switch>
          )}
        </For>
        <Show when={session.foldersFailed && session.guilds.length}>
          <div class="guild-item" data-tip-side="right" data-tip="重新加载服务器文件夹" aria-label="重新加载服务器文件夹" {...press(() => void loadFolders())}>
            <div class="guild-btn folder-retry"><RotateCw size={20} /></div>
          </div>
        </Show>
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
