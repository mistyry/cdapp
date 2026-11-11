// 火山 RTC SDK 桥接。SDK 在缓存目录中，启动时下载后按完整路径加载，因此链接为延迟加载。
// 全部 SDK 调用在专用线程执行，该线程运行消息循环。SDK 回调在其内部线程触发，只转发给调用方。
// 只用 Win32 原语，不引入 C++ 运行库依赖，cdapp.exe 保持单文件运行。
#include "volc_bridge.h"

#include <windows.h>
#include <timeapi.h>
#include "bytertc_room.h"
#include "bytertc_video.h"

using namespace bytertc;

namespace {

constexpr UINT WM_VOLC_CALL = WM_APP + 1;

SRWLOCK g_lock = SRWLOCK_INIT;
HANDLE g_thread = nullptr;
DWORD g_thread_id = 0;

// 以下只在 SDK 线程读写。
IRTCVideo* g_video = nullptr;
IRTCRoom* g_room = nullptr;
ScreenVideoEncoderConfig g_enc{};
MediaStreamType g_media = kMediaStreamTypeVideo;
bool g_published = false;
bool g_timer = false;
char g_log_dir[MAX_PATH * 3] = {0};

// 回调目标。SDK 内部线程读取，SDK 线程在创建引擎前写入、销毁引擎后清空。
SRWLOCK g_cb_lock = SRWLOCK_INIT;
volc_event_fn g_cb = nullptr;
void* g_user = nullptr;

void emit(int32_t kind, int32_t a = 0, int32_t b = 0, int32_t c = 0, int32_t d = 0) {
    AcquireSRWLockShared(&g_cb_lock);
    if (g_cb) g_cb(g_user, kind, a, b, c, d);
    ReleaseSRWLockShared(&g_cb_lock);
}

void set_cb(volc_event_fn cb, void* user) {
    AcquireSRWLockExclusive(&g_cb_lock);
    g_cb = cb;
    g_user = user;
    ReleaseSRWLockExclusive(&g_cb_lock);
}

// 处理器的生命周期须长于引擎，因此为静态对象。
struct VideoHandler : IRTCVideoEventHandler {
    void onError(int err) override { emit(VOLC_EV_ERROR, err); }
    void onFirstLocalVideoFrameCaptured(StreamIndex index, VideoFrameInfo info) override {
        if (index == kStreamIndexScreen) emit(VOLC_EV_FIRST_FRAME, info.width, info.height);
    }
    void onLocalVideoSizeChanged(StreamIndex index, const VideoFrameInfo& info) override {
        if (index == kStreamIndexScreen) emit(VOLC_EV_SIZE_CHANGED, info.width, info.height);
    }
    void onLocalVideoStateChanged(StreamIndex index, LocalVideoStreamState state,
                                  LocalVideoStreamError error) override {
        if (index == kStreamIndexScreen) emit(VOLC_EV_VIDEO_STATE, state, error);
    }
} g_video_handler;

struct RoomHandler : IRTCRoomEventHandler {
    void onRoomStateChanged(const char*, const char*, int state, const char*) override {
        emit(VOLC_EV_ROOM_STATE, state);
    }
    void onTokenWillExpire() override { emit(VOLC_EV_TOKEN_WILL_EXPIRE); }
    void onLocalStreamStats(const LocalStreamStats& s) override {
        if (!s.is_screen) return;
        const LocalVideoStats& v = s.video_stats;
        emit(VOLC_EV_STATS, v.sent_kbitrate, v.encoder_output_frame_rate, v.encoded_frame_width,
             v.encoded_frame_height);
    }
} g_room_handler;

// 投递到 SDK 线程的调用。
struct Call {
    void (*fn)(void*);
    void* ctx;
    HANDLE done;
};

DWORD WINAPI sdk_thread(LPVOID ready) {
    MSG msg;
    // 先建立消息队列，再通知启动方可以投递。
    PeekMessageW(&msg, nullptr, WM_USER, WM_USER, PM_NOREMOVE);
    SetEvent(static_cast<HANDLE>(ready));
    while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
        if (msg.hwnd == nullptr && msg.message == WM_VOLC_CALL) {
            auto* call = reinterpret_cast<Call*>(msg.lParam);
            call->fn(call->ctx);
            SetEvent(call->done);
            continue;
        }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    return 0;
}

DWORD sdk_thread_id() {
    AcquireSRWLockShared(&g_lock);
    DWORD tid = g_thread_id;
    ReleaseSRWLockShared(&g_lock);
    return tid;
}

// 在 SDK 线程执行并等待返回。线程未启动时返回 false。
bool run_on_sdk(void (*fn)(void*), void* ctx) {
    DWORD tid = sdk_thread_id();
    if (tid == 0) return false;
    if (GetCurrentThreadId() == tid) {
        fn(ctx);
        return true;
    }
    Call call{fn, ctx, CreateEventW(nullptr, TRUE, FALSE, nullptr)};
    if (!call.done) return false;
    bool ok = PostThreadMessageW(tid, WM_VOLC_CALL, 0, reinterpret_cast<LPARAM>(&call)) != 0;
    if (ok) WaitForSingleObject(call.done, INFINITE);
    CloseHandle(call.done);
    return ok;
}

// 以 lambda 形式在 SDK 线程执行。lambda 不捕获需要析构的对象。
template <class F>
bool on_sdk(F& f) {
    return run_on_sdk([](void* p) { (*static_cast<F*>(p))(); }, &f);
}

void copy_str(char* dst, size_t cap, const char* src) {
    if (!src) {
        dst[0] = 0;
        return;
    }
    lstrcpynA(dst, src, static_cast<int>(cap));
}

void apply_log() {
    if (!g_log_dir[0]) return;
    LogConfig lc;
    lc.log_path = g_log_dir;
    lc.log_level = LocalLogLevel::kWarning;
    setLogConfig(lc);
}

// 停止推流的全部步骤，只在 SDK 线程调用。
void teardown() {
    if (g_room) {
        if (g_published) g_room->unpublishScreen(g_media);
        g_room->leaveRoom();
        g_room->destroy();
        g_room = nullptr;
    }
    if (g_video) {
        g_video->stopScreenVideoCapture();
        g_video = nullptr;
        destroyRTCVideo();
    }
    g_published = false;
    if (g_timer) {
        timeEndPeriod(1);
        g_timer = false;
    }
    set_cb(nullptr, nullptr);
}

void load_on_sdk(const wchar_t* dir, int32_t& rc, DWORD& err) {
    // 只从系统目录与显式添加的目录查找，附属 DLL 也从 SDK 目录找到。
    if (!SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_USER_DIRS) ||
        !AddDllDirectory(dir)) {
        err = GetLastError();
        rc = VOLC_ERR_LOAD;
        return;
    }
    wchar_t dll[MAX_PATH * 2];
    size_t n = lstrlenW(dir);
    static const wchar_t kName[] = L"\\VolcEngineRTC.dll";
    if (n + ARRAYSIZE(kName) > ARRAYSIZE(dll)) {
        err = ERROR_FILENAME_EXCED_RANGE;
        rc = VOLC_ERR_LOAD;
        return;
    }
    CopyMemory(dll, dir, n * sizeof(wchar_t));
    CopyMemory(dll + n, kName, sizeof kName);
    // 按完整路径加载，之后延迟加载按模块名找到这个已加载的模块。
    if (!LoadLibraryExW(dll, nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)) {
        err = GetLastError();
        rc = VOLC_ERR_LOAD;
        return;
    }
    rc = VOLC_OK;
}

}  // namespace

extern "C" int32_t volc_start(void) {
    AcquireSRWLockExclusive(&g_lock);
    int32_t rc = VOLC_OK;
    if (!g_thread) {
        HANDLE ready = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        DWORD tid = 0;
        HANDLE th = ready ? CreateThread(nullptr, 0, sdk_thread, ready, 0, &tid) : nullptr;
        if (th) {
            WaitForSingleObject(ready, INFINITE);
            g_thread = th;
            g_thread_id = tid;
        } else {
            rc = VOLC_ERR_THREAD;
        }
        if (ready) CloseHandle(ready);
    }
    ReleaseSRWLockExclusive(&g_lock);
    return rc;
}

extern "C" void volc_stop(void) {
    auto f = [] { teardown(); };
    on_sdk(f);
    AcquireSRWLockExclusive(&g_lock);
    HANDLE th = g_thread;
    DWORD tid = g_thread_id;
    g_thread = nullptr;
    g_thread_id = 0;
    ReleaseSRWLockExclusive(&g_lock);
    if (!th) return;
    PostThreadMessageW(tid, WM_QUIT, 0, 0);
    WaitForSingleObject(th, INFINITE);
    CloseHandle(th);
}

extern "C" int32_t volc_load(const uint16_t* dir, uint32_t* win32_error) {
    int32_t rc = VOLC_ERR_NOT_STARTED;
    DWORD err = 0;
    auto f = [&] { load_on_sdk(reinterpret_cast<const wchar_t*>(dir), rc, err); };
    on_sdk(f);
    if (win32_error) *win32_error = err;
    return rc;
}

extern "C" const char* volc_version(void) {
    const char* v = nullptr;
    auto f = [&] { v = getSDKVersion(); };
    on_sdk(f);
    return v;
}

extern "C" int32_t volc_log(const char* dir) {
    int32_t rc = VOLC_ERR_NOT_STARTED;
    auto f = [&] {
        copy_str(g_log_dir, sizeof g_log_dir, dir);
        CreateDirectoryA(g_log_dir, nullptr);
        rc = VOLC_OK;
    };
    on_sdk(f);
    return rc;
}

extern "C" int32_t volc_sources(volc_source* out, int32_t cap) {
    int32_t total = VOLC_ERR_NOT_STARTED;
    auto f = [&] {
        bool temp = g_video == nullptr;
        if (temp) {
            apply_log();
            // 列源不进房，appId 不会被校验。
            g_video = createRTCVideo("000000000000000000000000", &g_video_handler, "{}");
        }
        if (!g_video) {
            total = VOLC_ERR_ENGINE;
            return;
        }
        IScreenCaptureSourceList* list = g_video->getScreenCaptureSourceList();
        total = list ? list->getCount() : 0;
        for (int32_t i = 0; list && i < total && i < cap; i++) {
            ScreenCaptureSourceInfo s = list->getSourceInfo(i);
            volc_source& o = out[i];
            o.kind = static_cast<int32_t>(s.type);
            o.id = reinterpret_cast<uint64_t>(s.source_id);
            o.pid = s.pid;
            o.primary = s.primary_monitor ? 1 : 0;
            o.x = s.region_rect.x;
            o.y = s.region_rect.y;
            o.width = s.region_rect.width;
            o.height = s.region_rect.height;
            copy_str(o.name, sizeof o.name, s.source_name);
            copy_str(o.application, sizeof o.application, s.application);
        }
        if (list) list->release();
        if (temp) {
            g_video = nullptr;
            destroyRTCVideo();
        }
    };
    on_sdk(f);
    return total;
}

extern "C" int32_t volc_live_start(const volc_live* live, volc_event_fn cb, void* user, int32_t* sdk_rc) {
    int32_t rc = VOLC_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (g_room) {
            rc = VOLC_ERR_STATE;
            return;
        }
        set_cb(cb, user);
        // 实测默认计时器精度下 SDK 内部采集只有约 22 帧，提高到 1 ms 后为 30 帧。推流期间生效，停止时恢复。
        timeBeginPeriod(1);
        g_timer = true;
        apply_log();
        g_video = createRTCVideo(live->app_id, &g_video_handler, "{}");
        if (!g_video) {
            set_cb(nullptr, nullptr);
            rc = VOLC_ERR_ENGINE;
            return;
        }
        g_room = g_video->createRTCRoom(live->room);
        if (!g_room) {
            teardown();
            rc = VOLC_ERR_ENGINE;
            return;
        }
        g_room->setRTCRoomEventHandler(&g_room_handler);
        UserInfo ui;
        ui.uid = live->uid;
        RTCRoomConfig rcfg;
        rcfg.room_profile_type = kRoomProfileTypeCommunication;
        rcfg.is_auto_publish = false;
        rcfg.is_auto_subscribe_audio = false;
        rcfg.is_auto_subscribe_video = false;
        raw = g_room->joinRoom(live->token, ui, rcfg);
        if (raw != 0) {
            teardown();
            rc = VOLC_ERR_JOIN;
            return;
        }
        // 官方客户端顺序：采集边框、编码运行参数、屏幕编码配置、开始采集。
        g_video->setRuntimeParameters("{\"rtc.use_wgc_highlight\":true}");
        g_video->setRuntimeParameters(
            "{\"rtc.screen_video_encoder_configs\":[{\"codec_mode\":\"auto\",\"codec_name\":\"auto\"}]}");
        g_enc.width = live->width;
        g_enc.height = live->height;
        g_enc.frame_rate = live->frame_rate;
        g_enc.max_bitrate = live->max_kbps;
        g_enc.min_bitrate = 0;
        // 默认为清晰优先，会把帧率压到 15。
        g_enc.encoder_preference = kScreenVideoEncodePreferenceFramerate;
        raw = g_video->setScreenVideoEncoderConfig(g_enc);
        if (raw != 0) {
            teardown();
            rc = VOLC_ERR_ENCODER;
            return;
        }
        if (live->audio) {
            g_video->setScreenAudioSourceType(kAudioSourceTypeExternal);
            g_video->setScreenAudioStreamIndex(kStreamIndexScreen);
            g_media = kMediaStreamTypeBoth;
        } else {
            g_media = kMediaStreamTypeVideo;
        }
        ScreenCaptureSourceInfo src;
        src.type = static_cast<ScreenCaptureSourceType>(live->source_kind);
        src.source_id = reinterpret_cast<view_t>(live->source_id);
        ScreenCaptureParameters p;
        p.capture_mouse_cursor = kMouseCursorCaptureStateOn;
        p.highlight_config.enable_highlight = false;
        raw = g_video->startScreenVideoCapture(src, p);
        if (raw != 0) {
            teardown();
            rc = VOLC_ERR_CAPTURE;
            return;
        }
        rc = VOLC_OK;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t volc_publish(int32_t* sdk_rc) {
    int32_t rc = VOLC_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_room) {
            rc = VOLC_ERR_STATE;
            return;
        }
        raw = g_room->publishScreen(g_media);
        g_published = raw == 0;
        rc = g_published ? VOLC_OK : VOLC_ERR_PUBLISH;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t volc_encoder(int32_t width, int32_t height, int32_t frame_rate, int32_t max_kbps,
                                int32_t* sdk_rc) {
    int32_t rc = VOLC_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_video || !g_room) {
            rc = VOLC_ERR_STATE;
            return;
        }
        g_enc.width = width;
        g_enc.height = height;
        g_enc.frame_rate = frame_rate;
        g_enc.max_bitrate = max_kbps;
        raw = g_video->setScreenVideoEncoderConfig(g_enc);
        rc = raw == 0 ? VOLC_OK : VOLC_ERR_ENCODER;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t volc_token(const char* token, int32_t* sdk_rc) {
    int32_t rc = VOLC_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_room) {
            rc = VOLC_ERR_STATE;
            return;
        }
        raw = g_room->updateToken(token);
        rc = raw == 0 ? VOLC_OK : VOLC_ERR_TOKEN;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" void volc_live_stop(void) {
    auto f = [] { teardown(); };
    on_sdk(f);
}
