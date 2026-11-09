// 自绘图标：lucide 中没有对应样式的放这里。尺寸与 KOOK 一致，描边随 currentColor。
export function Caret() {
  return (
    <svg width="8" height="8" viewBox="0 0 8 8" fill="currentColor"><path d="M0.6 2.2h6.8L4 6z" /></svg>
  );
}

// 受限角标的遮罩：在右下角挖出空隙，角标与主图形不粘连。
const CUT = "M0 0h16v9.2a4.6 4.6 0 0 0-6.8 6.8H0z";

function Frame(p: { size?: number; cut?: boolean; children: any; badge?: any }) {
  const s = p.size ?? 16;
  return (
    <svg width={s} height={s} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      {/* 各实例的遮罩内容相同，重复 ID 不影响结果。 */}
      <defs><mask id="ch-cut" maskUnits="userSpaceOnUse"><path d={CUT} fill="#fff" stroke="none" /></mask></defs>
      <g mask={p.cut ? "url(#ch-cut)" : undefined}>{p.children}</g>
      {p.badge}
    </svg>
  );
}

// 眼睛角标：右下角，表示频道自身限制了查看或连接。
const EyeBadge = () => (
  <g stroke-width="1.3">
    <path d="M9.6 12.6c.8-1.3 1.9-2 3.1-2s2.3.7 3.1 2c-.8 1.3-1.9 2-3.1 2s-2.3-.7-3.1-2z" />
    <circle cx="12.7" cy="12.6" r="0.7" fill="currentColor" stroke="none" />
  </g>
);

const HASH = "M6 2.5 4.8 13.5M11.2 2.5 10 13.5M2.5 5.8h11.2M2.3 10.2h11.2";
const SPEAKER = "M2.5 6.2h2.6L8.6 3v10L5.1 9.8H2.5z";
const WAVES = "M11 6.2v3.6M13.4 4.8v6.4";

export function KookHash(p: { size?: number; restricted?: boolean }) {
  return <Frame size={p.size} cut={p.restricted} badge={p.restricted ? <EyeBadge /> : undefined}><path d={HASH} /></Frame>;
}

export function VoiceIcon(p: { size?: number; restricted?: boolean }) {
  return (
    <Frame size={p.size} cut={p.restricted} badge={p.restricted ? <EyeBadge /> : undefined}>
      <path d={SPEAKER} />
      <path d={WAVES} />
    </Frame>
  );
}

// 密码频道：圆角锁，锁体内为横向钥匙槽。
export function LockIcon(p: { size?: number; restricted?: boolean }) {
  return (
    <Frame size={p.size} cut={p.restricted} badge={p.restricted ? <EyeBadge /> : undefined}>
      <path d="M5 7V5.2a3 3 0 0 1 6 0V7" />
      <rect x="2.8" y="7" width="10.4" height="7" rx="2.4" />
      <path d="M6.8 10.5h2.4" />
    </Frame>
  );
}

// 降噪关闭：沿用 lucide audio-lines-off 的路径，斜杠用警示色。
export function DenoiseOff(p: { size?: number }) {
  const s = p.size ?? 20;
  return (
    <svg width={s} height={s} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M10 10v11M10 3v1.35M14 14v1M14 8v.35M18 5v7.35M2 10v3M22 10v3M6 6v11" />
      <path d="m2 2 20 20" stroke="var(--c-danger)" />
    </svg>
  );
}

// 成员已被本地静音。喇叭取自 lucide Volume2，斜杠为危险色。
export function SpeakerOffMark(p: { size?: number }) {
  const s = p.size ?? 20;
  return (
    <svg width={s} height={s} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z" />
      <path d="M16 9a5 5 0 0 1 0 6M19.364 18.364a9 9 0 0 0 0-12.728" />
      <path d="m2 2 20 20" stroke="var(--c-danger)" />
    </svg>
  );
}
