import { For, Match, Switch, createSignal, type JSX } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { emojiUrl, mentionLabel, parse, type Block, type Inline } from "../../lib/kmarkdown";
import { names, session } from "../../stores/session";

// 链接交给系统浏览器打开，不在应用内导航。href 已经过 safeUrl 校验，只会是 http/https。
export function open(e: MouseEvent, href: string) {
  e.preventDefault();
  openUrl(href).catch(() => window.open(href, "_blank", "noopener,noreferrer"));
}

// 是否提及本人：按用户 ID 判断，@全体成员与 @在线成员同样视为提及。
const mentionsMe = (n: Inline) => n.t === "met" && (n.id === "all" || n.id === "here" || n.id === session.me?.id);

function Emoji(p: { name: string; id: string }) {
  const [broken, setBroken] = createSignal(false);
  return (
    <Switch>
      <Match when={broken()}>{`:${p.name}:`}</Match>
      <Match when={!broken()}>
        <img class="emoji" src={emojiUrl(p.id)} alt={`:${p.name}:`} title={p.name} loading="lazy" referrerPolicy="no-referrer" onError={() => setBroken(true)} />
      </Match>
    </Switch>
  );
}

function Spoiler(p: { children: JSX.Element }) {
  const [shown, setShown] = createSignal(false);
  return <span class={`spoiler ${shown() ? "shown" : ""}`} title={shown() ? undefined : "点击显示"} onClick={() => setShown(true)}>{p.children}</span>;
}

function Inlines(p: { nodes: Inline[] }): JSX.Element {
  return (
    <For each={p.nodes}>
      {(n): JSX.Element => {
        switch (n.t) {
          case "text": return n.v;
          case "br": return <br />;
          case "b": return <strong><Inlines nodes={n.c} /></strong>;
          case "i": return <em><Inlines nodes={n.c} /></em>;
          case "s": return <s><Inlines nodes={n.c} /></s>;
          case "u": return <u><Inlines nodes={n.c} /></u>;
          case "spl": return <Spoiler><Inlines nodes={n.c} /></Spoiler>;
          case "code": return <code>{n.v}</code>;
          case "link": return <a class="link" href={n.href} title={n.href} rel="noopener noreferrer" target="_blank" onClick={(e) => open(e, n.href)}><Inlines nodes={n.c} /></a>;
          case "emj": return <Emoji name={n.name} id={n.id} />;
          default: return <span class={`mention ${mentionsMe(n) ? "me" : ""}`} data-id={n.id}>{mentionLabel(n, names)}</span>;
        }
      }}
    </For>
  );
}

function Blocks(p: { blocks: Block[] }): JSX.Element {
  return (
    <For each={p.blocks}>
      {(b): JSX.Element => {
        if (b.t === "p") return <div class="kmd-p"><Inlines nodes={b.c} /></div>;
        if (b.t === "pre") return <pre class="kmd-pre"><code>{b.v}</code></pre>;
        return <blockquote class="kmd-quote"><Blocks blocks={b.c} /></blockquote>;
      }}
    </For>
  );
}

export default function Kmd(p: { text: string }) {
  return <div class="kmd"><Blocks blocks={parse(p.text)} /></div>;
}
