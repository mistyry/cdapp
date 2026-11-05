import { For, Show, createSignal, onCleanup } from "solid-js";
import { CircleHelp, Headphones, HeadphoneOff, Menu, Mic, MicOff, PhoneOff, Settings, UserPlus, X } from "lucide-solid";
import type { Channel } from "../../stores/types";
import { accessState, enterVoice, loadDms, permsOf, retryAccess, selectChannel, selectDm, session, visible, voiceBlock } from "../../stores/session";
import { canRejoin, deviceSummary, dismissVoice, isSpeaking, leaveVoice, mic, micFailed, micHint, micText, output, qualityText, setPeerVolume, toggleDeafen, toggleMic, voice, voiceActive, voiceStatusText, volumeOf, type VoicePeer } from "../../stores/voice";
import { DM_VIEW, currentChannelId, currentChatCode, ui, setUi } from "../../stores/ui";
import Avatar from "../common/Avatar";
import { Caret, KookHash, LockIcon, VoiceIcon } from "../common/icons";
import { channelIcon } from "../../lib/channelIcon";

// 语音成员。说话时头像高亮，点击或右键其他成员可调节音量。
function RoomMember(p: { peer: VoicePeer }) {
  const user = () => ({ id: p.peer.id, name: p.peer.name || "未知用户", avatar: p.peer.avatar || undefined, online: true });
  const [open, setOpen] = createSignal(false);
  const self = () => p.peer.id === voice.me?.id;
  const toggle = (e: MouseEvent) => {
    if (self()) return;
    e.preventDefault();
    setOpen(!open());
  };
  return (
    <>
      <div class={`voice-member ${isSpeaking(p.peer.id) ? "speaking" : ""}`} onContextMenu={toggle} onClick={toggle} title={self() ? undefined : "调节音量"}>
        <Avatar user={user()} size={20} />
        <span class="voice-name">{user().name}</span>
        <Show when={!self() && volumeOf(p.peer.id) !== 100}><span class="voice-vol">{volumeOf(p.peer.id)}%</span></Show>
        <Show when={p.peer.micOff || p.peer.serverMute}><span title={p.peer.serverMute ? "已被服务器禁麦" : "已关闭麦克风"}><MicOff size={14} /></span></Show>
        <Show when={p.peer.headsetOff}><span title="已关闭耳机"><HeadphoneOff size={14} /></span></Show>
      </div>
      <Show when={open() && !self()}>
        <div class="voice-member-volume" onClick={(e) => e.stopPropagation()}>
          <input type="range" min="0" max="200" step="5" value={volumeOf(p.peer.id)} aria-label={`${user().name} 的音量`}
            onInput={(e) => void setPeerVolume(p.peer.id, Number(e.currentTarget.value))} />
          <span>{volumeOf(p.peer.id)}%</span>
          <button class="link-btn" onClick={() => void setPeerVolume(p.peer.id, 100)}>恢复默认</button>
        </div>
      </Show>
    </>
  );
}

const roomPeers = () => (voice.me ? [voice.me, ...voice.peers] : voice.peers);

function ChannelItem(p: { c: Channel }) {
  const active = () => currentChannelId() === p.c.id;
  const inRoom = () => p.c.type === "voice" && voice.channelId === p.c.id && voiceActive() && voice.me !== null;
  const preview = () => active() && p.c.preview;
  const unread = () => session.unread[p.c.id];
  // 可查看但不能连接的语音频道保留显示，并说明原因。
  const noConnect = () => p.c.type === "voice" && voiceBlock(p.c.id) === "denied";
  const icon = () => channelIcon(p.c, permsOf(p.c.id));
  const hint = () => icon().label;
  return (
    <>
      <div
        title={hint()}
        aria-disabled={noConnect() || undefined}
        class={`channel ${p.c.type === "unknown" || noConnect() ? "unsupported" : ""} ${active() ? "active" : ""} ${preview() ? "has-preview" : ""} ${!active() && !unread() && p.c.type === "text" ? "muted" : ""}`}
        onClick={() => {
          // 语音频道点击即加入，已在该频道时不重复加入。
          if (p.c.type === "voice") return void (noConnect() || enterVoice(p.c.id));
          if (p.c.type !== "text") return;
          selectChannel(p.c.id);
        }}
      >
        <div class="channel-row">
          <span class="type" role="img" aria-label={icon().label}>
            {icon().base === "text" ? <KookHash restricted={icon().restricted} />
              : icon().lock ? <LockIcon restricted={icon().restricted} />
              : icon().base === "voice" ? <VoiceIcon restricted={icon().restricted} />
              : <CircleHelp size={16} stroke-width={1.8} aria-hidden="true" />}
          </span>
          <span class="label">{p.c.name}</span>
          <Show when={p.c.type === "text"}>
            <span class="ctrl"><UserPlus size={16} stroke-width={1.6} /><Settings size={16} stroke-width={1.6} /></span>
          </Show>
          <Show when={p.c.type === "voice"}>
            <span class="len">{String(p.c.users?.length ?? 0).padStart(2, "0")}/{p.c.limit ?? "∞"}</span>
          </Show>
          <Show when={unread() && !active() ? unread() : undefined}>
            {(u) => (
              <Show when={u().mentions > 0} fallback={<span class="dot" title="有未读消息" />}>
                <span class="badge at" title={`${u().mentions} 条提及`}>@{u().mentions > 1 ? u().mentions : ""}</span>
              </Show>
            )}
          </Show>
        </div>
        <Show when={preview()}><div class="preview">{p.c.preview}</div></Show>
      </div>
      {/* 本端所在的语音频道以语音服务器的房间成员为准，其余频道显示频道列表中的在线用户。 */}
      <Show when={inRoom()} fallback={
        <For each={p.c.users}>
          {(id) => <Show when={session.users[id]}>{(u) => <div class="voice-member"><Avatar user={u()} size={20} />{u().name}</div>}</Show>}
        </For>
      }>
        <For each={roomPeers()}>{(peer) => <RoomMember peer={peer} />}</For>
      </Show>
    </>
  );
}

// 私信会话列表。只显示对方、最近消息与未读，不显示推荐或广告类内容。
function DmList() {
  const onScroll = (e: Event) => {
    const el = e.currentTarget as HTMLElement;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 200) void loadDms(true);
  };
  const l = () => session.dmList;
  return (
    <div class="channel-list dm-list" onScroll={onScroll}>
      <For each={session.dms}>
        {(c) => {
          const user = () => ({ id: c.target.id, name: c.target.nickname || c.target.username || "未知用户", avatar: c.target.avatar || undefined, online: c.target.online });
          const active = () => currentChatCode() === c.code;
          return (
            <div class={`dm ${active() ? "active" : ""} ${c.unread && !active() ? "unread" : ""}`} onClick={() => selectDm(c.code)}>
              <Avatar user={user()} size={32} status />
              <div class="dm-info">
                <div class="dm-name">{user().name}<Show when={c.target.system}><span class="bot-tag">系统</span></Show><Show when={c.target.bot}><span class="bot-tag">机器人</span></Show></div>
                <div class="dm-preview">{c.preview}</div>
              </div>
              <Show when={c.unread && !active()}><span class="badge">{c.unread > 99 ? "99+" : c.unread}</span></Show>
            </div>
          );
        }}
      </For>
      <Show when={l().loading}><div class="member-note">加载中</div></Show>
      <Show when={l().failed}><div class="member-note">私信加载失败 <button class="retry" onClick={() => loadDms(l().loaded)}>重试</button></div></Show>
      <Show when={l().loaded && !l().loading && !l().failed && !session.dms.length}><div class="member-note">还没有私信</div></Show>
    </div>
  );
}

// 语音状态栏：当前语音频道、连接状态与断开，麦克风与耳机开关、失败提示。
function VoiceBox() {
  const name = () => {
    for (const g of session.guilds) for (const grp of g.groups) for (const c of grp.channels) if (c.id === voice.channelId) return `${c.name} / ${g.name}`;
    return "语音频道";
  };
  const noSay = () => voiceActive() && voice.me !== null && !voice.me.permissionSay;
  // 每 0.5 秒刷新重连倒计时。
  const [now, setNow] = createSignal(Date.now());
  const timer = setInterval(() => setNow(Date.now()), 500);
  onCleanup(() => clearInterval(timer));
  const wait = () => (voice.phase === "reconnecting" && voice.retryAt ? Math.max(0, Math.ceil((voice.retryAt - now()) / 1000)) : 0);
  return (
    <div class="voice-box">
      <Show when={voice.phase !== "idle"}>
        <div class={`voice-status ${voice.phase}`}>
          <div class="voice-info">
            <div class="voice-state">
              {voiceStatusText()}
              <Show when={wait() > 0}><span class="voice-wait">{` ${wait()} 秒后重试`}</span></Show>
              <Show when={voice.phase === "connected" && qualityText()}>
                {(q) => <span class={`voice-quality ${voice.quality}`} title="链路质量">{`质量${q()}`}</span>}
              </Show>
            </div>
            <div class="voice-channel" title={voice.message ?? undefined}>{name()}</div>
            <Show when={voice.phase === "reconnecting" || voice.phase === "disconnected" ? voice.message : null}>{(m) => <div class="voice-note">{m()}</div>}</Show>
            <Show when={canRejoin()}>
              <button class="rejoin-btn" onClick={() => canRejoin() && void enterVoice(voice.channelId)} title="重新加入该语音频道">重新加入</button>
            </Show>
            <Show when={noSay()}><div class="voice-note">当前设备没有发言权限，其他人听不到你</div></Show>
            <Show when={voiceActive() && !noSay() && (micHint() ?? (mic.status === "muted" ? null : mic.message))}>
              {(m) => (
                <div class="voice-note mic-note">
                  {m()}
                  <Show when={micFailed()}><button class="link-btn" disabled={mic.busy} onClick={() => void toggleMic()}>重试</button></Show>
                </div>
              )}
            </Show>
            <Show when={voiceActive() && output.message}>{(m) => <div class="voice-note">{m()}</div>}</Show>
            <Show when={voiceActive() && mic.status === "active"}>
              <div class="mic-meter" title={mic.device ?? undefined}><div style={{ width: `${mic.level}%` }} /></div>
            </Show>
            <Show when={voice.phase === "connected" ? deviceSummary() : null}>
              {(d) => <div class={`voice-devices ${d().ok ? "" : "warn"}`} title={d().title}>{d().text}</div>}
            </Show>
          </div>
          <Show when={voiceActive()} fallback={<button class="icon-btn" title="关闭提示" onClick={dismissVoice}><X size={18} /></button>}>
            <button class="icon-btn danger" title="断开语音" onClick={() => void leaveVoice()}><PhoneOff size={18} /></button>
          </Show>
        </div>
      </Show>
      <div class="voice-ctrls">
        <button class="mode" disabled title="语音感应与按键说话尚未实现，当前为持续开麦。没有回声消除，建议使用耳机">语音感应</button>
        <div class="ctrls">
          <button
            class={`icon-btn ${mic.muted || micFailed() ? "danger" : ""}`}
            disabled={mic.busy}
            aria-pressed={mic.muted}
            title={micFailed() ? `${micText()}，点击重试` : mic.muted ? "取消静音" : mic.device ? `静音，当前设备：${mic.device}` : "静音"}
            onClick={() => void toggleMic()}
          >
            {mic.muted || micFailed() ? <MicOff size={20} /> : <Mic size={20} />}
          </button>
          <button
            class={`icon-btn ${output.deafened ? "danger" : ""}`}
            disabled={output.busy}
            aria-pressed={output.deafened}
            title={output.deafened ? "打开耳机" : output.device ? `关闭耳机，当前设备：${output.device}` : "关闭耳机"}
            onClick={() => void toggleDeafen()}
          >
            {output.deafened ? <HeadphoneOff size={20} /> : <Headphones size={20} />}
          </button>
          <button class="icon-btn" title="语音设置" onClick={() => setUi("settings", "voice")}><Settings size={20} /></button>
        </div>
      </div>
    </div>
  );
}

export default function ChannelSidebar() {
  const guild = () => session.guilds.find((g) => g.id === ui.view) ?? { id: "", name: "", groups: [] };
  return (
    <aside class="sidebar">
      <div class="sidebar-top">
        <div class="sidebar-title">
          <span>{ui.view === DM_VIEW ? "私信" : guild().name}</span>
          <button class="icon-btn" title="服务器菜单"><Menu size={22} /></button>
        </div>
      </div>
      <div class="sidebar-line" />
      <Show when={ui.view === DM_VIEW}><DmList /></Show>
      <Show when={ui.view !== DM_VIEW && guild().id && accessState(guild().id) !== "ready"}>
        <div class="access-note">
          <Show when={accessState(guild().id) === "failed"} fallback="正在读取频道权限">
            无法读取频道权限 <button class="retry" onClick={() => void retryAccess(guild().id)}>重试</button>
          </Show>
        </div>
      </Show>
      <div class="channel-list" hidden={ui.view === DM_VIEW}>
        <For each={guild().groups}>
          {(grp) => {
            const key = () => `${guild().id}/${grp.id}`;
            const open = () => grp.loose || !ui.collapsed[key()];
            return (
              <>
                <Show when={!grp.loose && grp.channels.some(visible)}>
                <div class={`group ${open() ? "" : "collapsed"}`} onClick={() => setUi("collapsed", key(), (v) => !v)}>
                  <span class="caret"><Caret /></span>
                  <span class="name">{grp.name}</span>
                </div>
                </Show>
                <For each={grp.channels.filter((c) => visible(c) && (open() || c.id === currentChannelId()))}>{(c) => <ChannelItem c={c} />}</For>
              </>
            );
          }}
        </For>
      </div>
      <VoiceBox />
    </aside>
  );
}
