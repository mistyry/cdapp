import { Show } from "solid-js";

export default function GuildIcon(p: { name: string; icon?: string; size?: number; onClick?: () => void }) {
  const scale = () => (p.size ?? 48) / 48;
  return (
    <span class="guild-icon" style={{ width: `${p.size ?? 48}px`, height: `${p.size ?? 48}px` }}>
      <span class="guild-btn" style={scale() === 1 ? undefined : { transform: `scale(${scale()})`, "transform-origin": "0 0" }} onClick={p.onClick}>
        <Show when={p.icon} fallback={p.name.slice(0, 2)}>{(src) => <img src={src()} alt="" draggable={false} referrerPolicy="no-referrer" />}</Show>
      </span>
    </span>
  );
}
