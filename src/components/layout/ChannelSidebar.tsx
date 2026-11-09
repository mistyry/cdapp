import { For, Show, createEffect, createSignal, on, onCleanup } from "solid-js";
import { CircleHelp, Headphones, HeadphoneOff, Menu, Mic, MicOff, PhoneOff, Settings, UserPlus, X } from "lucide-solid";
import type { Channel } from "../../stores/types";
import { accessState, dropSelfFromVoice, enterVoice, loadDms, permsOf, retryAccess, selectChannel, selectDm, session, visible, voiceBlock } from "../../stores/session";
import { canRejoin, dismissVoice, isSpeaking, disconnectVoice, mic, micFailed, output, setPeerVolume, toggleDeafen, toggleMic, voice, voiceActive, voiceStatusText, volumeOf, type VoicePeer } from "../../stores/voice";
import { DM_VIEW, currentChannelId, currentChatCode, ui, setUi } from "../../stores/ui";
import RangeSlider from "../common/RangeSlider";
import Avatar from "../common/Avatar";
import { DeviceMenu, VolumePop, type AudioKind } from "./AudioPanel";
import { Caret, KookHash, LockIcon, VoiceIcon } from "../common/icons";
import { channelIcon } from "../../lib/channelIcon";

// 语音成员。说话时头像高亮，点击或右键其他成员可调节音量。
function RoomMember(p: { peer: VoicePeer }) {
  const user = () => ({ id: p.peer.id, name: p.peer.name || "未知用户", avatar: p.peer.avatar || undefined, online: true });
  const [open, setOpen] = createSignal(false);
  const self = () => p.peer.id === voice.me?.id;
  // 本人以本地开关为准，不等服务端回执。
  const micOff = () => (self() ? mic.muted : p.peer.micOff);
  const headsetOff = () => (self() ? output.deafened : p.peer.headsetOff);
  const toggle = (e: MouseEvent) => {
    if (self()) return;
    e.preventDefault();
    setOpen(!open());
  };
  return (
    <>
      <div class={`voice-member ${isSpeaking(p.peer.id) ? "speaking" : ""}`} onContextMenu={toggle} onClick={toggle} data-tip={self() ? undefined : "调节音量"}>
        <Avatar user={user()} size={20} />
        <span class="voice-name">{user().name}</span>
        <Show when={!self() && volumeOf(p.peer.id) !== 100}><span class="voice-vol">{volumeOf(p.peer.id)}%</span></Show>
        <Show when={micOff() || p.peer.serverMute}><span data-tip={p.peer.serverMute ? "已被服务器禁麦" : "已关闭麦克风"} aria-label={p.peer.serverMute ? "已被服务器禁麦" : "已关闭麦克风"} role="img"><MicOff size={14} /></span></Show>
        <Show when={headsetOff()}><span data-tip="已关闭耳机" aria-label="已关闭耳机" role="img"><HeadphoneOff size={14} /></span></Show>
      </div>
      <Show when={open() && !self()}>
        <div class="voice-member-volume" onClick={(e) => e.stopPropagation()}>
          <RangeSlider value={volumeOf(p.peer.id)} max={200} label={`${user().name} 的音量`} onInput={(v) => void setPeerVolume(p.peer.id, v)} />
          <button class="link-btn" onClick={() => void setPeerVolume(p.peer.id, 100)}>恢复默认</button>
        </div>
      </Show>
    </>
  );
}

const roomPeers = () => (voice.me ? [voice.me, ...voice.peers] : voice.peers);

function ChannelItem(p: { c: Channel }) {
  const active = () => currentChannelId() === p.c.id;
  // 本人头像在媒体连接成功、播放进场音效时出现，与官方客户端一致。
  const inRoom = () => p.c.type === "voice" && voice.channelId === p.c.id && voiceActive() && voice.phase !== "connecting" && voice.me !== null;
  const connectingHere = () => voice.channelId === p.c.id && voice.phase === "connecting";
  const preview = () => p.c.type === "text" && p.c.preview;
  const unread = () => session.unread[p.c.id];
  // 可查看但不能连接的语音频道保留显示，并说明原因。
  const noConnect = () => p.c.type === "voice" && voiceBlock(p.c.id) === "denied";
  const icon = () => channelIcon(p.c, permsOf(p.c.id));
  const canJoin = () => p.c.type === "voice" && !noConnect() && !inRoom();
  // 按下时已处于聚焦，本次单击即为第二下。
  let armed = false;
  return (
    <>
      <div
        aria-label={[p.c.name, ...icon().parts].join(" ")}
        aria-disabled={noConnect() || undefined}
        class={`channel ${p.c.type === "unknown" || noConnect() ? "unsupported" : ""} ${active() ? "active" : ""} ${preview() ? "has-preview" : ""}`}
        // 语音频道第一次单击聚焦并提示，保持聚焦时再次单击或回车加入，不限间隔；点击别处取消。已在该频道时不重复加入。
        tabIndex={canJoin() ? 0 : undefined}
        data-focus-tip={canJoin() ? "再次点击进入频道" : undefined}
        onPointerDown={(e) => (armed = canJoin() && document.activeElement === e.currentTarget)}
        onKeyDown={(e) => e.key === "Enter" && canJoin() && void enterVoice(p.c.id)}
        onClick={() => {
          if (p.c.type === "text") selectChannel(p.c.id);
          else if (armed && canJoin()) void enterVoice(p.c.id);
          armed = false;
        }}
      >
        <div class="channel-row">
          <span class="type" role="img" aria-label={icon().label} data-tip={icon().label}>
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
            <span class="len">{p.c.limit ? `${String(p.c.users?.length ?? 0).padStart(2, "0")}/${p.c.limit}` : p.c.users?.length ?? 0}</span>
          </Show>
          <Show when={unread() && !active() ? unread() : undefined}>
            {(u) => (
              <Show when={u().mentions > 0} fallback={<span class="dot" data-tip="有未读消息" aria-label="有未读消息" role="img" />}>
                <span class="badge at" data-tip={`${u().mentions} 条提及`} aria-label={`${u().mentions} 条提及`}>@{u().mentions > 1 ? u().mentions : ""}</span>
              </Show>
            )}
          </Show>
        </div>
        <Show when={preview()}><div class="preview">{p.c.preview}</div></Show>
      </div>
      {/* 本端所在的语音频道以语音服务器的房间成员为准，其余频道显示频道列表中的在线用户。 */}
      <Show when={inRoom()} fallback={
        <For each={connectingHere() ? p.c.users?.filter((u) => u.id !== session.me?.id) : p.c.users}>
          {(u) => {
            // 本人在此频道但不是由本客户端连接。
            const elsewhere = () => u.id === session.me?.id && !(voice.channelId === p.c.id && voiceActive());
            return (
              <div class="voice-member" classList={{ elsewhere: elsewhere() }} data-tip={elsewhere() ? "已在其他设备连接" : undefined}
                aria-label={elsewhere() ? `${u.name} 已在其他设备连接` : undefined}>
                <Avatar user={u} size={20} />{u.name}
              </div>
            );
          }}
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

// 连接延迟柱状图，延迟越低亮起的柱越多。连接中与尚无测量时按良好显示。
function LatencyBars() {
  const level = () => {
    const r = voice.rtt;
    if (voice.phase === "connecting") return 4;
    if (voice.phase !== "connected") return 0;
    if (r === null) return 4;
    return r < 100 ? 4 : r < 200 ? 3 : r < 400 ? 2 : 1;
  };
  const tip = () => (voice.phase === "connected" || voice.phase === "connecting" ? `${voice.phase === "connected" ? voice.rtt ?? 0 : 0} ms` : voiceStatusText());
  return (
    <div class={`latency-bars l${level()} ${voice.phase}`} data-tip={tip()} role="img" aria-label={tip()}>
      <For each={[1, 2, 3, 4]}>{(i) => <span style={{ height: `${i * 25}%` }} classList={{ on: i <= level() }} />}</For>
    </div>
  );
}

// 语音状态栏：连接延迟、频道名与断开，麦克风与耳机开关。
function VoiceBox() {
  const name = () => {
    for (const g of session.guilds) for (const grp of g.groups) for (const c of grp.channels) if (c.id === voice.channelId) return c.name;
    return "语音频道";
  };
  // 右键打开设备菜单，按触发位置水平定位；悬停或键盘聚焦时显示音量条，拖动期间保持显示。
  const [menu, setMenu] = createSignal<{ kind: AudioKind; x: number } | null>(null);
  const [hover, setHover] = createSignal<AudioKind | null>(null);
  const [dragging, setDragging] = createSignal(false);
  let leaveTimer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(leaveTimer));
  const show = (k: AudioKind) => { clearTimeout(leaveTimer); setHover(k); };
  const hide = (k: AudioKind) => {
    clearTimeout(leaveTimer);
    leaveTimer = setTimeout(() => !dragging() && hover() === k && setHover(null), 150);
  };
  const hoverProps = (k: AudioKind) => ({
    onPointerEnter: () => show(k),
    onPointerLeave: () => hide(k),
    onFocusIn: () => show(k),
    onFocusOut: (e: FocusEvent) => !(e.currentTarget as Element).contains(e.relatedTarget as Node) && setHover(null),
  });
  // 连接中显示状态，连上后短暂显示已连接，再显示频道名。
  const [justConnected, setJustConnected] = createSignal(false);
  let flash: ReturnType<typeof setTimeout> | undefined;
  createEffect(on(() => voice.phase, (ph, prev) => {
    clearTimeout(flash);
    setJustConnected(ph === "connected" && (prev === "connecting" || prev === "reconnecting"));
    if (justConnected()) flash = setTimeout(() => setJustConnected(false), 1000);
  }));
  onCleanup(() => clearTimeout(flash));
  const label = () =>
    voice.phase === "connecting" || voice.phase === "reconnecting" ? { text: "连接中", cls: "pending" }
      : justConnected() ? { text: "已连接", cls: "done" }
        : null;
  let ctrls!: HTMLDivElement;
  const openMenu = (e: MouseEvent, kind: AudioKind) => {
    e.preventDefault();
    setHover(null);
    const box = ctrls.getBoundingClientRect();
    const btn = (e.currentTarget as Element).getBoundingClientRect();
    // 菜单以按钮为中心，靠近窗口边缘时向内收，保持完整可见。
    const half = Math.min(272, window.innerWidth - 24) / 2;
    const center = Math.max(8 + half, Math.min(window.innerWidth - 8 - half, btn.left + btn.width / 2));
    setMenu((m) => (m?.kind === kind ? null : { kind, x: center - box.left }));
  };
  let inBtn: HTMLButtonElement | undefined;
  let outBtn: HTMLButtonElement | undefined;
  return (
    <div class="voice-box">
      <Show when={voice.phase !== "idle" && (voice.phase !== "disconnected" || canRejoin())}>
        <div class={`voice-status ${voice.phase}`}>
          <LatencyBars />
          <Show when={label()} fallback={<div class="voice-channel">{name()}</div>}>
            {(t) => <div class={`voice-channel ${t().cls}`} role="status">{t().text}</div>}
          </Show>
          <Show when={canRejoin()}>
            <button class="rejoin-btn" onClick={() => canRejoin() && void enterVoice(voice.channelId)}>重新加入</button>
          </Show>
          <Show when={voiceActive()} fallback={<button class="icon-btn" data-tip="关闭" aria-label="关闭" onClick={dismissVoice}><X size={18} /></button>}>
            <button class="icon-btn danger" data-tip="断开语音" aria-label="断开语音" onClick={() => { const c = voice.channelId; void disconnectVoice(); dropSelfFromVoice(c); }}><PhoneOff size={18} /></button>
          </Show>
        </div>
      </Show>
      <div class="voice-ctrls" ref={ctrls}>
        <button class="mode" disabled data-tip="持续开麦" aria-label="语音感应" aria-description="持续开麦">语音感应</button>
        <div class="ctrls">
          <div class="ctrl-wrap" {...hoverProps("input")} onContextMenu={(e) => openMenu(e, "input")}>
            <button
              ref={inBtn}
              class={`icon-btn ${mic.muted || micFailed() ? "danger" : ""}`}
              disabled={mic.busy}
              aria-pressed={mic.muted}
              aria-haspopup="menu"
              aria-label={mic.muted ? "取消静音" : "静音"}
              onClick={() => void toggleMic()}
            >
              {mic.muted || micFailed() ? <MicOff size={20} /> : <Mic size={20} />}
            </button>
            <Show when={hover() === "input" && !menu()}><VolumePop kind="input" onDrag={setDragging} /></Show>
          </div>
          <div class="ctrl-wrap" {...hoverProps("output")} onContextMenu={(e) => openMenu(e, "output")}>
            <button
              ref={outBtn}
              class={`icon-btn ${output.deafened ? "danger" : ""}`}
              disabled={output.busy}
              aria-pressed={output.deafened}
              aria-haspopup="menu"
              aria-label={output.deafened ? "打开耳机" : "关闭耳机"}
              onClick={() => void toggleDeafen()}
            >
              {output.deafened ? <HeadphoneOff size={20} /> : <Headphones size={20} />}
            </button>
            <Show when={hover() === "output" && !menu()}><VolumePop kind="output" onDrag={setDragging} /></Show>
          </div>
          <button class="icon-btn" data-tip="语音设置" aria-label="语音设置" onClick={() => setUi("settings", "voice")}><Settings size={20} /></button>
        </div>
        <Show when={menu()} keyed>
          {(m) => <DeviceMenu kind={m.kind} x={m.x} onClose={(refocus) => { const b = m.kind === "input" ? inBtn : outBtn; setMenu(null); if (refocus) b?.focus(); }} />}
        </Show>
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
          <button class="icon-btn" data-tip="服务器菜单" aria-label="服务器菜单"><Menu size={22} /></button>
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
