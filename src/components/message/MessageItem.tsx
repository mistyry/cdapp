import { For, Match, Show, Switch, createSignal } from "solid-js";
import { Ellipsis, Pencil, Reply, SmilePlus, Trash2 } from "lucide-solid";
import type { Message, Reaction } from "../../stores/types";
import { emojiUrl, safeUrl } from "../../lib/kmarkdown";
import { Kind, isText, label, parseCards, sizeText } from "../../lib/content";
import { canDelete, canEdit, canReact, deleteMessage, editMessage, isReacting, jumpTo, plainContent, session, toggleReaction } from "../../stores/session";
import { currentKey, currentMsgKey, setUi } from "../../stores/ui";
import Avatar from "../common/Avatar";
import Kmd, { open } from "./Kmd";

// 图片只接受 http/https 地址；加载失败时显示说明与可点击的原地址，并保持固定高度。
function Picture(p: { src: string; alt?: string }) {
  const [broken, setBroken] = createSignal(false);
  const url = () => safeUrl(p.src);
  return (
    <Show when={url() && !broken()} fallback={
      <div class="msg-broken">
        图片无法显示{p.alt ? `：${p.alt}` : ""}
        <Show when={url()}>{(u) => <a class="link" href={u()} rel="noopener noreferrer" onClick={(e) => open(e, u())}>打开原图</a>}</Show>
      </div>
    }>
      <img class="msg-image" src={url()!} alt={p.alt ?? ""} loading="lazy" referrerPolicy="no-referrer" onError={() => setBroken(true)} />
    </Show>
  );
}

function Cards(p: { content: string }) {
  const cards = () => parseCards(p.content);
  return (
    <Show when={cards()?.length} fallback={<div class="msg-fallback">[卡片消息]</div>}>
      <For each={cards()!}>
        {(c) => (
          <div class="msg-card">
            <For each={c.parts}>
              {(part) => (
                <Switch>
                  <Match when={part.t === "image" && part}>{(x) => <Picture src={x().src} alt={x().alt} />}</Match>
                  <Match when={part.t === "file" && part}>
                    {(x) => (
                      <div class="msg-file">
                        {"[文件] "}
                        <Show when={x().url} fallback={<span>{x().name || "文件"}</span>}>
                          {(u) => <a class="link" href={u()} rel="noopener noreferrer" onClick={(e) => open(e, u())}>{x().name || "下载"}</a>}
                        </Show>
                        <Show when={x().size}><span class="size">{sizeText(x().size)}</span></Show>
                      </div>
                    )}
                  </Match>
                  <Match when={part.t === "header" && part}>{(x) => <div class="card-header">{x().v}</div>}</Match>
                  <Match when={part.t === "text" && part}>{(x) => (x().kmd ? <Kmd text={x().v} /> : <div class="kmd-p">{x().v}</div>)}</Match>
                </Switch>
              )}
            </For>
            <Show when={c.other || !c.parts.length}><div class="msg-fallback">[卡片消息中有暂不支持显示的内容]</div></Show>
          </div>
        )}
      </For>
    </Show>
  );
}

function Body(p: { msg: Message }) {
  return (
    <Switch fallback={<div class="msg-fallback">{label(p.msg.type, p.msg.attachment)}</div>}>
      <Match when={isText(p.msg.type)}><Kmd text={p.msg.content} /></Match>
      <Match when={p.msg.type === Kind.IMAGE}><Picture src={p.msg.content} /></Match>
      <Match when={p.msg.type === Kind.CARD}><Cards content={p.msg.content} /></Match>
      <Match when={p.msg.attachment?.url}>
        {(u) => (
          <div class="msg-file">
            {label(p.msg.type) + " "}
            <a class="link" href={u()} rel="noopener noreferrer" onClick={(e) => open(e, u())}>{p.msg.attachment!.name || "下载"}</a>
            <Show when={p.msg.attachment!.size}><span class="size">{sizeText(p.msg.attachment!.size)}</span></Show>
          </div>
        )}
      </Match>
    </Switch>
  );
}

// 常用 Unicode 表情。回应请求直接传表情字符。
const QUICK = ["👍", "❤️", "😂", "😮", "😢", "🎉", "👀", "🙏"];

// Unicode 表情直接显示字符；服务器表情（ID 含 /）按图片显示，失败时显示名称。
function ReactionIcon(p: { r: Reaction }) {
  const [broken, setBroken] = createSignal(false);
  return (
    <Show when={p.r.id.includes("/") && !broken()} fallback={<span>{p.r.id.includes("/") ? `:${p.r.name}:` : p.r.id}</span>}>
      <img class="emoji" src={emojiUrl(p.r.id)} alt={`:${p.r.name}:`} onError={() => setBroken(true)} />
    </Show>
  );
}

function Editor(p: { msg: Message; channelId: string; done: () => void }) {
  const [text, setText] = createSignal(p.msg.content);
  const save = async () => {
    const v = text();
    if (!v.trim()) return;
    p.done();
    await editMessage(p.channelId, p.msg.id, v);
  };
  return (
    <div class="msg-edit">
      <textarea ref={(el) => queueMicrotask(() => el.focus())} value={text()} onInput={(e) => setText(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") p.done();
          else if (e.key === "Enter" && !e.shiftKey && !e.isComposing) (e.preventDefault(), void save());
        }} />
      <div class="msg-edit-tip">Esc <a onClick={p.done}>取消</a> · Enter <a onClick={() => void save()}>保存</a></div>
    </div>
  );
}

export default function MessageItem(p: { msg: Message; first: boolean }) {
  const [editing, setEditing] = createSignal(false);
  const [menu, setMenu] = createSignal<"" | "react" | "more">("");
  const ch = () => currentMsgKey();
  const busy = () => Boolean(p.msg.pending);
  // 删除需二次确认：首次点击变为“确认删除”。
  const [confirming, setConfirming] = createSignal(false);
  const remove = () => {
    if (!confirming()) return setConfirming(true);
    setMenu("");
    setConfirming(false);
    void deleteMessage(ch(), p.msg.id);
  };
  const react = (r: { id: string; name: string }) => {
    setMenu("");
    void toggleReaction(ch(), p.msg.id, r);
  };
  const author = () => session.users[p.msg.author] ?? { id: p.msg.author, name: "未知用户", online: false };
  const reply = () => {
    if (!currentKey()) return;
    setUi("replies", currentKey(), {
      id: p.msg.id, authorId: p.msg.author, author: author().name, type: p.msg.type, content: p.msg.content,
      text: plainContent(p.msg.type, p.msg.content, p.msg.attachment),
    });
  };
  return (
    <div class={`msg ${p.first ? "first" : ""} ${p.msg.pending ? `pending ${p.msg.pending}` : ""}`} onMouseLeave={() => (setMenu(""), setConfirming(false))}>
      <div class="msg-wrap">
        <Show when={p.first} fallback={<span class="msg-small-time">{p.msg.short}</span>}>
          <Avatar user={author()} />
          <div class="msg-infos">
            <span class="msg-name" style={{ color: author().color }}>{author().name}</span>
            <span class="msg-time">{p.msg.time}</span>
          </div>
        </Show>
        <div class="msg-content">
          <Show when={p.msg.quote}>
            {(q) => (
              <div>
                <button class={`quote ${q().gone ? "gone" : ""}`} title={q().gone ? "原消息已删除" : "跳转到原消息"}
                  onClick={() => ch() && void jumpTo(ch(), q().id)}>
                  <b>{q().authorId ? session.users[q().authorId!]?.name ?? q().author : q().author}</b>
                  <span>{q().gone ? "原消息已删除" : isText(q().type) ? plainContent(q().type, q().content) : q().text}</span>
                </button>
              </div>
            )}
          </Show>
          <Show when={editing()} fallback={<Body msg={p.msg} />}>
            <Editor msg={p.msg} channelId={ch()} done={() => setEditing(false)} />
          </Show>
          <Show when={p.msg.updatedAt && !editing()}><span class="msg-edited" title="已编辑">（已编辑）</span></Show>
          <Show when={p.msg.pending}>{(s) => <span class="msg-pending">{s() === "editing" ? "保存中…" : "删除中…"}</span>}</Show>
          <Show when={p.msg.reactions?.length}>
            <div class="reactions">
              <For each={p.msg.reactions}>
                {(r) => (
                  <button class={`reaction ${r.me ? "me" : ""}`} title={r.name}
                    disabled={busy() || !canReact(ch()) || isReacting(ch(), p.msg.id, r.id)} onClick={() => react(r)}>
                    <ReactionIcon r={r} /><span>{r.count}</span>
                  </button>
                )}
              </For>
            </div>
          </Show>
        </div>
        <Show when={!busy() && !editing()}>
          <div class={`msg-menu ${menu() ? "open" : ""}`}>
            <Show when={canReact(ch())}>
              <button class="icon-btn" title="添加回应" onClick={() => setMenu(menu() === "react" ? "" : "react")}><SmilePlus size={20} stroke-width={1.6} /></button>
            </Show>
            <button class="icon-btn" title="回复" onClick={reply}><Reply size={20} stroke-width={1.6} /></button>
            <Show when={canEdit(ch(), p.msg) || canDelete(ch(), p.msg)}>
              <button class="icon-btn" title="更多" onClick={() => setMenu(menu() === "more" ? "" : "more")}><Ellipsis size={20} /></button>
            </Show>
            <Show when={menu() === "react"}>
              <div class="msg-pop emoji-pop">
                <For each={QUICK}>{(e) => <button class="icon-btn" onClick={() => react({ id: e, name: e })}>{e}</button>}</For>
              </div>
            </Show>
            <Show when={menu() === "more"}>
              <div class="msg-pop">
                <Show when={canEdit(ch(), p.msg)}>
                  <button onClick={() => (setMenu(""), setEditing(true))}><Pencil size={16} />编辑</button>
                </Show>
                <Show when={canDelete(ch(), p.msg)}>
                  <button class="danger" onClick={remove}><Trash2 size={16} />{confirming() ? "确认删除" : "删除"}</button>
                </Show>
              </div>
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
}
