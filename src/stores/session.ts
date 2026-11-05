// 会话状态入口。实现按领域拆分在 session/ 下，此处只转出公开接口。
export { session, setSession, report, accountGen, dmKey } from "./session/state";
export { names, plainContent, toGroups } from "./session/convert";
export { boot, enter, logout, expire, skipRestore } from "./session/auth";
export { loadGuilds, openDm, selectGuild, selectChannel, retryAccess } from "./session/nav";
export { loadHistory, loadOlder, loadNewer, jumpLatest, type JumpResult, jumpTo } from "./session/history";
export { canSend, msgKey, loadAccess, refreshAccess, permsOf, visible, sendBlock, voiceBlock, accessState, enterVoice, cancelPassword, pwPrompt } from "./session/access";
export { closeMembers, ensureMembers, refreshMembers, loadMoreMembers } from "./session/members";
export { cardFileSrcs, onUploadProgress, checkFile, stageFiles, unstage, submit, cancelUpload, send, retry, verify, discard } from "./session/outbox";
export { canEdit, canDelete, canReact, editMessage, deleteMessage, isReacting, toggleReaction } from "./session/edit";
export { resync, reconnectNow, networkBack, onGatewayStatus, onEvent } from "./session/gateway";
export { syncUnread, guildUnread, readCurrent } from "./session/unread";
export { loadDms, dmOf, selectDm, openDmWith, loadDmHistory, loadDmOlder, dmUnreadTotal } from "./session/dm";
