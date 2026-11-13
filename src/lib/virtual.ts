export function prefix(heights: number[]): number[] {
  const out = new Array<number>(heights.length + 1);
  out[0] = 0;
  for (let i = 0; i < heights.length; i++) out[i + 1] = out[i] + heights[i];
  return out;
}

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

export function visibleRange(offsets: number[], top: number, height: number, overscan: number): [number, number] {
  const n = offsets.length - 1;
  if (n <= 0) return [0, 0];
  const start = indexAt(offsets, Math.max(0, top - overscan));
  const end = Math.min(n, indexAt(offsets, top + height + overscan) + 1);
  return [start, Math.max(start, end)];
}
