// Agora RTC SDK 观看桥接。SDK 在缓存目录中，启动时下载后按完整路径加载。
// 只导入两个 C 函数，以 GetProcAddress 取得，不链接导入库；其余调用经引擎对象的虚函数表。
// 只用 Win32 原语，不引入 C++ 运行库依赖，cdapp.exe 保持单文件运行。
// 引擎只在观看期间存在。Agora 的接口可在任意线程调用，这里统一在调用线程执行并以锁串行。
#include "agora_bridge.h"

#include <windows.h>
#include "IAgoraRtcEngine.h"
#include "IAgoraMediaEngine.h"

using namespace agora;
using namespace agora::rtc;
using namespace agora::media;

namespace {

// DLL 导出的函数，agora_load 成功后有效。IRtcEngine::release 是导出的静态成员，按修饰名取得。
typedef IRtcEngine*(AGORA_CALL* CreateEngineFn)();
typedef const char*(AGORA_CALL* VersionFn)(int*);
typedef void(__cdecl* ReleaseFn)(void (*)());
CreateEngineFn g_create = nullptr;
VersionFn g_version = nullptr;
ReleaseFn g_release = nullptr;

// 串行化全部引擎调用。
SRWLOCK g_lock = SRWLOCK_INIT;
IRtcEngine* g_engine = nullptr;
IMediaEngine* g_media = nullptr;
char g_log_file[MAX_PATH * 3] = {0};

// 进房参数，重新进房时复用。只在持有 g_lock 时读写。
char g_token[1024] = {0};
char g_channel[256] = {0};
uid_t g_uid = 0;
// 分享者，回调线程读取。设置回调前写入、清空回调后才改。
uid_t g_liver = 0;

// 回调目标。SDK 内部线程读取，持有 g_lock 时写入。
SRWLOCK g_cb_lock = SRWLOCK_INIT;
volc_event_fn g_cb = nullptr;
volc_frame_fn g_frame = nullptr;
void* g_user = nullptr;

void emit(int32_t kind, int32_t a = 0, int32_t b = 0, int32_t c = 0, int32_t d = 0) {
    AcquireSRWLockShared(&g_cb_lock);
    if (g_cb) g_cb(g_user, kind, a, b, c, d);
    ReleaseSRWLockShared(&g_cb_lock);
}

void set_cb(volc_event_fn cb, volc_frame_fn frame, void* user) {
    AcquireSRWLockExclusive(&g_cb_lock);
    g_cb = cb;
    g_frame = frame;
    g_user = user;
    ReleaseSRWLockExclusive(&g_cb_lock);
}

// 无符号十进制解析，非数字或超出 32 位时返回 false。
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

// 处理器的生命周期须长于引擎，因此为静态对象。
struct Handler : IRtcEngineEventHandler {
    void onJoinChannelSuccess(const char*, uid_t, int) override { emit(VOLC_WEV_ROOM_STATE, 0); }
    void onRejoinChannelSuccess(const char*, uid_t, int) override { emit(VOLC_WEV_ROOM_STATE, 0); }
    void onError(int err, const char*) override { emit(VOLC_WEV_ROOM_STATE, -err); }
    void onUserJoined(uid_t uid, int) override {
        if (uid == g_liver) emit(VOLC_WEV_PUBLISH, 1);
    }
    void onUserOffline(uid_t uid, USER_OFFLINE_REASON_TYPE reason) override {
        if (uid == g_liver) emit(VOLC_WEV_PUBLISH, 0, static_cast<int32_t>(reason));
    }
    void onTokenPrivilegeWillExpire(const char*) override { emit(VOLC_WEV_TOKEN_WILL_EXPIRE); }
    void onRemoteVideoStats(const RemoteVideoStats& s) override {
        if (s.uid == g_liver) emit(VOLC_WEV_STATS, s.receivedBitrate, s.width, s.height, s.totalFrozenTime);
    }
    // 连接被服务端断开或令牌失效时上报为房间错误，交给会话处理。
    void onConnectionStateChanged(CONNECTION_STATE_TYPE state, CONNECTION_CHANGED_REASON_TYPE reason) override {
        if (state == CONNECTION_STATE_FAILED) emit(VOLC_WEV_ROOM_STATE, -10000 - static_cast<int32_t>(reason));
    }
} g_handler;

// 编码后的远端帧。SDK 内部线程触发。
struct EncodedObserver : IVideoEncodedFrameObserver {
    bool onEncodedVideoFrameReceived(const char*, uid_t uid, const uint8_t* data, size_t length,
                                     const EncodedVideoFrameInfo& info) override {
        if (uid != g_liver) return true;
        // 编码沿用火山的编号：1 为 H.264，其他编码不解码。
        const int32_t codec = info.codecType == VIDEO_CODEC_H264 ? 1 : 0;
        AcquireSRWLockShared(&g_cb_lock);
        if (g_frame)
            g_frame(g_user, data, static_cast<int32_t>(length), codec,
                    info.frameType == VIDEO_FRAME_TYPE_KEY_FRAME ? 1 : 0, info.width, info.height,
                    static_cast<int64_t>(info.captureTimeMs) * 1000);
        ReleaseSRWLockShared(&g_cb_lock);
        return true;
    }
} g_encoded;

void copy_str(char* dst, size_t cap, const char* src) { lstrcpynA(dst, src ? src : "", static_cast<int>(cap)); }

// 以当前参数进房，只在持有 g_lock 时调用。
int join() {
    // 实测观众角色收不到画面，与官方 Web 端相同改用主播角色，不发布任何流。
    ChannelMediaOptions o;
    o.clientRoleType = CLIENT_ROLE_BROADCASTER;
    o.autoSubscribeAudio = true;
    o.autoSubscribeVideo = true;
    o.publishCameraTrack = false;
    o.publishMicrophoneTrack = false;
    o.publishScreenTrack = false;
    VideoSubscriptionOptions so;
    so.encodedFrameOnly = true;
    so.type = VIDEO_STREAM_HIGH;
    g_engine->setRemoteVideoSubscriptionOptions(g_liver, so);
    return g_engine->joinChannel(g_token, g_channel, g_uid, o);
}

void teardown() {
    if (g_engine) {
        g_engine->leaveChannel();
        if (g_media) g_media->registerVideoEncodedFrameObserver(nullptr);
        g_media = nullptr;
        // 回调为空时同步销毁，等内部线程退出后返回，之后不再回调。
        g_release(nullptr);
        g_engine = nullptr;
    }
    set_cb(nullptr, nullptr, nullptr);
    g_liver = 0;
    SecureZeroMemory(g_token, sizeof g_token);
}

}  // namespace

extern "C" int32_t agora_load(const uint16_t* dir, uint32_t* win32_error) {
    const wchar_t* d = reinterpret_cast<const wchar_t*>(dir);
    DWORD err = 0;
    int32_t rc = VOLC_OK;
    // 附属 DLL 从 SDK 目录找到。火山桥接已设置默认搜索目录，这里只追加目录。
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
    // 导出的全局函数，不需要引擎。
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
    uid_t uid = 0, liver = 0;
    if (g_engine) {
        rc = VOLC_ERR_STATE;
    } else if (!g_create) {
        rc = VOLC_ERR_NOT_STARTED;
    } else if (!parse_uid(w->uid, &uid) || !parse_uid(w->liver, &liver)) {
        rc = VOLC_ERR_SOURCE;
    } else {
        g_uid = uid;
        g_liver = liver;
        copy_str(g_token, sizeof g_token, w->token);
        copy_str(g_channel, sizeof g_channel, w->room);
        set_cb(cb, frame, user);
        g_engine = g_create();
        RtcEngineContext ctx;
        ctx.appId = w->app_id;
        ctx.eventHandler = &g_handler;
        ctx.channelProfile = CHANNEL_PROFILE_LIVE_BROADCASTING;
        if (g_log_file[0]) {
            ctx.logConfig.filePath = g_log_file;
            ctx.logConfig.level = commons::LOG_LEVEL::LOG_LEVEL_WARN;
        }
        raw = g_engine ? g_engine->initialize(ctx) : -1;
        if (raw != 0) {
            if (g_engine) g_release(nullptr);
            g_engine = nullptr;
            teardown();
            rc = VOLC_ERR_ENGINE;
        } else {
            g_engine->enableVideo();
            g_engine->queryInterface(AGORA_IID_MEDIA_ENGINE, reinterpret_cast<void**>(&g_media));
            if (g_media) g_media->registerVideoEncodedFrameObserver(&g_encoded);
            raw = join();
            if (raw != 0) {
                teardown();
                rc = VOLC_ERR_JOIN;
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
    if (g_engine) {
        g_engine->leaveChannel();
        raw = join();
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
    if (g_engine) {
        copy_str(g_token, sizeof g_token, token);
        raw = g_engine->renewToken(token);
        rc = raw == 0 ? VOLC_OK : VOLC_ERR_TOKEN;
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (sdk_rc) *sdk_rc = raw;
    return rc;
}

extern "C" void agora_watch_stop(void) {
    AcquireSRWLockExclusive(&g_lock);
    teardown();
    ReleaseSRWLockExclusive(&g_lock);
}
