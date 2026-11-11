import { For, type JSX, Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { anyMod, banFromGuild, kickFromGuild, kickFromVoice, modPerms, moveTargets, moveToVoice, setServerMute } from "../../stores/moderation";
import { Portal } from "solid-js/web";
import { AudioLines, Check, ChevronRight, Volume2, CircleHelp, Headphones, HeadphoneOff, Menu, Mic, MicOff, Monitor, PhoneOff, Settings, UserPlus, X } from "lucide-solid";
import type { Channel } from "../../stores/types";
import { accessState, dropSelfFromVoice, enterVoice, loadDms, permsOf, retryAccess, selectChannel, selectDm, session, visible, voiceBlock } from "../../stores/session";
import { canGoLive, canRejoin, denoise, setDenoise, dismissVoice, isSpeaking, disconnectVoice, mic, micFailed, output, peerDenoised, setPeerDenoise, setPeerVolume, toggleDeafen, togglePeerMute, toggleMic, voice, voiceActive, voiceStatusText, volumeOf, type VoicePeer } from "../../stores/voice";
import { shareSdkReady } from "../../stores/sounds";
import { share, sharing, stopShare } from "../../stores/share";

// 分享状态，用于可访问描述。
const shareStatus = () => (share.phase === "live" && share.fps ? `${share.width}×${share.height}，${share.fps} 帧` : undefined);
import { DM_VIEW, currentChannelId, currentChatCode, ui, setUi } from "../../stores/ui";
import RangeSlider from "../common/RangeSlider";
import Avatar from "../common/Avatar";
import { DenoiseMenu, DeviceMenu, VolumePop, type AudioKind } from "./AudioPanel";
import { openSharePicker } from "./ScreenSharePicker";
import { Caret, DenoiseOff, HeadphoneOffIcon, MicOffIcon, SpeakerOffMark, KookHash, LockIcon, VoiceIcon } from "../common/icons";
import { channelIcon } from "../../lib/channelIcon";

// 语音成员右键菜单。对本人提供麦克风与耳机开关，对他人提供音量调节。
function MemberMenu(p: { peer: VoicePeer; self: boolean; name: string; x: number; y: number; onClose: () => void }) {
  let root!: HTMLDivElement;
  const [pos, setPos] = createSignal({ x: p.x, y: p.y });
  onMount(() => {
    const r = root.getBoundingClientRect();
    setPos({ x: Math.max(8, Math.min(p.x, innerWidth - r.width - 8)), y: Math.max(8, Math.min(p.y, innerHeight - r.height - 8)) });
    root.querySelector<HTMLElement>("button, [role=slider]")?.focus();
  });
  // 次级菜单渲染在 body 下，同样算菜单内部。
  const onDown = (e: PointerEvent) => {
    const t = e.target as Element;
    if (!root.contains(t) && !t.closest?.(".member-menu")) p.onClose();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    p.onClose();
  };
  document.addEventListener("pointerdown", onDown, true);
  document.addEventListener("keydown", onKey, true);
  window.addEventListener("blur", p.onClose);
  onCleanup(() => {
    document.removeEventListener("pointerdown", onDown, true);
    document.removeEventListener("keydown", onKey, true);
    window.removeEventListener("blur", p.onClose);
  });
  const toggleItem = (label: string, on: boolean, act: () => void, danger = false) => (
    <button class="audio-item" classList={{ danger }} role="menuitemcheckbox" aria-checked={on} onClick={act}>
      <span class="audio-name">{label}</span>
      <span class="menu-trail menu-check" classList={{ on }}><span>{on ? <Check size={12} stroke-width={3} /> : null}</span></span>
    </button>
  );
  return (
    <Portal>
      <div class="member-menu" ref={root} role="menu" aria-label={p.name} style={{ left: `${pos().x}px`, top: `${pos().y}px` }} onContextMenu={(e) => e.preventDefault()}>
        <Show when={p.self} fallback={
          <div class="member-volume">
            <div class="member-volume-head">用户音量</div>
            <div class="member-volume-row">
              <RangeSlider value={volumeOf(p.peer.id)} max={200} label={`${p.name} 的音量`} onInput={(v) => void setPeerVolume(p.peer.id, v)} />
              <button class="icon-btn menu-trail" aria-pressed={volumeOf(p.peer.id) === 0} data-tip={volumeOf(p.peer.id) === 0 ? "取消静音" : "静音"} aria-label={volumeOf(p.peer.id) === 0 ? "取消静音" : "静音"} onClick={() => void togglePeerMute(p.peer.id)}>
                {volumeOf(p.peer.id) === 0 ? <SpeakerOffMark size={18} /> : <Volume2 size={18} />}
              </button>
            </div>
            {toggleItem("降噪", peerDenoised(p.peer.id), () => void setPeerDenoise(p.peer.id, !peerDenoised(p.peer.id)))}
            <ModSection peer={p.peer} onDone={p.onClose} toggleItem={toggleItem} />
          </div>
        }>
          {toggleItem("关闭麦克风", mic.muted, () => void toggleMic())}
          {toggleItem("关闭耳机", output.deafened, () => void toggleDeafen())}
        </Show>
      </div>
    </Portal>
  );
}

type ToggleItem = (label: string, on: boolean, act: () => void, danger?: boolean) => JSX.Element;

// 成员管理。只列出本人在当前语音频道有权限的操作；踢出服务器与黑名单需再次点击确认。
function ModSection(p: { peer: VoicePeer; onDone: () => void; toggleItem: ToggleItem }) {
  const ch = () => voice.channelId;
  const perms = () => modPerms(ch());
  const [moveOpen, setMoveOpen] = createSignal(false);
  let subMenu: HTMLDivElement | undefined;
  // 指针穿过两级菜单之间的间隙时稍后再关闭。
  let closeTimer: ReturnType<typeof setTimeout> | undefined;
  const openSub = () => { clearTimeout(closeTimer); setMoveOpen(true); };
  const closeSub = () => { clearTimeout(closeTimer); closeTimer = setTimeout(() => setMoveOpen(false), 200); };
  onCleanup(() => clearTimeout(closeTimer));
  // 次级菜单默认在右侧，与触发项顶部对齐；右侧放不下时改到左侧，并限制在窗口内。
  let subHost!: HTMLDivElement;
  const placeSub = (el: HTMLDivElement) => {
    const host = subHost.getBoundingClientRect();
    const menu = subHost.closest(".member-menu")!.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    const right = menu.right + 4;
    const x = right + r.width <= innerWidth - 8 ? right : Math.max(8, menu.left - 4 - r.width);
    const y = Math.max(8, Math.min(host.top - 6, innerHeight - r.height - 8));
    el.style.left = `${x}px`;
    el.style.top = `${y}px`;
    el.style.visibility = "visible";
  };
  const [armed, setArmed] = createSignal<"kick" | "ban" | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const act = async (f: () => Promise<string | null>, close = true) => {
    if (busy()) return;
    setBusy(true);
    setError(null);
    const err = await f();
    setBusy(false);
    if (err) setError(err);
    else if (close) p.onDone();
  };
  const confirmThen = (key: "kick" | "ban", f: () => Promise<string | null>) => {
    if (armed() !== key) return void setArmed(key);
    setArmed(null);
    void act(f);
  };
  return (
    <Show when={anyMod(perms())}>
      <div class="menu-sep" role="separator" />
      <Show when={perms().voiceManage}>
        <button class="audio-item danger" role="menuitem" disabled={busy()} onClick={() => void act(() => kickFromVoice(ch(), p.peer.id))}>踢出语音频道</button>
      </Show>
      <Show when={perms().kick}>
        <button class="audio-item danger" role="menuitem" disabled={busy()} onClick={() => confirmThen("kick", () => kickFromGuild(ch(), p.peer.id))}>
          {armed() === "kick" ? "确认踢出服务器" : "踢出服务器"}
        </button>
      </Show>
      <Show when={perms().ban}>
        <button class="audio-item danger" role="menuitem" disabled={busy()} onClick={() => confirmThen("ban", () => banFromGuild(ch(), p.peer.id))}>
          {armed() === "ban" ? "确认踢出并加入黑名单" : "踢出并加入黑名单"}
        </button>
      </Show>
      <Show when={perms().voiceManage}>
        <div class="menu-sub-host" ref={subHost} onPointerEnter={openSub} onPointerLeave={closeSub}>
          <button class="audio-item" role="menuitem" aria-haspopup="menu" aria-expanded={moveOpen()}
            onClick={() => setMoveOpen(true)}
            onKeyDown={(e) => {
              if (e.key !== "ArrowRight") return;
              e.preventDefault();
              setMoveOpen(true);
              queueMicrotask(() => subMenu?.querySelector<HTMLElement>("button")?.focus());
            }}>
            <span class="audio-name">移动到其他语音频道</span>
            <span class="menu-trail menu-caret"><ChevronRight size={16} /></span>
          </button>
          <Show when={moveOpen()}>
            <Portal>
            <div class="member-menu menu-sub" onPointerEnter={openSub} onPointerLeave={closeSub} ref={(el) => { subMenu = el; requestAnimationFrame(() => placeSub(el)); }} role="menu" aria-label="移动到其他语音频道"
              onKeyDown={(e) => {
                if (e.key !== "ArrowLeft") return;
                e.preventDefault();
                setMoveOpen(false);
                subHost.querySelector<HTMLElement>("button")?.focus();
              }}>
              <For each={moveTargets(ch())} fallback={<div class="audio-note">没有其他语音频道</div>}>
                {(c) => (
                  <button class="audio-item" role="menuitem" disabled={busy()} onClick={() => void act(() => moveToVoice(c.id, p.peer.id))}>
                    <span class="audio-name">{c.name}</span>
                  </button>
                )}
              </For>
            </div>
            </Portal>
          </Show>
        </div>
      </Show>
      <Show when={perms().mute}>
        {p.toggleItem("服务器闭麦", p.peer.serverMute, () => void act(() => setServerMute(ch(), p.peer.id, "mute", !p.peer.serverMute), false), true)}
      </Show>
      <Show when={perms().deafen}>
        {p.toggleItem("服务器静音", Boolean(p.peer.serverDeafen), () => void act(() => setServerMute(ch(), p.peer.id, "deafen", !p.peer.serverDeafen), false), true)}
      </Show>
      <Show when={error()}>{(m) => <div class="audio-note warn" role="alert">{m()}</div>}</Show>
    </Show>
  );
}

// 语音成员。说话时整行高亮，右键打开成员菜单。
function RoomMember(p: { peer: VoicePeer }) {
  const user = () => ({ id: p.peer.id, name: p.peer.name || "未知用户", avatar: p.peer.avatar || undefined, online: true });
  const [menu, setMenu] = createSignal<{ x: number; y: number } | null>(null);
  const self = () => p.peer.id === voice.me?.id;
  // 本人以本地开关为准，不等服务端回执。
  const micOff = () => (self() ? mic.muted : p.peer.micOff);
  const headsetOff = () => (self() ? output.deafened : p.peer.headsetOff);
  const open = (e: MouseEvent) => {
    e.preventDefault();
    setMenu({ x: e.clientX, y: e.clientY });
  };
  return (
    <>
      <div class={`voice-member ${isSpeaking(p.peer.id) ? "speaking" : ""}`} onContextMenu={open} aria-haspopup="menu">
        <Avatar user={user()} size={24} />
        <span class="voice-name">{user().name}</span>
        <Show when={isSpeaking(p.peer.id) && !micOff() && !p.peer.serverMute}><span class="voice-speaking" aria-label="正在说话" role="img"><Mic size={14} /></span></Show>
        {/* 服务器闭麦与静音时显示红色图标，替代成员自己的开关状态。 */}
        <Show when={p.peer.serverMute} fallback={<Show when={micOff()}><span data-tip="已关闭麦克风" aria-label="已关闭麦克风" role="img"><MicOff size={14} /></span></Show>}>
          <span class="server-off" data-tip="已被服务器闭麦" aria-label="已被服务器闭麦" role="img"><MicOff size={14} /></span>
        </Show>
        <Show when={p.peer.serverDeafen} fallback={<Show when={headsetOff()}><span data-tip="已关闭耳机" aria-label="已关闭耳机" role="img"><HeadphoneOff size={14} /></span></Show>}>
          <span class="server-off" data-tip="已被服务器静音" aria-label="已被服务器静音" role="img"><HeadphoneOff size={14} /></span>
        </Show>
        <Show when={!self() && volumeOf(p.peer.id) === 0}><span data-tip="已静音" aria-label="已静音" role="img"><SpeakerOffMark size={14} /></span></Show>
      </div>
      <Show when={menu()}>
        {(m) => <MemberMenu peer={p.peer} self={self()} name={user().name} x={m().x} y={m().y} onClose={() => setMenu(null)} />}
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
                <Avatar user={u} size={24} />{u.name}
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
  const [menu, setMenu] = createSignal<{ kind: AudioKind | "denoise"; x: number } | null>(null);
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
  const openMenu = (e: MouseEvent, kind: AudioKind | "denoise") => {
    e.preventDefault();
    setHover(null);
    const box = ctrls.getBoundingClientRect();
    const btn = (e.currentTarget as Element).getBoundingClientRect();
    // 菜单以按钮为中心，靠近窗口边缘时向内收，保持完整可见。
    const half = Math.min(232, window.innerWidth - 24) / 2;
    const center = Math.max(8 + half, Math.min(window.innerWidth - 8 - half, btn.left + btn.width / 2));
    setMenu((m) => (m?.kind === kind ? null : { kind, x: center - box.left }));
  };
  let dnBtn: HTMLButtonElement | undefined;
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
      <Show when={voiceActive()}>
        <div class="voice-share">
          <Show when={sharing()} fallback={
            <button class="share-btn" disabled={!canGoLive() || !shareSdkReady()} onClick={openSharePicker}><Monitor size={18} />屏幕共享</button>
          }>
            <div class="share-live">
              <button class="share-btn" disabled={share.phase !== "live"} onClick={openSharePicker}
                aria-label="共享画质" aria-description={shareStatus()}>
                <Monitor size={18} />{share.phase === "live" ? `${share.resolution}p ${share.frameRate} 帧` : "正在开启共享"}
              </button>
              <button class="share-stop" data-tip="停止共享" aria-label="停止共享" onClick={() => void stopShare()}><X size={18} /></button>
            </div>
          </Show>
        </div>
      </Show>
      <div class="voice-ctrls" ref={ctrls}>
        <button class="mode" disabled data-tip="持续开麦" aria-label="语音感应" aria-description="持续开麦">语音感应</button>
        <div class="ctrls">
          <button
            ref={dnBtn}
            class="icon-btn"
            aria-pressed={denoise()}
            aria-haspopup="menu"
            onContextMenu={(e) => openMenu(e, "denoise")}
            data-tip={denoise() ? "关闭降噪" : "开启降噪"}
            aria-label={denoise() ? "关闭降噪" : "开启降噪"}
            onClick={() => void setDenoise(!denoise())}
          >
            {denoise() ? <AudioLines size={20} /> : <DenoiseOff size={20} />}
          </button>
          <div class="ctrl-wrap" {...hoverProps("input")} onContextMenu={(e) => openMenu(e, "input")}>
            <button
              ref={inBtn}
              class="icon-btn"
              disabled={mic.busy}
              aria-pressed={mic.muted}
              aria-haspopup="menu"
              aria-label={mic.muted ? "取消静音" : "静音"}
              onClick={() => void toggleMic()}
            >
              {mic.muted || micFailed() ? <MicOffIcon size={20} /> : <Mic size={20} />}
            </button>
            <Show when={hover() === "input" && !menu()}><VolumePop kind="input" onDrag={setDragging} /></Show>
          </div>
          <div class="ctrl-wrap" {...hoverProps("output")} onContextMenu={(e) => openMenu(e, "output")}>
            <button
              ref={outBtn}
              class="icon-btn"
              disabled={output.busy}
              aria-pressed={output.deafened}
              aria-haspopup="menu"
              aria-label={output.deafened ? "打开耳机" : "关闭耳机"}
              onClick={() => void toggleDeafen()}
            >
              {output.deafened ? <HeadphoneOffIcon size={20} /> : <Headphones size={20} />}
            </button>
            <Show when={hover() === "output" && !menu()}><VolumePop kind="output" onDrag={setDragging} /></Show>
          </div>
        </div>
        <Show when={menu()} keyed>
          {(m) => {
            const close = (refocus: boolean) => { const b = m.kind === "denoise" ? dnBtn : m.kind === "input" ? inBtn : outBtn; setMenu(null); if (refocus) b?.focus(); };
            return m.kind === "denoise" ? <DenoiseMenu x={m.x} onClose={close} /> : <DeviceMenu kind={m.kind} x={m.x} onClose={close} />;
          }}
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
