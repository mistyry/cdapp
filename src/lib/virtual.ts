// 窗口化列表的纯计算部分：按实测或估计高度求偏移，按滚动位置求需要渲染的区间。

// offsets[i] 为第 i 项顶部位置，offsets[n] 为总高度。
export function prefix(heights: number[]): number[] {
  const out = new Array<number>(heights.length + 1);
  out[0] = 0;
  for (let i = 0; i < heights.length; i++) out[i + 1] = out[i] + heights[i];
  return out;
}

// 底部超过 y 的第一项（y 落在该项内或之前）。
export function indexAt(offsets: number[], y: number): number {
  let lo = 0;
  let hi = offsets.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (offsets[mid + 1] > y) hi = mid;
    else lo = mid + 1;
  }
  return Math.min(lo, Math.max(0, offsets.length - 2));
}

// 可视区 [top, top + height) 前后各扩展 overscan 像素，返回 [start, end)。
export function visibleRange(offsets: number[], top: number, height: number, overscan: number): [number, number] {
  const n = offsets.length - 1;
  if (n <= 0) return [0, 0];
  const start = indexAt(offsets, Math.max(0, top - overscan));
  const end = Math.min(n, indexAt(offsets, top + height + overscan) + 1);
  return [start, Math.max(start, end)];
}
