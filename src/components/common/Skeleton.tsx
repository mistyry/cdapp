import { For, Show, type JSX } from "solid-js";

const LABELS = [64, 92, 52, 120, 78, 104, 60, 86];
const NAMES = [72, 96, 58, 112, 84, 66, 100, 76];
const MESSAGES: { head: boolean; lines: number[] }[] = [
  { head: true, lines: [62] },
  { head: true, lines: [88, 41] },
  { head: false, lines: [53] },
  { head: true, lines: [74] },
  { head: true, lines: [91, 84, 36] },
  { head: false, lines: [47] },
  { head: false, lines: [68] },
  { head: true, lines: [58] },
  { head: true, lines: [79, 28] },
];

const pick = <T,>(list: T[], i: number) => list[i % list.length];
const range = (n: number) => Array.from({ length: n }, (_, i) => i);

function Bone(p: { w?: number | string; h?: number; round?: boolean; class?: string }) {
  return (
    <i class={`bone ${p.round ? "round" : ""} ${p.class ?? ""}`}
      style={{ width: typeof p.w === "number" ? `${p.w}px` : p.w, height: p.h === undefined ? undefined : `${p.h}px` }} />
  );
}

function Loading(p: { children: JSX.Element }) {
  return (
    <div class="skel" role="status" aria-label="加载中">
      <div aria-hidden="true">{p.children}</div>
    </div>
  );
}

export function ChannelSkeleton() {
  return (
    <Loading>
      <For each={[3, 4, 2]}>
        {(n, g) => (
          <>
            <div class="group"><span class="caret"><Bone w={10} h={10} /></span><span class="name"><Bone w={pick(NAMES, g() + 2) - 20} h={10} /></span></div>
            <For each={range(n)}>
              {(i) => (
                <div class="channel"><div class="channel-row">
                  <span class="type"><Bone w={16} h={16} /></span>
                  <span class="label"><Bone w={pick(LABELS, g() * 4 + i)} h={12} /></span>
                </div></div>
              )}
            </For>
          </>
        )}
      </For>
    </Loading>
  );
}

export function MessageSkeleton() {
  return (
    <Loading>
      <For each={MESSAGES}>
        {(m, k) => (
          <div class={`msg ${m.head ? "first" : ""}`}><div class="msg-wrap">
            <Show when={m.head}>
              <Bone class="avatar" w={40} h={40} round />
              <div class="msg-infos"><Bone w={pick(NAMES, k())} h={14} /><span class="msg-time"><Bone w={38} h={10} /></span></div>
            </Show>
            <For each={m.lines}>{(w) => <div class="msg-content"><Bone w={`${w}%`} h={14} /></div>}</For>
          </div></div>
        )}
      </For>
    </Loading>
  );
}

function PersonRow(p: { i: number; dm?: boolean }) {
  return (
    <div class={p.dm ? "dm" : "member"}>
      <Bone class="avatar" w={32} h={32} round />
      <Show when={p.dm} fallback={<div class="info"><div class="nm"><Bone w={pick(NAMES, p.i)} h={12} /></div></div>}>
        <div class="dm-info">
          <div class="dm-name"><Bone w={pick(NAMES, p.i)} h={12} /></div>
          <div class="dm-preview"><Bone w={pick(LABELS, p.i) + 40} h={10} /></div>
        </div>
      </Show>
    </div>
  );
}

export function MemberSkeleton() {
  return (
    <Loading>
      <For each={[6, 4]}>
        {(n, g) => (
          <>
            <div class="member-title"><Bone w={g() ? 52 : 60} h={12} /></div>
            <For each={range(n)}>{(i) => <PersonRow i={g() * 6 + i} />}</For>
          </>
        )}
      </For>
    </Loading>
  );
}

export function DmSkeleton() {
  return (
    <Loading>
      <For each={range(7)}>{(i) => <PersonRow i={i} dm />}</For>
    </Loading>
  );
}
