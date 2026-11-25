use crate::config::{InputSchema, JuxuConfig};
use crate::engine::{EngineOutput, JuxuEngine};
use crate::english::EnDictView;
use crate::format::{AuxView, DictView};
use crate::json::{push_candidates_json, push_json_str};
use crate::pinyin_dict::PinyinDictView;
use crate::user_data::{clamp_key_touch_offset, TouchKeyOffset, UserStore};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

pub struct JuxuEngineHandle {
    pub inner: JuxuEngine,
}

#[inline]
fn cstr_to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(ptr).to_str().ok() }
    }
}

#[inline]
fn into_c_string(s: String) -> *mut c_char {
    let clean = if s.as_bytes().contains(&0) {
        s.replace('\0', "")
    } else {
        s
    };
    CString::new(clean)
        .unwrap_or_else(|_| CString::new("{}").unwrap())
        .into_raw()
}

fn serialize_action_and_state(engine: &JuxuEngine, act: Option<&EngineOutput>) -> String {
    use std::fmt::Write as _;
    let mut j = String::with_capacity(1024);
    j.push('{');
    let consumed = act.is_some_and(|a| a.consumed);
    let bell = act.is_some_and(|a| a.bell);
    let commit = act.map_or("", |a| a.commit_text.as_str());
    let notice = act.map_or("", |a| a.notice.as_str());
    let include_touch_offsets = act.is_none_or(|a| !a.commit_text.is_empty());

    let _ = write!(
        j,
        "\"consumed\":{},\"bell\":{},\"asciiMode\":{},\"schemaId\":{},\"page\":{},\"totalPages\":{},\"vertical\":{},",
        consumed,
        bell,
        engine.ascii_mode,
        engine.config.schema as u32,
        engine.current_page(),
        engine.total_pages(),
        engine.config.orientation == crate::config::CandidateOrientation::Vertical
    );

    j.push_str("\"schemaName\":");
    push_json_str(&mut j, engine.config.schema.display_name());
    j.push_str(",\"commitText\":");
    push_json_str(&mut j, commit);
    j.push_str(",\"preedit\":");
    push_json_str(&mut j, &engine.preedit());
    j.push_str(",\"segmentedCode\":");
    push_json_str(&mut j, &engine.segmented_code());
    j.push_str(",\"notice\":");
    push_json_str(&mut j, notice);

    let calibrated_keys = engine
        .user
        .touch_profile
        .keys
        .iter()
        .filter(|k| k.samples > 0)
        .count();
    let (valid_mask, priors) = engine.next_key_mask_and_priors();
    let _ = write!(
        j,
        ",\"userPhraseCount\":{},\"calibratedKeyCount\":{},\"validNextMask\":{},\"nextKeyPriors\":[",
        engine.user.phrases.len(),
        calibrated_keys,
        valid_mask
    );
    for (pi, &pv) in priors.iter().enumerate() {
        if pi > 0 {
            j.push(',');
        }
        let _ = write!(j, "{}", pv);
    }
    j.push(']');
    if include_touch_offsets && calibrated_keys > 0 {
        j.push_str(",\"touchOffsets\":[");
        for (ti, tk) in engine.user.touch_profile.keys.iter().enumerate() {
            if ti > 0 {
                j.push(',');
            }
            if tk.samples == 0 || (tk.dx.abs() < 1e-4 && tk.dy.abs() < 1e-4) {
                j.push_str("0,0");
            } else {
                let _ = write!(j, "{:.3},{:.3}", tk.dx, tk.dy);
            }
        }
        j.push(']');
    }
    if engine.is_t9_pinyin_mode() {
        let (t9_mask, t9_priors) = engine.t9_next_key_mask_and_priors();
        let _ = write!(
            j,
            ",\"t9Mode\":true,\"t9ValidNextMask\":{},\"t9NextKeyPriors\":[",
            t9_mask
        );
        for (pi, &pv) in t9_priors.iter().enumerate() {
            if pi > 0 {
                j.push(',');
            }
            let _ = write!(j, "{}", pv);
        }
        j.push(']');
        if include_touch_offsets {
            j.push_str(",\"t9TouchOffsets\":[");
            for (ti, tk) in engine.user.touch_profile.t9_keys.iter().enumerate() {
                if ti > 0 {
                    j.push(',');
                }
                if tk.samples == 0 || (tk.dx.abs() < 1e-4 && tk.dy.abs() < 1e-4) {
                    j.push_str("0,0");
                } else {
                    let _ = write!(j, "{:.3},{:.3}", tk.dx, tk.dy);
                }
            }
            j.push(']');
        }
        j.push_str(",\"t9LockedPrefix\":");
        push_json_str(&mut j, &engine.t9_locked_prefix());
        j.push_str(",\"t9Syllables\":[");
        for (si, syl) in engine.t9_syllable_options().iter().enumerate() {
            if si > 0 {
                j.push(',');
            }
            push_json_str(&mut j, syl);
        }
        j.push(']');
    }
    j.push_str(",\"candidates\":");
    push_candidates_json(&mut j, engine.page_candidates());
    j.push('}');
    j
}

#[no_mangle]
pub extern "C" fn juxu_engine_new() -> *mut JuxuEngineHandle {
    catch_unwind(|| {
        Box::into_raw(Box::new(JuxuEngineHandle {
            inner: JuxuEngine::default(),
        }))
    })
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_free(handle: *mut JuxuEngineHandle) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !handle.is_null() {
            unsafe {
                drop(Box::from_raw(handle));
            }
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_string_free(ptr: *mut c_char) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !ptr.is_null() {
            unsafe {
                drop(CString::from_raw(ptr));
            }
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_load_dict_mmap(
    handle: *mut JuxuEngineHandle,
    schema_id: u32,
    path: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return false;
        };
        let Some(p) = cstr_to_str(path) else {
            return false;
        };
        match DictView::open_mmap(p) {
            Ok(dict) => {
                h.inner.load_dict(InputSchema::from_id(schema_id), dict);
                true
            }
            Err(_) => false,
        }
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_load_aux_mmap(
    handle: *mut JuxuEngineHandle,
    path: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return false;
        };
        let Some(p) = cstr_to_str(path) else {
            return false;
        };
        match AuxView::open_mmap(p) {
            Ok(aux) => {
                h.inner.set_aux(aux);
                true
            }
            Err(_) => false,
        }
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_load_pinyin_dict_mmap(
    handle: *mut JuxuEngineHandle,
    path: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return false;
        };
        let Some(p) = cstr_to_str(path) else {
            return false;
        };
        match PinyinDictView::from_file(p) {
            Ok(dict) => {
                h.inner.load_pinyin_dict(dict);
                true
            }
            Err(_) => false,
        }
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_load_english_dict_mmap(
    handle: *mut JuxuEngineHandle,
    path: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return false;
        };
        let Some(p) = cstr_to_str(path) else {
            return false;
        };
        match EnDictView::open_mmap(p) {
            Ok(dict) => {
                h.inner.load_english_dict(dict);
                true
            }
            Err(_) => false,
        }
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_load_emoji_table_mmap(
    handle: *mut JuxuEngineHandle,
    path: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return false;
        };
        let Some(p) = cstr_to_str(path) else {
            return false;
        };
        match crate::emoji::EmojiTable::open_mmap(p) {
            Ok(table) => {
                h.inner.load_emoji_table(table);
                true
            }
            Err(_) => false,
        }
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_retain_only_schema_dict(
    handle: *mut JuxuEngineHandle,
    schema_id: u32,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(h) = unsafe { handle.as_mut() } {
            h.inner
                .retain_only_schema_dict(InputSchema::from_id(schema_id));
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_has_active_dict(handle: *const JuxuEngineHandle) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_ref() }) else {
            return false;
        };
        if h.inner.config.schema.is_pinyin() {
            h.inner.pinyin_dict.is_some()
        } else {
            h.inner.active_dict().is_some()
        }
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_load_config(
    handle: *mut JuxuEngineHandle,
    path: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return false;
        };
        let Some(p) = cstr_to_str(path) else {
            return false;
        };
        match JuxuConfig::load_file(p) {
            Ok(cfg) => {
                h.inner.set_config(cfg);
                true
            }
            Err(_) => false,
        }
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_save_config(
    handle: *const JuxuEngineHandle,
    path: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_ref() }) else {
            return false;
        };
        let Some(p) = cstr_to_str(path) else {
            return false;
        };
        h.inner.config.save_file(p).is_ok()
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_set_config_toml(
    handle: *mut JuxuEngineHandle,
    toml_str: *const c_char,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return;
        };
        if let Some(s) = cstr_to_str(toml_str) {
            h.inner.set_config(JuxuConfig::from_toml_str(s));
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_get_config_toml(handle: *const JuxuEngineHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_ref() }) else {
            return into_c_string(String::new());
        };
        into_c_string(h.inner.config.to_toml_string())
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_set_config_json(
    handle: *mut JuxuEngineHandle,
    json_str: *const c_char,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return;
        };
        if let Some(s) = cstr_to_str(json_str) {
            h.inner.set_config(JuxuConfig::from_json_str(s));
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_get_config_json(handle: *const JuxuEngineHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_ref() }) else {
            return into_c_string("{}".to_string());
        };
        into_c_string(h.inner.config.to_json_string())
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_config_toml_to_json(toml_str: *const c_char) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let cfg = cstr_to_str(toml_str).map_or_else(JuxuConfig::default, JuxuConfig::from_toml_str);
        into_c_string(cfg.to_json_string())
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_config_json_to_toml(json_str: *const c_char) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let cfg = cstr_to_str(json_str).map_or_else(JuxuConfig::default, JuxuConfig::from_json_str);
        into_c_string(cfg.to_toml_string())
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_load_user_data(
    handle: *mut JuxuEngineHandle,
    path: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return false;
        };
        let Some(p) = cstr_to_str(path) else {
            return false;
        };
        match UserStore::load_file(p) {
            Ok(mut store) => {
                if let Some(d) = h.inner.active_dict() {
                    store.reindex_phrases(d);
                }
                if let Some(ref pd) = h.inner.pinyin_dict {
                    store.reindex_pinyin_phrases(pd);
                }
                h.inner.user = store;
                true
            }
            Err(_) => false,
        }
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_import_user_data_tsv(
    handle: *mut JuxuEngineHandle,
    tsv_str: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return false;
        };
        let Some(src) = cstr_to_str(tsv_str) else {
            return false;
        };
        let mut store = UserStore::from_tsv_str(src);
        if let Some(d) = h.inner.active_dict() {
            store.reindex_phrases(d);
        }
        if let Some(ref pd) = h.inner.pinyin_dict {
            store.reindex_pinyin_phrases(pd);
        }
        h.inner.user = store;
        true
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_user_data_seq(handle: *const JuxuEngineHandle) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        unsafe { handle.as_ref() }
            .map(|h| h.inner.user.seq)
            .unwrap_or(0)
    }))
    .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn juxu_engine_export_user_data_tsv(
    handle: *mut JuxuEngineHandle,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string(String::new());
        };
        let tsv = h.inner.user.to_tsv_string();
        h.inner.user.dirty = false;
        into_c_string(tsv)
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_is_user_dirty(handle: *const JuxuEngineHandle) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        unsafe { handle.as_ref() }
            .map(|h| h.inner.user.dirty)
            .unwrap_or(false)
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_get_schema(handle: *const JuxuEngineHandle) -> u32 {
    catch_unwind(AssertUnwindSafe(|| {
        unsafe { handle.as_ref() }
            .map(|h| h.inner.config.schema as u32)
            .unwrap_or(86)
    }))
    .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn juxu_engine_toggle_ascii_json(handle: *mut JuxuEngineHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string("{}".to_string());
        };
        let out = h.inner.toggle_ascii_mode();
        into_c_string(serialize_action_and_state(&h.inner, Some(&out)))
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_set_key_layout(
    handle: *mut JuxuEngineHandle,
    centers_xy_52: *const f32,
    origin_x: f32,
    origin_y: f32,
    pitch_x: f32,
    pitch_y: f32,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return;
        };
        h.inner.key_layout.origin_x = origin_x;
        h.inner.key_layout.origin_y = origin_y;
        h.inner.key_layout.pitch_x = pitch_x;
        h.inner.key_layout.pitch_y = pitch_y;
        if !centers_xy_52.is_null() {
            let slice = unsafe { core::slice::from_raw_parts(centers_xy_52, 52) };
            for i in 0..26 {
                h.inner.key_layout.centers[i] = (slice[i * 2], slice[i * 2 + 1]);
            }
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_set_t9_layout(
    handle: *mut JuxuEngineHandle,
    centers_xy_16: *const f32,
    origin_x: f32,
    origin_y: f32,
    pitch_x: f32,
    pitch_y: f32,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return;
        };
        h.inner.key_layout.t9_origin_x = origin_x;
        h.inner.key_layout.t9_origin_y = origin_y;
        h.inner.key_layout.t9_pitch_x = pitch_x;
        h.inner.key_layout.t9_pitch_y = pitch_y;
        if !centers_xy_16.is_null() {
            let slice = unsafe { core::slice::from_raw_parts(centers_xy_16, 16) };
            for i in 0..8 {
                h.inner.key_layout.t9_centers[i] = (slice[i * 2], slice[i * 2 + 1]);
            }
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_reset_touch_profile(handle: *mut JuxuEngineHandle) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(h) = unsafe { handle.as_mut() } {
            h.inner.user.touch_profile.reset();
            h.inner.user.dirty = true;
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_set_touch_profile(
    handle: *mut JuxuEngineHandle,
    offsets_xy_52: *const f32,
    counts_26: *const u32,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return;
        };
        if offsets_xy_52.is_null() || counts_26.is_null() {
            return;
        }
        let offsets = unsafe { core::slice::from_raw_parts(offsets_xy_52, 52) };
        let counts = unsafe { core::slice::from_raw_parts(counts_26, 26) };
        if offsets.iter().any(|offset| !offset.is_finite()) {
            return;
        }
        h.inner.user.touch_profile.keys = std::array::from_fn(|i| {
            let (cdx, cdy) =
                clamp_key_touch_offset(b'a' + i as u8, offsets[i * 2], offsets[i * 2 + 1]);
            TouchKeyOffset {
                dx: cdx,
                dy: cdy,
                samples: counts[i],
            }
        });
        h.inner.user.dirty = true;
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_add_user_phrase(
    handle: *mut JuxuEngineHandle,
    phrase: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string(String::new());
        };
        let Some(text) = cstr_to_str(phrase) else {
            return into_c_string(String::new());
        };
        into_c_string(h.inner.add_user_phrase(text))
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_remove_user_phrase(
    handle: *mut JuxuEngineHandle,
    phrase: *const c_char,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return false;
        };
        let Some(text) = cstr_to_str(phrase) else {
            return false;
        };
        h.inner.user.remove_phrase(text)
    }))
    .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn juxu_engine_list_user_phrases_json(
    handle: *const JuxuEngineHandle,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_ref() }) else {
            return into_c_string("[]".to_string());
        };
        let dict = h.inner.active_dict();
        let mut items: Vec<_> = h.inner.user.phrases.values().collect();
        items.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.text.cmp(&b.text)));
        let mut j = String::from("[");
        for (i, p) in items.into_iter().take(200).enumerate() {
            if i > 0 {
                j.push(',');
            }
            let code_str = dict
                .and_then(|d| d.encode_phrase(&p.text))
                .map(|c| c.as_string())
                .unwrap_or_default();
            j.push_str("{\"text\":");
            push_json_str(&mut j, &p.text);
            j.push_str(",\"code\":");
            push_json_str(&mut j, &code_str);
            j.push_str(&format!(
                ",\"count\":{},\"auto\":{}}}",
                p.count, p.auto_composed
            ));
        }
        j.push(']');
        into_c_string(j)
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_clear(handle: *mut JuxuEngineHandle) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(h) = unsafe { handle.as_mut() } {
            h.inner.clear_composition();
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_trim_memory(handle: *mut JuxuEngineHandle) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(h) = unsafe { handle.as_mut() } {
            h.inner.trim_idle_memory();
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_abandon(handle: *mut JuxuEngineHandle, timestamp_ms: u64) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(h) = unsafe { handle.as_mut() } {
            h.inner.abandon_composition(timestamp_ms);
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_notify_post_commit_backspace(
    handle: *mut JuxuEngineHandle,
    timestamp_ms: u64,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(h) = unsafe { handle.as_mut() } {
            h.inner.notify_post_commit_backspace(timestamp_ms);
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_commit_pending_json(
    handle: *mut JuxuEngineHandle,
    timestamp_ms: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string("{}".to_string());
        };
        let out = h.inner.commit_pending(timestamp_ms);
        into_c_string(serialize_action_and_state(&h.inner, Some(&out)))
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_reset_quote_state(handle: *mut JuxuEngineHandle) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(h) = unsafe { handle.as_mut() } {
            h.inner.reset_quote_state();
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_handle_key_json(
    handle: *mut JuxuEngineHandle,
    keycode: u16,
    unicode_scalar: u32,
    has_cmd_or_ctrl: bool,
    timestamp_ms: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string("{}".to_string());
        };
        let ch = if unicode_scalar == 0 {
            None
        } else {
            char::from_u32(unicode_scalar)
        };
        let out = h
            .inner
            .handle_key_event(keycode, ch, has_cmd_or_ctrl, timestamp_ms);
        into_c_string(serialize_action_and_state(&h.inner, Some(&out)))
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_push_touch_json(
    handle: *mut JuxuEngineHandle,
    reported_ascii: u8,
    x: f32,
    y: f32,
    is_normalized: bool,
    timestamp_ms: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string("{}".to_string());
        };
        let out = h
            .inner
            .push_touch_letter(reported_ascii, x, y, is_normalized, timestamp_ms);
        into_c_string(serialize_action_and_state(&h.inner, Some(&out)))
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_push_t9_touch_json(
    handle: *mut JuxuEngineHandle,
    reported_digit: u8,
    x: f32,
    y: f32,
    is_normalized: bool,
    timestamp_ms: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string("{}".to_string());
        };
        let out = h
            .inner
            .push_touch_t9(reported_digit, x, y, is_normalized, timestamp_ms);
        into_c_string(serialize_action_and_state(&h.inner, Some(&out)))
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_lock_t9_syllable_json(
    handle: *mut JuxuEngineHandle,
    syllable_utf8: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string("{}".to_string());
        };
        let Some(syl) = cstr_to_str(syllable_utf8) else {
            return into_c_string(serialize_action_and_state(&h.inner, None));
        };
        let out = h.inner.lock_t9_syllable(syl);
        into_c_string(serialize_action_and_state(&h.inner, Some(&out)))
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_select_candidate_json(
    handle: *mut JuxuEngineHandle,
    page_rel_index: u32,
    timestamp_ms: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string("{}".to_string());
        };
        let out = h
            .inner
            .select_page_candidate(page_rel_index as usize, timestamp_ms);
        into_c_string(serialize_action_and_state(&h.inner, out.as_ref()))
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_page_turn_json(
    handle: *mut JuxuEngineHandle,
    delta: i32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string("{}".to_string());
        };
        let changed = if delta < 0 {
            h.inner.page_prev()
        } else {
            h.inner.page_next()
        };
        let out = EngineOutput {
            consumed: true,
            preedit: h.inner.preedit(),
            bell: !changed,
            ..Default::default()
        };
        into_c_string(serialize_action_and_state(&h.inner, Some(&out)))
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_select_abs_candidate_json(
    handle: *mut JuxuEngineHandle,
    abs_index: u32,
    timestamp_ms: u64,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string("{}".to_string());
        };
        let out = h
            .inner
            .select_abs_candidate(abs_index as usize, timestamp_ms);
        into_c_string(serialize_action_and_state(&h.inner, out.as_ref()))
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_all_candidates_json(
    handle: *const JuxuEngineHandle,
    max_count: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_ref() }) else {
            return into_c_string("[]".to_string());
        };
        let limit = (max_count as usize).clamp(1, 500);
        let all = h.inner.all_candidates();
        let end = limit.min(all.len());
        let mut j = String::with_capacity(end * 80 + 2);
        push_candidates_json(&mut j, &all[..end]);
        into_c_string(j)
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_touch_profile_json(
    handle: *const JuxuEngineHandle,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_ref() }) else {
            return into_c_string("[]".to_string());
        };
        let mut j = String::from("[");
        for (i, t) in h.inner.user.touch_profile.keys.iter().enumerate() {
            if i > 0 {
                j.push(',');
            }
            let ch = (b'a' + i as u8) as char;
            j.push_str(&format!(
                "{{\"key\":\"{}\",\"dx\":{:.4},\"dy\":{:.4},\"samples\":{}}}",
                ch, t.dx, t.dy, t.samples
            ));
        }
        j.push(']');
        into_c_string(j)
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_state_json(handle: *const JuxuEngineHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_ref() }) else {
            return into_c_string("{}".to_string());
        };
        into_c_string(serialize_action_and_state(&h.inner, None))
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_set_context(
    handle: *mut JuxuEngineHandle,
    context_utf8: *const c_char,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return;
        };
        if let Some(ctx) = cstr_to_str(context_utf8) {
            h.inner.set_recent_context(ctx);
        } else {
            h.inner.clear_context();
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_accept_prediction(
    handle: *mut JuxuEngineHandle,
    text_utf8: *const c_char,
    timestamp_ms: u64,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return;
        };
        if let Some(txt) = cstr_to_str(text_utf8) {
            h.inner.accept_prediction(txt, timestamp_ms);
        }
    }));
}

#[no_mangle]
pub extern "C" fn juxu_engine_predict_next_json(
    handle: *const JuxuEngineHandle,
    max_count: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_ref() }) else {
            return into_c_string("[]".to_string());
        };
        let preds = h.inner.predict_next_words(max_count as usize);
        let mut j = String::with_capacity(128);
        j.push('[');
        for (idx, w) in preds.iter().enumerate() {
            if idx > 0 {
                j.push(',');
            }
            push_json_str(&mut j, w);
        }
        j.push(']');
        into_c_string(j)
    }))
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn juxu_engine_drain_eval_log_jsonl(
    handle: *mut JuxuEngineHandle,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(h) = (unsafe { handle.as_mut() }) else {
            return into_c_string(String::new());
        };
        into_c_string(h.inner.drain_eval_log_jsonl())
    }))
    .unwrap_or(ptr::null_mut())
}
