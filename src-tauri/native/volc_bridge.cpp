#include "volc_bridge.h"

#include <windows.h>
#include <timeapi.h>
#include "bytertc_room.h"
#include "bytertc_video.h"

using namespace bytertc;

namespace {

constexpr UINT WM_VOLC_CALL = WM_APP + 1;
constexpr UINT WM_VOLC_PEER = WM_APP + 2;
constexpr char kMuteVideo[] = "mute-video";
constexpr char kUnmuteVideo[] = "unmute-video";

SRWLOCK g_lock = SRWLOCK_INIT;
HANDLE g_thread = nullptr;
DWORD g_thread_id = 0;

IRTCVideo* g_video = nullptr;
IRTCRoom* g_room = nullptr;
ScreenVideoEncoderConfig g_enc{};
MediaStreamType g_media = kMediaStreamTypeVideo;
bool g_published = false;
bool g_paused = false;
bool g_timer = false;
bool g_audio = false;
bool g_preview = false;
char g_log_dir[MAX_PATH * 3] = {0};
char g_app_id[64] = {0};

IRTCRoom* g_watch_room = nullptr;
char g_watch_room_id[128] = {0};
char g_liver[64] = {0};
int g_watch_volume = 100;

SRWLOCK g_engine_lock = SRWLOCK_INIT;

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

SRWLOCK g_wcb_lock = SRWLOCK_INIT;
volc_event_fn g_wcb = nullptr;
volc_frame_fn g_wframe = nullptr;
void* g_wuser = nullptr;

void wemit(int32_t kind, int32_t a = 0, int32_t b = 0, int32_t c = 0, int32_t d = 0) {
    AcquireSRWLockShared(&g_wcb_lock);
    if (g_wcb) g_wcb(g_wuser, kind, a, b, c, d);
    ReleaseSRWLockShared(&g_wcb_lock);
}

void set_wcb(volc_event_fn cb, volc_frame_fn frame, void* user) {
    AcquireSRWLockExclusive(&g_wcb_lock);
    g_wcb = cb;
    g_wframe = frame;
    g_wuser = user;
    ReleaseSRWLockExclusive(&g_wcb_lock);
}

SRWLOCK g_ocb_lock = SRWLOCK_INIT;
bool g_own = false;
volc_event_fn g_ocb = nullptr;
volc_frame_fn g_oframe = nullptr;
void* g_ouser = nullptr;

void oemit(int32_t kind, int32_t a = 0, int32_t b = 0) {
    AcquireSRWLockShared(&g_ocb_lock);
    if (g_ocb) g_ocb(g_ouser, kind, a, b, 0, 0);
    ReleaseSRWLockShared(&g_ocb_lock);
}

void set_ocb(volc_event_fn cb, volc_frame_fn frame, void* user) {
    AcquireSRWLockExclusive(&g_ocb_lock);
    g_ocb = cb;
    g_oframe = frame;
    g_ouser = user;
    ReleaseSRWLockExclusive(&g_ocb_lock);
}

struct LocalEncodedObserver : ILocalEncodedVideoFrameObserver {
    void onLocalEncodedVideoFrame(StreamIndex index, const IEncodedVideoFrame& f) override {
        if (index != kStreamIndexScreen) return;
        AcquireSRWLockShared(&g_ocb_lock);
        if (g_oframe)
            g_oframe(g_ouser, f.data(), f.dataSize(), static_cast<int32_t>(f.codecType()),
                     f.pictureType() == kVideoPictureTypeI ? 1 : 0, f.width(), f.height(), f.timestampUs());
        ReleaseSRWLockShared(&g_ocb_lock);
    }
} g_local_observer;

bool is_watched(const char* room, const char* uid, StreamIndex index) {
    return index == kStreamIndexScreen && uid && room && lstrcmpA(uid, g_liver) == 0 &&
           lstrcmpA(room, g_watch_room_id) == 0;
}

struct EncodedObserver : IRemoteEncodedVideoFrameObserver {
    void onRemoteEncodedVideoFrame(const RemoteStreamKey& k, const IEncodedVideoFrame& f) override {
        AcquireSRWLockShared(&g_wcb_lock);
        if (g_wframe && is_watched(k.room_id, k.user_id, k.stream_index))
            g_wframe(g_wuser, f.data(), f.dataSize(), static_cast<int32_t>(f.codecType()),
                     f.pictureType() == kVideoPictureTypeI ? 1 : 0, f.width(), f.height(), f.timestampUs());
        ReleaseSRWLockShared(&g_wcb_lock);
    }
} g_encoded_observer;

struct VideoHandler : IRTCVideoEventHandler {
    void onError(int err) override { emit(SHARE_EV_ERROR, err); }
    void onFirstLocalVideoFrameCaptured(StreamIndex index, VideoFrameInfo info) override {
        if (index == kStreamIndexScreen) emit(SHARE_EV_FIRST_FRAME, info.width, info.height);
    }
    void onLocalVideoSizeChanged(StreamIndex index, const VideoFrameInfo& info) override {
        if (index == kStreamIndexScreen) emit(SHARE_EV_SIZE_CHANGED, info.width, info.height);
    }
    void onLocalVideoStateChanged(StreamIndex index, LocalVideoStreamState state,
                                  LocalVideoStreamError error) override {
        if (index == kStreamIndexScreen) emit(SHARE_EV_VIDEO_STATE, state, error);
    }
    void onVideoDeviceStateChanged(const char*, RTCVideoDeviceType type, MediaDeviceState state,
                                   MediaDeviceError) override {
        if (type != kRTCVideoDeviceTypeScreenCaptureDevice) return;
        if (state == kMediaDeviceStatePaused) emit(SHARE_EV_PAUSED, 1);
        else if (state == kMediaDeviceStateResumed) emit(SHARE_EV_PAUSED, 0);
    }
} g_video_handler;

DWORD sdk_thread_id();

struct RoomHandler : IRTCRoomEventHandler {
    void onRoomStateChanged(const char*, const char*, int state, const char*) override {
        emit(SHARE_EV_ROOM_STATE, state);
    }
    void onTokenWillExpire() override { emit(SHARE_EV_TOKEN_WILL_EXPIRE); }
    void onUserJoined(const UserInfo& info, int) override {
        if (!info.uid) return;
        const int n = lstrlenA(info.uid) + 1;
        char* uid = static_cast<char*>(HeapAlloc(GetProcessHeap(), 0, n));
        if (!uid) return;
        CopyMemory(uid, info.uid, n);
        const DWORD tid = sdk_thread_id();
        if (!tid || !PostThreadMessageW(tid, WM_VOLC_PEER, 0, reinterpret_cast<LPARAM>(uid)))
            HeapFree(GetProcessHeap(), 0, uid);
    }
    void onLocalStreamStats(const LocalStreamStats& s) override {
        if (!s.is_screen) return;
        const LocalVideoStats& v = s.video_stats;
        emit(SHARE_EV_STATS, v.sent_kbitrate, v.encoder_output_frame_rate, v.encoded_frame_width,
             v.encoded_frame_height);
    }
} g_room_handler;

struct WatchRoomHandler : IRTCRoomEventHandler {
    void onRoomStateChanged(const char*, const char*, int state, const char*) override {
        wemit(SHARE_WEV_ROOM_STATE, state);
    }
    void onTokenWillExpire() override { wemit(SHARE_WEV_TOKEN_WILL_EXPIRE); }
    void onUserPublishScreen(const char* uid, MediaStreamType) override {
        if (uid && lstrcmpA(uid, g_liver) == 0) wemit(SHARE_WEV_PUBLISH, 1);
    }
    void onUserUnpublishScreen(const char* uid, MediaStreamType, StreamRemoveReason reason) override {
        if (uid && lstrcmpA(uid, g_liver) == 0) wemit(SHARE_WEV_PUBLISH, 0, reason);
    }
    void onRemoteStreamStats(const RemoteStreamStats& s) override {
        const RemoteVideoStats& v = s.video_stats;
        if (!v.is_screen || !s.uid || lstrcmpA(s.uid, g_liver) != 0) return;
        wemit(SHARE_WEV_STATS, v.received_kbitrate, v.width, v.height, v.stall_duration);
    }
    void onRoomMessageReceived(const char* uid, const char* message) override { on_message(uid, message); }
    void onUserMessageReceived(const char* uid, const char* message) override { on_message(uid, message); }
    static void on_message(const char* uid, const char* message) {
        if (!uid || !message || lstrcmpA(uid, g_liver) != 0) return;
        if (lstrcmpA(message, kMuteVideo) == 0) wemit(SHARE_WEV_PAUSED, 1);
        else if (lstrcmpA(message, kUnmuteVideo) == 0) wemit(SHARE_WEV_PAUSED, 0);
    }
} g_watch_handler;

HWINEVENTHOOK g_hook_destroy = nullptr;
HWINEVENTHOOK g_hook_show = nullptr;
HWND g_source_window = nullptr;

void make_opaque(HWND hwnd) {
    wchar_t cls[64];
    if (!GetClassNameW(hwnd, cls, ARRAYSIZE(cls)) || lstrcmpW(cls, L"ByteRTCTransparentWindow") != 0) return;
    COLORREF key = 0;
    BYTE alpha = 0;
    DWORD flags = 0;
    if (GetLayeredWindowAttributes(hwnd, &key, &alpha, &flags) && alpha != 255)
        SetLayeredWindowAttributes(hwnd, key, 255, flags | LWA_ALPHA);
}

BOOL CALLBACK opaque_existing(HWND hwnd, LPARAM) {
    DWORD pid = 0;
    GetWindowThreadProcessId(hwnd, &pid);
    if (pid == GetCurrentProcessId()) make_opaque(hwnd);
    return TRUE;
}

void CALLBACK on_win_event(HWINEVENTHOOK hook, DWORD event, HWND hwnd, LONG obj, LONG, DWORD, DWORD) {
    if (obj != OBJID_WINDOW || !hwnd) return;
    if (hook == g_hook_destroy && event == EVENT_OBJECT_DESTROY && hwnd == g_source_window) {
        g_source_window = nullptr;
        emit(SHARE_EV_SOURCE_GONE);
    } else if (hook == g_hook_show && event == EVENT_OBJECT_SHOW) {
        make_opaque(hwnd);
    }
}

void hook_windows(HWND source) {
    g_source_window = source;
    if (source) {
        DWORD pid = 0;
        GetWindowThreadProcessId(source, &pid);
        g_hook_destroy = SetWinEventHook(EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY, nullptr, on_win_event, pid, 0,
                                         WINEVENT_OUTOFCONTEXT);
    }
    g_hook_show = SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, nullptr, on_win_event, GetCurrentProcessId(),
                                  0, WINEVENT_OUTOFCONTEXT);
    EnumWindows(opaque_existing, 0);
}

void unhook_windows() {
    if (g_hook_destroy) UnhookWinEvent(g_hook_destroy);
    if (g_hook_show) UnhookWinEvent(g_hook_show);
    g_hook_destroy = g_hook_show = nullptr;
    g_source_window = nullptr;
}

struct Call {
    void (*fn)(void*);
    void* ctx;
    HANDLE done;
};

DWORD WINAPI sdk_thread(LPVOID ready) {
    MSG msg;
    PeekMessageW(&msg, nullptr, WM_USER, WM_USER, PM_NOREMOVE);
    SetEvent(static_cast<HANDLE>(ready));
    while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
        if (msg.hwnd == nullptr && msg.message == WM_VOLC_CALL) {
            auto* call = reinterpret_cast<Call*>(msg.lParam);
            call->fn(call->ctx);
            SetEvent(call->done);
            continue;
        }
        if (msg.hwnd == nullptr && msg.message == WM_VOLC_PEER) {
            char* uid = reinterpret_cast<char*>(msg.lParam);
            if (g_room && g_paused) g_room->sendUserMessage(uid, kMuteVideo, kMessageConfigReliableOrdered);
            HeapFree(GetProcessHeap(), 0, uid);
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

void destroy_engine() {
    if (!g_video) return;
    AcquireSRWLockExclusive(&g_engine_lock);
    g_video = nullptr;
    g_audio = false;
    ReleaseSRWLockExclusive(&g_engine_lock);
    destroyRTCVideo();
    g_app_id[0] = 0;
}

void release_engine() {
    if (!g_room && !g_watch_room && !g_preview) destroy_engine();
}

IRTCVideo* engine_for(const char* app_id) {
    if (g_video && lstrcmpA(g_app_id, app_id) != 0) {
        if (g_room || g_watch_room) return nullptr;
        destroy_engine();
    }
    if (!g_video) {
        apply_log();
        g_video = createRTCVideo(app_id, &g_video_handler, "{}");
        if (!g_video) return nullptr;
        lstrcpynA(g_app_id, app_id, sizeof g_app_id);
        g_video->registerRemoteEncodedVideoFrameObserver(&g_encoded_observer);
        if (g_own) g_video->registerLocalEncodedVideoFrameObserver(&g_local_observer);
    }
    return g_video;
}

void end_own() {
    if (!g_own) return;
    oemit(SHARE_WEV_ENDED);
    g_own = false;
    if (g_video) g_video->registerLocalEncodedVideoFrameObserver(nullptr);
    set_ocb(nullptr, nullptr, nullptr);
}

void teardown() {
    unhook_windows();
    end_own();
    if (g_room) {
        if (g_published) g_room->unpublishScreen(g_media);
        g_room->leaveRoom();
        g_room->destroy();
        g_room = nullptr;
    }
    g_paused = false;
    if (g_video) {
        g_video->stopScreenVideoCapture();
        AcquireSRWLockExclusive(&g_engine_lock);
        g_audio = false;
        ReleaseSRWLockExclusive(&g_engine_lock);
    }
    g_published = false;
    if (g_timer) {
        timeEndPeriod(1);
        g_timer = false;
    }
    set_cb(nullptr, nullptr);
    release_engine();
}

void watch_teardown() {
    if (g_watch_room) {
        g_watch_room->leaveRoom();
        g_watch_room->destroy();
        g_watch_room = nullptr;
    }
    set_wcb(nullptr, nullptr, nullptr);
    g_watch_room_id[0] = 0;
    g_liver[0] = 0;
    release_engine();
}

void load_on_sdk(const wchar_t* dir, int32_t& rc, DWORD& err) {
    if (!SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_USER_DIRS) ||
        !AddDllDirectory(dir)) {
        err = GetLastError();
        rc = SHARE_ERR_LOAD;
        return;
    }
    wchar_t dll[MAX_PATH * 2];
    size_t n = lstrlenW(dir);
    static const wchar_t kName[] = L"\\VolcEngineRTC.dll";
    if (n + ARRAYSIZE(kName) > ARRAYSIZE(dll)) {
        err = ERROR_FILENAME_EXCED_RANGE;
        rc = SHARE_ERR_LOAD;
        return;
    }
    CopyMemory(dll, dir, n * sizeof(wchar_t));
    CopyMemory(dll + n, kName, sizeof kName);
    if (!LoadLibraryExW(dll, nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)) {
        err = GetLastError();
        rc = SHARE_ERR_LOAD;
        return;
    }
    rc = SHARE_OK;
}

constexpr char kPreviewAppId[] = "000000000000000000000000";

void close_preview() {
    if (!g_preview) return;
    g_preview = false;
    release_engine();
}

template <class F>
bool with_engine(F& f) {
    bool temp = g_video == nullptr;
    if (temp && !engine_for(kPreviewAppId)) return false;
    f(g_video);
    if (temp) release_engine();
    return true;
}

uint8_t clamp8(int v) { return static_cast<uint8_t>(v < 0 ? 0 : (v > 255 ? 255 : v)); }

bool to_bgra(IVideoFrame* fr, uint8_t* out, int32_t w, int32_t h) {
    switch (fr->pixelFormat()) {
        case kVideoPixelFormatBGRA:
        case kVideoPixelFormatRGBA:
        case kVideoPixelFormatARGB: {
            int b = 0, g = 1, r = 2;
            if (fr->pixelFormat() == kVideoPixelFormatRGBA) {
                b = 2;
                r = 0;
            }
            const uint8_t* src = fr->getPlaneData(0);
            const int stride = fr->getPlaneStride(0);
            for (int32_t y = 0; y < h; y++) {
                const uint8_t* s = src + static_cast<size_t>(y) * stride;
                uint8_t* d = out + static_cast<size_t>(y) * w * 4;
                for (int32_t x = 0; x < w; x++, s += 4, d += 4) {
                    d[0] = s[b];
                    d[1] = s[g];
                    d[2] = s[r];
                    d[3] = 255;
                }
            }
            return true;
        }
        case kVideoPixelFormatI420: {
            const uint8_t* py = fr->getPlaneData(0);
            const uint8_t* pu = fr->getPlaneData(1);
            const uint8_t* pv = fr->getPlaneData(2);
            const int sy = fr->getPlaneStride(0), su = fr->getPlaneStride(1), sv = fr->getPlaneStride(2);
            for (int32_t y = 0; y < h; y++) {
                uint8_t* d = out + static_cast<size_t>(y) * w * 4;
                for (int32_t x = 0; x < w; x++, d += 4) {
                    const int c = py[y * sy + x] - 16;
                    const int u = pu[(y / 2) * su + x / 2] - 128;
                    const int v = pv[(y / 2) * sv + x / 2] - 128;
                    d[0] = clamp8((298 * c + 516 * u + 128) >> 8);
                    d[1] = clamp8((298 * c - 100 * u - 208 * v + 128) >> 8);
                    d[2] = clamp8((298 * c + 409 * v + 128) >> 8);
                    d[3] = 255;
                }
            }
            return true;
        }
        default:
            return false;
    }
}

}

extern "C" int32_t volc_start(void) {
    AcquireSRWLockExclusive(&g_lock);
    int32_t rc = SHARE_OK;
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
    auto f = [] {
        watch_teardown();
        teardown();
        close_preview();
    };
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
    int32_t rc = SHARE_ERR_NOT_STARTED;
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
    int32_t rc = SHARE_ERR_NOT_STARTED;
    auto f = [&] {
        copy_str(g_log_dir, sizeof g_log_dir, dir);
        CreateDirectoryA(g_log_dir, nullptr);
        rc = SHARE_OK;
    };
    on_sdk(f);
    return rc;
}

extern "C" int32_t volc_sources(volc_source* out, int32_t cap) {
    int32_t total = SHARE_ERR_NOT_STARTED;
    auto f = [&] {
        auto list_all = [&](IRTCVideo* v) {
            IScreenCaptureSourceList* list = v->getScreenCaptureSourceList();
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
        };
        if (!with_engine(list_all)) total = SHARE_ERR_ENGINE;
    };
    on_sdk(f);
    return total;
}

extern "C" int32_t volc_thumbnail(int32_t kind, uint64_t id, int32_t max_w, int32_t max_h, uint8_t* out,
                                  int32_t* w, int32_t* h) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    auto f = [&] {
        auto shot = [&](IRTCVideo* v) {
            IVideoFrame* fr = v->getThumbnail(static_cast<ScreenCaptureSourceType>(kind),
                                              reinterpret_cast<view_t>(id), max_w, max_h);
            if (!fr) {
                rc = SHARE_ERR_SOURCE;
                return;
            }
            const int32_t fw = fr->width(), fh = fr->height();
            if (fw <= 0 || fh <= 0 || fw > max_w || fh > max_h || !to_bgra(fr, out, fw, fh)) {
                rc = SHARE_ERR_SOURCE;
            } else {
                *w = fw;
                *h = fh;
                rc = SHARE_OK;
            }
            fr->release();
        };
        if (!with_engine(shot)) rc = SHARE_ERR_ENGINE;
    };
    on_sdk(f);
    return rc;
}

extern "C" int32_t volc_preview_open(void) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    auto f = [&] {
        if (!g_video && !engine_for(kPreviewAppId)) {
            rc = SHARE_ERR_ENGINE;
            return;
        }
        g_preview = true;
        rc = SHARE_OK;
    };
    on_sdk(f);
    return rc;
}

extern "C" void volc_preview_close(void) {
    auto f = [] { close_preview(); };
    on_sdk(f);
}

extern "C" int32_t volc_live_start(const volc_live* live, volc_event_fn cb, void* user, int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (g_room) {
            rc = SHARE_ERR_STATE;
            return;
        }
        close_preview();
        set_cb(cb, user);
        timeBeginPeriod(1);
        g_timer = true;
        if (!engine_for(live->app_id)) {
            teardown();
            rc = SHARE_ERR_ENGINE;
            return;
        }
        g_room = g_video->createRTCRoom(live->room);
        if (!g_room) {
            teardown();
            rc = SHARE_ERR_ENGINE;
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
            rc = SHARE_ERR_JOIN;
            return;
        }
        g_video->setRuntimeParameters(
            "{\"rtc.screen_video_encoder_configs\":[{\"codec_mode\":\"auto\",\"codec_name\":\"auto\"}]}");
        g_enc.width = live->width;
        g_enc.height = live->height;
        g_enc.frame_rate = live->frame_rate;
        g_enc.max_bitrate = live->max_kbps;
        g_enc.min_bitrate = 0;
        g_enc.encoder_preference = kScreenVideoEncodePreferenceFramerate;
        raw = g_video->setScreenVideoEncoderConfig(g_enc);
        if (raw != 0) {
            teardown();
            rc = SHARE_ERR_ENCODER;
            return;
        }
        if (live->audio) {
            g_video->setScreenAudioSourceType(kAudioSourceTypeExternal);
            g_video->setScreenAudioStreamIndex(kStreamIndexScreen);
            g_media = kMediaStreamTypeBoth;
            AcquireSRWLockExclusive(&g_engine_lock);
            g_audio = true;
            ReleaseSRWLockExclusive(&g_engine_lock);
        } else {
            g_media = kMediaStreamTypeVideo;
        }
        ScreenCaptureSourceInfo src;
        src.type = static_cast<ScreenCaptureSourceType>(live->source_kind);
        src.source_id = reinterpret_cast<view_t>(live->source_id);
        ScreenCaptureParameters p;
        p.capture_mouse_cursor = kMouseCursorCaptureStateOn;
        p.highlight_config.enable_highlight = true;
        p.highlight_config.border_color = 0xFFFFD700;
        raw = g_video->startScreenVideoCapture(src, p);
        if (raw != 0) {
            teardown();
            rc = SHARE_ERR_CAPTURE;
            return;
        }
        hook_windows(src.type == kScreenCaptureSourceTypeWindow ? reinterpret_cast<HWND>(src.source_id) : nullptr);
        rc = SHARE_OK;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t volc_publish(int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_room) {
            rc = SHARE_ERR_STATE;
            return;
        }
        raw = g_room->publishScreen(g_media);
        g_published = raw == 0;
        rc = g_published ? SHARE_OK : SHARE_ERR_PUBLISH;
        if (g_published) oemit(SHARE_WEV_PUBLISH, 1);
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t volc_pause(int32_t paused, int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_room) {
            rc = SHARE_ERR_STATE;
            return;
        }
        rc = SHARE_OK;
        if ((paused != 0) == g_paused) return;
        g_paused = paused != 0;
        g_room->sendRoomMessage(g_paused ? kMuteVideo : kUnmuteVideo);
        oemit(SHARE_WEV_PAUSED, g_paused ? 1 : 0);
        if (g_paused && g_published) {
            raw = g_room->unpublishScreen(g_media);
            if (raw == 0) g_published = false;
        } else if (!g_paused && !g_published) {
            raw = g_room->publishScreen(g_media);
            g_published = raw == 0;
            if (!g_published) rc = SHARE_ERR_PUBLISH;
        }
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t volc_encoder(int32_t width, int32_t height, int32_t frame_rate, int32_t max_kbps,
                                int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_video || !g_room) {
            rc = SHARE_ERR_STATE;
            return;
        }
        g_enc.width = width;
        g_enc.height = height;
        g_enc.frame_rate = frame_rate;
        g_enc.max_bitrate = max_kbps;
        raw = g_video->setScreenVideoEncoderConfig(g_enc);
        rc = raw == 0 ? SHARE_OK : SHARE_ERR_ENCODER;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t volc_token(const char* token, int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_room) {
            rc = SHARE_ERR_STATE;
            return;
        }
        raw = g_room->updateToken(token);
        rc = raw == 0 ? SHARE_OK : SHARE_ERR_TOKEN;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" void volc_live_stop(void) {
    auto f = [] { teardown(); };
    on_sdk(f);
}

extern "C" int32_t volc_push_audio(const int16_t* pcm, int32_t frames, int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_STATE;
    int32_t raw = 0;
    AcquireSRWLockShared(&g_engine_lock);
    if (g_video && g_audio) {
        AudioFrameBuilder b;
        b.sample_rate = kAudioSampleRate48000;
        b.channel = kAudioChannelStereo;
        b.data = reinterpret_cast<uint8_t*>(const_cast<int16_t*>(pcm));
        b.data_size = static_cast<int64_t>(frames) * 2 * sizeof(int16_t);
        b.deep_copy = true;
        IAudioFrame* frame = buildAudioFrame(b);
        if (frame) {
            raw = g_video->pushScreenAudioFrame(frame);
            frame->release();
            rc = raw == 0 ? SHARE_OK : SHARE_ERR_AUDIO;
        } else {
            rc = SHARE_ERR_AUDIO;
        }
    }
    ReleaseSRWLockShared(&g_engine_lock);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

namespace {
RemoteStreamKey watch_key() { return RemoteStreamKey{g_watch_room_id, g_liver, kStreamIndexScreen}; }
int apply_watch_volume() { return g_video->setRemoteAudioPlaybackVolume(g_watch_room_id, g_liver, g_watch_volume); }
}

extern "C" int32_t volc_watch_start(const volc_watch* w, volc_event_fn cb, volc_frame_fn frame, void* user,
                                    int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (g_watch_room) {
            rc = SHARE_ERR_STATE;
            return;
        }
        close_preview();
        if (!engine_for(w->app_id)) {
            release_engine();
            rc = SHARE_ERR_ENGINE;
            return;
        }
        lstrcpynA(g_watch_room_id, w->room, sizeof g_watch_room_id);
        lstrcpynA(g_liver, w->liver, sizeof g_liver);
        g_watch_volume = w->volume;
        set_wcb(cb, frame, user);
        g_watch_room = g_video->createRTCRoom(w->room);
        if (!g_watch_room) {
            watch_teardown();
            rc = SHARE_ERR_ENGINE;
            return;
        }
        g_watch_room->setRTCRoomEventHandler(&g_watch_handler);
        raw = g_video->setVideoDecoderConfig(watch_key(), kVideoDecoderConfigEncode);
        if (raw != 0) {
            watch_teardown();
            rc = SHARE_ERR_ENGINE;
            return;
        }
        UserInfo ui;
        ui.uid = w->uid;
        RTCRoomConfig rcfg;
        rcfg.room_profile_type = kRoomProfileTypeCommunication;
        rcfg.is_auto_publish = false;
        rcfg.is_auto_subscribe_audio = false;
        rcfg.is_auto_subscribe_video = false;
        raw = g_watch_room->joinRoom(w->token, ui, rcfg);
        if (raw != 0) {
            watch_teardown();
            rc = SHARE_ERR_JOIN;
            return;
        }
        rc = SHARE_OK;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t volc_watch_subscribe(int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_watch_room) {
            rc = SHARE_ERR_STATE;
            return;
        }
        raw = g_watch_room->subscribeScreen(g_liver, kMediaStreamTypeBoth);
        if (raw == 0) apply_watch_volume();
        rc = raw == 0 ? SHARE_OK : VOLC_ERR_SUBSCRIBE;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t volc_watch_key_frame(int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_watch_room || !g_video) {
            rc = SHARE_ERR_STATE;
            return;
        }
        raw = g_video->requestRemoteVideoKeyFrame(watch_key());
        rc = raw == 0 ? SHARE_OK : VOLC_ERR_SUBSCRIBE;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t volc_watch_token(const char* token, int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_watch_room) {
            rc = SHARE_ERR_STATE;
            return;
        }
        raw = g_watch_room->updateToken(token);
        rc = raw == 0 ? SHARE_OK : SHARE_ERR_TOKEN;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t volc_watch_volume(int32_t volume, int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_watch_room || !g_video) {
            rc = SHARE_ERR_STATE;
            return;
        }
        g_watch_volume = volume;
        raw = apply_watch_volume();
        rc = raw == 0 ? SHARE_OK : SHARE_ERR_VOLUME;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" void volc_watch_stop(void) {
    auto f = [] { watch_teardown(); };
    on_sdk(f);
}

extern "C" int32_t volc_own_start(volc_event_fn cb, volc_frame_fn frame, void* user) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    auto f = [&] {
        if (g_own || !g_video || !g_room) {
            rc = SHARE_ERR_STATE;
            return;
        }
        set_ocb(cb, frame, user);
        g_own = true;
        g_video->registerLocalEncodedVideoFrameObserver(&g_local_observer);
        oemit(SHARE_WEV_ROOM_STATE, 0);
        if (g_published) oemit(SHARE_WEV_PUBLISH, 1);
        if (g_paused) oemit(SHARE_WEV_PAUSED, 1);
        rc = SHARE_OK;
    };
    on_sdk(f);
    return rc;
}

extern "C" int32_t volc_own_key_frame(int32_t* sdk_rc) {
    int32_t rc = SHARE_ERR_NOT_STARTED;
    int32_t raw = 0;
    auto f = [&] {
        if (!g_own || !g_video || !g_room) {
            rc = SHARE_ERR_STATE;
            return;
        }
        ScreenVideoEncoderConfig shrunk = g_enc;
        if (shrunk.width > 64 && shrunk.height > 64) {
            shrunk.width -= 16;
            shrunk.height -= 16;
            g_video->setScreenVideoEncoderConfig(shrunk);
            Sleep(100);
        }
        raw = g_video->setScreenVideoEncoderConfig(g_enc);
        rc = raw == 0 ? SHARE_OK : SHARE_ERR_ENCODER;
    };
    on_sdk(f);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" void volc_own_stop(void) {
    auto f = [] {
        if (!g_own) return;
        g_own = false;
        if (g_video) g_video->registerLocalEncodedVideoFrameObserver(nullptr);
        set_ocb(nullptr, nullptr, nullptr);
    };
    on_sdk(f);
}
