use crate::code::{cangjie_radicals_str, is_cjk_ideograph, PackedCode};
use crate::config::{FourthEmptyBehavior, InputSchema, JuxuConfig, ShiftToggleMode};
use crate::english::{resolve_english_display, EnDictView};
use crate::format::{AuxView, DictView, ENTRY_FLAG_SYMBOL};
use crate::neural::{NeuralContextTracker, RwkvState};
use crate::pinyin::{
    pinyin_next_key_priors, shuangpin_next_key_priors, t9_next_key_priors, t9_syllable_options,
    PinyinCandidate, PinyinDecoder, PinyinPath, PinyinStream,
};
use crate::pinyin_dict::PinyinDictView;
use crate::spaceless::{Decoder, Segment, Stream};
use crate::typo::{
    generate_hypotheses_with_options, letter_to_t9_digit, softmax_confidences,
    syllable_to_t9_digits, CorrectionKind, KeyLayout, KeyObs,
};
use crate::user_data::UserStore;
use std::collections::{HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CandidateBadge {
    None = 0,
    Shortcode = 1,
    TypoCorrected = 2,
    Wildcard = 3,
    PinyinLookup = 4,
    UserPhrase = 5,
    Emoji = 6,
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub text: String,
    pub matched_code: PackedCode,
    pub comment: String,
    pub badge: CandidateBadge,
    pub confidence: f32,
    pub is_exact: bool,
}

#[derive(Clone, Debug, Default)]
pub struct EngineOutput {
    pub consumed: bool,
    pub commit_text: String,
    pub preedit: String,
    pub bell: bool,
    pub notice: String,
}

pub const KEY_EQUAL: u16 = 0x18;
pub const KEY_MINUS: u16 = 0x1B;
pub const KEY_RETURN: u16 = 0x24;
pub const KEY_TAB: u16 = 0x30;
pub const KEY_SPACE: u16 = 0x31;
pub const KEY_BACKSPACE: u16 = 0x33;
pub const KEY_ESCAPE: u16 = 0x35;
pub const KEY_ENTER_KP: u16 = 0x4C;
pub const KEY_PAGE_UP: u16 = 0x74;
pub const KEY_PAGE_DOWN: u16 = 0x79;
pub const KEY_LEFT: u16 = 0x7B;
pub const KEY_RIGHT: u16 = 0x7C;
pub const KEY_DOWN: u16 = 0x7D;
pub const KEY_UP: u16 = 0x7E;

/// Calibrated weights combining spatial keystroke log-likelihood, lexical prior,
/// corpus N-gram context, and `Micro-RWKV-7` neural state log-probability.
#[derive(Clone, Copy, Debug)]
pub struct LmWeights {
    pub spatial_scale: f32,
    pub dict_prior_scale: f32,
    pub corpus_unigram_scale: f32,
    pub internal_bigram_scale: f32,
    pub bigram_scale: f32,
    pub trigram_scale: f32,
    pub cross_trigram_scale: f32,
    pub phrase_bonus: f32,
    pub neural_scale: f32,
}

impl LmWeights {
    pub const fn default_ngram() -> Self {
        Self {
            spatial_scale: 2.35,
            dict_prior_scale: 0.45,
            corpus_unigram_scale: 0.58,
            internal_bigram_scale: 0.35,
            bigram_scale: 0.95,
            trigram_scale: 1.00,
            cross_trigram_scale: 0.65,
            phrase_bonus: 0.65,
            neural_scale: 0.0,
        }
    }

    pub const fn default_neural() -> Self {
        Self {
            spatial_scale: 2.94,
            dict_prior_scale: 0.30,
            corpus_unigram_scale: 0.22,
            internal_bigram_scale: 0.28,
            bigram_scale: 0.65,
            trigram_scale: 0.65,
            cross_trigram_scale: 0.45,
            phrase_bonus: 0.65,
            neural_scale: 0.68,
        }
    }
}

impl Default for LmWeights {
    fn default() -> Self {
        Self::default_neural()
    }
}

pub struct JuxuEngine {
    pub dicts: [Option<DictView>; 9],
    dict_key_masks: [Vec<u32>; 9],
    pub pinyin_dict: Option<PinyinDictView>,
    pub english_dict: EnDictView,
    pub emoji_table: Option<crate::emoji::EmojiTable>,
    pub aux: Option<AuxView>,
    pub config: JuxuConfig,
    pub user: UserStore,
    pub key_layout: KeyLayout,
    pub lm_weights: LmWeights,
    pub neural_tracker: NeuralContextTracker,

    pub ascii_mode: bool,
    pub(crate) raw_code: String,
    stream: Stream,
    pinyin_stream: PinyinStream,
    pinyin_candidates: Vec<PinyinCandidate>,
    pinyin_paths: Vec<PinyinPath>,
    t9_locked_syls: Vec<(usize, String)>,
    pinyin_partial_acc: String,
    committed_stream_codes: VecDeque<String>,
    pub(crate) observations: Vec<KeyObs>,
    pub(crate) candidates: Vec<Candidate>,
    spaceless_candidate_starts: Vec<Option<usize>>,
    pub(crate) page: usize,
    pub(crate) last_committed: String,
    pub(crate) recent_context: Vec<char>,
    double_quote_open: bool,
    single_quote_open: bool,
    pub(crate) hold_fourth_for_split: bool,
    pub(crate) pending_empty_split: Option<(usize, String)>,
    pub(crate) top_typo_spatial_logp: Option<f32>,
    eval_logger: crate::eval_log::EvalLogger,
}

impl Default for JuxuEngine {
    fn default() -> Self {
        Self::new(JuxuConfig::default())
    }
}

impl JuxuEngine {
    pub fn new(config: JuxuConfig) -> Self {
        Self {
            dicts: [None, None, None, None, None, None, None, None, None],
            dict_key_masks: std::array::from_fn(|_| Vec::new()),
            pinyin_dict: None,
            english_dict: EnDictView::builtin_default(),
            emoji_table: None,
            aux: None,
            config,
            user: UserStore::default(),
            key_layout: KeyLayout::default(),
            lm_weights: LmWeights::default(),
            neural_tracker: NeuralContextTracker::default(),
            ascii_mode: false,
            raw_code: String::new(),
            stream: Stream::default(),
            pinyin_stream: PinyinStream::default(),
            pinyin_candidates: Vec::new(),
            pinyin_paths: Vec::new(),
            t9_locked_syls: Vec::new(),
            pinyin_partial_acc: String::new(),
            committed_stream_codes: VecDeque::new(),
            observations: Vec::new(),
            candidates: Vec::new(),
            spaceless_candidate_starts: Vec::new(),
            page: 0,
            last_committed: String::new(),
            recent_context: Vec::with_capacity(8),
            double_quote_open: false,
            single_quote_open: false,
            hold_fourth_for_split: false,
            pending_empty_split: None,
            top_typo_spatial_logp: None,
            eval_logger: crate::eval_log::EvalLogger::default(),
        }
    }

    pub fn drain_eval_log_jsonl(&mut self) -> String {
        self.eval_logger.drain_jsonl()
    }

    fn eval_context_string(&self) -> String {
        self.recent_context.iter().collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn log_commit_event(
        &mut self,
        timestamp_ms: u64,
        event_kind: &'static str,
        keys: String,
        touches: Vec<[f32; 2]>,
        rank: usize,
        explicit: bool,
        chosen: &str,
        top1: &str,
        typo: &'static str,
        lm_chosen: Option<f32>,
        cands: Vec<crate::eval_log::EvalCandSnapshot>,
    ) {
        if !self.config.eval_logging
            || chosen.is_empty()
            || crate::eval_log::is_sensitive_numeric_text(chosen)
            || crate::eval_log::is_sensitive_numeric_text(&keys)
        {
            return;
        }
        let schema = self.config.schema.as_tag();
        let ctx = crate::eval_log::scrub_numeric_context(&self.eval_context_string());
        let lm_top1 = if rank > 0 || chosen != top1 {
            self.compute_rwkv_mean_nll(top1)
        } else {
            lm_chosen
        };
        let lm_overruled = match (lm_chosen, lm_top1) {
            (Some(c), Some(t)) if event_kind == "commit" && rank > 0 && chosen != top1 => {
                c + 0.05 < t
            }
            _ => false,
        };
        let ev = crate::eval_log::EvalEvent {
            ts_ms: timestamp_ms,
            schema,
            event: event_kind,
            ctx: ctx.clone(),
            keys: keys.clone(),
            touches,
            rank,
            explicit,
            chosen: chosen.to_string(),
            top1: top1.to_string(),
            typo,
            lm_chosen,
            lm_top1,
            lm_overruled,
            cands,
        };
        self.eval_logger.push(&ev);
        self.eval_logger.last_commit = Some(crate::eval_log::LastCommitSnapshot {
            ts_ms: timestamp_ms,
            schema,
            ctx,
            keys,
            chosen: chosen.to_string(),
            top1: top1.to_string(),
            rank,
            typo,
        });
    }

    fn log_abandon_event(&mut self, timestamp_ms: u64) {
        if !self.config.eval_logging
            || self.raw_code.len() < 2
            || crate::eval_log::is_sensitive_numeric_text(&self.raw_code)
        {
            return;
        }
        let top1 = self
            .candidates
            .first()
            .map(|c| c.text.clone())
            .unwrap_or_default();
        let typo = self
            .candidates
            .first()
            .map(crate::eval_log::candidate_typo_tag)
            .unwrap_or("none");
        let ev = crate::eval_log::EvalEvent {
            ts_ms: timestamp_ms,
            schema: self.config.schema.as_tag(),
            event: "abandon",
            ctx: crate::eval_log::scrub_numeric_context(&self.eval_context_string()),
            keys: self.raw_code.clone(),
            touches: crate::eval_log::touches_from_obs(&self.observations),
            rank: 0,
            explicit: false,
            chosen: String::new(),
            top1,
            typo,
            lm_chosen: None,
            lm_top1: None,
            lm_overruled: false,
            cands: crate::eval_log::top_cand_snapshots(&self.candidates),
        };
        self.eval_logger.push(&ev);
    }

    fn log_backspace_after_commit_if_recent(&mut self, timestamp_ms: u64) {
        if !self.config.eval_logging {
            return;
        }
        let Some(last) = self.eval_logger.last_commit.take() else {
            return;
        };
        if timestamp_ms.saturating_sub(last.ts_ms) <= 4_000 {
            let ev = crate::eval_log::EvalEvent {
                ts_ms: timestamp_ms,
                schema: last.schema,
                event: "backspace_after_commit",
                ctx: last.ctx,
                keys: last.keys,
                touches: Vec::new(),
                rank: last.rank,
                explicit: true,
                chosen: last.chosen,
                top1: last.top1,
                typo: last.typo,
                lm_chosen: None,
                lm_top1: None,
                lm_overruled: false,
                cands: Vec::new(),
            };
            self.eval_logger.push(&ev);
        }
    }

    fn push_cjk_context_chars(&mut self, text: &str) {
        for ch in text.chars() {
            if is_cjk_ideograph(ch) {
                self.recent_context.push(ch);
            } else {
                self.recent_context.clear();
            }
        }
        if self.recent_context.len() > 8 {
            let drain_n = self.recent_context.len() - 8;
            self.recent_context.drain(..drain_n);
        }
    }

    /// Explicitly set rolling CJK context (e.g. from surrounding text or offline evaluation).
    pub fn set_recent_context(&mut self, text: &str) {
        let prev_ctx = self.recent_context.clone();
        self.recent_context.clear();
        self.push_cjk_context_chars(text);
        if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
            let chars: Vec<char> = text.chars().collect();
            let neural_ctx: String = chars
                .iter()
                .skip(chars.len().saturating_sub(64))
                .collect();
            self.neural_tracker.set_context_str(&nv, &neural_ctx);
        }
        if self.recent_context != prev_ctx {
            self.last_committed = self.recent_context.iter().collect();
        }
        if self.is_composing() && self.uses_spaceless() {
            self.rebuild_candidates();
        }
    }

    /// Incrementally append committed CJK/punctuation text to rolling context.
    pub fn append_recent_context(&mut self, text: &str) {
        self.push_cjk_context_chars(text);
        if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
            if !text.is_empty() {
                self.neural_tracker.commit_cjk_str(&nv, text);
            } else {
                self.neural_tracker.reset_to_bos(Some(&nv));
            }
        }
        self.last_committed = self.recent_context.iter().collect();
    }

    /// Reset smart quote open/close alternation state.
    pub fn reset_quote_state(&mut self) {
        self.double_quote_open = false;
        self.single_quote_open = false;
    }

    /// Clear rolling sentence context (e.g. cursor jump or new text field).
    pub fn clear_context(&mut self) {
        self.recent_context.clear();
        self.last_committed.clear();
        self.reset_quote_state();
        self.user.break_single_burst();
        self.pinyin_stream.clear();
        let nv = self.aux.as_ref().and_then(|a| a.neural_view());
        self.neural_tracker.reset_to_bos(nv.as_ref());
    }

    /// Record a tapped idle-bar predicted word.
    pub fn accept_prediction(&mut self, text: &str, timestamp_ms: u64) {
        if text.is_empty() {
            return;
        }
        let dict_idx = self.config.schema.slot_index();
        let prev = self.last_committed.clone();
        let _ = self.user.record_commit(
            &prev,
            text,
            timestamp_ms,
            false,
            self.config.auto_phrase_threshold,
            self.dicts[dict_idx].as_ref(),
            self.pinyin_dict.as_ref(),
            false,
            None,
        );
        self.last_committed = text.to_string();
        self.push_cjk_context_chars(text);
        if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
            self.neural_tracker.commit_cjk_str(&nv, text);
        }
    }

    fn rebuild_slot_key_masks(&mut self, slot: usize) {
        self.dict_key_masks[slot].clear();
        let (Some(ref dict), Some(nv)) = (
            self.dicts[slot].as_ref(),
            self.aux.as_ref().and_then(|a| a.neural_view()),
        ) else {
            return;
        };
        let v_cjk = nv
            .offs
            .ascii_trie
            .ext_base_id
            .min(nv.offs.summary.vocab_size);
        let mut table = vec![0u32; v_cjk];
        for (tok, &ch) in nv.offs.tok_to_char.iter().enumerate().take(v_cjk).skip(2) {
            if ch == '\0' {
                continue;
            }
            if let Some(meta) = dict.char_meta(ch) {
                let mut mask = 0u32;
                if let Some(&b) = meta.shortest_code.as_string().as_bytes().first() {
                    if b.is_ascii_lowercase() {
                        mask |= 1u32 << (b - b'a');
                    }
                }
                if let Some(&b) = meta.full_code.as_string().as_bytes().first() {
                    if b.is_ascii_lowercase() {
                        mask |= 1u32 << (b - b'a');
                    }
                }
                table[tok] = mask;
            }
        }
        self.dict_key_masks[slot] = table;
    }

    pub fn load_dict(&mut self, schema: InputSchema, dict: DictView) {
        let slot = schema.slot_index();
        if slot >= self.dicts.len() {
            return;
        }
        self.dicts[slot] = Some(dict);
        self.rebuild_slot_key_masks(slot);
        if self.config.schema == schema {
            if let Some(ref d) = self.dicts[slot] {
                self.user.reindex_phrases(d);
            }
            if !self.raw_code.is_empty() {
                self.rebuild_candidates();
            }
        }
    }

    pub fn retain_only_schema_dict(&mut self, schema: InputSchema) {
        if schema.is_pinyin() {
            for slot in &mut self.dicts {
                *slot = None;
            }
            for mask_vec in &mut self.dict_key_masks {
                mask_vec.clear();
            }
        } else {
            self.pinyin_dict = None;
            self.pinyin_stream.clear();
            self.pinyin_paths.clear();
            self.pinyin_candidates.clear();
            let keep = schema.slot_index();
            for (idx, slot) in self.dicts.iter_mut().enumerate() {
                if idx != keep {
                    *slot = None;
                    self.dict_key_masks[idx].clear();
                }
            }
        }
    }

    pub fn load_pinyin_dict(&mut self, dict: PinyinDictView) {
        self.user.reindex_pinyin_phrases(&dict);
        self.pinyin_dict = Some(dict);
        if self.config.schema.is_pinyin() && !self.raw_code.is_empty() {
            self.rebuild_candidates();
        }
    }

    pub fn load_emoji_table(&mut self, table: crate::emoji::EmojiTable) {
        self.emoji_table = Some(table);
        if !self.raw_code.is_empty() {
            self.rebuild_candidates();
        }
    }

    pub fn add_user_phrase(&mut self, text: &str) -> String {
        let trimmed = text.trim();
        let char_len = trimmed.chars().count();
        if !(2..=12).contains(&char_len) {
            return String::new();
        }
        let slot = self.config.schema.slot_index();
        let dict_ref = self
            .dicts
            .get(slot)
            .and_then(|d| d.as_ref())
            .or_else(|| self.dicts.iter().find_map(|d| d.as_ref()));
        let code_opt =
            self.user
                .add_phrase_with_pinyin(trimmed, false, dict_ref, self.pinyin_dict.as_ref());
        if let Some(code) = code_opt {
            return code.as_string();
        }
        if let Some(ref pd) = self.pinyin_dict {
            let mut syls = Vec::new();
            for ch in trimmed.chars() {
                if let Some(&sid) = pd.char_syllables(ch).first() {
                    syls.push(pd.syllable_str(sid));
                } else {
                    return String::new();
                }
            }
            if !syls.is_empty() {
                return syls.join("'");
            }
        }
        String::new()
    }

    pub fn load_english_dict(&mut self, dict: EnDictView) {
        self.english_dict = dict;
        if !self.raw_code.is_empty() {
            self.rebuild_candidates();
        }
    }

    pub fn set_aux(&mut self, aux: AuxView) {
        if let Some(nv) = aux.neural_view() {
            self.neural_tracker.init_with_model(&nv);
            self.lm_weights = LmWeights::default_neural();
        } else {
            self.neural_tracker = NeuralContextTracker::default();
            self.lm_weights = LmWeights::default_ngram();
        }
        self.aux = Some(aux);
        for slot in 0..self.dicts.len() {
            if self.dicts[slot].is_some() {
                self.rebuild_slot_key_masks(slot);
            } else {
                self.dict_key_masks[slot].clear();
            }
        }
        if !self.raw_code.is_empty() {
            self.rebuild_candidates();
        }
    }

    pub fn active_dict(&self) -> Option<&DictView> {
        let slot = self.config.schema.slot_index();
        self.dicts.get(slot).and_then(|d| d.as_ref())
    }

    pub fn set_config(&mut self, new_config: JuxuConfig) {
        let schema_changed = self.config.schema != new_config.schema;
        let mode_changed = self.config.spaceless != new_config.spaceless;
        self.config = new_config;
        if schema_changed {
            let slot = self.config.schema.slot_index();
            if let Some(Some(ref d)) = self.dicts.get(slot) {
                self.user.reindex_phrases(d);
            }
        }
        if mode_changed || schema_changed {
            self.clear_composition();
        } else if !self.raw_code.is_empty() {
            self.rebuild_candidates();
        }
    }

    pub fn set_schema(&mut self, schema: InputSchema) {
        if self.config.schema != schema {
            self.config.schema = schema;
            let slot = self.config.schema.slot_index();
            if let Some(Some(ref d)) = self.dicts.get(slot) {
                self.user.reindex_phrases(d);
            }
            self.clear_composition();
        }
    }

    #[inline]
    pub fn raw_code(&self) -> &str {
        &self.raw_code
    }

    pub fn preedit(&self) -> String {
        if self.is_pinyin_mode() {
            self.pinyin_paths
                .first()
                .map_or_else(|| self.raw_code.clone(), |p| p.text.clone())
        } else if self.uses_spaceless() {
            self.stream.preview(&self.raw_code)
        } else if self.config.schema.is_cangjie() {
            if self.config.z_omnipotent && self.raw_code.starts_with('z') {
                self.raw_code.clone()
            } else {
                cangjie_radicals_str(&self.raw_code)
            }
        } else {
            self.raw_code.clone()
        }
    }

    pub fn segmented_code(&self) -> String {
        if self.is_pinyin_mode() && self.is_composing() {
            return self
                .pinyin_paths
                .first()
                .map_or_else(|| self.raw_code.clone(), |p| p.segmented_code(&self.raw_code));
        }
        if !self.uses_spaceless() || !self.is_composing() {
            return String::new();
        }
        let pending = self.stream.segmented_code(&self.raw_code);
        if self.committed_stream_codes.is_empty() {
            pending
        } else {
            let committed: Vec<&str> = self
                .committed_stream_codes
                .iter()
                .map(String::as_str)
                .collect();
            format!("{} │ {}", committed.join("·"), pending)
        }
    }

    #[inline]
    pub(crate) fn is_pinyin_mode(&self) -> bool {
        self.config.schema.is_pinyin() && self.pinyin_dict.is_some()
    }

    #[inline]
    pub fn is_t9_pinyin_mode(&self) -> bool {
        self.config.schema == InputSchema::Pinyin
            && self.config.pinyin_t9_layout
            && self.pinyin_dict.is_some()
    }

    fn t9_locked_key_span(&self) -> usize {
        let n = self.observations.len();
        let mut pos = 0usize;
        for (klen, _) in &self.t9_locked_syls {
            while pos < n && self.observations[pos].ch == b'\'' {
                pos += 1;
            }
            if pos >= n || *klen == 0 {
                break;
            }
            pos = (pos + *klen).min(n);
            if pos < n && self.observations[pos].ch == b'\'' {
                pos += 1;
            }
        }
        pos
    }

    pub fn t9_syllable_options(&self) -> Vec<String> {
        if !self.is_t9_pinyin_mode() || self.observations.is_empty() {
            return Vec::new();
        }
        let Some(ref pdict) = self.pinyin_dict else {
            return Vec::new();
        };
        t9_syllable_options(
            &self.observations,
            &self.t9_locked_syls,
            pdict,
            &self.pinyin_paths,
        )
    }

    pub fn t9_locked_prefix(&self) -> String {
        self.t9_locked_syls
            .iter()
            .map(|(_, s)| s.as_str())
            .collect::<Vec<_>>()
            .join("'")
    }

    pub fn t9_next_key_mask_and_priors(&self) -> (u8, [u8; 8]) {
        if !self.is_t9_pinyin_mode() || self.ascii_mode {
            return (0xFF, [100u8; 8]);
        }
        let Some(ref pdict) = self.pinyin_dict else {
            return (0xFF, [100u8; 8]);
        };
        let rwkv_starts = self.rwkv_pinyin_start_priors(pdict);
        let locked_span = self.t9_locked_key_span();
        t9_next_key_priors(&self.raw_code, locked_span, pdict, rwkv_starts.as_ref())
    }

    pub fn lock_t9_syllable(&mut self, syl: &str) -> EngineOutput {
        if !self.is_t9_pinyin_mode() || self.observations.is_empty() {
            return EngineOutput::default();
        }
        let Some(ref pdict) = self.pinyin_dict else {
            return EngineOutput::default();
        };
        let clean = syl.trim().to_ascii_lowercase();
        if pdict.lookup_syllable_id(&clean).is_none() {
            return EngineOutput::default();
        }
        let klen = syllable_to_t9_digits(&clean).len();
        if klen == 0 {
            return EngineOutput::default();
        }
        let start = self.t9_locked_key_span();
        if start + klen > self.observations.len() {
            return EngineOutput::default();
        }
        self.t9_locked_syls.push((klen, clean));
        self.page = 0;
        self.rebuild_pinyin_candidates();
        EngineOutput {
            consumed: true,
            preedit: self.preedit(),
            ..Default::default()
        }
    }

    #[inline]
    pub(crate) fn is_spaceless_active(&self) -> bool {
        self.uses_spaceless()
    }

    pub(crate) fn is_z_lookup_mode(&self) -> bool {
        self.config.schema.supports_z_pinyin()
            && self.config.z_omnipotent
            && self.raw_code.starts_with('z')
            && !self.raw_code.bytes().any(|b| b.is_ascii_uppercase())
            && self.raw_code.len() <= 7
    }

    fn is_zz_symbol_full_code(&self) -> bool {
        if !self.raw_code.starts_with("zz") || self.raw_code.len() != 4 {
            return false;
        }
        let Some(pc) = PackedCode::from_ascii(&self.raw_code) else {
            return false;
        };
        self.active_dict()
            .is_some_and(|d| !d.lookup_exact(pc, true).is_empty())
    }

    fn is_z_pinyin_stream(&self) -> bool {
        if !self.is_z_lookup_mode() {
            return false;
        }
        if !self.raw_code.starts_with("zz")
            || self.raw_code.len() > self.config.schema.max_code_len()
        {
            return true;
        }
        if self.is_zz_symbol_full_code() {
            return false;
        }
        self.aux
            .as_ref()
            .is_some_and(|a| !a.lookup_pinyin(&self.raw_code.as_bytes()[1..], 1).is_empty())
    }

    pub(crate) fn uses_spaceless(&self) -> bool {
        !self.config.schema.is_pinyin()
            && self.config.spaceless
            && self.active_dict().is_some()
            && self.aux.as_ref().is_some_and(AuxView::has_neural)
            && !(self.config.schema.supports_z_pinyin()
                && self.config.z_omnipotent
                && self.raw_code.starts_with('z'))
    }

    #[inline]
    pub fn is_composing(&self) -> bool {
        !self.raw_code.is_empty()
    }

    #[inline]
    pub fn all_candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    #[inline]
    pub fn current_page(&self) -> usize {
        self.page
    }

    pub fn total_pages(&self) -> usize {
        let ps = (self.config.page_size as usize).max(1);
        if self.candidates.is_empty() {
            0
        } else {
            (self.candidates.len() + ps - 1) / ps
        }
    }

    pub fn page_candidates(&self) -> &[Candidate] {
        let ps = (self.config.page_size as usize).max(1);
        let start = self.page * ps;
        if start >= self.candidates.len() {
            &[]
        } else {
            let end = (start + ps).min(self.candidates.len());
            &self.candidates[start..end]
        }
    }

    pub fn clear_composition(&mut self) {
        self.raw_code.clear();
        self.stream = Stream::default();
        self.pinyin_stream.clear();
        self.pinyin_candidates.clear();
        self.pinyin_paths.clear();
        self.t9_locked_syls.clear();
        self.pinyin_partial_acc.clear();
        self.committed_stream_codes.clear();
        self.observations.clear();
        self.candidates.clear();
        self.spaceless_candidate_starts.clear();
        self.page = 0;
        self.hold_fourth_for_split = false;
        self.pending_empty_split = None;
        self.top_typo_spatial_logp = None;
    }

    /// Reclaim excess transient heap allocations when idle or under iOS extension memory pressure.
    pub fn trim_idle_memory(&mut self) {
        self.neural_tracker.trim_ring(2);
        if !self.is_composing() {
            self.raw_code.shrink_to(32);
            self.pinyin_candidates.shrink_to_fit();
            self.pinyin_paths.shrink_to_fit();
            self.t9_locked_syls.shrink_to_fit();
            self.pinyin_partial_acc.shrink_to_fit();
            self.committed_stream_codes.shrink_to_fit();
            self.observations.shrink_to(32);
            self.candidates.shrink_to(16);
            self.spaceless_candidate_starts.shrink_to(16);
        }
    }

    /// Flush and commit any in-progress composition (e.g. on focus loss / app switch / Return-commit).
    pub fn commit_pending(&mut self, timestamp_ms: u64) -> EngineOutput {
        if !self.is_composing() {
            return EngineOutput::default();
        }
        if self.is_spaceless_active() {
            return self.finish_spaceless(timestamp_ms);
        }
        let mut accumulated = String::new();
        let mut notice = String::new();
        for _ in 0..16 {
            if !self.is_composing() {
                break;
            }
            if !self.candidates.is_empty() {
                if let Some(out) = self.commit_candidate(0, timestamp_ms, false) {
                    accumulated.push_str(&out.commit_text);
                    if !out.notice.is_empty() {
                        notice = out.notice;
                    }
                    continue;
                }
            }
            accumulated.push_str(&self.raw_code);
            self.clear_composition();
            break;
        }
        if self.is_composing() {
            accumulated.push_str(&self.raw_code);
            self.clear_composition();
        }
        EngineOutput {
            consumed: true,
            commit_text: accumulated,
            preedit: String::new(),
            bell: false,
            notice,
        }
    }

    /// Toggle between Chinese mode and English ASCII mode (e.g. Shift tap).
    pub fn toggle_ascii_mode(&mut self) -> EngineOutput {
        let mut out = EngineOutput {
            consumed: true,
            ..Default::default()
        };
        if !self.raw_code.is_empty() {
            match self.config.shift_toggle_mode {
                ShiftToggleMode::CommitCode => {
                    out.commit_text = self.raw_code.clone();
                }
                ShiftToggleMode::CommitFirst => {
                    out.commit_text = self
                        .candidates
                        .first()
                        .map(|c| c.text.clone())
                        .unwrap_or_else(|| self.raw_code.clone());
                }
                ShiftToggleMode::Clear => {}
            }
            self.clear_composition();
            self.user.break_single_burst();
            self.clear_context();
        }
        self.ascii_mode = !self.ascii_mode;
        if self.ascii_mode {
            self.clear_context();
        }
        out
    }

    pub fn select_page_candidate(
        &mut self,
        rel_idx: usize,
        timestamp_ms: u64,
    ) -> Option<EngineOutput> {
        let ps = (self.config.page_size as usize).max(1);
        let abs_idx = self.page * ps + rel_idx;
        self.select_abs_candidate(abs_idx, timestamp_ms)
    }

    pub fn select_abs_candidate(
        &mut self,
        abs_idx: usize,
        timestamp_ms: u64,
    ) -> Option<EngineOutput> {
        self.commit_candidate(abs_idx, timestamp_ms, true)
    }

    /// Emoji are output only: they are not learned or logged, and the word they stand for feeds the context.
    fn finish_emoji_commit(&mut self, emoji_text: String, source: Option<String>) -> EngineOutput {
        self.clear_composition();
        self.user.break_single_burst();
        if let Some(src) = source {
            self.last_committed = src.clone();
            self.append_recent_context(&src);
        }
        EngineOutput {
            consumed: true,
            commit_text: emoji_text,
            ..Default::default()
        }
    }

    fn commit_candidate(
        &mut self,
        abs_idx: usize,
        timestamp_ms: u64,
        explicit: bool,
    ) -> Option<EngineOutput> {
        if self.is_pinyin_mode() {
            let pc = self.pinyin_candidates.get(abs_idx)?;
            if pc.candidate.badge == CandidateBadge::Emoji {
                let text = pc.candidate.text.clone();
                let source = self.pinyin_candidates[..abs_idx]
                    .iter()
                    .rev()
                    .find(|p| p.candidate.badge != CandidateBadge::Emoji)
                    .map(|p| p.candidate.text.clone());
                return Some(self.finish_emoji_commit(text, source));
            }
            let commit_text = pc.candidate.text.clone();
            let consumed = pc.consumed_keys.min(self.observations.len());
            let pc_boundaries = pc.boundaries.clone();
            let pc_is_exact = pc.candidate.is_exact;
            let learn = self.config.dynamic_word_freq && (explicit || pc_is_exact);
            let first_choice = (learn && explicit && abs_idx > 0)
                .then(|| self.candidates.first().map(|c| c.text.clone()))
                .flatten();
            let prev_lc = self.last_committed.clone();
            let prev_rc = self.recent_context.clone();
            self.user.begin_commit_undo(
                timestamp_ms,
                &prev_lc,
                &prev_rc,
                (learn && self.config.dynamic_word_freq).then_some(commit_text.as_str()),
            );
            let eval_lm_chosen = self.step_and_learn_commit_single_pass(
                &commit_text,
                learn && self.config.dynamic_word_freq,
                explicit,
                first_choice.as_deref(),
                self.config.eval_logging,
            );
            if self.config.eval_logging {
                let top1 = self
                    .candidates
                    .first()
                    .map(|c| c.text.clone())
                    .unwrap_or_default();
                let eval_keys: String = if consumed > 0 && consumed <= self.raw_code.len() {
                    self.raw_code[..consumed].to_string()
                } else {
                    self.raw_code.clone()
                };
                let eval_obs_end = consumed.min(self.observations.len());
                let eval_touches =
                    crate::eval_log::touches_from_obs(&self.observations[..eval_obs_end]);
                let eval_typo = pc_boundaries
                    .iter()
                    .find(|b| b.correction != CorrectionKind::Exact)
                    .map(|b| crate::eval_log::correction_kind_tag(b.correction))
                    .unwrap_or(if !pc_is_exact { "typo_cand" } else { "none" });
                let eval_cands = crate::eval_log::top_cand_snapshots(&self.candidates);
                let event_kind = if consumed > 0 && consumed < self.observations.len() {
                    "partial_commit"
                } else {
                    "commit"
                };
                self.log_commit_event(
                    timestamp_ms,
                    event_kind,
                    eval_keys,
                    eval_touches,
                    abs_idx,
                    explicit,
                    &commit_text,
                    &top1,
                    eval_typo,
                    eval_lm_chosen,
                    eval_cands,
                );
            }
            let mut notice = String::new();
            if learn && commit_text.bytes().any(|b| b.is_ascii_alphabetic()) {
                self.user.record_english_words_in_text(&commit_text, false);
            }
            self.last_committed = commit_text.clone();
            self.push_cjk_context_chars(&commit_text);
            if self.config.typo.enabled && self.config.typo.touch_hitbox_adapt {
                if self.is_t9_pinyin_mode() {
                    if !pc_boundaries.is_empty() {
                        let mut seg_start = 0usize;
                        for b in &pc_boundaries {
                            let seg_end = b.keys.min(consumed);
                            if seg_end > seg_start
                                && (b.correction == CorrectionKind::Exact
                                    || (explicit && b.correction != CorrectionKind::Transposition))
                            {
                                let target_digits: Vec<u8> = b
                                    .syl_display
                                    .bytes()
                                    .filter_map(letter_to_t9_digit)
                                    .collect();
                                let seg_obs: Vec<&KeyObs> = self.observations[seg_start..seg_end]
                                    .iter()
                                    .filter(|o| (b'2'..=b'9').contains(&o.ch))
                                    .collect();
                                if target_digits.len() == seg_obs.len() {
                                    for (obs, &target_digit) in
                                        seg_obs.iter().zip(target_digits.iter())
                                    {
                                        if obs.has_touch {
                                            let (cx, cy) =
                                                self.key_layout.t9_center_of(target_digit);
                                            self.user.touch_profile.observe_t9(
                                                target_digit,
                                                obs.norm_x - cx,
                                                obs.norm_y - cy,
                                            );
                                            self.user.dirty = true;
                                        }
                                    }
                                }
                            }
                            seg_start = seg_end;
                        }
                    } else if pc_is_exact {
                        for obs in self.observations.iter().take(consumed) {
                            if obs.has_touch && (b'2'..=b'9').contains(&obs.ch) {
                                let (cx, cy) = self.key_layout.t9_center_of(obs.ch);
                                self.user.touch_profile.observe_t9(
                                    obs.ch,
                                    obs.norm_x - cx,
                                    obs.norm_y - cy,
                                );
                                self.user.dirty = true;
                            }
                        }
                    }
                } else if !pc_boundaries.is_empty() {
                    let mut seg_start = 0usize;
                    for b in &pc_boundaries {
                        let seg_end = b.keys.min(consumed);
                        if seg_end > seg_start
                            && (b.correction == CorrectionKind::Exact
                                || (explicit && b.correction != CorrectionKind::Transposition))
                        {
                            let target_letters: Vec<u8> = b
                                .syl_display
                                .bytes()
                                .filter(|ch| ch.is_ascii_lowercase())
                                .collect();
                            let seg_obs: Vec<&KeyObs> = self.observations[seg_start..seg_end]
                                .iter()
                                .filter(|o| o.ch.is_ascii_lowercase())
                                .collect();
                            if target_letters.len() == seg_obs.len() {
                                for (obs, &target_key) in
                                    seg_obs.iter().zip(target_letters.iter())
                                {
                                    if obs.has_touch {
                                        let (cx, cy) = self.key_layout.center_of(target_key);
                                        self.user.touch_profile.observe(
                                            target_key,
                                            obs.norm_x - cx,
                                            obs.norm_y - cy,
                                        );
                                        self.user.dirty = true;
                                    }
                                }
                            }
                        }
                        seg_start = seg_end;
                    }
                } else if pc_is_exact {
                    for obs in self.observations.iter().take(consumed) {
                        if obs.has_touch && obs.ch.is_ascii_lowercase() {
                            let (cx, cy) = self.key_layout.center_of(obs.ch);
                            self.user
                                .touch_profile
                                .observe(obs.ch, obs.norm_x - cx, obs.norm_y - cy);
                            self.user.dirty = true;
                        }
                    }
                }
            }
            if consumed > 0 && consumed < self.observations.len() {
                if learn && commit_text.chars().all(is_cjk_ideograph) {
                    self.pinyin_partial_acc.push_str(&commit_text);
                    if self.pinyin_partial_acc.chars().count() > 4 {
                        self.pinyin_partial_acc.clear();
                    }
                } else {
                    self.pinyin_partial_acc.clear();
                }
                if !self.t9_locked_syls.is_empty() {
                    let mut rem = consumed;
                    while let Some((klen, _)) = self.t9_locked_syls.first() {
                        if *klen <= rem {
                            rem -= *klen;
                            self.t9_locked_syls.remove(0);
                        } else {
                            self.t9_locked_syls.clear();
                            break;
                        }
                    }
                }
                self.raw_code.drain(..consumed);
                self.observations.drain(..consumed);
                self.pinyin_stream.clear();
                self.page = 0;
                self.rebuild_pinyin_candidates();
                return Some(EngineOutput {
                    consumed: true,
                    commit_text,
                    preedit: self.preedit(),
                    bell: false,
                    notice,
                });
            }
            if learn
                && !self.pinyin_partial_acc.is_empty()
                && commit_text.chars().all(is_cjk_ideograph)
            {
                let mut full_assembled = std::mem::take(&mut self.pinyin_partial_acc);
                full_assembled.push_str(&commit_text);
                let dict_idx = self.config.schema.slot_index();
                let dict_for_phrase = self
                    .dicts
                    .get(dict_idx)
                    .and_then(|d| d.as_ref())
                    .or_else(|| self.dicts.iter().find_map(|d| d.as_ref()));
                if let Some(new_word) = self.user.promote_assembled_immediately(
                    &full_assembled,
                    self.config.auto_phrase,
                    dict_for_phrase,
                    self.pinyin_dict.as_ref(),
                ) {
                    notice = format!("自动成词：{}", new_word);
                }
            }
            self.clear_composition();
            return Some(EngineOutput {
                consumed: true,
                commit_text,
                preedit: String::new(),
                bell: false,
                notice,
            });
        }

        let cand = self.candidates.get(abs_idx)?.clone();
        if cand.badge == CandidateBadge::Emoji {
            let source = self.candidates[..abs_idx]
                .iter()
                .rev()
                .find(|c| c.badge != CandidateBadge::Emoji)
                .map(|c| c.text.clone());
            return Some(self.finish_emoji_commit(cand.text, source));
        }
        let learn = self.config.dynamic_word_freq && (explicit || cand.is_exact);
        let first_choice =
            (learn && explicit && abs_idx > 0).then(|| self.candidates[0].text.clone());
        let mut notice = String::new();
        let eval_top1 = self
            .candidates
            .first()
            .map(|c| c.text.clone())
            .unwrap_or_default();
        let eval_keys = self.raw_code.clone();
        let eval_touches = crate::eval_log::touches_from_obs(&self.observations);
        let eval_typo = crate::eval_log::candidate_typo_tag(&cand);
        let eval_cands = if self.config.eval_logging {
            crate::eval_log::top_cand_snapshots(&self.candidates)
        } else {
            Vec::new()
        };
        let commit_text = if self.uses_spaceless() {
            let tail_start = self
                .spaceless_candidate_starts
                .get(abs_idx)
                .copied()
                .flatten();
            let segments = if cand.matched_code.is_empty() {
                let path = self
                    .stream
                    .paths()
                    .iter()
                    .find(|path| path.text == cand.text)?;
                path.segments(self.raw_code.len())
            } else {
                let start = tail_start.unwrap_or_else(|| {
                    self.raw_code.len().saturating_sub(cand.matched_code.len())
                });
                let mut segments = if start == 0 {
                    Vec::new()
                } else {
                    self.stream.paths().first()?.segments(start)
                };
                segments.push(Segment {
                    text: cand.text,
                    code: cand.matched_code,
                    correction: CorrectionKind::Exact,
                });
                segments
            };
            let selected: String = segments.iter().map(|segment| segment.text.as_str()).collect();
            if self.config.eval_logging {
                let lm_chosen = self.compute_rwkv_mean_nll(&selected);
                self.log_commit_event(
                    timestamp_ms,
                    "commit",
                    eval_keys,
                    eval_touches,
                    abs_idx,
                    explicit,
                    &selected,
                    &eval_top1,
                    eval_typo,
                    lm_chosen,
                    eval_cands,
                );
            }
            if learn {
                let prev_lc = self.last_committed.clone();
                let prev_rc = self.recent_context.clone();
                self.user
                    .begin_commit_undo(timestamp_ms, &prev_lc, &prev_rc, Some(&selected));
                self.learn_continuation(&selected, explicit, first_choice.as_deref());
                if explicit && selected.bytes().any(|b| b.is_ascii_alphabetic()) {
                    self.user.record_english_words_in_text(&selected, true);
                }
            } else {
                let prev_lc = self.last_committed.clone();
                let prev_rc = self.recent_context.clone();
                self.user
                    .begin_commit_undo(timestamp_ms, &prev_lc, &prev_rc, None);
            }
            self.finalize_stream_segments(segments, true, false, timestamp_ms, &mut notice)
        } else {
            let prev_lc = self.last_committed.clone();
            let prev_rc = self.recent_context.clone();
            self.user.begin_commit_undo(
                timestamp_ms,
                &prev_lc,
                &prev_rc,
                (learn && self.config.dynamic_word_freq).then_some(cand.text.as_str()),
            );
            let eval_lm_chosen = self.step_and_learn_commit_single_pass(
                &cand.text,
                learn,
                explicit,
                first_choice.as_deref(),
                self.config.eval_logging,
            );
            if learn && explicit && cand.text.bytes().any(|b| b.is_ascii_alphabetic()) {
                self.user.record_english_words_in_text(&cand.text, true);
            }
            if self.config.eval_logging {
                self.log_commit_event(
                    timestamp_ms,
                    "commit",
                    eval_keys,
                    eval_touches,
                    abs_idx,
                    explicit,
                    &cand.text,
                    &eval_top1,
                    eval_typo,
                    eval_lm_chosen,
                    eval_cands,
                );
            }
            let touch_code = (!cand.matched_code.is_empty()
                && !matches!(
                    cand.badge,
                    CandidateBadge::PinyinLookup | CandidateBadge::Wildcard
                ))
            .then_some(cand.matched_code);
            self.finalize_commit_with_prestepped(
                &cand.text,
                touch_code,
                timestamp_ms,
                false,
                true,
                &mut notice,
            );
            cand.text
        };
        self.clear_composition();
        Some(EngineOutput {
            consumed: true,
            commit_text,
            preedit: String::new(),
            bell: false,
            notice,
        })
    }

    pub fn page_prev(&mut self) -> bool {
        if self.page > 0 {
            self.page -= 1;
            true
        } else {
            false
        }
    }

    pub fn page_next(&mut self) -> bool {
        if self.page + 1 < self.total_pages() {
            self.page += 1;
            true
        } else {
            false
        }
    }

    pub fn valid_next_mask(&self) -> u32 {
        const ALL_26: u32 = 0x03FF_FFFF;
        if self.ascii_mode {
            return ALL_26;
        }
        if self.is_pinyin_mode() {
            if let Some(ref pdict) = self.pinyin_dict {
                let (mask, _) = if self.config.schema.is_shuangpin() {
                    shuangpin_next_key_priors(&self.raw_code, pdict, None)
                } else {
                    pinyin_next_key_priors(&self.raw_code, pdict, None)
                };
                return if mask == 0 { ALL_26 } else { mask };
            }
            return ALL_26;
        }
        if self.uses_spaceless() {
            return ALL_26;
        }
        let bytes = self.raw_code.as_bytes();
        let max_len = self.config.schema.max_code_len();
        if bytes.is_empty()
            || bytes.len() >= max_len
            || (self.config.schema.is_wubi() && bytes.contains(&b'z'))
            || (self.config.schema.is_cangjie()
                && self.config.z_omnipotent
                && bytes.starts_with(b"z"))
        {
            return ALL_26;
        }
        let mut mask = self.user.valid_next_letters_mask(bytes);
        if let Some(dict) = self.active_dict() {
            mask |= dict.valid_next_letters_mask(bytes, self.config.enable_rare_charset);
            if self.config.typo.enabled
                && !self.observations.is_empty()
                && self.observations.iter().any(|o| o.has_touch)
            {
                let hyps = generate_hypotheses_with_options(
                    &self.observations,
                    &self.key_layout,
                    &self.user.touch_profile,
                    &self.config.typo,
                    self.config.schema.allows_z_code(),
                );
                let mut added = 0usize;
                for h in hyps {
                    if h.kind == CorrectionKind::TouchBoundary {
                        let (buf, hlen) = h.code.to_bytes_buf();
                        if hlen == bytes.len() {
                            mask |= dict.valid_next_letters_mask(
                                &buf[..hlen],
                                self.config.enable_rare_charset,
                            );
                            mask |= self.user.valid_next_letters_mask(&buf[..hlen]);
                            added += 1;
                            if added >= 2 {
                                break;
                            }
                        }
                    }
                }
            }
        }
        if mask == 0 {
            return ALL_26;
        }
        if self.config.schema.supports_z_pinyin() && self.config.z_omnipotent {
            mask |= 1u32 << 25;
        }
        mask
    }

    fn rwkv_pinyin_start_priors(&self, pdict: &PinyinDictView) -> Option<[f32; 26]> {
        let nv = self.aux.as_ref()?.neural_view()?;
        if !self.neural_tracker.is_ready() {
            return None;
        }
        let state = self
            .pinyin_paths
            .first()
            .and_then(|p| p.ready_state())
            .unwrap_or(&self.neural_tracker.active);
        let top = nv.top_next_chars(state, 32);
        if top.is_empty() {
            return None;
        }
        let mut mass = [0.0f32; 26];
        let mut total = 0.0f32;
        for (ch, lp) in top {
            let p = lp.clamp(-13.5, 0.0).exp();
            let mask = pdict.char_first_letters_mask(ch);
            if mask == 0 {
                continue;
            }
            let share = p / (mask.count_ones() as f32);
            for (idx, m) in mass.iter_mut().enumerate() {
                if (mask & (1u32 << idx)) != 0 {
                    *m += share;
                }
            }
            total += p;
        }
        if total <= 1e-6 {
            return None;
        }
        for v in &mut mass {
            *v /= total;
        }
        Some(mass)
    }

    fn rwkv_dict_first_key_priors(&self, state: &RwkvState, dict: &DictView) -> Option<[u8; 26]> {
        let nv = self.aux.as_ref()?.neural_view()?;
        if !self.neural_tracker.is_ready() {
            return None;
        }
        let slot = self.config.schema.slot_index();
        if let Some(table) = self.dict_key_masks.get(slot).filter(|t| !t.is_empty()) {
            return nv.project_first_key_priors(state, table);
        }
        let top = nv.top_next_chars(state, 32);
        if top.is_empty() {
            return None;
        }
        let mut mass = [0.0f32; 26];
        let mut total = 0.0f32;
        for (ch, lp) in top {
            let Some(meta) = dict.char_meta(ch) else {
                continue;
            };
            let p = lp.clamp(-13.5, 0.0).exp();
            let mut mask = 0u32;
            if let Some(&b) = meta.shortest_code.as_string().as_bytes().first() {
                if b.is_ascii_lowercase() {
                    mask |= 1u32 << (b - b'a');
                }
            }
            if let Some(&b) = meta.full_code.as_string().as_bytes().first() {
                if b.is_ascii_lowercase() {
                    mask |= 1u32 << (b - b'a');
                }
            }
            if mask == 0 {
                continue;
            }
            let share = p / (mask.count_ones() as f32);
            for (idx, m) in mass.iter_mut().enumerate() {
                if (mask & (1u32 << idx)) != 0 {
                    *m += share;
                }
            }
            total += p;
        }
        if total <= 1e-6 {
            return None;
        }
        let mut out = [100u8; 26];
        let uniform = 1.0 / 26.0;
        for idx in 0..26usize {
            let p = (mass[idx] / total).max(1e-4);
            let delta = (p / uniform).ln().clamp(-2.5, 3.0);
            out[idx] = (100.0 + 22.0 * delta).round().clamp(68.0, 172.0) as u8;
        }
        Some(out)
    }

    pub fn next_key_mask_and_priors(&self) -> (u32, [u8; 26]) {
        const ALL_26: u32 = 0x03FF_FFFF;
        let mut out = [100u8; 26];
        if self.ascii_mode {
            return (ALL_26, out);
        }
        if !self.config.typo.enabled {
            return (self.valid_next_mask(), out);
        }
        if self.is_pinyin_mode() {
            if let Some(ref pdict) = self.pinyin_dict {
                let rwkv_starts = self.rwkv_pinyin_start_priors(pdict);
                let (mask, priors) = if self.config.schema.is_shuangpin() {
                    shuangpin_next_key_priors(&self.raw_code, pdict, rwkv_starts.as_ref())
                } else {
                    pinyin_next_key_priors(&self.raw_code, pdict, rwkv_starts.as_ref())
                };
                return (if mask == 0 { ALL_26 } else { mask }, priors);
            }
            return (ALL_26, out);
        }
        if self.uses_spaceless() {
            if let Some(dict) = self.active_dict() {
                let state = self
                    .stream
                    .best_state()
                    .unwrap_or(&self.neural_tracker.active);
                if let Some(priors) = self.rwkv_dict_first_key_priors(state, dict) {
                    return (ALL_26, priors);
                }
            }
            return (ALL_26, out);
        }
        let bytes = self.raw_code.as_bytes();
        let max_len = self.config.schema.max_code_len();
        if bytes.len() >= max_len
            || (self.config.schema.is_wubi() && bytes.contains(&b'z'))
            || (self.config.schema.is_cangjie()
                && self.config.z_omnipotent
                && bytes.starts_with(b"z"))
        {
            return (ALL_26, out);
        }

        if bytes.is_empty() {
            if self.neural_tracker.is_ready() {
                if let Some(dict) = self.active_dict() {
                    if let Some(priors) =
                        self.rwkv_dict_first_key_priors(&self.neural_tracker.active, dict)
                    {
                        return (ALL_26, priors);
                    }
                }
            }
            return (ALL_26, out);
        }

        let mask = self.valid_next_mask();
        if mask == ALL_26 || mask == 0 {
            return (if mask == 0 { ALL_26 } else { mask }, out);
        }
        for (idx, o) in out.iter_mut().enumerate() {
            if (mask & (1u32 << idx)) == 0 {
                *o = 0;
            }
        }

        if let (Some(nv), Some(dict)) = (
            self.aux.as_ref().and_then(|a| a.neural_view()),
            self.active_dict(),
        ) {
            if self.neural_tracker.is_ready() {
                let best_conts =
                    dict.best_continuation_per_letter(bytes, self.config.enable_rare_charset);
                let max_keys = if self.config.schema.allows_z_code() {
                    26usize
                } else {
                    25usize
                };
                let mut raw_s = [f32::NEG_INFINITY; 26];
                let mut sum_s = 0.0f32;
                let mut cnt_s = 0usize;

                for idx in 0..max_keys {
                    if out[idx] == 0 {
                        continue;
                    }
                    if let Some(e) = best_conts[idx] {
                        let ns = nv.score_phrase_fast(&self.neural_tracker.active, e.text);
                        let dp = 0.16 * ((e.weight as f32) + 50.0).ln();
                        let s = ns + dp;
                        raw_s[idx] = s;
                        sum_s += s;
                        cnt_s += 1;
                    }
                }
                if cnt_s >= 2 {
                    let mean_s = sum_s / (cnt_s as f32);
                    for idx in 0..max_keys {
                        if raw_s[idx].is_finite() {
                            out[idx] = (100.0 + 15.0 * (raw_s[idx] - mean_s))
                                .round()
                                .clamp(54.0, 176.0) as u8;
                        }
                    }
                }
            }
        }
        (mask, out)
    }

    pub fn next_key_priors(&self) -> [u8; 26] {
        self.next_key_mask_and_priors().1
    }

    pub fn push_touch_letter(
        &mut self,
        reported_ch: u8,
        view_x: f32,
        view_y: f32,
        is_normalized: bool,
        timestamp_ms: u64,
    ) -> EngineOutput {
        let (nx, ny) = if is_normalized {
            (view_x, view_y)
        } else {
            self.key_layout.view_to_norm(view_x, view_y)
        };
        let reported_lower = reported_ch.to_ascii_lowercase();
        let adapted_ch = if !is_normalized && reported_lower.is_ascii_lowercase() {
            reported_lower
        } else if self.config.typo.enabled && self.config.typo.touch_hitbox_adapt {
            let nearest = self
                .key_layout
                .nearest_key(nx, ny, &self.user.touch_profile, true);
            let mask = self.valid_next_mask();
            if nearest != reported_lower
                && reported_lower.is_ascii_lowercase()
                && mask != 0x03FF_FFFF
            {
                let rep_bit = 1u32 << (reported_lower - b'a');
                let near_bit = 1u32 << (nearest - b'a');
                if (mask & rep_bit) != 0 && (mask & near_bit) == 0 {
                    let (cx, cy) = self.key_layout.center_of(reported_lower);
                    let (dx, dy) = self.user.touch_profile.offset_for(reported_lower);
                    let d2 = (nx - (cx + dx)).powi(2) + (ny - (cy + dy)).powi(2);
                    if d2 <= 0.66 * 0.66 {
                        reported_lower
                    } else {
                        nearest
                    }
                } else {
                    nearest
                }
            } else {
                nearest
            }
        } else {
            reported_lower
        };
        let obs = KeyObs::touch(adapted_ch, nx, ny, timestamp_ms);
        self.push_code_observation(obs)
    }

    pub fn push_touch_t9(
        &mut self,
        reported_ch: u8,
        view_x: f32,
        view_y: f32,
        is_normalized: bool,
        timestamp_ms: u64,
    ) -> EngineOutput {
        if reported_ch == b'\'' || reported_ch == b'1' {
            if self.raw_code.is_empty() || self.raw_code.ends_with('\'') {
                return EngineOutput {
                    consumed: true,
                    preedit: self.preedit(),
                    ..Default::default()
                };
            }
            let obs = KeyObs::physical(b'\'', timestamp_ms);
            return self.push_code_observation(obs);
        }
        if !(b'2'..=b'9').contains(&reported_ch) {
            return EngineOutput::default();
        }
        let (nx, ny) = if is_normalized {
            (view_x, view_y)
        } else {
            self.key_layout.t9_view_to_norm(view_x, view_y)
        };
        let adapted = if !is_normalized {
            reported_ch
        } else if self.config.typo.enabled && self.config.typo.touch_hitbox_adapt {
            self.key_layout
                .nearest_t9_key(nx, ny, &self.user.touch_profile, true)
        } else {
            reported_ch
        };
        let obs = KeyObs::touch(adapted, nx, ny, timestamp_ms);
        self.push_code_observation(obs)
    }

    pub fn handle_key_event(
        &mut self,
        keycode: u16,
        ch: Option<char>,
        has_cmd_or_ctrl: bool,
        timestamp_ms: u64,
    ) -> EngineOutput {
        if has_cmd_or_ctrl {
            if ch == Some('=') && !self.ascii_mode {
                if let Some(out) = self.manual_create_phrase_from_recent() {
                    return out;
                }
            }
            return EngineOutput::default();
        }

        if self.ascii_mode {
            return EngineOutput::default();
        }

        match keycode {
            KEY_BACKSPACE => {
                if self.raw_code.is_empty() {
                    self.rollback_recent_commit_on_backspace(timestamp_ms, true);
                    return EngineOutput::default();
                }
                self.raw_code.pop();
                self.observations.pop();
                while !self.t9_locked_syls.is_empty()
                    && self.t9_locked_key_span() > self.observations.len()
                {
                    self.t9_locked_syls.pop();
                }
                self.page = 0;
                if self.raw_code.is_empty() {
                    self.clear_composition();
                } else if self.uses_spaceless() {
                    self.update_spaceless();
                    self.show_spaceless_candidates();
                } else {
                    self.rebuild_candidates();
                }
                return EngineOutput {
                    consumed: true,
                    preedit: self.preedit(),
                    ..Default::default()
                };
            }
            KEY_ESCAPE => {
                if self.raw_code.is_empty() {
                    return EngineOutput::default();
                }
                self.log_abandon_event(timestamp_ms);
                self.clear_composition();
                return EngineOutput {
                    consumed: true,
                    ..Default::default()
                };
            }
            KEY_RETURN | KEY_ENTER_KP => {
                if self.raw_code.is_empty() {
                    self.user.break_single_burst();
                    self.clear_context();
                    return EngineOutput::default();
                }
                if self.is_t9_pinyin_mode() {
                    if let Some(out) = self.select_page_candidate(0, timestamp_ms) {
                        return out;
                    }
                }
                let raw = self.raw_code.clone();
                let lower = raw.to_ascii_lowercase();
                let in_en_dict = self.english_dict.lookup_exact(&lower).is_some();
                let prev_lc = self.last_committed.clone();
                let prev_rc = self.recent_context.clone();
                self.user
                    .begin_commit_undo(timestamp_ms, &prev_lc, &prev_rc, None);
                self.user
                    .record_english_word_from_return(&raw, in_en_dict, !self.is_pinyin_mode());
                self.clear_composition();
                self.user.break_single_burst();
                if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
                    self.neural_tracker.commit_cjk_str(&nv, &raw);
                }
                self.last_committed = raw.clone();
                self.recent_context.clear();
                return EngineOutput {
                    consumed: true,
                    commit_text: raw,
                    ..Default::default()
                };
            }
            KEY_SPACE => {
                if self.raw_code.is_empty() {
                    self.user.break_single_burst();
                    self.clear_context();
                    return EngineOutput::default();
                }
                if let Some(out) = self.select_page_candidate(0, timestamp_ms) {
                    return out;
                }
                if self.uses_spaceless() {
                    return self.finish_spaceless(timestamp_ms);
                }
                let bell = true;
                if self.config.fourth_empty_behavior == FourthEmptyBehavior::Clear {
                    self.clear_composition();
                }
                return EngineOutput {
                    consumed: true,
                    preedit: self.raw_code.clone(),
                    bell,
                    ..Default::default()
                };
            }
            KEY_UP | KEY_PAGE_UP => {
                if !self.raw_code.is_empty() {
                    self.page_prev();
                    return EngineOutput {
                        consumed: true,
                        preedit: self.preedit(),
                        ..Default::default()
                    };
                }
                return EngineOutput::default();
            }
            KEY_DOWN | KEY_PAGE_DOWN | KEY_TAB => {
                if !self.raw_code.is_empty() {
                    self.page_next();
                    return EngineOutput {
                        consumed: true,
                        preedit: self.preedit(),
                        ..Default::default()
                    };
                }
                return EngineOutput::default();
            }
            KEY_MINUS if ch.is_none() => {
                if !self.raw_code.is_empty() {
                    self.page_prev();
                    return EngineOutput {
                        consumed: true,
                        preedit: self.preedit(),
                        ..Default::default()
                    };
                }
            }
            KEY_EQUAL if ch.is_none() => {
                if !self.raw_code.is_empty() {
                    self.page_next();
                    return EngineOutput {
                        consumed: true,
                        preedit: self.preedit(),
                        ..Default::default()
                    };
                }
            }
            _ => {}
        }

        let Some(c) = ch else {
            return EngineOutput::default();
        };

        if c.is_ascii_lowercase() {
            let obs = KeyObs::physical(c as u8, timestamp_ms);
            return self.push_code_observation(obs);
        }
        if self.is_t9_pinyin_mode() && ('2'..='9').contains(&c) {
            let obs = KeyObs::physical(c as u8, timestamp_ms);
            return self.push_code_observation(obs);
        }
        if (c == '\'' || (c == '1' && self.is_t9_pinyin_mode()))
            && self.is_pinyin_mode()
            && !self.raw_code.is_empty()
        {
            let obs = KeyObs::physical(b'\'', timestamp_ms);
            return self.push_code_observation(obs);
        }

        if c.is_ascii_uppercase() {
            let obs = KeyObs::physical(c as u8, timestamp_ms);
            if self.is_pinyin_mode() || self.uses_spaceless() {
                return self.push_code_observation(obs);
            }
            let already_english = self.raw_code.is_empty()
                || self.raw_code.bytes().any(|b| b.is_ascii_uppercase())
                || (!self.is_z_lookup_mode()
                    && self.raw_code.len() > self.config.schema.max_code_len());
            if already_english {
                return self.push_code_observation(obs);
            }
            let mut commit_prefix = String::new();
            let mut notice = String::new();
            if let Some(first) = self.candidates.first().cloned() {
                self.finalize_automatic_candidate(&first, timestamp_ms, &mut notice);
                commit_prefix.push_str(&first.text);
            }
            self.clear_composition();
            let mut out = self.push_code_observation(obs);
            if !commit_prefix.is_empty() {
                commit_prefix.push_str(&out.commit_text);
                out.commit_text = commit_prefix;
            }
            if out.notice.is_empty() && !notice.is_empty() {
                out.notice = notice;
            }
            return out;
        }

        if !self.raw_code.is_empty() {
            if ('1'..='9').contains(&c) {
                let idx = (c as u8 - b'1') as usize;
                if idx < self.config.page_size as usize {
                    if let Some(out) = self.select_page_candidate(idx, timestamp_ms) {
                        return out;
                    }
                }
                return EngineOutput {
                    consumed: true,
                    preedit: self.preedit(),
                    bell: true,
                    ..Default::default()
                };
            }
            if c == '-' || (c == '[' && !self.uses_spaceless()) {
                self.page_prev();
                return EngineOutput {
                    consumed: true,
                    preedit: self.preedit(),
                    ..Default::default()
                };
            }
            if c == '=' || (c == ']' && !self.uses_spaceless()) {
                self.page_next();
                return EngineOutput {
                    consumed: true,
                    preedit: self.preedit(),
                    ..Default::default()
                };
            }
            if self.config.semicolon_quote_select && (c == ';' || c == '\'') {
                let idx = if c == ';' { 1 } else { 2 };
                if let Some(out) = self.select_page_candidate(idx, timestamp_ms) {
                    return out;
                }
            }
        }

        if self.is_composing() && self.uses_spaceless() && !c.is_whitespace() {
            let punctuation = if self.config.smart_punctuation {
                self.map_chinese_punctuation(c)
            } else {
                None
            };
            let punct_str = punctuation.unwrap_or_else(|| c.to_string());
            let mut out = self.finish_spaceless(timestamp_ms);
            out.commit_text.push_str(&punct_str);
            self.user.break_single_burst();
            self.append_recent_context(&punct_str);
            return out;
        }

        if self.config.smart_punctuation {
            if let Some(mapped) = self.map_chinese_punctuation(c) {
                let mut text = String::new();
                let mut notice = String::new();
                if !self.raw_code.is_empty() {
                    if let Some(first) = self.candidates.first().cloned() {
                        self.finalize_automatic_candidate(&first, timestamp_ms, &mut notice);
                        text.push_str(&first.text);
                    }
                    self.clear_composition();
                }
                self.user.break_single_burst();
                self.append_recent_context(&mapped);
                text.push_str(&mapped);
                return EngineOutput {
                    consumed: true,
                    commit_text: text,
                    notice,
                    ..Default::default()
                };
            }
        }

        EngineOutput::default()
    }

    fn push_code_observation(&mut self, obs: KeyObs) -> EngineOutput {
        let push_ch = if obs.is_upper {
            obs.ch.to_ascii_uppercase() as char
        } else {
            obs.ch as char
        };
        if self.is_pinyin_mode() {
            self.raw_code.push(push_ch);
            self.observations.push(obs);
            self.page = 0;
            self.rebuild_pinyin_candidates();
            return EngineOutput {
                consumed: true,
                preedit: self.preedit(),
                ..Default::default()
            };
        }
        let mut commit_prefix = String::new();
        let mut notice = String::new();
        if self.uses_spaceless() && obs.ch.is_ascii_lowercase() {
            if obs.is_upper
                || obs.ch != b'z'
                || !self.config.z_omnipotent
                || !self.config.schema.supports_z_pinyin()
            {
                return self.push_spaceless(obs);
            }
            if self.is_composing() {
                let out = self.finish_spaceless(obs.timestamp_ms);
                commit_prefix = out.commit_text;
                notice = out.notice;
            }
        }

        let is_z_pinyin_mode = self.is_z_pinyin_stream();
        let schema_max_len = self.config.schema.max_code_len();
        let max_len = if is_z_pinyin_mode { 7 } else { schema_max_len };

        let already_english_buf =
            self.raw_code.bytes().any(|b| b.is_ascii_uppercase()) || self.raw_code.len() > max_len;
        let next_combined =
            if !is_z_pinyin_mode && self.raw_code.len() >= max_len && self.raw_code.len() < 32 {
                let mut s = self.raw_code.clone();
                s.push(push_ch);
                Some(s)
            } else {
                None
            };
        let extend_as_english = already_english_buf
            || obs.is_upper
            || next_combined
                .as_deref()
                .is_some_and(|comb| self.should_extend_fixed_as_english(comb));

        let mut resolved_via_fifth_split = false;
        if self.raw_code.len() >= max_len && !extend_as_english {
            if !is_z_pinyin_mode && self.config.fifth_code_push_commit {
                if schema_max_len == 4
                    && self.raw_code.len() == 4
                    && self.try_resolve_fifth_key_split(obs, &mut commit_prefix, &mut notice)
                {
                    resolved_via_fifth_split = true;
                } else {
                    if let Some(first) = self.candidates.first().cloned() {
                        self.finalize_automatic_candidate(&first, obs.timestamp_ms, &mut notice);
                        commit_prefix.push_str(&first.text);
                    }
                    self.clear_composition();
                }
            } else {
                return EngineOutput {
                    consumed: true,
                    preedit: self.preedit(),
                    bell: true,
                    ..Default::default()
                };
            }
        }

        if !resolved_via_fifth_split {
            self.raw_code.push(push_ch);
            self.observations.push(obs);
            self.page = 0;
            self.rebuild_candidates();
        }

        let now_z_pinyin = self.is_z_pinyin_stream();
        let now_has_upper = self.raw_code.bytes().any(|b| b.is_ascii_uppercase());

        if !now_z_pinyin && !now_has_upper && self.raw_code.len() == schema_max_len {
            if self.config.fifth_code_push_commit
                && !self.looks_like_english_word_or_prefix(&self.raw_code)
            {
                if let Some((cut, c1_text)) = self.pending_empty_split.take() {
                    if cut >= 1
                        && cut < self.raw_code.len()
                        && self.observations.len() == self.raw_code.len()
                    {
                        let tail_code = self.raw_code[cut..].to_string();
                        let tail_obs = self.observations[cut..].to_vec();
                        self.finalize_commit(&c1_text, None, obs.timestamp_ms, &mut notice);
                        commit_prefix.push_str(&c1_text);
                        self.raw_code = tail_code;
                        self.observations = tail_obs;
                        self.page = 0;
                        self.hold_fourth_for_split = false;
                        self.pending_empty_split = None;
                        self.rebuild_candidates();
                        return EngineOutput {
                            consumed: true,
                            commit_text: commit_prefix,
                            preedit: self.preedit(),
                            bell: false,
                            notice,
                        };
                    }
                }
            }

            if self.candidates.is_empty() {
                if self.looks_like_english_word_or_prefix(&self.raw_code) {
                    self.build_english_buffer_candidates();
                    return EngineOutput {
                        consumed: true,
                        commit_text: commit_prefix,
                        preedit: self.preedit(),
                        bell: false,
                        notice,
                    };
                }
                match self.config.fourth_empty_behavior {
                    FourthEmptyBehavior::Block => {
                        self.raw_code.pop();
                        self.observations.pop();
                        self.rebuild_candidates();
                        return EngineOutput {
                            consumed: true,
                            commit_text: commit_prefix,
                            preedit: self.preedit(),
                            bell: true,
                            notice,
                        };
                    }
                    FourthEmptyBehavior::Clear => {
                        self.clear_composition();
                        return EngineOutput {
                            consumed: true,
                            commit_text: commit_prefix,
                            preedit: String::new(),
                            bell: true,
                            notice,
                        };
                    }
                }
            } else if self.config.fourth_code_auto_commit && self.should_auto_commit_fourth() {
                let first = self.candidates[0].clone();
                self.finalize_automatic_candidate(&first, obs.timestamp_ms, &mut notice);
                commit_prefix.push_str(&first.text);
                self.clear_composition();
                return EngineOutput {
                    consumed: true,
                    commit_text: commit_prefix,
                    preedit: String::new(),
                    bell: false,
                    notice,
                };
            }
        }

        EngineOutput {
            consumed: true,
            commit_text: commit_prefix,
            preedit: self.preedit(),
            bell: false,
            notice,
        }
    }

    fn update_spaceless(&mut self) {
        let (Some(dict), Some(aux)) = (
            self.dicts[self.config.schema.slot_index()].as_ref(),
            self.aux.as_ref(),
        ) else {
            return;
        };
        let Some(model) = aux.neural_view() else {
            return;
        };
        self.stream.update(
            &self.observations,
            &Decoder {
                dict,
                english: &self.english_dict,
                raw_code: &self.raw_code,
                aux,
                model,
                config: &self.config,
                user: &self.user,
                context: &self.recent_context,
                layout: &self.key_layout,
            },
            &self.neural_tracker.active,
        );
    }

    fn show_spaceless_candidates(&mut self) {
        let paths = self.stream.paths();
        let scores: Vec<f32> = paths.iter().map(|path| path.score).collect();
        let mut entries: Vec<(Candidate, Option<usize>)> = paths
            .iter()
            .zip(softmax_confidences(&scores))
            .map(|(path, confidence)| {
                let comment = path.correction_hint(self.raw_code.len());
                let is_exact = comment.is_empty();
                (
                    Candidate {
                        text: path.text.clone(),
                        matched_code: PackedCode::EMPTY,
                        comment,
                        badge: if is_exact {
                            CandidateBadge::None
                        } else {
                            CandidateBadge::TypoCorrected
                        },
                        confidence,
                        is_exact,
                    },
                    None,
                )
            })
            .collect();
        let Some(dict) = self.dicts[self.config.schema.slot_index()].as_ref() else {
            self.candidates = entries.into_iter().map(|(c, _)| c).collect();
            self.spaceless_candidate_starts = vec![None; self.candidates.len()];
            return;
        };
        let lookup_singles = |code| {
            dict.lookup_exact(code, self.config.enable_rare_charset)
                .into_iter()
                .filter(|hit| hit.is_single_char() && hit.flags & ENTRY_FLAG_SYMBOL == 0)
                .collect::<Vec<_>>()
        };
        let exact = PackedCode::from_ascii(&self.raw_code)
            .map(|code| (code, lookup_singles(code)))
            .filter(|(_, hits)| !hits.is_empty());
        if let Some((code, hits)) = exact.or_else(|| {
            let start = paths.first()?.tail_start();
            if start == 0 {
                return None;
            }
            let code = PackedCode::from_ascii(&self.raw_code[start..])?;
            Some((code, lookup_singles(code)))
        }) {
            let start = self.raw_code.len() - code.len();
            let comment = paths
                .first()
                .map_or_else(String::new, |path| path.correction_hint(start));
            let is_exact = comment.is_empty();
            let mut singles: Vec<(Candidate, Option<usize>)> = hits
                .into_iter()
                .filter(|hit| {
                    entries
                        .first()
                        .is_none_or(|(first, _)| first.text != hit.text)
                })
                .map(|hit| {
                    (
                        Candidate {
                            text: hit.text.to_owned(),
                            matched_code: code,
                            comment: comment.clone(),
                            badge: if is_exact {
                                CandidateBadge::None
                            } else {
                                CandidateBadge::TypoCorrected
                            },
                            confidence: 0.0,
                            is_exact,
                        },
                        Some(start),
                    )
                })
                .collect();
            let lead = usize::from(!entries.is_empty());
            let mut alternatives = entries.split_off(lead);
            alternatives.retain(|(candidate, _)| {
                !singles
                    .iter()
                    .any(|(single, _)| single.text == candidate.text)
            });
            let visible = singles.len().min(2);
            entries.extend(singles.drain(..visible));
            entries.extend(alternatives);
            entries.extend(singles);
        }

        {
            let schema_max_len = self.config.schema.max_code_len();
            let mut en_spans: Vec<(usize, &str)> = Vec::with_capacity(2);
            if (2..=schema_max_len).contains(&self.raw_code.len()) {
                en_spans.push((0, self.raw_code.as_str()));
            }
            if let Some(ts) = paths.first().map(|p| p.tail_start()).filter(|&s| s > 0) {
                let tail = &self.raw_code[ts..];
                if (3..=schema_max_len).contains(&tail.len()) {
                    en_spans.push((ts, tail));
                }
            }
            for (span_start, span_raw) in en_spans {
                if !span_raw.bytes().all(|b| b.is_ascii_alphabetic()) {
                    continue;
                }
                let lower = span_raw.to_ascii_lowercase();
                let Some(pcode) = PackedCode::from_ascii(&lower) else {
                    continue;
                };
                let user_hit = self.user.lookup_english_exact(&lower);
                let dict_hit = self.english_dict.lookup_with_morphology(&lower);
                let min_dict_w = if lower.len() == 2 { 52_000 } else { 34_000 };
                let eligible = user_hit.is_some()
                    || dict_hit.as_ref().is_some_and(|(_, w, _)| *w >= min_dict_w);
                if !eligible {
                    continue;
                }
                let disp = resolve_english_display(
                    span_raw,
                    user_hit.map(|x| x.0),
                    dict_hit.as_ref().map(|x| x.0.as_str()),
                );
                let full_check = if span_start == 0 {
                    disp.clone()
                } else if let Some(first_path) = paths.first() {
                    let prefix_text: String = first_path
                        .segments(span_start)
                        .into_iter()
                        .map(|s| s.text)
                        .collect();
                    format!("{prefix_text}{disp}")
                } else {
                    disp.clone()
                };
                if !entries
                    .iter()
                    .any(|(c, _)| c.text == disp || c.text == full_check)
                {
                    let insert_idx = entries
                        .len()
                        .min(if user_hit.is_some() { 1 } else { 2 })
                        .max(usize::from(!entries.is_empty()));
                    entries.insert(
                        insert_idx,
                        (
                            Candidate {
                                text: disp,
                                matched_code: pcode,
                                comment: String::new(),
                                badge: CandidateBadge::None,
                                confidence: 0.25,
                                is_exact: true,
                            },
                            Some(span_start),
                        ),
                    );
                    break;
                }
            }
        }

        const PREFIX_CANDIDATE_TARGET: usize = 15;
        let schema_max_len = self.config.schema.max_code_len();
        if entries.len() < PREFIX_CANDIDATE_TARGET && !self.raw_code.is_empty() {
            let need = PREFIX_CANDIDATE_TARGET - entries.len();
            let mut seen: HashSet<String> = entries.iter().map(|(c, _)| c.text.clone()).collect();
            let mut prefix_targets: Vec<(usize, PackedCode)> = Vec::with_capacity(2);
            if self.raw_code.len() < schema_max_len {
                if let Some(code) = PackedCode::from_ascii(&self.raw_code) {
                    prefix_targets.push((0, code));
                }
            }
            if let Some(tail_start) = paths.first().map(|p| p.tail_start()).filter(|&s| s > 0) {
                let tail_slice = &self.raw_code[tail_start..];
                if !tail_slice.is_empty() && tail_slice.len() < schema_max_len {
                    if let Some(code) = PackedCode::from_ascii(tail_slice) {
                        prefix_targets.push((tail_start, code));
                    }
                }
            }

            for (prefix_start, prefix_code) in prefix_targets {
                let before_len = entries.len();
                for (pcode, utext, _) in self.user.lookup_prefix(prefix_code, 4) {
                    if entries.len() >= PREFIX_CANDIDATE_TARGET {
                        break;
                    }
                    if seen.insert(utext.to_string()) {
                        let full_str = pcode.as_string();
                        let tail = full_str.get(prefix_code.len()..).unwrap_or("");
                        entries.push((
                            Candidate {
                                text: utext.to_string(),
                                matched_code: pcode,
                                comment: if tail.is_empty() {
                                    String::new()
                                } else {
                                    format!("…{}", tail)
                                },
                                badge: CandidateBadge::UserPhrase,
                                confidence: 0.0,
                                is_exact: true,
                            },
                            Some(prefix_start),
                        ));
                    }
                }
                for e in dict.lookup_longer_prefix(prefix_code, need, false) {
                    if entries.len() >= PREFIX_CANDIDATE_TARGET {
                        break;
                    }
                    if seen.insert(e.text.to_string()) {
                        let full_str = e.code.as_string();
                        let tail = full_str.get(prefix_code.len()..).unwrap_or("");
                        entries.push((
                            Candidate {
                                text: e.text.to_string(),
                                matched_code: e.code,
                                comment: if tail.is_empty() {
                                    String::new()
                                } else {
                                    format!("…{}", tail)
                                },
                                badge: CandidateBadge::None,
                                confidence: 0.0,
                                is_exact: true,
                            },
                            Some(prefix_start),
                        ));
                    }
                }
                if entries.len() > before_len {
                    break;
                }
            }
        }

        self.candidates = Vec::with_capacity(entries.len());
        self.spaceless_candidate_starts = Vec::with_capacity(entries.len());
        for (cand, start) in entries {
            self.candidates.push(cand);
            self.spaceless_candidate_starts.push(start);
        }
        if let Some(table) = self
            .emoji_table
            .as_ref()
            .filter(|_| self.config.emoji_candidates)
        {
            crate::emoji::inject_spaceless_candidates(
                table,
                &mut self.candidates,
                &mut self.spaceless_candidate_starts,
            );
        }
    }

    fn push_spaceless(&mut self, obs: KeyObs) -> EngineOutput {
        let push_ch = if obs.is_upper {
            obs.ch.to_ascii_uppercase() as char
        } else {
            obs.ch as char
        };
        self.raw_code.push(push_ch);
        self.observations.push(obs);
        self.page = 0;
        self.update_spaceless();
        let dict = self.dicts[self.config.schema.slot_index()].as_ref().unwrap();
        let stable = self.stream.take_stable_prefix(
            &self.raw_code,
            dict,
            &self.english_dict,
            &self.user,
            &self.config,
        );
        let mut out = EngineOutput {
            consumed: true,
            ..Default::default()
        };
        if let Some(prefix) = stable {
            let full_seg_text: String = prefix.segments.iter().map(|s| s.text.as_str()).collect();
            let prev_lc = self.last_committed.clone();
            let prev_rc = self.recent_context.clone();
            self.user.begin_commit_undo(
                obs.timestamp_ms,
                &prev_lc,
                &prev_rc,
                self.config
                    .dynamic_word_freq
                    .then_some(full_seg_text.as_str()),
            );
            out.commit_text = self.finalize_stream_segments(
                prefix.segments,
                false,
                true,
                obs.timestamp_ms,
                &mut out.notice,
            );
            self.raw_code.drain(..prefix.keys);
            self.observations.drain(..prefix.keys);
            self.committed_stream_codes.extend(prefix.codes);
            while self.committed_stream_codes.len() > 32 {
                self.committed_stream_codes.pop_front();
            }
        }
        self.show_spaceless_candidates();
        out.preedit = self.preedit();
        out
    }

    fn finish_spaceless(&mut self, timestamp_ms: u64) -> EngineOutput {
        if let Some(out) = self.commit_candidate(0, timestamp_ms, false) {
            return out;
        }
        let text = self.preedit();
        self.clear_composition();
        self.user.break_single_burst();
        self.clear_context();
        EngineOutput {
            consumed: true,
            commit_text: text,
            ..Default::default()
        }
    }

    fn finalize_stream_segments(
        &mut self,
        segments: Vec<Segment>,
        confirmed_by_user: bool,
        force_passive: bool,
        timestamp_ms: u64,
        notice: &mut String,
    ) -> String {
        let passive_clause = force_passive || segments.len() >= 3;
        let mut text = String::new();
        let mut start = 0;
        for segment in segments {
            let end = start + segment.code.len();
            let learn_seg = !confirmed_by_user
                && segment.correction == CorrectionKind::Exact
                && self.config.dynamic_word_freq;
            let _ = self.step_and_learn_commit_single_pass(
                &segment.text,
                learn_seg,
                false,
                None,
                false,
            );
            if segment.correction == CorrectionKind::Exact
                || (confirmed_by_user && segment.correction != CorrectionKind::Transposition)
            {
                self.learn_touch_offsets(segment.code, start..end);
            }
            self.finalize_commit_with_prestepped(
                &segment.text,
                None,
                timestamp_ms,
                passive_clause,
                true,
                notice,
            );
            text.push_str(&segment.text);
            start = end;
        }
        text
    }

    fn learn_continuation(&mut self, text: &str, explicit: bool, first_choice: Option<&str>) {
        if !self.config.dynamic_word_freq {
            return;
        }
        let neural = self
            .aux
            .as_ref()
            .and_then(|aux| aux.neural_view())
            .filter(|_| self.neural_tracker.is_ready())
            .map(|view| (view, self.neural_tracker.active.clone()));
        self.user.continuations.observe(
            &self.recent_context,
            text,
            explicit,
            first_choice,
            neural,
        );
    }

    /// Single-pass RWKV commit unroll: simultaneously computes `mean_nll` (if requested),
    /// `PersonalContinuations` state signatures, and advances `neural_tracker` without duplicate passes.
    fn step_and_learn_commit_single_pass(
        &mut self,
        text: &str,
        learn: bool,
        explicit: bool,
        first_choice: Option<&str>,
        need_mean_nll: bool,
    ) -> Option<f32> {
        let do_learn = learn && self.config.dynamic_word_freq;
        if !text.is_empty() && text.chars().all(|c| ('\u{3400}'..'\u{A000}').contains(&c)) {
            if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
                if !self.neural_tracker.is_ready() {
                    self.neural_tracker.init_with_model(&nv);
                }
                let chars: Vec<char> = text.chars().collect();
                let n = chars.len();
                let mut st = self.neural_tracker.active.clone();
                let mut cjk_toks = Vec::with_capacity(n);
                let mut sigs = if do_learn {
                    Vec::with_capacity(n)
                } else {
                    Vec::new()
                };
                let mut sum_nll = 0.0f32;
                for (i, &ch) in chars.iter().enumerate() {
                    if need_mean_nll {
                        let lp = nv.score_next_char(&st, ch).clamp(-13.5, -0.01);
                        sum_nll += -lp;
                    }
                    if do_learn {
                        sigs.push(crate::personalization::state_signature(&st));
                    }
                    let tok = nv.char_to_token(ch);
                    cjk_toks.push(tok as u16);
                    nv.step_token_heads(tok, &mut st, i + 1 == n);
                }
                let mean_nll = need_mean_nll.then_some(sum_nll / (n as f32));
                let profile_id = nv.offs.profile_id;
                self.neural_tracker
                    .adopt_cjk_commit_state(&nv, &cjk_toks, st);
                if do_learn {
                    self.user.continuations.observe_prestepped_sigs(
                        &self.recent_context,
                        text,
                        explicit,
                        first_choice,
                        profile_id,
                        &sigs,
                    );
                }
                return mean_nll;
            }
        }
        let mean_nll = if need_mean_nll {
            self.compute_rwkv_mean_nll(text)
        } else {
            None
        };
        if do_learn {
            self.learn_continuation(text, explicit, first_choice);
        }
        if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
            if !text.is_empty() {
                self.neural_tracker.commit_cjk_str(&nv, text);
            } else {
                self.neural_tracker.reset_to_bos(Some(&nv));
            }
        }
        mean_nll
    }

    fn finalize_automatic_candidate(
        &mut self,
        candidate: &Candidate,
        timestamp_ms: u64,
        notice: &mut String,
    ) {
        let do_learn = candidate.is_exact && self.config.dynamic_word_freq;
        let prev_lc = self.last_committed.clone();
        let prev_rc = self.recent_context.clone();
        self.user.begin_commit_undo(
            timestamp_ms,
            &prev_lc,
            &prev_rc,
            do_learn.then_some(candidate.text.as_str()),
        );
        let _ =
            self.step_and_learn_commit_single_pass(&candidate.text, do_learn, false, None, false);
        let code = (!candidate.matched_code.is_empty()
            && !matches!(
                candidate.badge,
                CandidateBadge::PinyinLookup | CandidateBadge::Wildcard
            ))
        .then_some(candidate.matched_code);
        self.finalize_commit_with_prestepped(
            &candidate.text,
            code,
            timestamp_ms,
            false,
            true,
            notice,
        );
    }

    fn learn_touch_offsets(&mut self, code: PackedCode, range: std::ops::Range<usize>) {
        if !self.config.typo.enabled
            || !self.config.typo.touch_hitbox_adapt
            || range.len() != code.len()
        {
            return;
        }
        let Some(observations) = self.observations.get(range) else {
            return;
        };
        let (keys, len) = code.to_bytes_buf();
        for (obs, &key) in observations.iter().zip(&keys[..len]) {
            if obs.has_touch {
                let (cx, cy) = self.key_layout.center_of(key);
                self.user
                    .touch_profile
                    .observe(key, obs.norm_x - cx, obs.norm_y - cy);
                self.user.dirty = true;
            }
        }
    }

    fn rollback_recent_commit_on_backspace(&mut self, timestamp_ms: u64, clear_ctx_on_miss: bool) {
        self.log_backspace_after_commit_if_recent(timestamp_ms);
        self.user.break_single_burst();
        if let Some((prev_last, prev_ctx)) = self
            .user
            .rollback_last_commit_if_within(timestamp_ms, 4_000)
        {
            self.last_committed = prev_last;
            self.recent_context = prev_ctx;
        } else if clear_ctx_on_miss {
            self.recent_context.clear();
        }
        let nv = self.aux.as_ref().and_then(|a| a.neural_view());
        self.neural_tracker.pop_commit_or_reset(nv.as_ref());
    }

    pub fn abandon_composition(&mut self, timestamp_ms: u64) {
        self.log_abandon_event(timestamp_ms);
        self.clear_composition();
        self.user.break_single_burst();
    }

    pub fn notify_post_commit_backspace(&mut self, timestamp_ms: u64) {
        self.rollback_recent_commit_on_backspace(timestamp_ms, false);
    }

    /// Compute full mean per-character RWKV NLL `(-lp_1 - lp_2 - ... - lp_n) / n` from active context for `eval_log`.
    fn compute_rwkv_mean_nll(&self, text: &str) -> Option<f32> {
        let nv = self.aux.as_ref()?.neural_view()?;
        if !self.neural_tracker.is_ready() {
            return None;
        }
        let mut chars = text.chars().filter(|&c| is_cjk_ideograph(c));
        let c1 = chars.next()?;
        let mut sum_nll = -nv
            .score_next_char(&self.neural_tracker.active, c1)
            .clamp(-13.5, -0.01);
        let mut count = 1.0f32;
        let mut st = self.neural_tracker.active.clone();
        let mut peek = chars.peekable();
        if peek.peek().is_some() {
            nv.step_token(nv.char_to_token(c1), &mut st);
            while let Some(ch) = peek.next() {
                sum_nll += -nv.score_next_char(&st, ch).clamp(-13.5, -0.01);
                count += 1.0;
                if peek.peek().is_some() {
                    nv.step_token(nv.char_to_token(ch), &mut st);
                }
            }
        }
        Some(sum_nll / count)
    }

    pub(crate) fn finalize_commit(
        &mut self,
        text: &str,
        matched_code: Option<PackedCode>,
        timestamp_ms: u64,
        notice: &mut String,
    ) {
        let _ = self.step_and_learn_commit_single_pass(text, false, false, None, false);
        self.finalize_commit_with_prestepped(
            text,
            matched_code,
            timestamp_ms,
            false,
            true,
            notice,
        );
    }

    fn finalize_commit_with_prestepped(
        &mut self,
        text: &str,
        matched_code: Option<PackedCode>,
        timestamp_ms: u64,
        passive_stream: bool,
        already_stepped_tracker: bool,
        notice: &mut String,
    ) {
        if let Some(code) = matched_code {
            self.learn_touch_offsets(code, 0..self.observations.len());
        }

        let dict_idx = self.config.schema.slot_index();
        let dict_opt = self.dicts.get(dict_idx).and_then(|d| d.as_ref());
        let prev = self.last_committed.clone();
        let nv_for_iso = self.aux.as_ref().and_then(|a| a.neural_view());
        let bos_ref = self.neural_tracker.bos_state();
        let neural_iso = nv_for_iso.as_ref().zip(bos_ref);
        let learned = self.user.record_commit(
            &prev,
            text,
            timestamp_ms,
            self.config.auto_phrase,
            self.config.auto_phrase_threshold,
            dict_opt,
            self.pinyin_dict.as_ref(),
            passive_stream,
            neural_iso,
        );
        if let Some(new_word) = learned {
            if let Some(d) = dict_opt {
                if let Some(c) = d.encode_phrase(&new_word) {
                    *notice = format!("自动成词：{} ({})", new_word, c.as_string());
                }
            }
        }

        let is_py = self.is_pinyin_mode();
        if text.bytes().any(|b| b.is_ascii_alphabetic()) {
            let mut start: Option<usize> = None;
            for (idx, ch) in text.char_indices() {
                if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                    if start.is_none() {
                        start = Some(idx);
                    }
                } else if let Some(s) = start.take() {
                    let tok = &text[s..idx];
                    if tok.len() >= 2 && tok.bytes().any(|b| b.is_ascii_alphabetic()) {
                        let has_upper = tok.bytes().any(|b| b.is_ascii_uppercase());
                        let lower = tok.to_ascii_lowercase();
                        let in_en = self.english_dict.lookup_exact(&lower).is_some();
                        if has_upper || (is_py && in_en) {
                            self.user.record_english_word(tok);
                        }
                    }
                }
            }
            if let Some(s) = start {
                let tok = &text[s..];
                if tok.len() >= 2 && tok.bytes().any(|b| b.is_ascii_alphabetic()) {
                    let has_upper = tok.bytes().any(|b| b.is_ascii_uppercase());
                    let lower = tok.to_ascii_lowercase();
                    let in_en = self.english_dict.lookup_exact(&lower).is_some();
                    if has_upper || (is_py && in_en) {
                        self.user.record_english_word(tok);
                    }
                }
            }
        }
        self.last_committed = text.to_string();
        self.push_cjk_context_chars(text);
        if !already_stepped_tracker {
            if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
                if !text.is_empty() {
                    self.neural_tracker.commit_cjk_str(&nv, text);
                } else {
                    self.neural_tracker.reset_to_bos(Some(&nv));
                }
            }
        }
    }

    fn manual_create_phrase_from_recent(&mut self) -> Option<EngineOutput> {
        let dict_idx = self.config.schema.slot_index();
        let d = self.dicts[dict_idx].as_ref()?;
        if self.last_committed.chars().count() >= 2 {
            let phrase = self.last_committed.clone();
            let code = self.user.add_phrase_with_pinyin(
                &phrase,
                false,
                Some(d),
                self.pinyin_dict.as_ref(),
            )?;
            return Some(EngineOutput {
                consumed: true,
                notice: format!("已收录词条：{} ({})", phrase, code.as_string()),
                ..Default::default()
            });
        }
        None
    }

    fn map_chinese_punctuation(&mut self, c: char) -> Option<String> {
        let s = match c {
            ',' => "，",
            '.' => "。",
            '?' => "？",
            '!' => "！",
            ':' => "：",
            ';' => "；",
            '(' => "（",
            ')' => "）",
            '[' => "【",
            ']' => "】",
            '{' => "「",
            '}' => "」",
            '<' => "《",
            '>' => "》",
            '\\' => "、",
            '^' => "……",
            '_' => "——",
            '$' => "￥",
            '"' => {
                self.double_quote_open = !self.double_quote_open;
                if self.double_quote_open {
                    "“"
                } else {
                    "”"
                }
            }
            '\'' => {
                self.single_quote_open = !self.single_quote_open;
                if self.single_quote_open {
                    "‘"
                } else {
                    "’"
                }
            }
            _ => return None,
        };
        Some(s.to_string())
    }

    fn rebuild_pinyin_candidates(&mut self) {
        self.candidates.clear();
        self.pinyin_candidates.clear();
        self.pinyin_paths.clear();
        let Some(ref pdict) = self.pinyin_dict else {
            return;
        };
        if self.raw_code.is_empty() {
            return;
        }
        let nv = self.aux.as_ref().and_then(|a| a.neural_view());
        let initial_state = if nv.is_some() {
            self.neural_tracker.active.clone()
        } else {
            RwkvState::default()
        };
        let decoder = PinyinDecoder {
            dict: pdict,
            english: &self.english_dict,
            model: nv,
            config: &self.config,
            user: &self.user,
            context: &self.recent_context,
            layout: &self.key_layout,
        };
        let (mut cands, paths) = if self.is_t9_pinyin_mode()
            && self
                .observations
                .iter()
                .any(|o| (b'2'..=b'9').contains(&o.ch))
        {
            self.pinyin_stream.update_and_decode_t9(
                &self.observations,
                &self.raw_code,
                &self.t9_locked_syls,
                &decoder,
                &initial_state,
            )
        } else {
            self.pinyin_stream.update_and_decode(
                &self.observations,
                &self.raw_code,
                &decoder,
                &initial_state,
            )
        };
        if let Some(table) = self
            .emoji_table
            .as_ref()
            .filter(|_| self.config.emoji_candidates)
        {
            crate::emoji::inject(table, &mut cands, self.observations.len());
        }
        self.candidates = cands.iter().map(|pc| pc.candidate.clone()).collect();
        self.pinyin_candidates = cands;
        self.pinyin_paths = paths;
    }

    pub fn rebuild_candidates(&mut self) {
        self.rebuild_candidates_inner();
        if !self.is_pinyin_mode() && !self.uses_spaceless() {
            if let Some(table) = self
                .emoji_table
                .as_ref()
                .filter(|_| self.config.emoji_candidates)
            {
                crate::emoji::inject_candidates(table, &mut self.candidates);
            }
        }
    }

    fn rebuild_candidates_inner(&mut self) {
        self.candidates.clear();
        self.pinyin_candidates.clear();
        self.pinyin_paths.clear();
        self.stream = Stream::default();
        self.hold_fourth_for_split = false;
        self.pending_empty_split = None;
        self.top_typo_spatial_logp = None;
        if self.raw_code.is_empty() {
            return;
        }
        if self.is_pinyin_mode() {
            self.rebuild_pinyin_candidates();
            return;
        }
        let slot = self.config.schema.slot_index();
        if slot >= self.dicts.len() || self.dicts[slot].is_none() {
            return;
        }
        if self.uses_spaceless() {
            self.update_spaceless();
            self.show_spaceless_candidates();
            return;
        }

        if self.raw_code.bytes().any(|b| b.is_ascii_uppercase()) {
            self.build_english_buffer_candidates();
            return;
        }

        if self.is_z_lookup_mode() {
            self.build_z_mode_candidates(slot);
            return;
        }

        if self.raw_code.len() > self.config.schema.max_code_len() {
            self.build_english_buffer_candidates();
            return;
        }

        if self.config.schema.is_wubi()
            && self.config.z_omnipotent
            && self.raw_code.contains('z')
        {
            self.build_wildcard_candidates(slot);
            return;
        }

        self.build_standard_and_typo_candidates(slot);
    }
}

#[cfg(test)]
mod perf_correctness_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn long_pinyin_backspace_and_t9_incremental_parity() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let py_path = root.join("Resources/Dicts/pinyin.pydict");
        let aux_path = root.join("Resources/Dicts/wubi_aux.wbaux");
        if !py_path.exists() || !aux_path.exists() {
            return;
        }
        let mut eng = JuxuEngine::default();
        eng.load_pinyin_dict(PinyinDictView::from_file(&py_path).expect("load pydict"));
        eng.set_aux(AuxView::open_mmap(&aux_path).expect("load wbaux"));
        eng.config.schema = InputSchema::Pinyin;

        let keys = "jintianwomenyiqiquchifanranhouzaikanyixianzhegedaimawenti";
        let mut forward_top1: Vec<String> = Vec::with_capacity(keys.len());
        for (i, b) in keys.bytes().enumerate() {
            let (cx, cy) = eng.key_layout.center_of(b);
            eng.push_touch_letter(b, cx, cy, true, 1000 + i as u64 * 40);
            forward_top1.push(
                eng.page_candidates()
                    .first()
                    .map(|c| c.text.clone())
                    .unwrap_or_default(),
            );
        }
        for step in 1..=35usize {
            eng.handle_key_event(KEY_BACKSPACE, None, false, 4000 + step as u64 * 40);
            let expected = &forward_top1[keys.len() - 1 - step];
            let got = eng
                .page_candidates()
                .first()
                .map(|c| c.text.as_str())
                .unwrap_or("");
            assert_eq!(got, expected, "backspace parity mismatch at step {step}");
        }
        eng.clear_composition();

        eng.config.pinyin_t9_layout = true;
        let t9_digits = "546842696636947478244326";
        for (i, d) in t9_digits.bytes().enumerate() {
            let (cx, cy) = eng.key_layout.t9_center_of(d);
            eng.push_touch_t9(d, cx, cy, true, 6000 + i as u64 * 50);
        }
        let inc_top1 = eng
            .page_candidates()
            .first()
            .map(|c| c.text.clone())
            .unwrap_or_default();
        assert!(!inc_top1.is_empty());

        let mut cold_eng = JuxuEngine::default();
        cold_eng.load_pinyin_dict(PinyinDictView::from_file(&py_path).unwrap());
        cold_eng.set_aux(AuxView::open_mmap(&aux_path).unwrap());
        cold_eng.config.schema = InputSchema::Pinyin;
        cold_eng.config.pinyin_t9_layout = true;
        for (i, d) in t9_digits.bytes().enumerate() {
            let (cx, cy) = cold_eng.key_layout.t9_center_of(d);
            cold_eng
                .observations
                .push(KeyObs::touch(d, cx, cy, 6000 + i as u64 * 50));
            cold_eng.raw_code.push(d as char);
        }
        cold_eng.rebuild_pinyin_candidates();
        let cold_top1 = cold_eng
            .page_candidates()
            .first()
            .map(|c| c.text.clone())
            .unwrap_or_default();
        assert_eq!(inc_top1, cold_top1);
    }
}
