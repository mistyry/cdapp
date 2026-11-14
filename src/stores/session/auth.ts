import { api, errorText, setUnauthorizedHandler, type Raw } from "../../lib/api";
import { forgetAccountCache } from "../../lib/accountState";
import { loadPeerVolumes, resetVoice } from "../voice";
import { loadShareAudio, resetShareAudio } from "../share";
import { loadStageCollapsed, resetUi } from "../ui";
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

export async function boot() {
  reset("boot");
  const gen = st.account;
  try {
    const r = await api.restoreSession();
    if (gen !== st.account) return;
    if (r.status === "ready") await enter(r.me);
    else setSession({ phase: "login", notice: r.status === "expired" ? `登录已失效：${r.message}` : null });
  } catch (e) {
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
    void loadPeerVolumes(me.id);
    void loadShareAudio(me.id);
    void loadStageCollapsed(me.id);
    await startGateway(gen);
  } catch (e) {
    if (gen !== st.account) return;
    if (session.phase !== "ready") {
      setSession({ phase: "login", notice: `登录失败：${errorText(e)}` });
      return;
    }
    report(e);
  }
  if (gen === st.account) await Promise.all([loadGuilds(), syncUnread(), loadDms()]);
}

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
  resetShareAudio();
}

export async function logout() {
  reset("login");
  await api.logout().catch(report);
}

export function expire(message: string, gen = st.account) {
  if (gen !== st.account || session.phase !== "ready") return;
  reset("login", `登录已失效：${message}`);
  api.voiceLeave().catch(() => {});
}
setUnauthorizedHandler(expire, () => st.account);

export function skipRestore() {
  reset("login");
}
