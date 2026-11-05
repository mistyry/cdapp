import { Show } from "solid-js";

// 服务器图标。侧边栏与表情页签共用同一份渲染：始终按 48px 绘制，较小尺寸整体等比缩放，
// 因此缩小后的图形、圆角、底色与无图标时的名称文字与侧边栏完全一致。
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
