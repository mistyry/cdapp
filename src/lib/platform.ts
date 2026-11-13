export type Platform = "macos" | "windows" | "linux" | "other";

export function platformOf(ua: string): Platform {
  if (ua.includes("Macintosh")) return "macos";
  if (ua.includes("Windows")) return "windows";
  if (ua.includes("Linux")) return "linux";
  return "other";
}

export const platform: Platform = typeof navigator === "undefined" ? "other" : platformOf(navigator.userAgent);
export const isMac = platform === "macos";
export const isWindows = platform === "windows";

export async function hasBackdrop(p: Platform = platform): Promise<boolean> {
  if (p === "macos") return true;
  if (p !== "windows") return false;
  const data = (navigator as Navigator & { userAgentData?: { getHighEntropyValues(h: string[]): Promise<{ platformVersion?: string }> } }).userAgentData;
  try {
    const v = (await data?.getHighEntropyValues(["platformVersion"]))?.platformVersion;
    return Number.parseInt(v ?? "", 10) >= 13;
  } catch {
    return false;
  }
}

export const dragAttr = (inset: boolean, v: "" | "deep") => (inset ? v : undefined);
