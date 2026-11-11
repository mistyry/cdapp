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
    VOLC_ERR_ENGINE = -4,
    VOLC_ERR_SOURCE = -5,
    VOLC_ERR_STATE = -6,
    // 以下为 SDK 接口返回非 0，原始返回值写入 *sdk_rc。
    VOLC_ERR_JOIN = -10,
    VOLC_ERR_ENCODER = -11,
    VOLC_ERR_CAPTURE = -12,
    VOLC_ERR_PUBLISH = -13,
    VOLC_ERR_TOKEN = -14,
    VOLC_ERR_AUDIO = -15,
    VOLC_ERR_SUBSCRIBE = -16,
};

// 事件编号。回调在 SDK 内部线程触发，接收方只能转发，不得阻塞或回调 SDK。
enum {
    // a 为房间状态，0 为已进房，非 0 为错误或警告码。
    VOLC_EV_ROOM_STATE = 1,
    // a、b 为首帧宽高。
    VOLC_EV_FIRST_FRAME = 2,
    // a、b 为采集尺寸变化后的宽高。
    VOLC_EV_SIZE_CHANGED = 3,
    // 令牌将在 30 秒内过期。
    VOLC_EV_TOKEN_WILL_EXPIRE = 4,
    // a 为发送码率 kbps，b 为编码输出帧率，c 为编码宽，d 为编码高。
    VOLC_EV_STATS = 5,
    // a 为引擎错误码。
    VOLC_EV_ERROR = 6,
    // a 为本地视频流状态，b 为错误码。窗口关闭等采集失败由此上报。
    VOLC_EV_VIDEO_STATE = 7,
    // 共享的窗口已销毁。由系统窗口事件触发，SDK 不一定上报采集失败。
    VOLC_EV_SOURCE_GONE = 8,
};

// 观看事件编号，经 volc_watch_start 的回调上报，与推流事件分开。
enum {
    // a 为房间状态，0 为已进房，非 0 为错误或警告码。
    VOLC_WEV_ROOM_STATE = 1,
    // 分享者发布或取消发布屏幕流。a 为 1 发布、0 取消，b 为取消原因。
    VOLC_WEV_PUBLISH = 2,
    // 令牌将在 30 秒内过期。
    VOLC_WEV_TOKEN_WILL_EXPIRE = 3,
    // a 为接收码率 kbps，b、c 为宽高，d 为卡顿次数。
    VOLC_WEV_STATS = 4,
};

// 编码后的远端屏幕帧。codec 1 为 H.264，2 为 ByteVC1；key 非 0 为关键帧；ts_us 为帧时间戳。
// 在 SDK 内部线程触发，data 只在回调期间有效，接收方须复制，不得阻塞或回调 SDK。
typedef void (*volc_frame_fn)(void* user, const uint8_t* data, int32_t size, int32_t codec, int32_t key, int32_t width,
                              int32_t height, int64_t ts_us);

typedef void (*volc_event_fn)(void* user, int32_t kind, int32_t a, int32_t b, int32_t c, int32_t d);

// 源名称与应用路径的缓冲长度，UTF-8，超出截断。
#define VOLC_NAME_LEN 256

typedef struct {
    // 1 为窗口，2 为显示器，与 SDK 的 ScreenCaptureSourceType 一致。
    int32_t kind;
    // 源 ID，窗口为 HWND，显示器为 HMONITOR。
    uint64_t id;
    int32_t pid;
    int32_t primary;
    int32_t x, y, width, height;
    char name[VOLC_NAME_LEN];
    char application[VOLC_NAME_LEN];
} volc_source;

// 开播参数。字符串在调用期间有效。
typedef struct {
    const char* app_id;
    const char* room;
    const char* uid;
    const char* token;
    int32_t source_kind;
    uint64_t source_id;
    int32_t width, height, frame_rate, max_kbps;
    // 0 只推画面，1 带声音。声音由调用方经外部源推送。
    int32_t audio;
} volc_live;

// 启动 SDK 线程。重复调用直接返回 VOLC_OK。
int32_t volc_start(void);

// 停止 SDK 线程并等待退出。
void volc_stop(void);

// 从目录加载 VolcEngineRTC.dll，附属 DLL 在同一目录。dir 为以 0 结尾的 UTF-16 路径。
// 失败时 *win32_error 为 GetLastError 的值。
int32_t volc_load(const uint16_t* dir, uint32_t* win32_error);

// SDK 版本，须在加载成功后调用。返回值由 SDK 持有。
const char* volc_version(void);

// 设置 SDK 日志目录，UTF-8。须在加载后、首次创建引擎前调用。
int32_t volc_log(const char* dir);

// 列出可共享的显示器与窗口，最多写入 cap 项，返回总数。须在加载成功后调用。
// 没有引擎时用临时引擎，列完销毁。
int32_t volc_sources(volc_source* out, int32_t cap);

// 源缩略图，缩放到 max_w×max_h 以内并保持宽高比，转为 BGRA 写入 out。
// out 至少 max_w*max_h*4 字节。成功时 *w、*h 为实际尺寸，返回 VOLC_OK。
// 源不存在或格式不支持时返回 VOLC_ERR_SOURCE。
int32_t volc_thumbnail(int32_t kind, uint64_t id, int32_t max_w, int32_t max_h, uint8_t* out, int32_t* w,
                       int32_t* h);

// 选源对话框打开期间保留一个预览引擎，列源与缩略图不再每次新建。已有引擎时无操作。
// 开播时自动释放，之后需重新打开。
int32_t volc_preview_open(void);

// 释放预览引擎。推流中或未打开时无操作。
void volc_preview_close(void);

// 创建引擎、进房、开始采集。开播前释放预览引擎。成功后等进房与首帧事件再调用 volc_publish。
// 已在推流时返回 VOLC_ERR_STATE。cb 在 volc_live_stop 返回前保持有效。
int32_t volc_live_start(const volc_live* live, volc_event_fn cb, void* user, int32_t* sdk_rc);

// 发布屏幕流。
int32_t volc_publish(int32_t* sdk_rc);

// 重新提交编码配置。
int32_t volc_encoder(int32_t width, int32_t height, int32_t frame_rate, int32_t max_kbps, int32_t* sdk_rc);

// 更换令牌。
int32_t volc_token(const char* token, int32_t* sdk_rc);

// 停止发布与采集，离房并销毁引擎。返回后不再触发回调。未在推流时无操作。
void volc_live_stop(void);

// 推一帧屏幕音频：48 kHz 立体声 S16 交错，frames 为每声道采样数，应为 480 即 10 ms。
// 开播时 audio 为 1 才有效。可在任意线程调用，与 volc_live_stop 互斥。
int32_t volc_push_audio(const int16_t* pcm, int32_t frames, int32_t* sdk_rc);

// 观看参数。字符串在调用期间有效。liver 为分享者用户 ID。
typedef struct {
    const char* app_id;
    const char* room;
    const char* uid;
    const char* token;
    const char* liver;
} volc_watch;

// 观看与推流、预览共用进程内唯一的引擎，三者都结束后才销毁。
// 进房但不订阅，等 VOLC_WEV_PUBLISH 后调用 volc_watch_subscribe。SDK 不解码画面，编码帧经 frame 回调交出；
// 屏幕声音由 SDK 在本进程播放。已在观看时返回 VOLC_ERR_STATE。cb 与 frame 在 volc_watch_stop 返回前保持有效。
int32_t volc_watch_start(const volc_watch* watch, volc_event_fn cb, volc_frame_fn frame, void* user, int32_t* sdk_rc);

// 订阅分享者的屏幕画面与声音。分享者重新发布后需再次调用。
int32_t volc_watch_subscribe(int32_t* sdk_rc);

// 请求分享者发送关键帧，用于开始或重新开始解码。
int32_t volc_watch_key_frame(int32_t* sdk_rc);

// 更换观看令牌。
int32_t volc_watch_token(const char* token, int32_t* sdk_rc);

// 停止观看并离房。返回后不再触发观看回调。未在观看时无操作。
void volc_watch_stop(void);

#ifdef __cplusplus
}
#endif
