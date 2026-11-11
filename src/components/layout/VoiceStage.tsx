import { For, type JSX, Show, createSignal, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { Headphones, LogOut, Mic, Volume2 } from "lucide-solid";
import { dropSelfFromVoice, session } from "../../stores/session";
import { disconnectVoice, isSpeaking, mic, micFailed, output, setPeerVolume, toggleDeafen, toggleMic, togglePeerMute, voice, voiceActive, volumeOf as peerVolume, type VoicePeer } from "../../stores/voice";
import { HeadphoneOffIcon, MicOffIcon, SpeakerOffMark, VoiceIcon } from "../common/icons";
import { DeviceMenu, VolumeTrack, setVolumeOf, volumeOf, type AudioKind } from "./AudioPanel";

// 语音舞台：在语音频道中时，在所有服务器与私信页面顶部显示语音频道与成员卡片。参照 Oopz 网页端。
export const stageShown = () => voiceActive() && voice.phase !== "connecting" && voice.me !== null;

function channelOf(id: string) {
  for (const g of session.guilds) for (const grp of g.groups) for (const c of grp.channels) if (c.id === id) return c;
  return undefined;
}

// 与语音栏一致：悬停或键盘聚焦时在按钮上方显示竖向音量条，拖动期间保持显示，离开 150 ms 后隐藏。
// 舞台可滚动，音量条渲染到 body 并按按钮位置固定定位，不被裁剪。
function HoverVolume(p: { label: string; value: () => number; set: (v: number) => void; children: JSX.Element; onContextMenu?: (e: MouseEvent) => void }) {
  const [at, setAt] = createSignal<{ x: number; y: number } | null>(null);
  const [dragging, setDragging] = createSignal(false);
  let wrap!: HTMLDivElement;
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));
  const show = () => {
    clearTimeout(timer);
    const r = wrap.getBoundingClientRect();
    setAt({ x: r.left + r.width / 2, y: window.innerHeight - r.top });
  };
  const hide = () => {
    clearTimeout(timer);
    timer = setTimeout(() => !dragging() && setAt(null), 150);
  };
  return (
    <div
      ref={wrap}
      class="stage-ctrl-wrap"
      onPointerEnter={show}
      onPointerLeave={hide}
      onFocusIn={show}
      onFocusOut={(e) => !wrap.contains(e.relatedTarget as Node) && setAt(null)}
      onContextMenu={(e) => { setAt(null); p.onContextMenu?.(e); }}
    >
      {p.children}
      <Show when={at()}>
        {(a) => (
          <Portal>
            <div class="stage-vol-anchor" style={{ left: `${a().x}px`, bottom: `${a().y}px` }} onPointerEnter={show} onPointerLeave={hide}>
              <VolumeTrack label={p.label} value={p.value} set={p.set} onDrag={(d) => { setDragging(d); if (!d) hide(); }} />
            </div>
          </Portal>
        )}
      </Show>
    </div>
  );
}

function Card(p: { peer: VoicePeer }) {
  const self = () => p.peer.id === voice.me?.id;
  const name = () => p.peer.name || "未知用户";
  // 本人服务器闭麦与静音优先于本地开关。
  const micOff = () => p.peer.serverMute || mic.muted || micFailed();
  const headsetOff = () => Boolean(p.peer.serverDeafen) || output.deafened;
  const speaking = () => isSpeaking(p.peer.id) && !p.peer.serverMute && !(self() ? mic.muted : p.peer.micOff);
  const peerMuted = () => peerVolume(p.peer.id) === 0;
  // 右键麦克风或耳机打开设备菜单，以按钮为中心定位。
  const [menu, setMenu] = createSignal<{ kind: AudioKind; x: number; y: number } | null>(null);
  const openMenu = (kind: AudioKind) => (e: MouseEvent) => {
    e.preventDefault();
    const r = (e.currentTarget as Element).getBoundingClientRect();
    // 菜单宽 232px，靠近窗口边缘时向内收。
    const half = Math.min(232, window.innerWidth - 24) / 2;
    const x = Math.max(8 + half, Math.min(window.innerWidth - 8 - half, r.left + r.width / 2));
    setMenu((m) => (m?.kind === kind ? null : { kind, x, y: window.innerHeight - r.top }));
  };
  return (
    <div class={`stage-card ${speaking() ? "speaking" : ""}`}>
      <div class="stage-art">
        <Show when={p.peer.avatar} fallback={<span class="stage-initial">{name().slice(0, 1)}</span>}>
          {(src) => <img src={src()} alt="" draggable={false} referrerPolicy="no-referrer" />}
        </Show>
      </div>
      <div class="stage-ctrl">
        <div class="stage-name">{name()}</div>
        <div class="stage-bar">
          <Show when={self()}>
            <HoverVolume label="麦克风音量" value={() => volumeOf("input")} set={(v) => setVolumeOf("input", v)} onContextMenu={openMenu("input")}>
              <button class="stage-btn" aria-busy={mic.busy} aria-pressed={mic.muted} aria-haspopup="menu" aria-label={mic.muted ? "取消静音" : "静音"} onClick={() => !mic.busy && void toggleMic()}>
                {micOff() ? <MicOffIcon size={18} /> : <Mic size={18} />}
              </button>
            </HoverVolume>
          </Show>
          <span class="stage-gap" />
          <Show when={self()} fallback={
            <HoverVolume label={`${name()} 的音量`} value={() => peerVolume(p.peer.id)} set={(v) => void setPeerVolume(p.peer.id, v)}>
              <button class="stage-btn" aria-pressed={peerMuted()} aria-label={peerMuted() ? `取消静音 ${name()}` : `静音 ${name()}`} onClick={() => void togglePeerMute(p.peer.id)}>
                {peerMuted() ? <SpeakerOffMark size={18} /> : <Volume2 size={18} />}
              </button>
            </HoverVolume>
          }>
            <HoverVolume label="扬声器音量" value={() => volumeOf("output")} set={(v) => setVolumeOf("output", v)} onContextMenu={openMenu("output")}>
              <button class="stage-btn" aria-busy={output.busy} aria-pressed={output.deafened} aria-haspopup="menu" aria-label={output.deafened ? "打开耳机" : "关闭耳机"} onClick={() => !output.busy && void toggleDeafen()}>
                {headsetOff() ? <HeadphoneOffIcon size={18} /> : <Headphones size={18} />}
              </button>
            </HoverVolume>
            <button class="stage-btn leave" data-tip="退出频道" aria-label="退出频道" onClick={() => { const c = voice.channelId; void disconnectVoice(); dropSelfFromVoice(c); }}>
              <LogOut size={18} />
            </button>
          </Show>
        </div>
      </div>
      <Show when={menu()} keyed>
        {(m) => (
          <Portal>
            <div class="stage-menu-anchor" style={{ left: `${m.x}px`, bottom: `${m.y}px` }}>
              <DeviceMenu kind={m.kind} x={0} onClose={() => setMenu(null)} />
            </div>
          </Portal>
        )}
      </Show>
    </div>
  );
}

export default function VoiceStage() {
  const peers = () => (voice.me ? [voice.me, ...voice.peers] : voice.peers);
  const ch = () => channelOf(voice.channelId);
  return (
    <section class="voice-stage" aria-label="语音频道">
      <div class="stage-head">
        <span class="stage-icon"><VoiceIcon /></span>
        <span class="stage-title">{ch()?.name ?? "语音频道"}</span>
        <span class="stage-count">{peers().length}{ch()?.limit ? ` / ${ch()!.limit}` : ""}</span>
      </div>
      <div class="stage-grid">
        <For each={peers()}>{(peer) => <Card peer={peer} />}</For>
      </div>
    </section>
  );
}
