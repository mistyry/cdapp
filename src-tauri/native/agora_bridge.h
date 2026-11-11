// Agora RTC SDK 观看桥接，仅 Windows。只用于观看分到 Agora 的分享者，不推流。
// 事件编号与帧回调沿用 volc_bridge.h 的观看定义。
#pragma once

#include "volc_bridge.h"

#ifdef __cplusplus
extern "C" {
#endif

// 从目录加载 agora_rtc_sdk.dll，附属 DLL 在同一目录。dir 为以 0 结尾的 UTF-16 路径。
int32_t agora_load(const uint16_t* dir, uint32_t* win32_error);

// SDK 版本，须在加载成功后调用。
const char* agora_version(void);

// 设置 SDK 日志文件，UTF-8，须在 agora_watch_start 前调用。
void agora_log(const char* file);

// 以主播角色进房但不发布，自动订阅分享者的画面与声音。画面只取编码帧，经 frame 交出；声音由 SDK 在本进程播放。
// liver 须为数字用户 ID。分享者在房间中时上报 VOLC_WEV_PUBLISH。已在观看时返回 VOLC_ERR_STATE。
int32_t agora_watch_start(const volc_watch* watch, volc_event_fn cb, volc_frame_fn frame, void* user, int32_t* sdk_rc);

// 离房后以同一令牌重新进房，用于取得新的关键帧。Agora 观众端没有请求关键帧的接口。
int32_t agora_watch_rejoin(int32_t* sdk_rc);

// 更换令牌。
int32_t agora_watch_token(const char* token, int32_t* sdk_rc);

// 停止观看，离房并销毁引擎。返回后不再触发回调。
void agora_watch_stop(void);

#ifdef __cplusplus
}
#endif
