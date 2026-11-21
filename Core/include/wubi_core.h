#ifndef WUBI_CORE_H
#define WUBI_CORE_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct WubiEngineHandle WubiEngineHandle;

WubiEngineHandle *wubi_engine_new(void);
void wubi_engine_free(WubiEngineHandle *handle);
void wubi_string_free(char *ptr);

bool wubi_engine_load_dict_mmap(WubiEngineHandle *handle, uint32_t schema_id, const char *path);
bool wubi_engine_load_pinyin_dict_mmap(WubiEngineHandle *handle, const char *path);
bool wubi_engine_load_aux_mmap(WubiEngineHandle *handle, const char *path);
void wubi_engine_retain_only_schema_dict(WubiEngineHandle *handle, uint32_t schema_id);

bool wubi_engine_load_config(WubiEngineHandle *handle, const char *path);
bool wubi_engine_save_config(const WubiEngineHandle *handle, const char *path);
void wubi_engine_set_config_toml(WubiEngineHandle *handle, const char *toml_str);
char *wubi_engine_get_config_toml(const WubiEngineHandle *handle);

bool wubi_engine_load_user_data(WubiEngineHandle *handle, const char *path);
bool wubi_engine_save_user_data(WubiEngineHandle *handle, const char *path);
bool wubi_engine_is_user_dirty(const WubiEngineHandle *handle);

void wubi_engine_set_schema(WubiEngineHandle *handle, uint32_t schema_id);
uint32_t wubi_engine_get_schema(const WubiEngineHandle *handle);

void wubi_engine_set_ascii_mode(WubiEngineHandle *handle, bool ascii_mode);
bool wubi_engine_get_ascii_mode(const WubiEngineHandle *handle);
char *wubi_engine_toggle_ascii_json(WubiEngineHandle *handle);

void wubi_engine_set_key_layout(
    WubiEngineHandle *handle,
    const float *centers_xy_52,
    float origin_x,
    float origin_y,
    float pitch_x,
    float pitch_y
);
void wubi_engine_reset_touch_profile(WubiEngineHandle *handle);
// Offsets and sample counts are indexed by key, from 'a' to 'z'.
void wubi_engine_set_touch_profile(
    WubiEngineHandle *handle,
    const float *offsets_xy_52,
    const uint32_t *counts_26
);

char *wubi_engine_add_user_phrase(WubiEngineHandle *handle, const char *phrase);
bool wubi_engine_remove_user_phrase(WubiEngineHandle *handle, const char *phrase);
char *wubi_engine_list_user_phrases_json(const WubiEngineHandle *handle);

void wubi_engine_clear(WubiEngineHandle *handle);
bool wubi_engine_is_composing(const WubiEngineHandle *handle);

char *wubi_engine_handle_key_json(
    WubiEngineHandle *handle,
    uint16_t keycode,
    uint32_t unicode_scalar,
    bool has_cmd_or_ctrl,
    uint64_t timestamp_ms
);

char *wubi_engine_push_touch_json(
    WubiEngineHandle *handle,
    uint8_t reported_ascii,
    float x,
    float y,
    bool is_normalized,
    uint64_t timestamp_ms
);

char *wubi_engine_select_candidate_json(
    WubiEngineHandle *handle,
    uint32_t page_rel_index,
    uint64_t timestamp_ms
);

char *wubi_engine_select_abs_candidate_json(
    WubiEngineHandle *handle,
    uint32_t abs_index,
    uint64_t timestamp_ms
);

char *wubi_engine_all_candidates_json(const WubiEngineHandle *handle, uint32_t max_count);
char *wubi_engine_touch_profile_json(const WubiEngineHandle *handle);

char *wubi_engine_page_turn_json(WubiEngineHandle *handle, int32_t delta);
char *wubi_engine_state_json(const WubiEngineHandle *handle);

void wubi_engine_set_context(WubiEngineHandle *handle, const char *context_utf8);
void wubi_engine_accept_prediction(
    WubiEngineHandle *handle,
    const char *text_utf8,
    uint64_t timestamp_ms
);
char *wubi_engine_predict_next_json(const WubiEngineHandle *handle, uint32_t max_count);

#ifdef __cplusplus
}
#endif

#endif /* WUBI_CORE_H */
