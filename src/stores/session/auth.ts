import { api, errorText, setUnauthorizedHandler, type Raw } from "../../lib/api";
import { forgetAccountCache } from "../../lib/accountState";
import { resetVoice } from "../voice";
import { resetUi } from "../ui";
import { resetAccess } from "./access";
import { remember, toUser } from "./convert";
import { loadDms, resetDm } from "./dm";
import { resetEdit } from "./edit";
import { resetGateway, startGateway, stopListeners } from "./gateway";
import { resetHistory } from "./history";
import { dropSubscription } from "./members";
import { loadGuilds, resetNav } from "./nav";
import { resetOutbox } from "./outbox";
import { DM_LIST0, report, session, setSession, st } from "./state";
import { resetUnread, syncUnread } from "./unread";

// 流程

export async function boot() {
  reset("boot");
  const gen = st.account;
  try {
    const r = await api.restoreSession();
    if (gen !== st.account) return;
    if (r.status === "ready") await enter(r.me);
    else setSession({ phase: "login", notice: r.status === "expired" ? `登录已失效：${r.message}` : null });
  } catch (e) {
    // 恢复失败但凭据未被确认失效：保留凭据，允许重试或改为扫码。
    if (gen === st.account && session.phase === "boot") setSession({ phase: "restore_failed", error: errorText(e) });
  }
}

export async function enter(meRaw?: Raw) {
  const gen = ++st.account;
  try {
    const me = toUser(meRaw ?? (await api.me()));
    if (gen !== st.account) return;
    me.online = true;
    remember(me);
    setSession({ me, phase: "ready", notice: null, error: null });
    await startGateway(gen);
  } catch (e) {
    if (gen !== st.account) return;
    if (session.phase !== "ready") {
      // 尚未取得用户信息：回到登录页并显示原因，不留空白页。
      setSession({ phase: "login", notice: `登录失败：${errorText(e)}` });
      return;
    }
    report(e);
  }
  // 私信列表随登录加载第一页，私信入口的未读角标与新会话推送不必等首次打开。
  if (gen === st.account) await Promise.all([loadGuilds(), syncUnread(), loadDms()]);
}

// 清除内存中的账号数据，使所有在途请求的结果失效。
export function reset(phase: "login" | "boot", notice: string | null = null) {
  st.epoch++;
  st.account++;
  forgetAccountCache();
  resetHistory();
  resetAccess();
  resetNav();
  dropSubscription();
  stopListeners();
  resetOutbox();
  setSession({ phase, notice, loadFailed: false, me: null, users: {}, guilds: [], folders: null, foldersFailed: false, messages: {}, history: {}, outbox: {}, unread: {}, access: {}, members: null, gateway: "offline", gatewayInfo: null, recovery: null, error: null, refs: { roles: {}, channels: {} }, focus: null, dms: [], dmList: { ...DM_LIST0 }, dmByUser: {} });
  resetDm();
  resetUnread();
  resetGateway();
  resetEdit();
  resetUi();
  resetVoice();
}

export async function logout() {
  reset("login");
  // 后端先停网关与语音再删凭据；删除失败时提示，内存状态已清空。
  await api.logout().catch(report);
}

// 任一请求确认凭据失效：后端已清除凭据并停止网关，前端回到登录页。
export function expire(message: string, gen = st.account) {
  if (gen !== st.account || session.phase !== "ready") return;
  reset("login", `登录已失效：${message}`);
  // 后端网关失效路径已退出语音；REST 失效路径由此补上，重复调用无副作用。
  api.voiceLeave().catch(() => {});
}
setUnauthorizedHandler(expire, () => st.account);

// 恢复失败后放弃保存的登录，改为扫码。凭据在扫码成功后被覆盖。
export function skipRestore() {
  reset("login");
}
