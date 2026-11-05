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
