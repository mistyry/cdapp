#pragma once

#include <stdint.h>

#include "share_bridge.h"

#ifdef __cplusplus
extern "C" {
#endif

enum {
    VOLC_ERR_THREAD = -2,
    VOLC_ERR_SUBSCRIBE = -16,
};

typedef void (*volc_frame_fn)(void* user, const uint8_t* data, int32_t size, int32_t codec, int32_t key, int32_t width,
                              int32_t height, int64_t ts_us);

typedef void (*volc_event_fn)(void* user, int32_t kind, int32_t a, int32_t b, int32_t c, int32_t d);

#define VOLC_NAME_LEN 256

typedef struct {
    int32_t kind;
    uint64_t id;
    int32_t pid;
    int32_t primary;
    int32_t x, y, width, height;
    char name[VOLC_NAME_LEN];
    char application[VOLC_NAME_LEN];
} volc_source;

typedef struct {
    const char* app_id;
    const char* room;
    const char* uid;
    const char* token;
    int32_t source_kind;
    uint64_t source_id;
    int32_t width, height, frame_rate, max_kbps;
    int32_t audio;
} volc_live;

int32_t volc_start(void);

void volc_stop(void);

int32_t volc_load(const uint16_t* dir, uint32_t* win32_error);

const char* volc_version(void);

int32_t volc_log(const char* dir);

int32_t volc_sources(volc_source* out, int32_t cap);

int32_t volc_thumbnail(int32_t kind, uint64_t id, int32_t max_w, int32_t max_h, uint8_t* out, int32_t* w,
                       int32_t* h);

int32_t volc_preview_open(void);

void volc_preview_close(void);

int32_t volc_live_start(const volc_live* live, volc_event_fn cb, void* user, int32_t* sdk_rc);

int32_t volc_publish(int32_t* sdk_rc);

int32_t volc_encoder(int32_t width, int32_t height, int32_t frame_rate, int32_t max_kbps, int32_t* sdk_rc);

int32_t volc_token(const char* token, int32_t* sdk_rc);

int32_t volc_pause(int32_t paused, int32_t* sdk_rc);

void volc_live_stop(void);

int32_t volc_push_audio(const int16_t* pcm, int32_t frames, int32_t* sdk_rc);

typedef struct {
    const char* app_id;
    const char* room;
    const char* uid;
    const char* token;
    const char* liver;
    int32_t volume;
} volc_watch;

int32_t volc_watch_start(const volc_watch* watch, volc_event_fn cb, volc_frame_fn frame, void* user, int32_t* sdk_rc);

int32_t volc_watch_subscribe(int32_t* sdk_rc);

int32_t volc_watch_key_frame(int32_t* sdk_rc);

int32_t volc_watch_token(const char* token, int32_t* sdk_rc);

int32_t volc_watch_volume(int32_t volume, int32_t* sdk_rc);

void volc_watch_stop(void);

int32_t volc_own_start(volc_event_fn cb, volc_frame_fn frame, void* user);

int32_t volc_own_key_frame(int32_t* sdk_rc);

void volc_own_stop(void);

#ifdef __cplusplus
}
#endif
