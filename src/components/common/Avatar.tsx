import { Show } from "solid-js";
import type { User } from "../../stores/types";

export default function Avatar(p: { user: User; size?: number; status?: boolean }) {
  const size = () => p.size ?? 40;
  return (
    <div class="avatar" style={{ width: `${size()}px`, height: `${size()}px`, "font-size": `${Math.round(size() * 0.4)}px` }}>
      <Show when={p.user.avatar} fallback={p.user.name.slice(0, 1)}>
        {(src) => <img src={src()} alt="" draggable={false} />}
      </Show>
      <Show when={p.status && p.user.online}><span class="status" /></Show>
    </div>
  );
}
