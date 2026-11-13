#pragma once

#include "volc_bridge.h"

#ifdef __cplusplus
extern "C" {
#endif

int32_t agora_load(const uint16_t* dir, uint32_t* win32_error);

const char* agora_version(void);

void agora_log(const char* file);

int32_t agora_watch_start(const volc_watch* watch, volc_event_fn cb, volc_frame_fn frame, void* user, int32_t* sdk_rc);

int32_t agora_watch_rejoin(int32_t* sdk_rc);

int32_t agora_watch_token(const char* token, int32_t* sdk_rc);

void agora_watch_stop(void);

int32_t agora_live_start(const volc_live* live, volc_event_fn cb, void* user, int32_t* sdk_rc);

int32_t agora_publish(int32_t* sdk_rc);

int32_t agora_encoder(int32_t width, int32_t height, int32_t frame_rate, int32_t max_kbps, int32_t* sdk_rc);

int32_t agora_token(const char* token, int32_t* sdk_rc);

void agora_live_stop(void);

int32_t agora_push_audio(const int16_t* pcm, int32_t frames, int32_t* sdk_rc);

#ifdef __cplusplus
}
#endif
