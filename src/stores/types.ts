import type { GuildAccess, Overwrite, UserOverwrite } from "../lib/perms";
import type { MemberDto } from "../lib/api";
import type { Attachment } from "../lib/content";
export type User = { id: string; name: string; tag?: string; avatar?: string; color?: string; online: boolean; activity?: string };
export type Channel = {
  id: string; name: string; type: "text" | "voice" | "unknown";
  preview?: string; limit?: number; users?: string[];
  // 频道权限覆盖，用于计算本人有效权限。
  overwrites?: Overwrite[]; userOverwrites?: UserOverwrite[];
};

// 服务器权限依据的加载状态。未知（loading / failed）时不视为已获授权。
export type Access = { status: "loading" | "ready" | "failed"; data?: GuildAccess };

// 当前频道成员列表（服务端分页）。key 为 服务器/频道，切换后旧列表丢弃。
export type MemberList = {
  key: string; guildId: string; channelId: string;
  items: MemberDto[]; page: number; pageTotal: number; total: number; online: number; offline: number;
  loading: boolean; failed: boolean;
};
// loose：未归属分组的顶层频道，不显示分组标题。
export type Group = { id: string; name: string; loose?: boolean; channels: Channel[] };
export type Guild = { id: string; name: string; icon?: string; groups: Group[]; loaded?: boolean; failed?: boolean };
// 引用的消息。id 为被引用消息的 rong_id；gone 表示已收到其删除事件。
export type Quote = { id?: string; authorId?: string; author: string; type: number; content: string; text: string; gone?: boolean };
// 消息回应。id 为表情 ID（Unicode 表情即字符本身，用于请求与事件匹配）；name 仅用于显示。
// users 记录已从事件或本端操作得知的用户状态（true 已回应，false 已取消），用于重复事件去重。
export type Reaction = { id: string; name: string; count: number; me: boolean; users: Record<string, boolean> };
// pending：本端编辑或删除进行中。updatedAt 为最后编辑时间（毫秒），0 或缺省表示未编辑。
export type Message = {
  id: string; rongId?: string; author: string; createAt: number; time: string; short: string;
  type: number; content: string; quote?: Quote; attachment?: Attachment;
  updatedAt?: number; reactions?: Reaction[]; pending?: "editing" | "deleting";
};

// 待发的图片或文件。url 为已上传的资源地址：有地址后重试只重发消息，不再上传。
// uploadId 标识当前一次上传，取消或重试后旧上传的迟到结果按其丢弃。sent 为已交给连接的字节数。
export type OutgoingFile = { name: string; size: number; image: boolean; url?: string; uploadId?: string; sent: number };

// 本地待发消息。failed 表示确定未送达；uncertain 表示可能已送达，须核对后再重发。
// uploading 仅用于文件，表示正在上传资源（可取消）。
export type Outgoing = {
  localId: string; channelId: string; text: string; reply?: { id: string; authorId?: string; author: string; type?: number; content?: string; text: string };
  createAt: number; status: "uploading" | "sending" | "failed" | "uncertain"; error?: string; file?: OutgoingFile;
};

// 频道历史状态。end 已到达最早消息；loading 正在拉取更早一页；failed 最近一次拉取失败（可重试）。
// newer 表示列表是跳转加载的片段，其后还有未加载的更新消息；此时不追加实时消息、不标记已读。
export type History = { end: boolean; loading: boolean; failed: boolean; newer?: boolean };

// 跳转目标。seq 每次跳转递增，列表据此滚动并高亮。
export type Focus = { channelId: string; id: string; seq: number };

// 频道未读。unread 表示有未读（服务端不提供条数）；mentions 为提及我的条数；readAt 为已知最后已读时间（毫秒）。
export type Unread = { guildId: string; unread: boolean; mentions: number; readAt: number };

// 私信会话列表。page 为已加载的页数（0 未加载）；按最近消息时间降序。
export type DmList = { loaded: boolean; loading: boolean; failed: boolean; page: number; pageTotal: number };
