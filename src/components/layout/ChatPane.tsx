import { For, Match, Show, Switch, createEffect, createSignal, on } from "solid-js";
import { CirclePlus, FileText, Image, Search, Smile, Users, X } from "lucide-solid";
import { canSend, cancelUpload, dmKey, dmOf, loadDmHistory, loadDmOlder, discard, stageFiles, submit as submitAll, unstage, loadAccess, loadGuilds, sendBlock, jumpLatest, loadHistory, loadNewer, loadOlder, retry, selectGuild, session, verify } from "../../stores/session";
import { DM_VIEW, currentChatCode, currentKey, currentMsgKey, ui, setUi } from "../../stores/ui";
import MessageList from "../message/MessageList";
import EmojiInput, { type EmojiInputApi } from "../message/EmojiInput";
import EmojiPicker from "../message/EmojiPicker";
import Kmd from "../message/Kmd";
import { KookHash } from "../common/icons";
import { sizeText } from "../../lib/content";

export default function ChatPane() {
  const channel = () => {
    const t = ui.target;
    if (t.kind !== "channel") return undefined;
    return session.guilds.find((g) => g.id === t.guildId)?.groups.flatMap((x) => x.channels).find((c) => c.id === t.channelId);
  };
  const sendable = () => canSend();
  // 不能发送的原因。权限未知时不视为已获授权。
  const blocked = () => {
    if (ui.target.kind === "dm") return dmOf(ui.target.chatId)?.target.system ? "系统会话不能回复" : "私信会话不可用";
    switch (sendBlock()) {
      case "loading": return "正在确认发送权限";
      case "failed": return "无法确认发送权限";
      case "denied": return "此频道为只读，你没有发言的权限";
      default: return "无法在此发送消息";
    }
  };
  const text = () => ui.drafts[currentKey()] ?? "";
  const reply = () => ui.replies[currentKey()];
  const clearReply = () => currentKey() && setUi("replies", currentKey(), undefined);
  const empty = () => {
    if (ui.view === DM_VIEW) return session.dmList.loaded && !session.dms.length ? "还没有私信" : "选择一个私信会话";
    const g = session.guilds.find((x) => x.id === ui.view);
    if (session.loadFailed) return <>服务器列表加载失败 <button class="retry" onClick={loadGuilds}>重试</button></>;
    if (!g) return session.guilds.length ? "" : "尚未加入任何服务器";
    if (g.failed) return <>频道加载失败 <button class="retry" onClick={() => selectGuild(g.id)}>重试</button></>;
    return g.loaded ? "没有可用的文字频道" : "加载中";
  };
  const outbox = () => session.outbox[currentMsgKey()] ?? [];
  // 提交即转入待发列表并清空输入，等待响应期间的新输入不受影响，也无法重复提交同一草稿。
  const submit = () => {
    const key = currentKey();
    if (submitAll(ui.target, text(), reply())) {
      setUi("drafts", key, "");
      setUi("replies", key, undefined);
    }
  };
  const staged = () => ui.staged[currentKey()] ?? [];
  // 附件被拒绝的原因（空文件、超限），下次选择或切换会话时清除。
  const [rejects, setRejects] = createSignal<string[]>([]);
  createEffect(on(currentKey, () => setRejects([]), { defer: true }));
  const [dragging, setDragging] = createSignal(false);
  const addFiles = (files: File[]) => setRejects(files.length ? stageFiles(ui.target, files) : []);
  let picker: HTMLInputElement | undefined;
  const onPaste = (e: ClipboardEvent) => {
    const files = [...(e.clipboardData?.files ?? [])];
    if (!files.length) return;
    e.preventDefault();
    addFiles(files);
  };
  const hasFiles = (e: DragEvent) => [...(e.dataTransfer?.types ?? [])].includes("Files");
  const onDragOver = (e: DragEvent) => {
    if (!hasFiles(e) || !sendable()) return;
    e.preventDefault();
    setDragging(true);
  };
  const onDrop = (e: DragEvent) => {
    setDragging(false);
    if (!hasFiles(e)) return;
    e.preventDefault();
    if (sendable()) addFiles([...(e.dataTransfer?.files ?? [])]);
  };

  let input: EmojiInputApi | undefined;
  const [picking, setPicking] = createSignal(false);
  let emojiBtn: HTMLButtonElement | undefined;
  // 切换会话时关闭选择器，避免插入到另一会话的草稿。
  createEffect(on(currentKey, () => setPicking(false), { defer: true }));
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      submit();
    }
    if (e.key === "Escape") clearReply();
  };

  // 待发消息（频道与私信共用，按当前会话的消息键）。
  const outboxView = () => (
    <>
              <For each={outbox()}>
                {(o) => (
                  <div class={`outgoing ${o.status}`}>
                    <Show when={o.reply}>{(q) => <div class="quote"><b>{q().author}</b><span>{q().text}</span></div>}</Show>
                    <Show when={o.file} fallback={<div class="text"><Kmd text={o.text} /></div>}>
                      {(f) => (
                        <div class="text file">
                          {f().image ? <Image size={16} /> : <FileText size={16} />} {f().name} <span class="size">{sizeText(f().size)}</span>
                          <Show when={o.status === "uploading"}>
                            <progress max={f().size} value={f().sent} />
                          </Show>
                        </div>
                      )}
                    </Show>
                    <div class="state">
                      <Switch>
                        <Match when={o.status === "uploading"}>
                          上传中 {o.file && o.file.size ? Math.floor((o.file.sent / o.file.size) * 100) : 0}%
                          <button onClick={() => cancelUpload(o.channelId, o.localId)}>取消</button>
                        </Match>
                        <Match when={o.status === "sending"}>发送中</Match>
                        <Match when={o.status === "failed"}>
                          {o.file && !o.file.url ? "上传失败" : "发送失败"}{o.error ? `：${o.error}` : ""}
                          <button onClick={() => retry(o.channelId, o.localId)}>重试</button>
                          <button onClick={() => discard(o.channelId, o.localId, true)}>编辑</button>
                          <button onClick={() => discard(o.channelId, o.localId, false)}>删除</button>
                        </Match>
                        <Match when={o.status === "uncertain"}>
                          无法确认是否已发送{o.error ? `：${o.error}` : ""}
                          <button onClick={() => verify(o.channelId, o.localId)}>核对</button>
                          <button onClick={() => discard(o.channelId, o.localId, false)}>忽略</button>
                        </Match>
                      </Switch>
                    </div>
                  </div>
                )}
              </For>
    </>
  );

  // 输入区（频道与私信共用）。目标在提交时由 ui.target 固定。
  const composer = (placeholder: () => string) => (
    <div class="send-ctrl">
      <Show when={reply()}>
        {(r) => (
          <div class="reply-bar">
            <span>回复 <b>{r().author}</b>：{r().text}</span>
            <button class="icon-btn" data-tip="取消回复" aria-label="取消回复" onClick={clearReply}><X size={16} /></button>
          </div>
        )}
      </Show>
      <Show when={staged().length || rejects().length}>
        <div class="staged">
          <For each={staged()}>
            {(f) => (
              <span class="chip">
                {f.image ? <Image size={14} /> : <FileText size={14} />} {f.name} <span class="size">{sizeText(f.size)}</span>
                <button class="icon-btn" data-tip="移除" aria-label="移除" onClick={() => unstage(currentKey(), f.id)}><X size={14} /></button>
              </span>
            )}
          </For>
          <For each={rejects()}>{(r) => <span class="reject">{r}</span>}</For>
        </div>
      </Show>
      <Show when={ui.target.kind === "channel" && sendBlock() === "failed"}>
        <div class="reply-bar"><span>无法确认发送权限</span><button class="retry" onClick={() => ui.target.kind === "channel" && loadAccess(ui.target.guildId)}>重试</button></div>
      </Show>
      <div class="send-wrap">
        <div class="left">
          <button ref={emojiBtn} class={`icon-btn ${picking() ? "on" : ""}`} data-tip="表情" aria-label="表情" aria-haspopup="dialog" aria-expanded={picking()}
            disabled={!sendable()} onClick={() => setPicking((v) => !v)}><Smile size={22} stroke-width={1.6} /></button>
        </div>
        <Show when={picking() && sendable()}>
          <EmojiPicker class="composer-picker" trigger={() => emojiBtn}
            purpose={{ use: "send", guildId: ui.target.kind === "channel" ? ui.target.guildId : null }}
            onPick={(x) => input?.insert(x)}
            onClose={(refocus) => { setPicking(false); if (refocus) input?.focus(); }} />
        </Show>
        <EmojiInput
          ref={(a) => (input = a)}
          disabled={!sendable()}
          placeholder={sendable() ? placeholder() : blocked()}
          value={text()}
          onInput={(v) => currentKey() && setUi("drafts", currentKey(), v)}
          onKeyDown={onKey}
          onPaste={onPaste}
        />
        <div class="right">
          <input ref={picker} type="file" multiple hidden onChange={(e) => { addFiles([...(e.currentTarget.files ?? [])]); e.currentTarget.value = ""; }} />
          <button class="icon-btn" data-tip="上传文件" aria-label="上传文件" disabled={!sendable()} onClick={() => picker?.click()}><CirclePlus size={22} stroke-width={1.6} /></button>
        </div>
      </div>
    </div>
  );

  // 私信会话。草稿、引用、附件与待发消息按 dm:chat_code 键存放，与频道互不影响；只经私信接口发送。
  // 以 chat_code 为键挂载，会话资料刷新不重建消息列表。
  const dmView = (code: string) => {
    const d = () => dmOf(code)?.target;
    const name = () => (d() ? d()!.nickname || d()!.username || "未知用户" : "私信");
    return (
      <>
        <header class="room-header">
          <span class="title">{name()}</span>
          <Show when={d()?.identifyNum}><span class="topic">#{d()!.identifyNum}</span></Show>
        </header>
        <MessageList
          messages={session.messages[dmKey(code)] ?? []}
          history={session.history[dmKey(code)]}
          onOlder={() => loadDmOlder(code)}
          onRetry={() => loadDmHistory(code)}
          onNewer={() => {}}
          onLatest={() => {}}
          focus={session.focus?.channelId === dmKey(code) ? session.focus : null}
          footerKey={outbox().length}
          footer={outboxView()}
        />
        {composer(() => `给 ${name()} 发私信`)}
      </>
    );
  };

  return (
    <main class={`chat ${dragging() ? "dropping" : ""}`} onDragOver={onDragOver} onDragLeave={(e) => e.currentTarget === e.target && setDragging(false)} onDrop={onDrop}>
      <Show when={channel()} fallback={<Show when={currentChatCode()} keyed fallback={<div class="chat-empty">{empty()}</div>}>{dmView}</Show>}>
        {(c) => (
          <>
            <header class="room-header">
              <span class="hash"><KookHash /></span>
              <span class="title">{c().name}</span>
              <span class="topic" />
              <div class="tools">
                <button class="icon-btn" data-tip="搜索" aria-label="搜索"><Search size={20} /></button>
                <button class={`icon-btn ${ui.showMembers ? "on" : ""}`} data-tip="成员列表" aria-label="成员列表" onClick={() => setUi("showMembers", (v) => !v)}>
                  <Users size={20} />
                </button>
              </div>
            </header>
            <Show when={c().id} keyed>
              {(id) => (
                <MessageList
                  messages={session.messages[id] ?? []}
                  history={session.history[id]}
                  onOlder={() => loadOlder(id)}
                  onRetry={() => loadHistory(id)}
                  onNewer={() => loadNewer(id)}
                  onLatest={() => jumpLatest(id)}
                  focus={session.focus?.channelId === id ? session.focus : null}
                  footerKey={outbox().length}
                  footer={<>
                    {outboxView()}
                  </>}
                />
              )}
            </Show>
            {composer(() => `给 #${c().name} 发消息`)}
          </>
        )}
      </Show>
    </main>
  );
}
