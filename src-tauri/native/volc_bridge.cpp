// 火山 RTC SDK 桥接。SDK 在缓存目录中，启动时下载后按完整路径加载，因此链接为延迟加载。
// 全部调用在专用线程执行，该线程运行消息循环。
// 只用 Win32 原语，不引入 C++ 运行库依赖，cdapp.exe 保持单文件运行。
#include "volc_bridge.h"

#include <windows.h>

#include "bytertc_video.h"

namespace {

constexpr UINT WM_VOLC_CALL = WM_APP + 1;

SRWLOCK g_lock = SRWLOCK_INIT;
HANDLE g_thread = nullptr;
DWORD g_thread_id = 0;

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

struct LoadCtx {
    const wchar_t* dir;
    int32_t rc;
    DWORD err;
};

void load_on_sdk(void* p) {
    auto* c = static_cast<LoadCtx*>(p);
    // 只从系统目录与显式添加的目录查找，附属 DLL 也从 SDK 目录找到。
    if (!SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_USER_DIRS) ||
        !AddDllDirectory(c->dir)) {
        c->err = GetLastError();
        c->rc = VOLC_ERR_LOAD;
        return;
    }
    wchar_t dll[MAX_PATH * 2];
    size_t n = lstrlenW(c->dir);
    static const wchar_t kName[] = L"\\VolcEngineRTC.dll";
    if (n + ARRAYSIZE(kName) > ARRAYSIZE(dll)) {
        c->err = ERROR_FILENAME_EXCED_RANGE;
        c->rc = VOLC_ERR_LOAD;
        return;
    }
    CopyMemory(dll, c->dir, n * sizeof(wchar_t));
    CopyMemory(dll + n, kName, sizeof kName);
    // 按完整路径加载，之后延迟加载按模块名找到这个已加载的模块。
    if (!LoadLibraryExW(dll, nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)) {
        c->err = GetLastError();
        c->rc = VOLC_ERR_LOAD;
        return;
    }
    c->rc = VOLC_OK;
}

void version_on_sdk(void* p) { *static_cast<const char**>(p) = bytertc::getSDKVersion(); }

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
    if (win32_error) *win32_error = 0;
    LoadCtx c{reinterpret_cast<const wchar_t*>(dir), VOLC_ERR_NOT_STARTED, 0};
    run_on_sdk(load_on_sdk, &c);
    if (win32_error) *win32_error = c.err;
    return c.rc;
}

extern "C" const char* volc_version(void) {
    const char* v = nullptr;
    run_on_sdk(version_on_sdk, &v);
    return v;
}
