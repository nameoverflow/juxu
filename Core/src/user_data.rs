use crate::code::PackedCode;
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

pub const MAX_TOUCH_OFFSET: f32 = 0.20; // Clamp adaptive hitbox shift to ±0.20 key-pitch

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
        entry.dx = ((1.0 - alpha) * entry.dx + alpha * residual_x)
            .clamp(-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET);
        entry.dy = ((1.0 - alpha) * entry.dy + alpha * residual_y)
            .clamp(-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET);
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

pub struct UserStore {
    pub seq: u64,
    pub phrases: HashMap<String, UserPhrase>,
    /// Active schema code -> list of `(text, count, last_seq)`
    code_index: HashMap<PackedCode, Vec<String>>,
    /// Full Pinyin index: `(syl_count, init_key)` -> list of `(syl_ids, text)`
    pinyin_index: HashMap<(u8, u32), Vec<(Vec<u16>, String)>>,
    pub unigram_counts: HashMap<String, u32>,
    pub word_bigrams: HashMap<(String, String), u32>,
    pub char_bigrams: HashMap<(char, char), u32>,
    /// Personal learned English / OOV Latin words: `lower_key -> (preferred_display, count, last_seq)`
    pub english_words: HashMap<String, (String, u32, u64)>,
    pub(crate) continuations: PersonalContinuations,
    pub touch_profile: TouchProfile,
    /// Rolling buffer of recent consecutive CJK characters `(ch, timestamp_ms, rwkv_surprisal, commit_id)`
    /// across both single-char and multi-char commits so 3-4 char words (e.g. "哈基" + "米" -> "哈基米") compose cleanly!
    recent_singles: Vec<(char, u64, Option<f32>, u64)>,
    /// Candidate consecutive n-grams waiting to reach `auto_phrase_threshold`
    pending_ngrams: HashMap<String, u8>,
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
            continuations: PersonalContinuations::default(),
            touch_profile: TouchProfile::default(),
            recent_singles: Vec::new(),
            pending_ngrams: HashMap::new(),
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

    pub fn lookup_prefix(&self, prefix: PackedCode, limit: usize) -> Vec<(PackedCode, &str, u32)> {
        let (lo, hi) = prefix.prefix_range();
        let mut out = Vec::new();
        for (&code, texts) in &self.code_index {
            if code > lo && code <= hi {
                for t in texts {
                    if let Some(p) = self.phrases.get(t) {
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
    /// Allows phrases learned in Wubi/Xiaohe/Pinyin to be immediately retrievable in Full Pinyin!
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

    /// Look up user phrases matching Full Pinyin `constraints`, sorted by `(count DESC, last_seq DESC)`.
    pub fn lookup_pinyin_phrases(
        &self,
        constraints: &[SylConstraint<'_>],
        pdict: &PinyinDictView,
        limit: usize,
    ) -> Vec<(&str, u32)> {
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
        let mut hits: Vec<(&str, u32, u64)> = Vec::new();
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
                if !hits.iter().any(|h| h.0 == up.text.as_str()) {
                    hits.push((up.text.as_str(), up.count, up.last_seq));
                }
            }
        }
        hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
        hits.truncate(limit);
        hits.into_iter().map(|(t, c, _)| (t, c)).collect()
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
        let char_len = text.chars().count();
        if char_len < 2 || char_len > 12 {
            return None;
        }
        self.seq += 1;
        let seq = self.seq;
        let entry = self
            .phrases
            .entry(text.to_string())
            .and_modify(|e| {
                e.count = e.count.saturating_add(1);
                e.last_seq = seq;
            })
            .or_insert_with(|| UserPhrase {
                text: text.to_string(),
                count: 1,
                last_seq: seq,
                auto_composed,
            });
        let _ = entry;
        self.dirty = true;
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

    /// Record a committed string, updating personal unigram/bigram frequencies and
    /// optionally triggering automatic consecutive single-character phrase formation.
    /// Returns `Some(newly_learned_phrase)` when auto-phrase promotes a new word!
    pub fn record_commit(
        &mut self,
        prev_committed: &str,
        committed: &str,
        timestamp_ms: u64,
        auto_phrase: bool,
        auto_threshold: u8,
        dict: Option<&DictView>,
    ) -> Option<String> {
        self.record_commit_with_surprisal(
            prev_committed,
            committed,
            timestamp_ms,
            auto_phrase,
            auto_threshold,
            dict,
            None,
            None,
        )
    }

    /// Extended commit recorder driven by `Micro-RWKV-7` surprisal (`-log P_rwkv`) and cross-mode
    /// Pinyin phrase indexing.
    #[allow(clippy::too_many_arguments)]
    pub fn record_commit_with_surprisal(
        &mut self,
        prev_committed: &str,
        committed: &str,
        timestamp_ms: u64,
        auto_phrase: bool,
        auto_threshold: u8,
        dict: Option<&DictView>,
        pdict: Option<&PinyinDictView>,
        surprisal: Option<f32>,
    ) -> Option<String> {
        if committed.is_empty() {
            return None;
        }
        self.seq += 1;
        let seq = self.seq;
        self.dirty = true;

        *self
            .unigram_counts
            .entry(committed.to_string())
            .or_insert(0) += 1;

        if let Some(up) = self.phrases.get_mut(committed) {
            up.count = up.count.saturating_add(1);
            up.last_seq = seq;
        }

        if !prev_committed.is_empty() {
            if self.word_bigrams.len() < 8192 {
                *self
                    .word_bigrams
                    .entry((prev_committed.to_string(), committed.to_string()))
                    .or_insert(0) += 1;
            }
            if let (Some(c_prev), Some(c_next)) =
                (prev_committed.chars().last(), committed.chars().next())
            {
                if self.char_bigrams.len() < 8192 {
                    *self.char_bigrams.entry((c_prev, c_next)).or_insert(0) += 1;
                }
            }
        }

        // Rolling CJK burst tracking for auto-phrase (supports 2..=4 char phrases across
        // single-char bursts AND mixed word+char bursts like "哈基" + "米" -> "哈基米"!).
        let committed_chars: Vec<char> = committed.chars().collect();
        if committed_chars.is_empty()
            || committed_chars.len() > 3
            || !committed_chars.iter().all(|&c| is_cjk_ideograph(c))
        {
            self.recent_singles.clear();
            return None;
        }

        if let Some(&(_, last_ts, _, _)) = self.recent_singles.last() {
            if timestamp_ms > 0 && last_ts > 0 && timestamp_ms.saturating_sub(last_ts) > 5000 {
                self.recent_singles.clear();
            }
        }
        for &ch in &committed_chars {
            self.recent_singles.push((ch, timestamp_ms, surprisal, seq));
        }
        if self.recent_singles.len() > 6 {
            let drain_n = self.recent_singles.len() - 6;
            self.recent_singles.drain(..drain_n);
        }

        if !auto_phrase || self.recent_singles.len() < 2 {
            return None;
        }

        // Check longest suffix FIRST (4 -> 3 -> 2 chars) so a 3-char word like "哈基米" is NEVER
        // short-circuited by its 2-char tail "基米", and require spanning >= 2 distinct commits!
        let n = self.recent_singles.len();
        let mut promoted: Option<String> = None;
        for cand_len in [4usize, 3usize, 2usize] {
            if n < cand_len {
                continue;
            }
            let slice = &self.recent_singles[n - cand_len..n];
            let first_cid = slice[0].3;
            let last_cid = slice[cand_len - 1].3;
            if first_cid == last_cid {
                // Entire suffix came from a single already-committed word; skip.
                continue;
            }
            let cand: String = slice.iter().map(|x| x.0).collect();
            let mut sum_s = 0.0f32;
            let mut cnt_s = 0usize;
            for item in &slice[1..] {
                if let Some(s) = item.2 {
                    sum_s += s;
                    cnt_s += 1;
                }
            }
            let mean_surp = (cnt_s > 0).then(|| sum_s / (cnt_s as f32));
            if let Some(word) = self.bump_pending_ngram(&cand, auto_threshold, dict, pdict, mean_surp)
            {
                promoted = Some(word);
                // If a longer 3-4 char phrase just promoted, stop so we don't also pollute with its shorter tail!
                break;
            }
        }
        promoted
    }

    /// Promote a multi-character phrase assembled by the user (e.g. from Pinyin multi-segment selection)
    /// when it is not already in the dictionary.
    pub fn record_assembled_phrase(
        &mut self,
        phrase: &str,
        mean_surprisal: Option<f32>,
        auto_phrase: bool,
        auto_threshold: u8,
        dict: Option<&DictView>,
        pdict: Option<&PinyinDictView>,
    ) -> Option<String> {
        if !auto_phrase {
            return None;
        }
        let len = phrase.chars().count();
        if !(2..=4).contains(&len) || !phrase.chars().all(is_cjk_ideograph) {
            return None;
        }
        self.bump_pending_ngram(phrase, auto_threshold, dict, pdict, mean_surprisal)
    }

    fn bump_pending_ngram(
        &mut self,
        phrase: &str,
        threshold: u8,
        dict: Option<&DictView>,
        pdict: Option<&PinyinDictView>,
        mean_surprisal: Option<f32>,
    ) -> Option<String> {
        if self.phrases.contains_key(phrase) {
            return None;
        }
        if let Some(d) = dict {
            let Some(code) = d.encode_phrase(phrase) else {
                return None;
            };
            // Already exists in built-in dictionary? No need to duplicate
            if d.lookup_exact(code, false)
                .iter()
                .any(|e| e.text == phrase)
            {
                return None;
            }
        }
        // Pure probability + repeat gating (no hardcoded function-word blacklist!):
        // - High RWKV cohesion (`surp < 4.0`, e.g. "微信" missing from Wubi86 dict):
        //   strong evidence of a real missing modern word -> accelerate promotion (+2)!
        // - Otherwise (e.g. coined terms like "哈基米" / "星锚" or accidental pairs):
        //   standard +1 so it promotes cleanly once repeated `threshold` times!
        let bump: u8 = if let Some(surp) = mean_surprisal {
            if surp < 4.0 {
                2
            } else {
                1
            }
        } else {
            1
        };
        let cnt = self.pending_ngrams.entry(phrase.to_string()).or_insert(0);
        *cnt = cnt.saturating_add(bump);
        if *cnt >= threshold.max(1) {
            self.pending_ngrams.remove(phrase);
            self.add_phrase_with_pinyin(phrase, true, dict, pdict);
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
        if !(2..=48).contains(&trimmed.len()) {
            return;
        }
        if !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+' | b'#'))
        {
            return;
        }
        if !trimmed.bytes().any(|b| b.is_ascii_alphabetic()) {
            return;
        }
        let lower = trimmed.to_ascii_lowercase();
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
    pub fn next_words_after(&self, recent_context: &[char], last_committed: &str) -> Vec<(String, f32)> {
        if self.word_bigrams.is_empty() || (recent_context.is_empty() && last_committed.is_empty()) {
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
            // Require >= 2 observations for 1-char prev so one-off typos don't stick
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

    pub fn save_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = String::from("# PrisWubi User Data v1\n");
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
                            store.touch_profile.keys[idx] = TouchKeyOffset {
                                dx: dx.clamp(-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET),
                                dy: dy.clamp(-MAX_TOUCH_OFFSET, MAX_TOUCH_OFFSET),
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
                    if lower.len() >= 2 && lower.len() <= 48 && !disp.is_empty() {
                        store.english_words.insert(lower, (disp, count, last_seq));
                    }
                }
                "CONT_SEQ" if parts.len() == 2 => store.continuations.load_clock(parts[1]),
                "CONT_MODEL" if parts.len() == 2 => store.continuations.load_model(parts[1]),
                "CONT" => store.continuations.load_row(&parts[1..]),
                _ => {}
            }
        }
        store.dirty = false;
        Ok(store)
    }
}

fn escape_tab(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

#[inline]
fn is_cjk_ideograph(ch: char) -> bool {
    let u = ch as u32;
    (0x4E00..=0x9FFF).contains(&u) || (0x3400..=0x4DBF).contains(&u)
}
