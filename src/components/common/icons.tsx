// 自绘图标：lucide 中没有对应样式的放这里。尺寸与 KOOK 一致，描边随 currentColor。
export function Caret() {
  return (
    <svg width="8" height="8" viewBox="0 0 8 8" fill="currentColor"><path d="M0.6 2.2h6.8L4 6z" /></svg>
  );
}
export function KookHash(p: { size?: number }) {
  const s = p.size ?? 16;
  return (
    <svg width={s} height={s} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
      <path d="M6 2.5 4.8 13.5M11.2 2.5 10 13.5M2.5 5.8h11.2M2.3 10.2h11.2" />
    </svg>
  );
}
