#ifndef JUXU_CORE_H
#define JUXU_CORE_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct JuxuEngineHandle JuxuEngineHandle;

JuxuEngineHandle *juxu_engine_new(void);
void juxu_engine_free(JuxuEngineHandle *handle);
void juxu_string_free(char *ptr);

bool juxu_engine_load_dict_mmap(JuxuEngineHandle *handle, uint32_t schema_id, const char *path);
bool juxu_engine_load_pinyin_dict_mmap(JuxuEngineHandle *handle, const char *path);
bool juxu_engine_load_english_dict_mmap(JuxuEngineHandle *handle, const char *path);
bool juxu_engine_load_aux_mmap(JuxuEngineHandle *handle, const char *path);
void juxu_engine_retain_only_schema_dict(JuxuEngineHandle *handle, uint32_t schema_id);
bool juxu_engine_has_active_dict(const JuxuEngineHandle *handle);

bool juxu_engine_load_config(JuxuEngineHandle *handle, const char *path);
bool juxu_engine_save_config(const JuxuEngineHandle *handle, const char *path);
void juxu_engine_set_config_toml(JuxuEngineHandle *handle, const char *toml_str);
char *juxu_engine_get_config_toml(const JuxuEngineHandle *handle);

bool juxu_engine_load_user_data(JuxuEngineHandle *handle, const char *path);
bool juxu_engine_save_user_data(JuxuEngineHandle *handle, const char *path);
bool juxu_engine_is_user_dirty(const JuxuEngineHandle *handle);

void juxu_engine_set_schema(JuxuEngineHandle *handle, uint32_t schema_id);
uint32_t juxu_engine_get_schema(const JuxuEngineHandle *handle);

void juxu_engine_set_ascii_mode(JuxuEngineHandle *handle, bool ascii_mode);
bool juxu_engine_get_ascii_mode(const JuxuEngineHandle *handle);
char *juxu_engine_toggle_ascii_json(JuxuEngineHandle *handle);

void juxu_engine_set_key_layout(
    JuxuEngineHandle *handle,
    const float *centers_xy_52,
    float origin_x,
    float origin_y,
    float pitch_x,
    float pitch_y
);
void juxu_engine_set_t9_layout(
    JuxuEngineHandle *handle,
    const float *centers_xy_16,
    float origin_x,
    float origin_y,
    float pitch_x,
    float pitch_y
);
void juxu_engine_reset_touch_profile(JuxuEngineHandle *handle);
// Offsets and sample counts are indexed by key, from 'a' to 'z'.
void juxu_engine_set_touch_profile(
    JuxuEngineHandle *handle,
    const float *offsets_xy_52,
    const uint32_t *counts_26
);
// Offsets and sample counts are indexed by T9 physical key, from '2' to '9'.
void juxu_engine_set_t9_touch_profile(
    JuxuEngineHandle *handle,
    const float *offsets_xy_16,
    const uint32_t *counts_8
);

char *juxu_engine_add_user_phrase(JuxuEngineHandle *handle, const char *phrase);
bool juxu_engine_remove_user_phrase(JuxuEngineHandle *handle, const char *phrase);
char *juxu_engine_list_user_phrases_json(const JuxuEngineHandle *handle);

void juxu_engine_clear(JuxuEngineHandle *handle);
bool juxu_engine_is_composing(const JuxuEngineHandle *handle);

char *juxu_engine_handle_key_json(
    JuxuEngineHandle *handle,
    uint16_t keycode,
    uint32_t unicode_scalar,
    bool has_cmd_or_ctrl,
    uint64_t timestamp_ms
);

char *juxu_engine_push_touch_json(
    JuxuEngineHandle *handle,
    uint8_t reported_ascii,
    float x,
    float y,
    bool is_normalized,
    uint64_t timestamp_ms
);

char *juxu_engine_push_t9_touch_json(
    JuxuEngineHandle *handle,
    uint8_t reported_digit,
    float x,
    float y,
    bool is_normalized,
    uint64_t timestamp_ms
);

char *juxu_engine_lock_t9_syllable_json(
    JuxuEngineHandle *handle,
    const char *syllable_utf8
);

char *juxu_engine_select_candidate_json(
    JuxuEngineHandle *handle,
    uint32_t page_rel_index,
    uint64_t timestamp_ms
);

char *juxu_engine_select_abs_candidate_json(
    JuxuEngineHandle *handle,
    uint32_t abs_index,
    uint64_t timestamp_ms
);

char *juxu_engine_all_candidates_json(const JuxuEngineHandle *handle, uint32_t max_count);
char *juxu_engine_touch_profile_json(const JuxuEngineHandle *handle);

char *juxu_engine_page_turn_json(JuxuEngineHandle *handle, int32_t delta);
char *juxu_engine_state_json(const JuxuEngineHandle *handle);

void juxu_engine_set_context(JuxuEngineHandle *handle, const char *context_utf8);
void juxu_engine_accept_prediction(
    JuxuEngineHandle *handle,
    const char *text_utf8,
    uint64_t timestamp_ms
);
char *juxu_engine_predict_next_json(const JuxuEngineHandle *handle, uint32_t max_count);
char *juxu_engine_drain_eval_log_jsonl(JuxuEngineHandle *handle);

#ifdef __cplusplus
}
#endif

#endif /* JUXU_CORE_H */
