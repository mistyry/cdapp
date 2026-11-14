import { For, Show, createSignal } from "solid-js";
import { Portal } from "solid-js/web";
import { ChevronDown, ChevronUp, Headphones, LogOut, Mic, MonitorPlay, ScreenShare, Volume2 } from "lucide-solid";
import { dropSelfFromVoice, liveOf, session } from "../../stores/session";
import { share } from "../../stores/share";
import { selfWatchable, startWatch, watch, watching } from "../../stores/watch";
import ScreenView from "./ScreenView";
import { disconnectVoice, isSpeaking, mic, micFailed, output, setPeerVolume, toggleDeafen, toggleMic, togglePeerMute, voice, voiceActive, volumeOf as peerVolume, type VoicePeer } from "../../stores/voice";
import { HeadphoneOffIcon, MicOffIcon, SpeakerOffMark, VoiceIcon } from "../common/icons";
import { DeviceMenu, HoverVolume, setVolumeOf, volumeOf, type AudioKind } from "./AudioPanel";

export const stageShown = () => voiceActive() && voice.phase !== "connecting" && voice.me !== null;

function channelOf(id: string) {
  for (const g of session.guilds) for (const grp of g.groups) for (const c of grp.channels) if (c.id === id) return c;
  return undefined;
}

function Card(p: { peer: VoicePeer }) {
  const self = () => p.peer.id === voice.me?.id;
  const name = () => p.peer.name || "未知用户";
  const live = () => (self() ? share.phase === "live" : Boolean(liveOf(voice.channelId, p.peer.id)));
  const micOff = () => p.peer.serverMute || mic.muted || micFailed();
  const headsetOff = () => Boolean(p.peer.serverDeafen) || output.deafened;
  const speaking = () => isSpeaking(p.peer.id) && !p.peer.serverMute && !(self() ? mic.muted : p.peer.micOff);
  const peerMuted = () => peerVolume(p.peer.id) === 0;
  const [menu, setMenu] = createSignal<{ kind: AudioKind; x: number; y: number } | null>(null);
  const openMenu = (kind: AudioKind) => (e: MouseEvent) => {
    e.preventDefault();
    const r = (e.currentTarget as Element).getBoundingClientRect();
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
        <Show when={live()}>
          <span class="stage-live">LIVE</span>
          <span class="stage-sharing">正在屏幕共享</span>
          <Show when={self() ? selfWatchable() : true}>
            <button class="stage-watch" onClick={() => void startWatch(voice.channelId, p.peer.id)}>
              <MonitorPlay size={18} />观看屏幕共享
            </button>
          </Show>
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
  const [collapsed, setCollapsed] = createSignal(false);
  const anyLive = () => share.phase === "live" || voice.peers.some((p) => liveOf(voice.channelId, p.id));
  return (
    <section class={`voice-stage ${watching() && watch.mode === "stage" ? "watching" : ""} ${collapsed() ? "collapsed" : ""}`} aria-label="语音频道">
      <div class="stage-head">
        <span class="stage-icon"><VoiceIcon /></span>
        <span class="stage-title">{ch()?.name ?? "语音频道"}</span>
        <span class="stage-count">{peers().length}{ch()?.limit ? ` / ${ch()!.limit}` : ""}</span>
        <Show when={anyLive()}>
          <span class="stage-live-mark" data-tip="正在屏幕共享" aria-label="正在屏幕共享" role="img"><ScreenShare size={16} /></span>
        </Show>
        <span class="stage-gap" />
        <button class="icon-btn stage-fold" aria-expanded={!collapsed()} data-tip={collapsed() ? "展开" : "折叠"} aria-label={collapsed() ? "展开" : "折叠"} onClick={() => setCollapsed(!collapsed())}>
          {collapsed() ? <ChevronDown size={18} /> : <ChevronUp size={18} />}
        </button>
      </div>
      <Show when={!collapsed()}>
        <Show when={watching() && watch.mode === "stage"} fallback={
          <div class="stage-grid">
            <For each={peers()}>{(peer) => <Card peer={peer} />}</For>
          </div>
        }>
          <ScreenView />
        </Show>
      </Show>
    </section>
  );
}
