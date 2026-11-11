// 火山 RTC SDK 的纯 C 桥接，仅 Windows。
#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// 返回码。
enum {
    VOLC_OK = 0,
    VOLC_ERR_NOT_STARTED = -1,
    VOLC_ERR_THREAD = -2,
    VOLC_ERR_LOAD = -3,
};

// 启动 SDK 线程。重复调用直接返回 VOLC_OK。
int32_t volc_start(void);

// 停止 SDK 线程并等待退出。
void volc_stop(void);

// 从目录加载 VolcEngineRTC.dll，附属 DLL 在同一目录。dir 为以 0 结尾的 UTF-16 路径。
// 失败时 *win32_error 为 GetLastError 的值。
int32_t volc_load(const uint16_t* dir, uint32_t* win32_error);

// SDK 版本，须在加载成功后调用。返回值由 SDK 持有。
const char* volc_version(void);

#ifdef __cplusplus
}
#endif
