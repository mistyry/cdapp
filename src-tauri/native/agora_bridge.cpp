#include "agora_bridge.h"

#include <windows.h>
#include "IAgoraRtcEngine.h"
#include "IAgoraRtcEngineEx.h"
#include "IAgoraMediaEngine.h"

using namespace agora;
using namespace agora::rtc;
using namespace agora::media;

namespace {

typedef IRtcEngine*(AGORA_CALL* CreateEngineFn)();
typedef const char*(AGORA_CALL* VersionFn)(int*);
typedef void(__cdecl* ReleaseFn)(void (*)());
CreateEngineFn g_create = nullptr;
VersionFn g_version = nullptr;
ReleaseFn g_release = nullptr;

SRWLOCK g_lock = SRWLOCK_INIT;
IRtcEngineEx* g_engine = nullptr;
IMediaEngine* g_media = nullptr;
char g_log_file[MAX_PATH * 3] = {0};

bool g_pushing = false;
char g_push_token[1024] = {0};
char g_push_channel[256] = {0};
uid_t g_push_uid = 0;
bool g_capturing = false;
track_id_t g_track = 0;
ScreenCaptureParameters g_cap;

bool g_watching = false;
char g_watch_token[1024] = {0};
char g_watch_channel[256] = {0};
uid_t g_watch_uid = 0;
volatile LONG g_liver = 0;
volatile LONG g_frozen = 0;

SRWLOCK g_cb_lock = SRWLOCK_INIT;
volc_event_fn g_push_cb = nullptr;
void* g_push_user = nullptr;
bool g_audio = false;
volc_event_fn g_watch_cb = nullptr;
volc_frame_fn g_frame = nullptr;
void* g_watch_user = nullptr;
char g_frame_channel[256] = {0};

uid_t liver() { return static_cast<uid_t>(InterlockedCompareExchange(&g_liver, 0, 0)); }

void pemit(int32_t kind, int32_t a = 0, int32_t b = 0, int32_t c = 0, int32_t d = 0) {
    AcquireSRWLockShared(&g_cb_lock);
    if (g_push_cb) g_push_cb(g_push_user, kind, a, b, c, d);
    ReleaseSRWLockShared(&g_cb_lock);
}

void wemit(int32_t kind, int32_t a = 0, int32_t b = 0, int32_t c = 0, int32_t d = 0) {
    AcquireSRWLockShared(&g_cb_lock);
    if (g_watch_cb) g_watch_cb(g_watch_user, kind, a, b, c, d);
    ReleaseSRWLockShared(&g_cb_lock);
}

void copy_str(char* dst, size_t cap, const char* src) { lstrcpynA(dst, src ? src : "", static_cast<int>(cap)); }

void set_push_cb(volc_event_fn cb, void* user) {
    AcquireSRWLockExclusive(&g_cb_lock);
    g_push_cb = cb;
    g_push_user = user;
    g_audio = false;
    ReleaseSRWLockExclusive(&g_cb_lock);
}

void set_watch_cb(volc_event_fn cb, volc_frame_fn frame, void* user, const char* channel) {
    AcquireSRWLockExclusive(&g_cb_lock);
    g_watch_cb = cb;
    g_frame = frame;
    g_watch_user = user;
    copy_str(g_frame_channel, sizeof g_frame_channel, channel);
    ReleaseSRWLockExclusive(&g_cb_lock);
}

bool parse_uid(const char* s, uid_t* out) {
    if (!s || !*s) return false;
    unsigned long long v = 0;
    for (; *s; s++) {
        if (*s < '0' || *s > '9') return false;
        v = v * 10 + static_cast<unsigned>(*s - '0');
        if (v > 0xFFFFFFFFull) return false;
    }
    *out = static_cast<uid_t>(v);
    return true;
}

bool is_screen(VIDEO_SOURCE_TYPE s) { return s == VIDEO_SOURCE_SCREEN_PRIMARY; }

struct PushHandler : IRtcEngineEventHandler {
    void onJoinChannelSuccess(const char*, uid_t, int) override { pemit(VOLC_EV_ROOM_STATE, 0); }
    void onRejoinChannelSuccess(const char*, uid_t, int) override { pemit(VOLC_EV_ROOM_STATE, 0); }
    void onError(int err, const char*) override { pemit(VOLC_EV_ERROR, err); }
    void onTokenPrivilegeWillExpire(const char*) override { pemit(VOLC_EV_TOKEN_WILL_EXPIRE); }
    void onConnectionStateChanged(CONNECTION_STATE_TYPE state, CONNECTION_CHANGED_REASON_TYPE reason) override {
        if (state == CONNECTION_STATE_FAILED) pemit(VOLC_EV_ROOM_STATE, -10000 - static_cast<int32_t>(reason));
    }
    void onFirstLocalVideoFrame(VIDEO_SOURCE_TYPE source, int width, int height, int) override {
        if (is_screen(source)) pemit(VOLC_EV_FIRST_FRAME, width, height);
    }
    void onVideoSizeChanged(VIDEO_SOURCE_TYPE source, uid_t uid, int width, int height, int) override {
        if (uid == 0 && is_screen(source)) pemit(VOLC_EV_SIZE_CHANGED, width, height);
    }
    void onLocalVideoStateChanged(VIDEO_SOURCE_TYPE source, LOCAL_VIDEO_STREAM_STATE state,
                                  LOCAL_VIDEO_STREAM_REASON reason) override {
        if (!is_screen(source)) return;
        if (reason == LOCAL_VIDEO_STREAM_REASON_SCREEN_CAPTURE_WINDOW_CLOSED) pemit(VOLC_EV_SOURCE_GONE);
        pemit(VOLC_EV_VIDEO_STATE, state, reason);
    }
    void onLocalVideoStats(VIDEO_SOURCE_TYPE source, const LocalVideoStats& s) override {
        if (is_screen(source))
            pemit(VOLC_EV_STATS, s.sentBitrate, s.encoderOutputFrameRate, s.encodedFrameWidth, s.encodedFrameHeight);
    }
} g_push_handler;

struct WatchHandler : IRtcEngineEventHandler {
    void onJoinChannelSuccess(const char*, uid_t, int) override { wemit(VOLC_WEV_ROOM_STATE, 0); }
    void onRejoinChannelSuccess(const char*, uid_t, int) override { wemit(VOLC_WEV_ROOM_STATE, 0); }
    void onError(int err, const char*) override { wemit(VOLC_WEV_ROOM_STATE, -err); }
    void onTokenPrivilegeWillExpire(const char*) override { wemit(VOLC_WEV_TOKEN_WILL_EXPIRE); }
    void onConnectionStateChanged(CONNECTION_STATE_TYPE state, CONNECTION_CHANGED_REASON_TYPE reason) override {
        if (state == CONNECTION_STATE_FAILED) wemit(VOLC_WEV_ROOM_STATE, -10000 - static_cast<int32_t>(reason));
    }
    void onUserOffline(uid_t uid, USER_OFFLINE_REASON_TYPE reason) override {
        if (uid == liver()) wemit(VOLC_WEV_PUBLISH, 0, static_cast<int32_t>(reason));
    }
    void onRemoteVideoStateChanged(uid_t uid, REMOTE_VIDEO_STATE state, REMOTE_VIDEO_STATE_REASON reason,
                                   int) override {
        if (uid != liver()) return;
        if (state == REMOTE_VIDEO_STATE_STARTING || state == REMOTE_VIDEO_STATE_DECODING)
            wemit(VOLC_WEV_PUBLISH, 1);
        else if (state == REMOTE_VIDEO_STATE_STOPPED && reason == REMOTE_VIDEO_STATE_REASON_REMOTE_MUTED)
            wemit(VOLC_WEV_PUBLISH, 0, static_cast<int32_t>(reason));
    }
    void onRemoteVideoStats(const RemoteVideoStats& s) override {
        if (s.uid != liver()) return;
        const LONG total = s.totalFrozenTime > 0 ? s.totalFrozenTime : 0;
        const LONG prev = InterlockedExchange(&g_frozen, total);
        wemit(VOLC_WEV_STATS, s.receivedBitrate, s.width, s.height, total >= prev ? total - prev : total);
    }
} g_watch_handler;

struct EncodedObserver : IVideoEncodedFrameObserver {
    bool onEncodedVideoFrameReceived(const char* channel, uid_t uid, const uint8_t* data, size_t length,
                                     const EncodedVideoFrameInfo& info) override {
        if (uid != liver()) return true;
        int32_t codec = 0;
        switch (info.codecType) {
            case VIDEO_CODEC_H264: codec = 1; break;
            case VIDEO_CODEC_H265: codec = 2; break;
            case VIDEO_CODEC_AV1: codec = 3; break;
            case VIDEO_CODEC_VP8: codec = 4; break;
            case VIDEO_CODEC_VP9: codec = 5; break;
            default: break;
        }
        AcquireSRWLockShared(&g_cb_lock);
        if (g_frame && channel && lstrcmpA(channel, g_frame_channel) == 0)
            g_frame(g_watch_user, data, static_cast<int32_t>(length), codec,
                    info.frameType == VIDEO_FRAME_TYPE_KEY_FRAME ? 1 : 0, info.width, info.height,
                    static_cast<int64_t>(info.captureTimeMs) * 1000);
        ReleaseSRWLockShared(&g_cb_lock);
        return true;
    }
} g_encoded;

int32_t ensure_engine(const char* app_id, int* raw) {
    if (g_engine) return VOLC_OK;
    if (!g_create) return VOLC_ERR_NOT_STARTED;
    g_engine = static_cast<IRtcEngineEx*>(g_create());
    RtcEngineContext ctx;
    ctx.appId = app_id;
    ctx.eventHandler = &g_push_handler;
    ctx.channelProfile = CHANNEL_PROFILE_LIVE_BROADCASTING;
    if (g_log_file[0]) {
        ctx.logConfig.filePath = g_log_file;
        ctx.logConfig.level = commons::LOG_LEVEL::LOG_LEVEL_WARN;
    }
    *raw = g_engine ? g_engine->initialize(ctx) : -1;
    if (*raw != 0) {
        if (g_engine) g_release(nullptr);
        g_engine = nullptr;
        return VOLC_ERR_ENGINE;
    }
    g_engine->enableVideo();
    g_engine->queryInterface(AGORA_IID_MEDIA_ENGINE, reinterpret_cast<void**>(&g_media));
    if (g_media) g_media->registerVideoEncodedFrameObserver(&g_encoded);
    return VOLC_OK;
}

void release_if_idle() {
    if (g_pushing || g_watching || !g_engine) return;
    if (g_media) g_media->registerVideoEncodedFrameObserver(nullptr);
    g_media = nullptr;
    g_release(nullptr);
    g_engine = nullptr;
}

RtcConnection watch_conn() { return RtcConnection(g_watch_channel, g_watch_uid); }

int join_watch() {
    ChannelMediaOptions o;
    o.clientRoleType = CLIENT_ROLE_BROADCASTER;
    o.autoSubscribeAudio = true;
    o.autoSubscribeVideo = true;
    o.publishCameraTrack = false;
    o.publishMicrophoneTrack = false;
    o.publishScreenTrack = false;
    InterlockedExchange(&g_frozen, 0);
    const int raw = g_engine->joinChannelEx(g_watch_token, watch_conn(), o, &g_watch_handler);
    if (raw != 0) return raw;
    VideoSubscriptionOptions so;
    so.encodedFrameOnly = true;
    so.type = VIDEO_STREAM_HIGH;
    g_engine->setRemoteVideoSubscriptionOptionsEx(liver(), so, watch_conn());
    return 0;
}

void stop_watch() {
    set_watch_cb(nullptr, nullptr, nullptr, nullptr);
    if (g_engine && g_watching) g_engine->leaveChannelEx(watch_conn());
    g_watching = false;
    InterlockedExchange(&g_liver, 0);
    SecureZeroMemory(g_watch_token, sizeof g_watch_token);
    release_if_idle();
}

void stop_push() {
    set_push_cb(nullptr, nullptr);
    if (g_engine && g_pushing) {
        if (g_capturing) g_engine->stopScreenCapture();
        g_engine->leaveChannel();
        if (g_track && g_media) g_media->destroyCustomAudioTrack(g_track);
    }
    g_capturing = false;
    g_track = 0;
    g_pushing = false;
    SecureZeroMemory(g_push_token, sizeof g_push_token);
    release_if_idle();
}

bool monitor_rect(uint64_t id, rtc::Rectangle* out) {
    MONITORINFO mi;
    mi.cbSize = sizeof mi;
    if (!GetMonitorInfoW(reinterpret_cast<HMONITOR>(id), &mi)) return false;
    *out = rtc::Rectangle(mi.rcMonitor.left, mi.rcMonitor.top, mi.rcMonitor.right - mi.rcMonitor.left,
                          mi.rcMonitor.bottom - mi.rcMonitor.top);
    return true;
}

}

extern "C" int32_t agora_load(const uint16_t* dir, uint32_t* win32_error) {
    const wchar_t* d = reinterpret_cast<const wchar_t*>(dir);
    DWORD err = 0;
    int32_t rc = VOLC_OK;
    if (!SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_USER_DIRS) || !AddDllDirectory(d)) {
        err = GetLastError();
        rc = VOLC_ERR_LOAD;
    } else {
        wchar_t dll[MAX_PATH * 2];
        static const wchar_t kName[] = L"\\agora_rtc_sdk.dll";
        size_t n = lstrlenW(d);
        if (n + ARRAYSIZE(kName) > ARRAYSIZE(dll)) {
            err = ERROR_FILENAME_EXCED_RANGE;
            rc = VOLC_ERR_LOAD;
        } else {
            CopyMemory(dll, d, n * sizeof(wchar_t));
            CopyMemory(dll + n, kName, sizeof kName);
            HMODULE m = LoadLibraryExW(dll, nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
            if (!m) {
                err = GetLastError();
                rc = VOLC_ERR_LOAD;
            } else {
                g_create = reinterpret_cast<CreateEngineFn>(GetProcAddress(m, "createAgoraRtcEngine"));
                g_version = reinterpret_cast<VersionFn>(GetProcAddress(m, "getAgoraSdkVersion"));
                g_release = reinterpret_cast<ReleaseFn>(GetProcAddress(m, "?release@IRtcEngine@rtc@agora@@SAXP6AXXZ@Z"));
                if (!g_create || !g_version || !g_release) {
                    err = ERROR_PROC_NOT_FOUND;
                    rc = VOLC_ERR_LOAD;
                }
            }
        }
    }
    if (win32_error) *win32_error = err;
    return rc;
}

extern "C" const char* agora_version(void) {
    return g_version ? g_version(nullptr) : "";
}

extern "C" void agora_log(const char* file) {
    AcquireSRWLockExclusive(&g_lock);
    copy_str(g_log_file, sizeof g_log_file, file);
    ReleaseSRWLockExclusive(&g_lock);
}

extern "C" int32_t agora_watch_start(const volc_watch* w, volc_event_fn cb, volc_frame_fn frame, void* user,
                                     int32_t* sdk_rc) {
    int32_t rc = VOLC_OK;
    int raw = 0;
    AcquireSRWLockExclusive(&g_lock);
    uid_t uid = 0, lv = 0;
    if (g_watching) {
        rc = VOLC_ERR_STATE;
    } else if (!parse_uid(w->uid, &uid) || !parse_uid(w->liver, &lv)) {
        rc = VOLC_ERR_SOURCE;
    } else {
        g_watch_uid = uid;
        InterlockedExchange(&g_liver, static_cast<LONG>(lv));
        copy_str(g_watch_token, sizeof g_watch_token, w->token);
        copy_str(g_watch_channel, sizeof g_watch_channel, w->room);
        rc = ensure_engine(w->app_id, &raw);
        if (rc != VOLC_OK) {
            stop_watch();
        } else {
            set_watch_cb(cb, frame, user, g_watch_channel);
            raw = join_watch();
            if (raw != 0) {
                stop_watch();
                rc = VOLC_ERR_JOIN;
            } else {
                g_watching = true;
            }
        }
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t agora_watch_rejoin(int32_t* sdk_rc) {
    int32_t rc = VOLC_ERR_STATE;
    int raw = 0;
    AcquireSRWLockExclusive(&g_lock);
    if (g_engine && g_watching) {
        g_engine->leaveChannelEx(watch_conn());
        raw = join_watch();
        rc = raw == 0 ? VOLC_OK : VOLC_ERR_JOIN;
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t agora_watch_token(const char* token, int32_t* sdk_rc) {
    int32_t rc = VOLC_ERR_STATE;
    int raw = 0;
    AcquireSRWLockExclusive(&g_lock);
    if (g_engine && g_watching) {
        copy_str(g_watch_token, sizeof g_watch_token, token);
        ChannelMediaOptions o;
        o.token = g_watch_token;
        raw = g_engine->updateChannelMediaOptionsEx(o, watch_conn());
        rc = raw == 0 ? VOLC_OK : VOLC_ERR_TOKEN;
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" void agora_watch_stop(void) {
    AcquireSRWLockExclusive(&g_lock);
    if (g_watching) stop_watch();
    ReleaseSRWLockExclusive(&g_lock);
}

extern "C" int32_t agora_live_start(const volc_live* live, volc_event_fn cb, void* user, int32_t* sdk_rc) {
    int32_t rc = VOLC_OK;
    int raw = 0;
    AcquireSRWLockExclusive(&g_lock);
    uid_t uid = 0;
    rtc::Rectangle screen;
    const bool window = live->source_kind == 1;
    if (g_pushing) {
        rc = VOLC_ERR_STATE;
    } else if (!parse_uid(live->uid, &uid) || (!window && !monitor_rect(live->source_id, &screen))) {
        rc = VOLC_ERR_SOURCE;
    } else {
        g_push_uid = uid;
        copy_str(g_push_token, sizeof g_push_token, live->token);
        copy_str(g_push_channel, sizeof g_push_channel, live->room);
        rc = ensure_engine(live->app_id, &raw);
        if (rc != VOLC_OK) {
            stop_push();
        } else {
            set_push_cb(cb, user);
            g_pushing = true;
            if (live->audio && g_media) {
                AudioTrackConfig tc;
                tc.enableLocalPlayback = false;
                g_track = g_media->createCustomAudioTrack(AUDIO_TRACK_MIXABLE, tc);
            }
            ChannelMediaOptions o;
            o.clientRoleType = CLIENT_ROLE_BROADCASTER;
            o.autoSubscribeAudio = false;
            o.autoSubscribeVideo = false;
            o.publishCameraTrack = false;
            o.publishMicrophoneTrack = false;
            o.publishScreenTrack = false;
            raw = g_engine->joinChannel(g_push_token, g_push_channel, g_push_uid, o);
            if (raw != 0) {
                stop_push();
                rc = VOLC_ERR_JOIN;
            } else {
                g_cap = ScreenCaptureParameters();
                g_cap.dimensions = VideoDimensions(live->width, live->height);
                g_cap.frameRate = live->frame_rate;
                g_cap.bitrate = live->max_kbps;
                g_cap.captureMouseCursor = true;
                g_cap.enableHighLight = true;
                g_cap.highLightColor = 0xFFFFD700;
                g_cap.highLightWidth = 5;
                raw = window ? g_engine->startScreenCaptureByWindowId(static_cast<int64_t>(live->source_id),
                                                                      rtc::Rectangle(), g_cap)
                             : g_engine->startScreenCaptureByScreenRect(screen, rtc::Rectangle(), g_cap);
                if (raw != 0) {
                    stop_push();
                    rc = VOLC_ERR_CAPTURE;
                } else {
                    g_capturing = true;
                }
            }
        }
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t agora_publish(int32_t* sdk_rc) {
    int32_t rc = VOLC_ERR_STATE;
    int raw = 0;
    AcquireSRWLockExclusive(&g_lock);
    if (g_engine && g_pushing) {
        ChannelMediaOptions o;
        o.publishScreenTrack = true;
        if (g_track) {
            o.publishCustomAudioTrack = true;
            o.publishCustomAudioTrackId = static_cast<int>(g_track);
        }
        raw = g_engine->updateChannelMediaOptions(o);
        rc = raw == 0 ? VOLC_OK : VOLC_ERR_PUBLISH;
        if (raw == 0 && g_track) {
            AcquireSRWLockExclusive(&g_cb_lock);
            g_audio = true;
            ReleaseSRWLockExclusive(&g_cb_lock);
        }
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t agora_encoder(int32_t width, int32_t height, int32_t frame_rate, int32_t max_kbps, int32_t* sdk_rc) {
    int32_t rc = VOLC_ERR_STATE;
    int raw = 0;
    AcquireSRWLockExclusive(&g_lock);
    if (g_engine && g_pushing && g_capturing) {
        g_cap.dimensions = VideoDimensions(width, height);
        g_cap.frameRate = frame_rate;
        g_cap.bitrate = max_kbps;
        raw = g_engine->updateScreenCaptureParameters(g_cap);
        rc = raw == 0 ? VOLC_OK : VOLC_ERR_ENCODER;
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" int32_t agora_token(const char* token, int32_t* sdk_rc) {
    int32_t rc = VOLC_ERR_STATE;
    int raw = 0;
    AcquireSRWLockExclusive(&g_lock);
    if (g_engine && g_pushing) {
        copy_str(g_push_token, sizeof g_push_token, token);
        raw = g_engine->renewToken(g_push_token);
        rc = raw == 0 ? VOLC_OK : VOLC_ERR_TOKEN;
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" void agora_live_stop(void) {
    AcquireSRWLockExclusive(&g_lock);
    if (g_pushing) stop_push();
    ReleaseSRWLockExclusive(&g_lock);
}

extern "C" int32_t agora_push_audio(const int16_t* pcm, int32_t frames, int32_t* sdk_rc) {
    int32_t rc = VOLC_ERR_STATE;
    int raw = 0;
    if (TryAcquireSRWLockShared(&g_lock)) {
        AcquireSRWLockShared(&g_cb_lock);
        const bool on = g_audio;
        ReleaseSRWLockShared(&g_cb_lock);
        if (on && g_media && g_pushing) {
            IAudioFrameObserverBase::AudioFrame f;
            f.type = IAudioFrameObserverBase::FRAME_TYPE_PCM16;
            f.samplesPerChannel = frames;
            f.bytesPerSample = TWO_BYTES_PER_SAMPLE;
            f.channels = 2;
            f.samplesPerSec = 48000;
            f.buffer = const_cast<int16_t*>(pcm);
            raw = g_media->pushAudioFrame(&f, g_track);
            rc = raw == 0 ? VOLC_OK : VOLC_ERR_AUDIO;
        }
        ReleaseSRWLockShared(&g_lock);
    }
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}
