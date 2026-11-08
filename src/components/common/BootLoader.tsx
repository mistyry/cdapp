import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { api, type Saying } from "../../lib/api";
import { startDisc } from "../../lib/discLoader";

// 图形区参照官方 420×330，光盘略小于区域高度；矮窗口时按高度收小。
const MAX = 300;

// 启动加载动画。WebGL2 不可用时显示静态应用标识。
export default function BootLoader() {
  let canvas!: HTMLCanvasElement;
  let shadow!: SVGEllipseElement;
  const [failed, setFailed] = createSignal(false);
  const [saying, setSaying] = createSignal<Saying | null>(null);
  let stop: (() => void) | null = null;
  let disposed = false;

  const size = Math.round(Math.min(MAX, Math.max(160, window.innerHeight * 0.36)));
  onMount(() => {
    api.sayings().then(
      (list) => !disposed && !saying() && list.length && setSaying(list[Math.floor(Math.random() * list.length)]),
      () => {},
    );
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(size * dpr);
    canvas.height = Math.round(size * dpr);
    const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    void startDisc(canvas, {
      still,
      onFrame: ({ dx, dy }) => {
        // 盘升高时阴影缩小变淡
        const lift = (dy + 9) / 18;
        shadow.setAttribute("transform", `translate(${dx} 0) translate(256 300) scale(${1 - lift * 0.15} 1) translate(-256 -300)`);
        shadow.setAttribute("opacity", (0.2 - lift * 0.08).toFixed(3));
      },
    }).then(
      (s) => {
        if (disposed) s?.();
        else if (s) stop = s;
        else setFailed(true);
      },
      () => setFailed(true),
    );
  });
  onCleanup(() => {
    disposed = true;
    stop?.();
  });

  return (
    <div class="boot-screen" role="status" aria-label="加载中">
      <div class="boot-art">
        <div class="boot-disc" style={{ width: `${size}px`, height: `${size}px` }}>
          <Show when={!failed()} fallback={<img src="/logo-light.svg" alt="" draggable={false} />}>
            <svg viewBox="60 60 392 392" aria-hidden="true">
              <ellipse ref={shadow} cx="256" cy="300" rx="150" ry="22" fill="#2283D0" opacity=".16" />
            </svg>
            <canvas ref={canvas} aria-hidden="true" />
          </Show>
        </div>
      </div>
      <Show when={saying()}>
        {(s) => (
          <div class="boot-saying">
            <p class="words">"{s().words}"</p>
            <p class="by">——{s().role_name} 《{s().game}》</p>
          </div>
        )}
      </Show>
      <p class="boot-label">加载中...</p>
    </div>
  );
}
