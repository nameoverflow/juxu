use crate::code::{is_cjk_ideograph, PackedCode};
use crate::format::DictView;
use crate::personalization::PersonalContinuations;
use crate::pinyin_dict::{pack_init_key, PinyinDictView, SylConstraint};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static USER_DATA_WRITE_ID: AtomicU64 = AtomicU64::new(0);

const MAX_FREQ_MAP_LEN: usize = 8192;
const PRUNED_FREQ_MAP_LEN: usize = 6144;

fn prune_freq_map_keeping<K: Clone + Eq + std::hash::Hash + Ord>(
    map: &mut HashMap<K, u32>,
    keep: &K,
) {
    if map.len() <= MAX_FREQ_MAP_LEN {
        return;
    }
    let remove = map.len() - PRUNED_FREQ_MAP_LEN;
    let mut candidates: Vec<(K, u32)> = map
        .iter()
        .filter(|(k, _)| *k != keep)
        .map(|(k, &v)| (k.clone(), v))
        .collect();
    let remove = remove.min(candidates.len());
    candidates.select_nth_unstable_by(remove, |a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    for (k, _) in candidates.into_iter().take(remove) {
        map.remove(&k);
    }
}

pub const MAX_TOUCH_OFFSET: f32 = 0.20; // Clamp interior adaptive hitbox shift to ±0.20 key-pitch
pub const MAX_OUTER_EDGE_TOUCH_OFFSET: f32 = 0.32; // Unbounded outer keyboard margins ('a'/'q' left, 'l'/'p' right)

/// Asymmetric 26-key hitbox offset clamp: keys at an unbounded horizontal edge (`a`/`q` leftward,
/// `l`/`p` rightward) allow up to `0.32` pitch shift into the margin without encroaching on neighbors.
#[inline]
pub fn clamp_key_touch_offset(key_byte: u8, dx: f32, dy: f32) -> (f32, f32) {
    let (min_x, max_x) = match key_byte.to_ascii_lowercase() {
        b'a' | b'q' => (-MAX_OUTER_EDGE_TOUCH_OFFSET, MAX_TOUCH_OFFSET),
        b'l' | b'p' => (-MAX_TOUCH_OFFSET, MAX_OUTER_EDGE_TOUCH_OFFSET),
        _ => (-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET),
    };
    (
        dx.clamp(min_x, max_x),
        dy.clamp(-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET),
    )
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TouchKeyOffset {
    pub dx: f32,
    pub dy: f32,
    pub samples: u32,
}

#[derive(Clone, Debug)]
pub struct TouchProfile {
    pub keys: [TouchKeyOffset; 26],
    /// Independent adaptive touch offsets for 9-key T9 Pinyin physical keys `'2'..='9'`.
    pub t9_keys: [TouchKeyOffset; 8],
}

impl Default for TouchProfile {
    fn default() -> Self {
        Self {
            keys: [TouchKeyOffset::default(); 26],
            t9_keys: [TouchKeyOffset::default(); 8],
        }
    }
}

impl TouchProfile {
    /// Update personal touch center offset for `key` ('a'..='z') from observed residual
    /// `(obs_norm_x - canonical_cx, obs_norm_y - canonical_cy)`.
    pub fn observe(&mut self, key_byte: u8, residual_x: f32, residual_y: f32) {
        if !key_byte.is_ascii_lowercase() {
            return;
        }
        // Ignore wild outliers (> 0.85 key-widths away) so accidental fat-finger taps don't skew calibration
        if residual_x.abs() > 0.85 || residual_y.abs() > 0.85 {
            return;
        }
        let idx = (key_byte - b'a') as usize;
        let entry = &mut self.keys[idx];
        let alpha = if entry.samples < 10 { 0.14 } else { 0.06 };
        let raw_dx = (1.0 - alpha) * entry.dx + alpha * residual_x;
        let raw_dy = (1.0 - alpha) * entry.dy + alpha * residual_y;
        let (cdx, cdy) = clamp_key_touch_offset(key_byte, raw_dx, raw_dy);
        entry.dx = cdx;
        entry.dy = cdy;
        entry.samples = entry.samples.saturating_add(1);
    }

    /// Update personal touch center offset for T9 physical key `'2'..='9'`.
    pub fn observe_t9(&mut self, digit_byte: u8, residual_x: f32, residual_y: f32) {
        if !(b'2'..=b'9').contains(&digit_byte) {
            return;
        }
        if residual_x.abs() > 0.70 || residual_y.abs() > 0.70 {
            return;
        }
        let idx = (digit_byte - b'2') as usize;
        let entry = &mut self.t9_keys[idx];
        let alpha = if entry.samples < 10 { 0.14 } else { 0.06 };
        entry.dx = ((1.0 - alpha) * entry.dx + alpha * residual_x)
            .clamp(-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET);
        entry.dy = ((1.0 - alpha) * entry.dy + alpha * residual_y)
            .clamp(-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET);
        entry.samples = entry.samples.saturating_add(1);
    }

    pub fn reset(&mut self) {
        self.keys = [TouchKeyOffset::default(); 26];
        self.t9_keys = [TouchKeyOffset::default(); 8];
    }

    #[inline]
    pub fn offset_for(&self, key_byte: u8) -> (f32, f32) {
        if key_byte.is_ascii_lowercase() {
            let e = self.keys[(key_byte - b'a') as usize];
            (e.dx, e.dy)
        } else {
            (0.0, 0.0)
        }
    }

    #[inline]
    pub fn offset_for_t9(&self, digit_byte: u8) -> (f32, f32) {
        if (b'2'..=b'9').contains(&digit_byte) {
            let e = self.t9_keys[(digit_byte - b'2') as usize];
            (e.dx, e.dy)
        } else {
            (0.0, 0.0)
        }
    }
}

#[derive(Clone, Debug)]
pub struct UserPhrase {
    pub text: String,
    pub count: u32,
    pub last_seq: u64,
    pub auto_composed: bool,
}

#[derive(Clone, Default)]
pub(crate) struct CommitUndoSnapshot {
    pub ts_ms: u64,
    pub prev_seq: u64,
    pub prev_last_committed: String,
    pub prev_recent_context: Vec<char>,
    pub unigram_prev: Option<(String, Option<u32>)>,
    pub word_bigram_prev: Option<((String, String), Option<u32>)>,
    pub char_bigram_prev: Option<((char, char), Option<u32>)>,
    pub phrase_prev: Option<(String, Option<UserPhrase>)>,
    pub pending_prev: Vec<(String, Option<u8>)>,
    pub enwords_prev: Vec<(String, Option<(String, u32, u64)>)>,
    pub prev_recent_singles: Vec<(char, u64, u64, bool, bool, bool)>,
    pub cont_undo: Option<crate::personalization::ContinuationUndo>,
}

pub struct UserStore {
    pub seq: u64,
    pub phrases: HashMap<String, UserPhrase>,
    /// Active schema code -> list of `text`
    code_index: HashMap<PackedCode, Vec<String>>,
    /// Full Pinyin index: `(syl_count, init_key)` -> list of `(syl_ids, text)`
    pinyin_index: HashMap<(u8, u32), Vec<(Vec<u16>, String)>>,
    pub unigram_counts: HashMap<String, u32>,
    pub word_bigrams: HashMap<(String, String), u32>,
    pub char_bigrams: HashMap<(char, char), u32>,
    /// Personal learned English / OOV Latin words: `lower_key -> (preferred_display, count, last_seq)`
    pub english_words: HashMap<String, (String, u32, u64)>,
    /// Pending all-lowercase Return-committed OOV tokens waiting for a 2nd confirmation
    pending_enwords: HashMap<String, u8>,
    pub(crate) continuations: PersonalContinuations,
    pub touch_profile: TouchProfile,
    /// Rolling buffer of recent consecutive CJK characters
    /// `(ch, timestamp_ms, commit_id, is_commit_start, is_commit_end, passive_stream)`
    /// across both single-char and multi-char commits so 2..=4 char phrases compose cleanly without mid-word slicing.
    recent_singles: Vec<(char, u64, u64, bool, bool, bool)>,
    /// Candidate consecutive n-grams waiting to reach `auto_phrase_threshold`
    pending_ngrams: HashMap<String, u8>,
    pub(crate) last_undo: Option<CommitUndoSnapshot>,
    pub dirty: bool,
}

impl Default for UserStore {
    fn default() -> Self {
        Self {
            seq: 1,
            phrases: HashMap::new(),
            code_index: HashMap::new(),
            pinyin_index: HashMap::new(),
            unigram_counts: HashMap::new(),
            word_bigrams: HashMap::new(),
            char_bigrams: HashMap::new(),
            english_words: HashMap::new(),
            pending_enwords: HashMap::new(),
            continuations: PersonalContinuations::default(),
            touch_profile: TouchProfile::default(),
            recent_singles: Vec::new(),
            pending_ngrams: HashMap::new(),
            last_undo: None,
            dirty: false,
        }
    }
}

impl UserStore {
    /// Rebuild active Wubi phrase codes for all user phrases using `dict`'s character stems.
    /// This makes user phrases & auto-composed phrases work seamlessly across 86 / 98 / 新世纪!
    pub fn reindex_phrases(&mut self, dict: &DictView) {
        self.code_index.clear();
        for text in self.phrases.keys() {
            if let Some(code) = dict.encode_phrase(text) {
                self.code_index.entry(code).or_default().push(text.clone());
            }
        }
        for bucket in self.code_index.values_mut() {
            bucket.sort_by(|a, b| {
                let pa = &self.phrases[a];
                let pb = &self.phrases[b];
                pb.count
                    .cmp(&pa.count)
                    .then_with(|| pb.last_seq.cmp(&pa.last_seq))
            });
        }
    }

    pub fn valid_next_letters_mask(&self, prefix_bytes: &[u8]) -> u32 {
        if prefix_bytes.is_empty() || prefix_bytes.len() >= 5 || self.code_index.is_empty() {
            return 0;
        }
        let Some(pref) = PackedCode::from_bytes(prefix_bytes) else {
            return 0;
        };
        let (lo, hi) = pref.prefix_range();
        let pos = prefix_bytes.len();
        let mut mask = 0u32;
        for &code in self.code_index.keys() {
            if code >= lo && code <= hi {
                let d = code.digits()[pos];
                if (1..=26).contains(&d) {
                    mask |= 1u32 << (d - 1);
                }
            }
        }
        mask
    }

    /// Record a committed CJK sequence into pure probability-layer `PersonalContinuations`.
    pub fn observe_continuation(
        &mut self,
        prefix: &[char],
        text: &str,
        confirmed: bool,
        first_choice: Option<&str>,
        neural: Option<(crate::neural::NeuralView<'_>, crate::neural::RwkvState)>,
    ) {
        self.continuations
            .observe(prefix, text, confirmed, first_choice, neural);
        self.dirty = true;
    }

    pub fn lookup_exact(&self, code: PackedCode) -> Vec<(&str, u32, u64)> {
        let Some(list) = self.code_index.get(&code) else {
            return Vec::new();
        };
        list.iter()
            .filter_map(|t| {
                let p = self.phrases.get(t)?;
                Some((p.text.as_str(), p.count, p.last_seq))
            })
            .collect()
    }

    pub fn lookup_exact_detailed(&self, code: PackedCode) -> Vec<(&str, u32, u64, bool)> {
        let Some(list) = self.code_index.get(&code) else {
            return Vec::new();
        };
        list.iter()
            .filter_map(|t| {
                let p = self.phrases.get(t)?;
                Some((p.text.as_str(), p.count, p.last_seq, p.auto_composed))
            })
            .collect()
    }

    pub fn lookup_prefix(&self, prefix: PackedCode, limit: usize) -> Vec<(PackedCode, &str, u32)> {
        let (lo, hi) = prefix.prefix_range();
        let single_key_prefix = prefix.len() == 1;
        let mut out = Vec::new();
        for (&code, texts) in &self.code_index {
            if code > lo && code <= hi {
                for t in texts {
                    if let Some(p) = self.phrases.get(t) {
                        if single_key_prefix && p.auto_composed && p.count < 4 {
                            continue;
                        }
                        out.push((code, p.text.as_str(), p.count));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| b.2.cmp(&a.2)));
        out.truncate(limit);
        out
    }

    /// Rebuild Pinyin phrase index for all user phrases using `pdict`'s reverse char->syllable table.
    pub fn reindex_pinyin_phrases(&mut self, pdict: &PinyinDictView) {
        self.pinyin_index.clear();
        let texts: Vec<String> = self.phrases.keys().cloned().collect();
        for text in texts {
            self.index_phrase_pinyin(&text, pdict);
        }
    }

    fn index_phrase_pinyin(&mut self, text: &str, pdict: &PinyinDictView) {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        if !(2..=6).contains(&len) || !chars.iter().all(|&c| is_cjk_ideograph(c)) {
            return;
        }
        let mut readings: Vec<Vec<u16>> = vec![Vec::with_capacity(len)];
        for &ch in &chars {
            let syls = pdict.char_syllables(ch);
            if syls.is_empty() {
                return;
            }
            let mut next = Vec::new();
            'outer: for r in &readings {
                for &sid in syls.iter().take(3) {
                    let mut nr = r.clone();
                    nr.push(sid);
                    next.push(nr);
                    if next.len() >= 8 {
                        break 'outer;
                    }
                }
            }
            readings = next;
        }
        for syl_ids in readings {
            let inits: Vec<u8> = syl_ids.iter().map(|&sid| pdict.syllable_init(sid).0).collect();
            let Some(init_key) = pack_init_key(&inits) else {
                continue;
            };
            let bucket = self.pinyin_index.entry((len as u8, init_key)).or_default();
            if !bucket.iter().any(|(s, t)| s == &syl_ids && t == text) {
                bucket.push((syl_ids, text.to_owned()));
            }
        }
    }

    #[inline]
    pub fn has_pinyin_phrases(&self) -> bool {
        !self.pinyin_index.is_empty()
    }

    /// Look up confirmed user phrases (`!auto_composed || count >= 2`) matching Pinyin `constraints`,
    /// returning `(text, count, auto_composed)` sorted by `(manual_first, count DESC, last_seq DESC)`.
    pub fn lookup_pinyin_phrases_confirmed(
        &self,
        constraints: &[SylConstraint<'_>],
        pdict: &PinyinDictView,
        limit: usize,
    ) -> Vec<(&str, u32, bool)> {
        let len = constraints.len();
        if !(2..=6).contains(&len) || self.pinyin_index.is_empty() {
            return Vec::new();
        }
        let mut inits = [0u8; 6];
        for (i, c) in constraints.iter().enumerate() {
            inits[i] = match c {
                SylConstraint::Full(sid) => pdict.syllable_init(*sid).0,
                SylConstraint::Initial(init_id, _) => *init_id,
                SylConstraint::Prefix(p) => crate::pinyin_dict::base_initial_id(p),
            };
        }
        let Some(init_key) = pack_init_key(&inits[..len]) else {
            return Vec::new();
        };
        let Some(bucket) = self.pinyin_index.get(&(len as u8, init_key)) else {
            return Vec::new();
        };
        let mut hits: Vec<(&str, u32, u64, bool)> = Vec::new();
        for (syl_ids, text) in bucket {
            if syl_ids.len() != len {
                continue;
            }
            let mut matched = true;
            for j in 0..len {
                let sid = syl_ids[j];
                match constraints[j] {
                    SylConstraint::Full(req_sid) => {
                        if sid != req_sid {
                            matched = false;
                            break;
                        }
                    }
                    SylConstraint::Initial(_, req_digraph) => {
                        if req_digraph && !pdict.syllable_init(sid).1 {
                            matched = false;
                            break;
                        }
                    }
                    SylConstraint::Prefix(pref) => {
                        if !pdict.syllable_str(sid).starts_with(pref) {
                            matched = false;
                            break;
                        }
                    }
                }
            }
            if !matched {
                continue;
            }
            if let Some(up) = self.phrases.get(text) {
                if up.auto_composed && up.count < 2 {
                    continue;
                }
                if !hits.iter().any(|h| h.0 == up.text.as_str()) {
                    hits.push((up.text.as_str(), up.count, up.last_seq, up.auto_composed));
                }
            }
        }
        hits.sort_by(|a, b| {
            a.3.cmp(&b.3)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| b.2.cmp(&a.2))
        });
        hits.truncate(limit);
        hits.into_iter()
            .map(|(t, c, _, auto_c)| (t, c, auto_c))
            .collect()
    }

    pub fn add_phrase(
        &mut self,
        text: &str,
        auto_composed: bool,
        dict: Option<&DictView>,
    ) -> Option<PackedCode> {
        self.add_phrase_with_pinyin(text, auto_composed, dict, None)
    }

    pub fn add_phrase_with_pinyin(
        &mut self,
        text: &str,
        auto_composed: bool,
        dict: Option<&DictView>,
        pdict: Option<&PinyinDictView>,
    ) -> Option<PackedCode> {
        self.add_phrase_with_pinyin_strength(text, auto_composed, !auto_composed, dict, pdict)
    }

    pub fn add_phrase_with_pinyin_strength(
        &mut self,
        text: &str,
        auto_composed: bool,
        strong_explicit: bool,
        dict: Option<&DictView>,
        pdict: Option<&PinyinDictView>,
    ) -> Option<PackedCode> {
        let char_len = text.chars().count();
        if !(2..=12).contains(&char_len) {
            return None;
        }
        self.seq += 1;
        let seq = self.seq;
        let prev_phrase = self.phrases.get(text).cloned();
        self.phrases
            .entry(text.to_string())
            .and_modify(|e| {
                e.count = e.count.saturating_add(1);
                e.last_seq = seq;
                if !auto_composed {
                    e.auto_composed = false;
                }
            })
            .or_insert_with(|| UserPhrase {
                text: text.to_string(),
                count: 1,
                last_seq: seq,
                auto_composed,
            });
        if let Some(ref mut undo) = self.last_undo {
            if undo.phrase_prev.is_none() {
                undo.phrase_prev = Some((text.to_string(), prev_phrase));
            }
        }
        self.dirty = true;
        self.continuations.observe(
            &[],
            text,
            true,
            if strong_explicit { Some("") } else { None },
            None,
        );
        if let Some(pd) = pdict {
            self.index_phrase_pinyin(text, pd);
        }
        if let Some(d) = dict {
            if let Some(code) = d.encode_phrase(text) {
                let list = self.code_index.entry(code).or_default();
                if !list.iter().any(|s| s == text) {
                    list.push(text.to_string());
                }
                return Some(code);
            }
        }
        None
    }

    pub fn remove_phrase(&mut self, text: &str) -> bool {
        let existed = self.phrases.remove(text).is_some();
        if existed {
            for list in self.code_index.values_mut() {
                list.retain(|s| s != text);
            }
            for list in self.pinyin_index.values_mut() {
                list.retain(|(_, s)| s != text);
            }
            self.dirty = true;
        }
        existed
    }

    pub fn break_single_burst(&mut self) {
        self.recent_singles.clear();
    }

    /// Start tracking mutations for a new commit so an immediate Backspace (`<= 4s`) can revert
    /// personal continuations, counts, and any newly promoted auto-phrase cleanly.
    pub fn begin_commit_undo(
        &mut self,
        timestamp_ms: u64,
        prev_last_committed: &str,
        prev_recent_context: &[char],
        cont_text: Option<&str>,
    ) {
        let cont_undo = cont_text.map(|t| {
            self.continuations
                .capture_undo_before(prev_recent_context, t)
        });
        self.last_undo = Some(CommitUndoSnapshot {
            ts_ms: timestamp_ms,
            prev_seq: self.seq,
            prev_last_committed: prev_last_committed.to_owned(),
            prev_recent_context: prev_recent_context.to_vec(),
            prev_recent_singles: self.recent_singles.clone(),
            cont_undo,
            ..CommitUndoSnapshot::default()
        });
    }

    /// Revert all personal memory mutations from the last commit if Backspace occurred within `max_age_ms`.
    /// Returns `Some((prev_last_committed, prev_recent_context))` when a rollback was applied.
    pub fn rollback_last_commit_if_within(
        &mut self,
        timestamp_ms: u64,
        max_age_ms: u64,
    ) -> Option<(String, Vec<char>)> {
        let undo = self.last_undo.take()?;
        if timestamp_ms.saturating_sub(undo.ts_ms) > max_age_ms {
            return None;
        }
        self.seq = undo.prev_seq;
        self.recent_singles = undo.prev_recent_singles;
        if let Some((k, prev_v)) = undo.unigram_prev {
            if let Some(v) = prev_v {
                self.unigram_counts.insert(k, v);
            } else {
                self.unigram_counts.remove(&k);
            }
        }
        if let Some((k, prev_v)) = undo.word_bigram_prev {
            if let Some(v) = prev_v {
                self.word_bigrams.insert(k, v);
            } else {
                self.word_bigrams.remove(&k);
            }
        }
        if let Some((k, prev_v)) = undo.char_bigram_prev {
            if let Some(v) = prev_v {
                self.char_bigrams.insert(k, v);
            } else {
                self.char_bigrams.remove(&k);
            }
        }
        for (ng, prev_cnt) in undo.pending_prev {
            if let Some(c) = prev_cnt {
                self.pending_ngrams.insert(ng, c);
            } else {
                self.pending_ngrams.remove(&ng);
            }
        }
        if let Some((phrase, prev_up)) = undo.phrase_prev {
            if let Some(up) = prev_up {
                self.phrases.insert(phrase, up);
            } else {
                self.remove_phrase(&phrase);
            }
        }
        for (lower, prev_en) in undo.enwords_prev {
            if let Some(val) = prev_en {
                self.english_words.insert(lower, val);
            } else {
                self.english_words.remove(&lower);
            }
        }
        if let Some(cu) = undo.cont_undo {
            self.continuations.apply_undo(cu);
        }
        self.dirty = true;
        Some((undo.prev_last_committed, undo.prev_recent_context))
    }

    /// Record a committed string, updating personal unigram/bigram frequencies and
    /// optionally triggering automatic phrase formation (with zero function-word blocklists).
    /// Returns `Some(newly_learned_phrase)` when auto-phrase promotes a new word.
    #[allow(clippy::too_many_arguments)]
    pub fn record_commit(
        &mut self,
        prev_committed: &str,
        committed: &str,
        timestamp_ms: u64,
        auto_phrase: bool,
        auto_threshold: u8,
        dict: Option<&DictView>,
        pdict: Option<&PinyinDictView>,
        passive_stream: bool,
        neural: Option<(&crate::neural::NeuralView<'_>, &crate::neural::RwkvState)>,
    ) -> Option<String> {
        if committed.is_empty() {
            return None;
        }
        self.seq += 1;
        let seq = self.seq;
        self.dirty = true;

        let uni_key = committed.to_string();
        let prev_uni = self.unigram_counts.get(&uni_key).copied();
        let uni_entry = self.unigram_counts.entry(uni_key.clone()).or_insert(0);
        *uni_entry = uni_entry.saturating_add(1);
        if prev_uni.is_none() {
            prune_freq_map_keeping(&mut self.unigram_counts, &uni_key);
        }
        if let Some(ref mut undo) = self.last_undo {
            if undo.unigram_prev.is_none() {
                undo.unigram_prev = Some((uni_key, prev_uni));
            }
        }

        if let Some(up_prev) = self.phrases.get(committed).cloned() {
            if let Some(ref mut undo) = self.last_undo {
                if undo.phrase_prev.is_none() {
                    undo.phrase_prev = Some((committed.to_string(), Some(up_prev)));
                }
            }
            if let Some(up) = self.phrases.get_mut(committed) {
                up.count = up.count.saturating_add(1);
                up.last_seq = seq;
            }
        }

        if !prev_committed.is_empty() {
            let wb_key = (prev_committed.to_string(), committed.to_string());
            let prev_wb = self.word_bigrams.get(&wb_key).copied();
            let wb_entry = self.word_bigrams.entry(wb_key.clone()).or_insert(0);
            *wb_entry = wb_entry.saturating_add(1);
            if prev_wb.is_none() {
                prune_freq_map_keeping(&mut self.word_bigrams, &wb_key);
            }
            if let Some(ref mut undo) = self.last_undo {
                if undo.word_bigram_prev.is_none() {
                    undo.word_bigram_prev = Some((wb_key, prev_wb));
                }
            }

            if let (Some(c_prev), Some(c_next)) =
                (prev_committed.chars().last(), committed.chars().next())
            {
                let cb_key = (c_prev, c_next);
                let prev_cb = self.char_bigrams.get(&cb_key).copied();
                let cb_entry = self.char_bigrams.entry(cb_key).or_insert(0);
                *cb_entry = cb_entry.saturating_add(1);
                if prev_cb.is_none() {
                    prune_freq_map_keeping(&mut self.char_bigrams, &cb_key);
                }
                if let Some(ref mut undo) = self.last_undo {
                    if undo.char_bigram_prev.is_none() {
                        undo.char_bigram_prev = Some((cb_key, prev_cb));
                    }
                }
            }
        }

        // Rolling CJK burst tracking for auto-phrase (supports 2..=4 char phrases across
        // single-char bursts AND mixed word+char bursts like "哈基" + "米" -> "哈基米" or "最好" + "是" -> "最好是"!).
        let committed_chars: Vec<char> = committed.chars().collect();
        if committed_chars.is_empty()
            || committed_chars.len() > 3
            || !committed_chars.iter().all(|&c| is_cjk_ideograph(c))
        {
            self.recent_singles.clear();
            return None;
        }

        if let Some(&(_, last_ts, _, _, _, _)) = self.recent_singles.last() {
            if timestamp_ms > 0 && last_ts > 0 && timestamp_ms.saturating_sub(last_ts) > 5000 {
                self.recent_singles.clear();
            }
        }
        let clen = committed_chars.len();
        for (idx, &ch) in committed_chars.iter().enumerate() {
            self.recent_singles.push((
                ch,
                timestamp_ms,
                seq,
                idx == 0,
                idx + 1 == clen,
                passive_stream,
            ));
        }
        if self.recent_singles.len() > 6 {
            let drain_n = self.recent_singles.len() - 6;
            self.recent_singles.drain(..drain_n);
        }

        if !auto_phrase || self.recent_singles.len() < 2 {
            return None;
        }

        // Check longest suffix FIRST (4 -> 3 -> 2 chars) so a 3-char phrase like "哈基米", "布洛芬", "输入法",
        // or "最好是" is NEVER short-circuited or tail-amputated into its 2-char suffix ("基米", "洛芬", "入法").
        let n = self.recent_singles.len();
        let mut promoted: Option<String> = None;
        for cand_len in [4usize, 3usize, 2usize] {
            if n < cand_len {
                continue;
            }
            let slice = &self.recent_singles[n - cand_len..n];
            // Must start at the beginning of a committed token and end at the end of a committed token
            if !slice[0].3 || !slice[cand_len - 1].4 {
                continue;
            }
            let first_cid = slice[0].2;
            let last_cid = slice[cand_len - 1].2;
            if first_cid == last_cid {
                continue;
            }
            let cand: String = slice.iter().map(|x| x.0).collect();
            if !is_valid_auto_phrase_text(&cand) {
                continue;
            }
            if self.phrases.contains_key(&cand) {
                if cand_len >= 3 {
                    break;
                }
                continue;
            }
            if let Some(d) = dict {
                let Some(code) = d.encode_phrase(&cand) else {
                    continue;
                };
                if d.lookup_exact(code, false).iter().any(|e| e.text == cand) {
                    if cand_len >= 3 {
                        break;
                    }
                    continue;
                }
            }

            let all_single_commits = slice.iter().all(|x| x.3 && x.4);
            let any_passive = slice.iter().any(|x| x.5);
            let cohesion = neural.and_then(|(nv, bos)| nv.isolated_phrase_cohesion(bos, &cand));

            // Left-neighbor word seam guard: if `cand` is `[B, C]` (`cand_len == 2`) and `A + B` immediately
            // to the left is an established dictionary/user word with much stronger cohesion than `B + C`.
            if cand_len == 2 && n >= 3 {
                let prev_item = self.recent_singles[n - 3];
                let recent_enough = timestamp_ms == 0
                    || prev_item.1 == 0
                    || timestamp_ms.saturating_sub(prev_item.1) <= 5000;
                if recent_enough {
                    let left_pair: String = [prev_item.0, slice[0].0].iter().collect();
                    let left_is_word = self.phrases.contains_key(&left_pair)
                        || dict.is_some_and(|d| d.contains_exact_word(&left_pair).is_some());
                    if left_is_word {
                        if let (Some((_, pmi_bc)), Some((nv, bos))) = (cohesion, neural) {
                            if let Some((_, pmi_ab)) = nv.isolated_phrase_cohesion(bos, &left_pair)
                            {
                                if pmi_ab > pmi_bc + 1.8 && pmi_bc < 2.8 {
                                    continue;
                                }
                            }
                        }
                    }
                }
            }

            // Intent-tiered promotion WITHOUT any function-word / modal-particle blocklist.
            let (bump, eff_threshold): (u8, u8) = if let Some((iso_cond, iso_pmi)) = cohesion {
                if any_passive {
                    if iso_pmi <= 0.35 {
                        continue;
                    }
                    let thr = if iso_pmi >= 2.5 {
                        auto_threshold.max(2)
                    } else {
                        auto_threshold.max(3)
                    };
                    (1, thr)
                } else if cand_len == 2
                    && all_single_commits
                    && iso_cond > -3.85
                    && iso_pmi > 4.0
                {
                    (auto_threshold.max(2), auto_threshold)
                } else if iso_pmi < -1.5 {
                    (1, auto_threshold.max(3))
                } else {
                    (1, auto_threshold)
                }
            } else {
                let thr = if any_passive {
                    auto_threshold.max(3)
                } else {
                    auto_threshold
                };
                (1, thr)
            };

            if let Some(word) =
                self.bump_pending_ngram_exact(&cand, bump, eff_threshold, dict, pdict)
            {
                promoted = Some(word);
                break;
            }
            if cand_len >= 3 && cohesion.is_some_and(|(_, pmi)| pmi >= 2.8) {
                break;
            }
        }
        promoted
    }

    /// Immediately promote a `2..=4` CJK phrase explicitly assembled by the user via manual segmented
    /// selection within a single unbroken Pinyin preedit.
    pub fn promote_assembled_immediately(
        &mut self,
        phrase: &str,
        auto_phrase: bool,
        dict: Option<&DictView>,
        pdict: Option<&PinyinDictView>,
    ) -> Option<String> {
        if !auto_phrase || !is_valid_auto_phrase_text(phrase) {
            return None;
        }
        if let Some(up) = self.phrases.get_mut(phrase) {
            self.seq += 1;
            up.count = up.count.saturating_add(1);
            up.last_seq = self.seq;
            self.dirty = true;
            return None;
        }
        self.pending_ngrams.remove(phrase);
        self.add_phrase_with_pinyin_strength(phrase, true, true, dict, pdict);
        Some(phrase.to_string())
    }

    fn bump_pending_ngram_exact(
        &mut self,
        phrase: &str,
        bump: u8,
        threshold: u8,
        dict: Option<&DictView>,
        pdict: Option<&PinyinDictView>,
    ) -> Option<String> {
        if self.phrases.contains_key(phrase) {
            return None;
        }
        if let Some(d) = dict {
            let Some(code) = d.encode_phrase(phrase) else {
                return None;
            };
            if d.lookup_exact(code, false)
                .iter()
                .any(|e| e.text == phrase)
            {
                return None;
            }
        }
        let prev_cnt = self.pending_ngrams.get(phrase).copied();
        if let Some(ref mut undo) = self.last_undo {
            if !undo.pending_prev.iter().any(|(k, _)| k == phrase) {
                undo.pending_prev.push((phrase.to_string(), prev_cnt));
            }
        }
        let cnt = self.pending_ngrams.entry(phrase.to_string()).or_insert(0);
        *cnt = cnt.saturating_add(bump);
        if *cnt >= threshold.max(1) {
            self.pending_ngrams.remove(phrase);
            self.add_phrase_with_pinyin_strength(phrase, true, false, dict, pdict);
            return Some(phrase.to_string());
        }
        None
    }

    #[inline]
    pub fn unigram_boost(&self, text: &str) -> f32 {
        let c = self.unigram_counts.get(text).copied().unwrap_or(0);
        if c == 0 {
            0.0
        } else {
            ((c as f32) + 1.0).ln() * 0.65
        }
    }

    /// Record a user-committed English / OOV Latin word (preserving user casing) for future mixed decoding.
    pub fn record_english_word(&mut self, word: &str) {
        let trimmed = word.trim();
        if !is_plausible_english_token(trimmed) {
            return;
        }
        let lower = trimmed.to_ascii_lowercase();
        let prev_en = self.english_words.get(&lower).cloned();
        if let Some(ref mut undo) = self.last_undo {
            if !undo.enwords_prev.iter().any(|(k, _)| k == &lower) {
                undo.enwords_prev.push((lower.clone(), prev_en));
            }
        }
        let seq = self.seq;
        self.seq = self.seq.saturating_add(1);
        let has_upper = trimmed.bytes().any(|b| b.is_ascii_uppercase());
        self.english_words
            .entry(lower)
            .and_modify(|e| {
                e.1 = e.1.saturating_add(1);
                e.2 = seq;
                if has_upper || e.0 == e.0.to_ascii_lowercase() {
                    e.0 = trimmed.to_owned();
                }
            })
            .or_insert_with(|| (trimmed.to_owned(), 1, seq));
        self.dirty = true;
    }

    /// Guarded Return-key Latin learner: prevents a single accidental Return on an unmatched Wubi/Pinyin
    /// raw code (`vv`, `qdrg`, `cla`, `ghek`) from polluting `ENWORD`.
    pub fn record_english_word_from_return(
        &mut self,
        raw: &str,
        in_english_dict: bool,
        is_shape_schema: bool,
    ) {
        let trimmed = raw.trim();
        if !is_plausible_english_token(trimmed) {
            return;
        }
        let has_anchor = trimmed.bytes().any(|b| {
            b.is_ascii_uppercase()
                || b.is_ascii_digit()
                || matches!(b, b'-' | b'_' | b'.' | b'+' | b'#')
        });
        if has_anchor || in_english_dict {
            self.record_english_word(trimmed);
            return;
        }
        if is_shape_schema {
            if trimmed.len() < 3 {
                return;
            }
            let lower = trimmed.to_ascii_lowercase();
            if self.english_words.contains_key(&lower) {
                self.record_english_word(trimmed);
                return;
            }
            let c = self.pending_enwords.entry(lower.clone()).or_insert(0);
            *c = c.saturating_add(1);
            if *c >= 2 {
                self.pending_enwords.remove(&lower);
                self.record_english_word(trimmed);
            }
        } else {
            self.record_english_word(trimmed);
        }
    }

    /// Extract and record any Latin words (`len >= 2`) from a committed mixed string.
    pub fn record_english_words_in_text(&mut self, text: &str, record_all_latin: bool) {
        let mut start: Option<usize> = None;
        for (idx, ch) in text.char_indices() {
            let is_lat = ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-');
            if is_lat {
                if start.is_none() {
                    start = Some(idx);
                }
            } else if let Some(s) = start.take() {
                let tok = &text[s..idx];
                if tok.len() >= 2
                    && tok.bytes().any(|b| b.is_ascii_alphabetic())
                    && (record_all_latin || tok.bytes().any(|b| b.is_ascii_uppercase()))
                {
                    self.record_english_word(tok);
                }
            }
        }
        if let Some(s) = start {
            let tok = &text[s..];
            if tok.len() >= 2
                && tok.bytes().any(|b| b.is_ascii_alphabetic())
                && (record_all_latin || tok.bytes().any(|b| b.is_ascii_uppercase()))
            {
                self.record_english_word(tok);
            }
        }
    }

    pub fn lookup_english_exact(&self, lower: &str) -> Option<(&str, u32)> {
        self.english_words
            .get(lower)
            .map(|(disp, cnt, _)| (disp.as_str(), *cnt))
    }

    pub fn has_english_prefix(&self, lower: &str) -> bool {
        if lower.is_empty() || self.english_words.is_empty() {
            return false;
        }
        self.english_words.keys().any(|k| k.starts_with(lower))
    }

    pub fn lookup_english_prefix(&self, lower: &str, limit: usize) -> Vec<(&str, u32)> {
        if lower.is_empty() || limit == 0 || self.english_words.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<(&str, u32, u64, bool)> = self
            .english_words
            .iter()
            .filter(|(k, _)| k.starts_with(lower))
            .map(|(k, (disp, cnt, seq))| (disp.as_str(), *cnt, *seq, k.len() == lower.len()))
            .collect();
        hits.sort_by(|a, b| {
            b.3.cmp(&a.3)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| b.2.cmp(&a.2))
        });
        hits.truncate(limit);
        hits.into_iter().map(|(d, c, _, _)| (d, c)).collect()
    }

    /// Look up personal learned English words (`ENWORD`) matching a T9 digit sequence (`'2'..='9'`, `len 2..=12`).
    pub fn lookup_english_t9(&self, digits: &[u8], limit: usize) -> Vec<(&str, u32)> {
        if limit == 0 || self.english_words.is_empty() {
            return Vec::new();
        }
        let Some(target) = crate::english::pack_t9_digits_slice(digits) else {
            return Vec::new();
        };
        let mut hits: Vec<(&str, u32, u64)> = self
            .english_words
            .iter()
            .filter_map(|(k, (disp, cnt, seq))| {
                let wk = crate::english::pack_word_to_t9_key(k)?;
                (wk == target).then_some((disp.as_str(), *cnt, *seq))
            })
            .collect();
        hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
        hits.truncate(limit);
        hits.into_iter().map(|(d, c, _)| (d, c)).collect()
    }

    /// Personal language model log-boost given `prev_committed` context (in nats).
    #[inline]
    pub fn context_boost(&self, prev_committed: &str, candidate: &str) -> f32 {
        let mut boost = self.unigram_boost(candidate) * 0.5;
        if !prev_committed.is_empty() && !candidate.is_empty() {
            if let Some(&wc) = self
                .word_bigrams
                .get(&(prev_committed.to_string(), candidate.to_string()))
            {
                boost += ((wc as f32) + 1.0).ln() * 1.45;
            }
            if let (Some(cp), Some(cn)) =
                (prev_committed.chars().last(), candidate.chars().next())
            {
                if let Some(&cc) = self.char_bigrams.get(&(cp, cn)) {
                    boost += ((cc as f32) + 1.0).ln() * 0.85;
                }
            }
        }
        boost.min(4.5)
    }

    /// Personal next-word suggestions learned from the user's own `word_bigrams`.
    pub fn next_words_after(
        &self,
        recent_context: &[char],
        last_committed: &str,
    ) -> Vec<(String, f32)> {
        if self.word_bigrams.is_empty() || (recent_context.is_empty() && last_committed.is_empty())
        {
            return Vec::new();
        }
        let mut keys = Vec::<String>::with_capacity(5);
        if !last_committed.is_empty() {
            keys.push(last_committed.to_string());
        }
        let n = recent_context.len();
        for len in (1..=4.min(n)).rev() {
            let s: String = recent_context[n - len..n].iter().collect();
            if !keys.contains(&s) {
                keys.push(s);
            }
        }

        let mut scores = HashMap::<String, f32>::new();
        for ((prev, next), &cnt) in &self.word_bigrams {
            if next.is_empty() || cnt == 0 {
                continue;
            }
            if !keys.iter().any(|k| k == prev) {
                continue;
            }
            let prev_len = prev.chars().count();
            if prev_len == 1 && cnt < 2 {
                continue;
            }
            let len_bonus = if prev_len >= 2 { 1.4 } else { 0.0 };
            let sc = 12.5 + ((cnt as f32) + 1.0).ln() * 1.6 + len_bonus;
            let e = scores.entry(next.clone()).or_insert(f32::NEG_INFINITY);
            if sc > *e {
                *e = sc;
            }
        }
        let mut out: Vec<(String, f32)> = scores.into_iter().collect();
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        out.truncate(8);
        out
    }

    /// Serialize personal user store to its canonical TSV representation in memory (zero disk I/O).
    pub fn to_tsv_string(&self) -> String {
        let mut out = String::with_capacity(8192);
        out.push_str("# Juxu User Data v1\n");
        out.push_str(&format!("SEQ\t{}\n", self.seq));
        for (i, t) in self.touch_profile.keys.iter().enumerate() {
            if t.samples > 0 {
                let ch = (b'a' + i as u8) as char;
                out.push_str(&format!(
                    "TOUCH\t{}\t{:.5}\t{:.5}\t{}\n",
                    ch, t.dx, t.dy, t.samples
                ));
            }
        }
        for (i, t) in self.touch_profile.t9_keys.iter().enumerate() {
            if t.samples > 0 {
                let ch = (b'2' + i as u8) as char;
                out.push_str(&format!(
                    "TOUCH_T9\t{}\t{:.5}\t{:.5}\t{}\n",
                    ch, t.dx, t.dy, t.samples
                ));
            }
        }
        let mut phrases: Vec<&UserPhrase> = self.phrases.values().collect();
        phrases.sort_by(|a, b| b.count.cmp(&a.count));
        for p in phrases.into_iter().take(5000) {
            out.push_str(&format!(
                "PHRASE\t{}\t{}\t{}\t{}\n",
                escape_tab(&p.text),
                p.count,
                p.last_seq,
                if p.auto_composed { 1 } else { 0 }
            ));
        }
        let mut unis: Vec<(&String, &u32)> = self.unigram_counts.iter().collect();
        unis.sort_by(|a, b| b.1.cmp(a.1));
        for (w, c) in unis.into_iter().take(4000) {
            out.push_str(&format!("UNI\t{}\t{}\n", escape_tab(w), c));
        }
        let mut bgs: Vec<(&(String, String), &u32)> = self.word_bigrams.iter().collect();
        bgs.sort_by(|a, b| b.1.cmp(a.1));
        for ((p, n), c) in bgs.into_iter().take(4000) {
            out.push_str(&format!(
                "WBG\t{}\t{}\t{}\n",
                escape_tab(p),
                escape_tab(n),
                c
            ));
        }
        let mut en_words: Vec<(&String, &(String, u32, u64))> = self.english_words.iter().collect();
        en_words.sort_by(|a, b| b.1 .1.cmp(&a.1 .1).then_with(|| b.1 .2.cmp(&a.1 .2)));
        for (lower, (disp, cnt, seq)) in en_words.into_iter().take(2000) {
            out.push_str(&format!(
                "ENWORD\t{}\t{}\t{}\t{}\n",
                escape_tab(lower),
                escape_tab(disp),
                cnt,
                seq
            ));
        }
        self.continuations.write_to(&mut out);
        out
    }

    pub fn save_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let out = self.to_tsv_string();
        let write_id = USER_DATA_WRITE_ID.fetch_add(1, Ordering::Relaxed);
        let temporary_path = path.with_extension(format!("tmp.{}.{write_id}", std::process::id()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary_path)?;
        let result = file.write_all(out.as_bytes());
        drop(file);
        let result = result.and_then(|()| fs::rename(&temporary_path, path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result
    }

    pub fn load_file<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let src = fs::read_to_string(path)?;
        Ok(Self::from_tsv_str(&src))
    }

    pub fn from_tsv_str(src: &str) -> Self {
        let mut store = Self::default();
        for line in src.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            match parts.first().copied().unwrap_or("") {
                "SEQ" if parts.len() >= 2 => {
                    store.seq = parts[1].parse().unwrap_or(1);
                }
                "TOUCH" if parts.len() >= 5 => {
                    if let Some(ch) = parts[1].bytes().next() {
                        if ch.is_ascii_lowercase() {
                            let idx = (ch - b'a') as usize;
                            let dx: f32 = parts[2].parse().unwrap_or(0.0);
                            let dy: f32 = parts[3].parse().unwrap_or(0.0);
                            let samples: u32 = parts[4].parse().unwrap_or(0);
                            let (cdx, cdy) = clamp_key_touch_offset(ch, dx, dy);
                            store.touch_profile.keys[idx] = TouchKeyOffset {
                                dx: cdx,
                                dy: cdy,
                                samples,
                            };
                        }
                    }
                }
                "TOUCH_T9" if parts.len() >= 5 => {
                    if let Some(ch) = parts[1].bytes().next() {
                        if (b'2'..=b'9').contains(&ch) {
                            let idx = (ch - b'2') as usize;
                            let dx: f32 = parts[2].parse().unwrap_or(0.0);
                            let dy: f32 = parts[3].parse().unwrap_or(0.0);
                            let samples: u32 = parts[4].parse().unwrap_or(0);
                            store.touch_profile.t9_keys[idx] = TouchKeyOffset {
                                dx: dx.clamp(-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET),
                                dy: dy.clamp(-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET),
                                samples,
                            };
                        }
                    }
                }
                "PHRASE" if parts.len() >= 5 => {
                    let text = parts[1].to_string();
                    let count: u32 = parts[2].parse().unwrap_or(1);
                    let last_seq: u64 = parts[3].parse().unwrap_or(1);
                    let auto_composed = parts[4] == "1";
                    if auto_composed && count <= 1 && !is_valid_auto_phrase_text(&text) {
                        continue;
                    }
                    store.phrases.insert(
                        text.clone(),
                        UserPhrase {
                            text,
                            count,
                            last_seq,
                            auto_composed,
                        },
                    );
                }
                "UNI" if parts.len() >= 3 => {
                    if let Ok(c) = parts[2].parse::<u32>() {
                        store.unigram_counts.insert(parts[1].to_string(), c);
                    }
                }
                "WBG" if parts.len() >= 4 => {
                    if let Ok(c) = parts[3].parse::<u32>() {
                        let p = parts[1].to_string();
                        let n = parts[2].to_string();
                        if let (Some(cp), Some(cn)) = (p.chars().last(), n.chars().next()) {
                            *store.char_bigrams.entry((cp, cn)).or_insert(0) += c;
                        }
                        store.word_bigrams.insert((p, n), c);
                    }
                }
                "ENWORD" if parts.len() >= 5 => {
                    let lower = parts[1].to_ascii_lowercase();
                    let disp = parts[2].to_string();
                    let count: u32 = parts[3].parse().unwrap_or(1);
                    let last_seq: u64 = parts[4].parse().unwrap_or(1);
                    if is_plausible_english_token(&disp) && (2..=48).contains(&lower.len()) {
                        store.english_words.insert(lower, (disp, count, last_seq));
                    }
                }
                "CONT_SEQ" if parts.len() == 2 => store.continuations.load_clock(parts[1]),
                "CONT_MODEL" if parts.len() == 2 => store.continuations.load_model(parts[1]),
                "CONT_UNI" => store.continuations.load_unigrams(&parts[1..]),
                "CONT" => store.continuations.load_row(&parts[1..]),
                _ => {}
            }
        }
        store.dirty = false;
        store
    }
}

fn is_plausible_english_token(trimmed: &str) -> bool {
    if !(2..=48).contains(&trimmed.len()) {
        return false;
    }
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+' | b'#'))
    {
        return false;
    }
    if !trimmed.bytes().any(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    let has_non_lower = trimmed.bytes().any(|b| !b.is_ascii_lowercase());
    let has_vowel = trimmed
        .bytes()
        .any(|b| matches!(b.to_ascii_lowercase(), b'a' | b'e' | b'i' | b'o' | b'u' | b'y'));
    has_non_lower || has_vowel
}

fn escape_tab(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// Validate candidate automatic phrases purely by length (`2..=4` CJK ideographs) and non-stutter,
/// without banning ANY function character or modal particle (`的/了/是/吗/呢/吧/啊/嘛…`).
pub fn is_valid_auto_phrase_text(phrase: &str) -> bool {
    let chars: Vec<char> = phrase.chars().collect();
    let n = chars.len();
    if !(2..=4).contains(&n) || !chars.iter().all(|&c| is_cjk_ideograph(c)) {
        return false;
    }
    if n == 2 && chars[0] == chars[1] {
        return false;
    }
    if n == 4 && chars[0] == chars[2] && chars[1] == chars[3] {
        return false;
    }
    true
}
