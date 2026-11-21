use crate::config::{WubiConfig, WubiSchema};
use crate::engine::{EngineOutput, WubiEngine};
use crate::format::{AuxView, DictView};
use crate::pinyin_dict::PinyinDictView;
use crate::user_data::{TouchKeyOffset, UserStore, MAX_TOUCH_OFFSET};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

pub struct WubiEngineHandle {
    pub inner: WubiEngine,
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
    CString::new(s.replace('\0', ""))
        .unwrap_or_else(|_| CString::new("{}").unwrap())
        .into_raw()
}

fn json_escape(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn serialize_action_and_state(engine: &WubiEngine, act: Option<&EngineOutput>) -> String {
    let mut j = String::with_capacity(512);
    j.push('{');
    let consumed = act.map(|a| a.consumed).unwrap_or(false);
    let bell = act.map(|a| a.bell).unwrap_or(false);
    let commit = act.map(|a| a.commit_text.as_str()).unwrap_or("");
    let notice = act.map(|a| a.notice.as_str()).unwrap_or("");

    j.push_str(&format!(
        "\"consumed\":{},\"bell\":{},\"asciiMode\":{},\"schemaId\":{},\"page\":{},\"totalPages\":{},\"vertical\":{},",
        consumed,
        bell,
        engine.ascii_mode,
        engine.config.schema as u32,
        engine.current_page(),
        engine.total_pages(),
        engine.config.orientation == crate::config::CandidateOrientation::Vertical
    ));

    j.push_str("\"schemaName\":");
    json_escape(engine.config.schema.display_name(), &mut j);
    j.push_str(",\"commitText\":");
    json_escape(commit, &mut j);
    j.push_str(",\"preedit\":");
    json_escape(&engine.preedit(), &mut j);
    j.push_str(",\"segmentedCode\":");
    json_escape(&engine.segmented_code(), &mut j);
    j.push_str(",\"notice\":");
    json_escape(notice, &mut j);

    let calibrated_keys = engine
        .user
        .touch_profile
        .keys
        .iter()
        .filter(|k| k.samples > 0)
        .count();
    let (valid_mask, priors) = engine.next_key_mask_and_priors();
    j.push_str(&format!(
        ",\"userPhraseCount\":{},\"calibratedKeyCount\":{},\"validNextMask\":{},\"nextKeyPriors\":[",
        engine.user.phrases.len(),
        calibrated_keys,
        valid_mask
    ));
    for (pi, &pv) in priors.iter().enumerate() {
        if pi > 0 {
            j.push(',');
        }
        j.push_str(&pv.to_string());
    }
    j.push_str("],\"touchOffsets\":[");
    for (ti, tk) in engine.user.touch_profile.keys.iter().enumerate() {
        if ti > 0 {
            j.push(',');
        }
        if tk.samples == 0 || (tk.dx.abs() < 1e-4 && tk.dy.abs() < 1e-4) {
            j.push_str("0,0");
        } else {
            j.push_str(&format!("{:.3},{:.3}", tk.dx, tk.dy));
        }
    }
    j.push_str("],\"candidates\":[");

    for (idx, c) in engine.page_candidates().iter().enumerate() {
        if idx > 0 {
            j.push(',');
        }
        j.push_str("{\"index\":");
        j.push_str(&(idx + 1).to_string());
        j.push_str(",\"text\":");
        json_escape(&c.text, &mut j);
        j.push_str(",\"comment\":");
        json_escape(&c.comment, &mut j);
        j.push_str(&format!(
            ",\"badge\":{},\"confidence\":{:.3},\"isExact\":{}}}",
            c.badge as u8, c.confidence, c.is_exact
        ));
    }
    j.push_str("]}");
    j
}

#[no_mangle]
pub extern "C" fn wubi_engine_new() -> *mut WubiEngineHandle {
    Box::into_raw(Box::new(WubiEngineHandle {
        inner: WubiEngine::default(),
    }))
}

#[no_mangle]
pub extern "C" fn wubi_engine_free(handle: *mut WubiEngineHandle) {
    if !handle.is_null() {
        unsafe {
            drop(Box::from_raw(handle));
        }
    }
}

#[no_mangle]
pub extern "C" fn wubi_string_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            drop(CString::from_raw(ptr));
        }
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_load_dict_mmap(
    handle: *mut WubiEngineHandle,
    schema_id: u32,
    path: *const c_char,
) -> bool {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    let Some(p) = cstr_to_str(path) else {
        return false;
    };
    match DictView::open_mmap(p) {
        Ok(dict) => {
            h.inner.load_dict(WubiSchema::from_id(schema_id), dict);
            true
        }
        Err(_) => false,
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_load_aux_mmap(
    handle: *mut WubiEngineHandle,
    path: *const c_char,
) -> bool {
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
}

#[no_mangle]
pub extern "C" fn wubi_engine_load_pinyin_dict_mmap(
    handle: *mut WubiEngineHandle,
    path: *const c_char,
) -> bool {
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
}

#[no_mangle]
pub extern "C" fn wubi_engine_retain_only_schema_dict(
    handle: *mut WubiEngineHandle,
    schema_id: u32,
) {
    if let Some(h) = unsafe { handle.as_mut() } {
        h.inner.retain_only_schema_dict(WubiSchema::from_id(schema_id));
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_load_config(
    handle: *mut WubiEngineHandle,
    path: *const c_char,
) -> bool {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    let Some(p) = cstr_to_str(path) else {
        return false;
    };
    match WubiConfig::load_file(p) {
        Ok(cfg) => {
            h.inner.set_config(cfg);
            true
        }
        Err(_) => false,
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_save_config(
    handle: *const WubiEngineHandle,
    path: *const c_char,
) -> bool {
    let Some(h) = (unsafe { handle.as_ref() }) else {
        return false;
    };
    let Some(p) = cstr_to_str(path) else {
        return false;
    };
    h.inner.config.save_file(p).is_ok()
}

#[no_mangle]
pub extern "C" fn wubi_engine_set_config_toml(
    handle: *mut WubiEngineHandle,
    toml_str: *const c_char,
) {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return;
    };
    if let Some(s) = cstr_to_str(toml_str) {
        h.inner.set_config(WubiConfig::from_toml_str(s));
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_get_config_toml(handle: *const WubiEngineHandle) -> *mut c_char {
    let Some(h) = (unsafe { handle.as_ref() }) else {
        return into_c_string(String::new());
    };
    into_c_string(h.inner.config.to_toml_string())
}

#[no_mangle]
pub extern "C" fn wubi_engine_load_user_data(
    handle: *mut WubiEngineHandle,
    path: *const c_char,
) -> bool {
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
}

#[no_mangle]
pub extern "C" fn wubi_engine_save_user_data(
    handle: *mut WubiEngineHandle,
    path: *const c_char,
) -> bool {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    let Some(p) = cstr_to_str(path) else {
        return false;
    };
    if h.inner.user.save_file(p).is_ok() {
        h.inner.user.dirty = false;
        true
    } else {
        false
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_is_user_dirty(handle: *const WubiEngineHandle) -> bool {
    unsafe { handle.as_ref() }
        .map(|h| h.inner.user.dirty)
        .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn wubi_engine_set_schema(handle: *mut WubiEngineHandle, schema_id: u32) {
    if let Some(h) = unsafe { handle.as_mut() } {
        h.inner.set_schema(WubiSchema::from_id(schema_id));
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_get_schema(handle: *const WubiEngineHandle) -> u32 {
    unsafe { handle.as_ref() }
        .map(|h| h.inner.config.schema as u32)
        .unwrap_or(86)
}

#[no_mangle]
pub extern "C" fn wubi_engine_set_ascii_mode(handle: *mut WubiEngineHandle, ascii_mode: bool) {
    if let Some(h) = unsafe { handle.as_mut() } {
        h.inner.ascii_mode = ascii_mode;
        if ascii_mode {
            h.inner.clear_composition();
        }
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_get_ascii_mode(handle: *const WubiEngineHandle) -> bool {
    unsafe { handle.as_ref() }
        .map(|h| h.inner.ascii_mode)
        .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn wubi_engine_toggle_ascii_json(handle: *mut WubiEngineHandle) -> *mut c_char {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return into_c_string("{}".to_string());
    };
    let out = h.inner.toggle_ascii_mode();
    into_c_string(serialize_action_and_state(&h.inner, Some(&out)))
}

#[no_mangle]
pub extern "C" fn wubi_engine_set_key_layout(
    handle: *mut WubiEngineHandle,
    centers_xy_52: *const f32,
    origin_x: f32,
    origin_y: f32,
    pitch_x: f32,
    pitch_y: f32,
) {
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
}

#[no_mangle]
pub extern "C" fn wubi_engine_reset_touch_profile(handle: *mut WubiEngineHandle) {
    if let Some(h) = unsafe { handle.as_mut() } {
        h.inner.user.touch_profile.reset();
        h.inner.user.dirty = true;
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_set_touch_profile(
    handle: *mut WubiEngineHandle,
    offsets_xy_52: *const f32,
    counts_26: *const u32,
) {
    let Some(h) = (unsafe { handle.as_mut() }) else { return };
    if offsets_xy_52.is_null() || counts_26.is_null() {
        return;
    }
    let offsets = unsafe { core::slice::from_raw_parts(offsets_xy_52, 52) };
    let counts = unsafe { core::slice::from_raw_parts(counts_26, 26) };
    if offsets.iter().any(|offset| !offset.is_finite()) {
        return;
    }
    h.inner.user.touch_profile.keys = std::array::from_fn(|i| TouchKeyOffset {
        dx: offsets[i * 2].clamp(-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET),
        dy: offsets[i * 2 + 1].clamp(-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET),
        samples: counts[i],
    });
    h.inner.user.dirty = true;
}

#[no_mangle]
pub extern "C" fn wubi_engine_add_user_phrase(
    handle: *mut WubiEngineHandle,
    phrase: *const c_char,
) -> *mut c_char {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return into_c_string(String::new());
    };
    let Some(text) = cstr_to_str(phrase) else {
        return into_c_string(String::new());
    };
    let slot = h.inner.config.schema.slot_index();
    let encoded = h.inner.dicts[slot]
        .as_ref()
        .and_then(|d| d.encode_phrase(text));
    if let Some(code) = encoded {
        h.inner
            .user
            .add_phrase(text, false, h.inner.dicts[slot].as_ref());
        into_c_string(code.as_string())
    } else {
        into_c_string(String::new())
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_remove_user_phrase(
    handle: *mut WubiEngineHandle,
    phrase: *const c_char,
) -> bool {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    let Some(text) = cstr_to_str(phrase) else {
        return false;
    };
    h.inner.user.remove_phrase(text)
}

#[no_mangle]
pub extern "C" fn wubi_engine_list_user_phrases_json(
    handle: *const WubiEngineHandle,
) -> *mut c_char {
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
        json_escape(&p.text, &mut j);
        j.push_str(",\"code\":");
        json_escape(&code_str, &mut j);
        j.push_str(&format!(
            ",\"count\":{},\"auto\":{}}}",
            p.count, p.auto_composed
        ));
    }
    j.push(']');
    into_c_string(j)
}

#[no_mangle]
pub extern "C" fn wubi_engine_clear(handle: *mut WubiEngineHandle) {
    if let Some(h) = unsafe { handle.as_mut() } {
        h.inner.clear_composition();
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_is_composing(handle: *const WubiEngineHandle) -> bool {
    unsafe { handle.as_ref() }
        .map(|h| h.inner.is_composing())
        .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn wubi_engine_handle_key_json(
    handle: *mut WubiEngineHandle,
    keycode: u16,
    unicode_scalar: u32,
    has_cmd_or_ctrl: bool,
    timestamp_ms: u64,
) -> *mut c_char {
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
}

#[no_mangle]
pub extern "C" fn wubi_engine_push_touch_json(
    handle: *mut WubiEngineHandle,
    reported_ascii: u8,
    x: f32,
    y: f32,
    is_normalized: bool,
    timestamp_ms: u64,
) -> *mut c_char {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return into_c_string("{}".to_string());
    };
    let out = h
        .inner
        .push_touch_letter(reported_ascii, x, y, is_normalized, timestamp_ms);
    into_c_string(serialize_action_and_state(&h.inner, Some(&out)))
}

#[no_mangle]
pub extern "C" fn wubi_engine_select_candidate_json(
    handle: *mut WubiEngineHandle,
    page_rel_index: u32,
    timestamp_ms: u64,
) -> *mut c_char {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return into_c_string("{}".to_string());
    };
    let out = h
        .inner
        .select_page_candidate(page_rel_index as usize, timestamp_ms);
    into_c_string(serialize_action_and_state(&h.inner, out.as_ref()))
}

#[no_mangle]
pub extern "C" fn wubi_engine_page_turn_json(
    handle: *mut WubiEngineHandle,
    delta: i32,
) -> *mut c_char {
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
        preedit: h.inner.raw_code().to_string(),
        bell: !changed,
        ..Default::default()
    };
    into_c_string(serialize_action_and_state(&h.inner, Some(&out)))
}

#[no_mangle]
pub extern "C" fn wubi_engine_select_abs_candidate_json(
    handle: *mut WubiEngineHandle,
    abs_index: u32,
    timestamp_ms: u64,
) -> *mut c_char {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return into_c_string("{}".to_string());
    };
    let out = h
        .inner
        .select_abs_candidate(abs_index as usize, timestamp_ms);
    into_c_string(serialize_action_and_state(&h.inner, out.as_ref()))
}

#[no_mangle]
pub extern "C" fn wubi_engine_all_candidates_json(
    handle: *const WubiEngineHandle,
    max_count: u32,
) -> *mut c_char {
    let Some(h) = (unsafe { handle.as_ref() }) else {
        return into_c_string("[]".to_string());
    };
    let limit = (max_count as usize).clamp(1, 500);
    let mut j = String::from("[");
    for (idx, c) in h.inner.all_candidates().iter().take(limit).enumerate() {
        if idx > 0 {
            j.push(',');
        }
        j.push_str("{\"index\":");
        j.push_str(&(idx + 1).to_string());
        j.push_str(",\"text\":");
        json_escape(&c.text, &mut j);
        j.push_str(",\"comment\":");
        json_escape(&c.comment, &mut j);
        j.push_str(&format!(
            ",\"badge\":{},\"confidence\":{:.3},\"isExact\":{}}}",
            c.badge as u8, c.confidence, c.is_exact
        ));
    }
    j.push(']');
    into_c_string(j)
}

#[no_mangle]
pub extern "C" fn wubi_engine_touch_profile_json(
    handle: *const WubiEngineHandle,
) -> *mut c_char {
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
}

#[no_mangle]
pub extern "C" fn wubi_engine_state_json(handle: *const WubiEngineHandle) -> *mut c_char {
    let Some(h) = (unsafe { handle.as_ref() }) else {
        return into_c_string("{}".to_string());
    };
    into_c_string(serialize_action_and_state(&h.inner, None))
}

#[no_mangle]
pub extern "C" fn wubi_engine_set_context(
    handle: *mut WubiEngineHandle,
    context_utf8: *const c_char,
) {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return;
    };
    if let Some(ctx) = cstr_to_str(context_utf8) {
        h.inner.set_recent_context(ctx);
    } else {
        h.inner.clear_context();
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_accept_prediction(
    handle: *mut WubiEngineHandle,
    text_utf8: *const c_char,
    timestamp_ms: u64,
) {
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return;
    };
    if let Some(txt) = cstr_to_str(text_utf8) {
        h.inner.accept_prediction(txt, timestamp_ms);
    }
}

#[no_mangle]
pub extern "C" fn wubi_engine_predict_next_json(
    handle: *const WubiEngineHandle,
    max_count: u32,
) -> *mut c_char {
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
        json_escape(w, &mut j);
    }
    j.push(']');
    into_c_string(j)
}
