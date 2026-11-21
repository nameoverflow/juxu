use crate::code::{expand_wildcards, PackedCode};
use crate::config::{FourthEmptyBehavior, ShiftToggleMode, WubiConfig, WubiSchema};
use crate::format::{AuxView, DictView, ENTRY_FLAG_SYMBOL};
use crate::neural::{NeuralContextTracker, NeuralView, RwkvState};
use crate::personalization::Context as PersonalContext;
use crate::pinyin::{
    pinyin_next_key_priors, shuangpin_next_key_priors, PinyinCandidate, PinyinDecoder, PinyinPath,
    PinyinStream,
};
use crate::pinyin_dict::PinyinDictView;
use crate::spaceless::{Decoder, Segment, Stream};
use crate::typo::{
    generate_hypotheses_with_options, softmax_confidences, CorrectionKind, KeyLayout, KeyObs,
};
use crate::user_data::UserStore;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CandidateBadge {
    None = 0,
    /// 该字存在更短简码（如全码输入时提示 ⚡q）
    Shortcode = 1,
    /// 统计 + 触控热区误触纠正候选项
    TypoCorrected = 2,
    /// z 通配符匹配项
    Wildcard = 3,
    /// z 引导拼音反查项
    PinyinLookup = 4,
    /// 用户自造词 / 自动组词词条
    UserPhrase = 5,
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub text: String,
    pub matched_code: PackedCode,
    /// 附加编码或反查说明（如简码 `"q"`、纠错原码 `"trwu"`、拼音反查 `"k · khk (zhōng)"`）
    pub comment: String,
    pub badge: CandidateBadge,
    /// 归一化后验置信度 `0.0 ..= 1.0`
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

pub const KEY_BACKSPACE: u16 = 0x33;
pub const KEY_ESCAPE: u16 = 0x35;
pub const KEY_RETURN: u16 = 0x24;
pub const KEY_ENTER_KP: u16 = 0x4C;
pub const KEY_SPACE: u16 = 0x31;
pub const KEY_TAB: u16 = 0x30;
pub const KEY_UP: u16 = 0x7E;
pub const KEY_DOWN: u16 = 0x7D;
pub const KEY_LEFT: u16 = 0x7B;
pub const KEY_RIGHT: u16 = 0x7C;
pub const KEY_PAGE_UP: u16 = 0x74;
pub const KEY_PAGE_DOWN: u16 = 0x79;

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

pub struct WubiEngine {
    pub dicts: [Option<DictView>; 4],
    pub pinyin_dict: Option<PinyinDictView>,
    pub aux: Option<AuxView>,
    pub config: WubiConfig,
    pub user: UserStore,
    pub key_layout: KeyLayout,
    pub lm_weights: LmWeights,
    pub neural_tracker: NeuralContextTracker,

    pub ascii_mode: bool,
    raw_code: String,
    stream: Stream,
    pinyin_stream: PinyinStream,
    pinyin_candidates: Vec<PinyinCandidate>,
    pinyin_paths: Vec<PinyinPath>,
    committed_stream_codes: VecDeque<String>,
    observations: Vec<KeyObs>,
    candidates: Vec<Candidate>,
    page: usize,
    last_committed: String,
    recent_context: Vec<char>,
    double_quote_open: bool,
    single_quote_open: bool,
    /// When true, the current 4-letter exact match is contextually weak while a missed-space split
    /// is competitive, so suppress blind 4th-code auto-commit and let Space or Key #5 resolve it.
    hold_fourth_for_split: bool,
    /// When set on an empty 4-letter code, `push_code_observation` performs seamless smart push-commit:
    /// commits `(cut_index, commit_text)` and continues composing the remaining tail keys.
    pending_empty_split: Option<(usize, String)>,
    /// Spatial log-likelihood of the top 4-key typo candidate (when `candidates[0]` is a 4-key typo),
    /// carried into Key #5 disambiguation so a typo rescue pays its real spatial cost against `abc | de`.
    top_typo_spatial_logp: Option<f32>,
}

impl Default for WubiEngine {
    fn default() -> Self {
        Self::new(WubiConfig::default())
    }
}

impl WubiEngine {
    pub fn new(config: WubiConfig) -> Self {
        Self {
            dicts: [None, None, None, None],
            pinyin_dict: None,
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
            committed_stream_codes: VecDeque::new(),
            observations: Vec::new(),
            candidates: Vec::new(),
            page: 0,
            last_committed: String::new(),
            recent_context: Vec::with_capacity(8),
            double_quote_open: false,
            single_quote_open: false,
            hold_fourth_for_split: false,
            pending_empty_split: None,
            top_typo_spatial_logp: None,
        }
    }

    /// Explicitly set rolling CJK context (e.g. from surrounding text or offline evaluation).
    pub fn set_recent_context(&mut self, text: &str) {
        self.recent_context.clear();
        for ch in text.chars() {
            if is_cjk_char(ch) {
                self.recent_context.push(ch);
            } else {
                self.recent_context.clear();
            }
        }
        if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
            // Feed up to 64 chars (including cross-clause punctuation for V2 models) into Micro-RWKV-7.
            let chars: Vec<char> = text.chars().collect();
            let neural_ctx: String = chars
                .iter()
                .skip(chars.len().saturating_sub(64))
                .collect();
            self.neural_tracker.set_context_str(&nv, &neural_ctx);
        }
        if self.recent_context.len() > 8 {
            let drain_n = self.recent_context.len() - 8;
            self.recent_context.drain(..drain_n);
        }
        self.last_committed = self.recent_context.iter().collect();
        if self.is_composing() && self.uses_spaceless() {
            self.rebuild_candidates();
        }
    }

    /// Incrementally append committed CJK/punctuation text to rolling context (O(len(text)) instead of O(sentence)).
    pub fn append_recent_context(&mut self, text: &str) {
        for ch in text.chars() {
            if is_cjk_char(ch) {
                self.recent_context.push(ch);
            } else {
                self.recent_context.clear();
            }
        }
        if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
            if !text.is_empty() {
                self.neural_tracker.commit_cjk_str(&nv, text);
            } else {
                self.neural_tracker.reset_to_bos(Some(&nv));
            }
        }
        if self.recent_context.len() > 8 {
            let drain_n = self.recent_context.len() - 8;
            self.recent_context.drain(..drain_n);
        }
        self.last_committed = self.recent_context.iter().collect();
    }

    /// Clear rolling sentence context (e.g. cursor jump or new text field).
    pub fn clear_context(&mut self) {
        self.recent_context.clear();
        self.last_committed.clear();
        let nv = self.aux.as_ref().and_then(|a| a.neural_view());
        self.neural_tracker.reset_to_bos(nv.as_ref());
    }

    /// Record a tapped idle-bar predicted word: updates personal word/char n-grams and advances
    /// rolling context immediately so chained predictions refresh without waiting on host text proxy lag.
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
        );
        self.last_committed = text.to_string();
        for ch in text.chars() {
            if is_cjk_char(ch) {
                self.recent_context.push(ch);
            } else {
                self.recent_context.clear();
            }
        }
        if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
            if !text.is_empty() {
                self.neural_tracker.commit_cjk_str(&nv, text);
            } else {
                self.neural_tracker.reset_to_bos(Some(&nv));
            }
        }
        if self.recent_context.len() > 8 {
            let drain_n = self.recent_context.len() - 8;
            self.recent_context.drain(..drain_n);
        }
    }

    /// Lightweight idle next-word / next-char prediction reusing existing `WBAUX002` char N-grams,
    /// active dictionary phrase verification, and personal `UserStore` word bigrams (zero extra dict bytes).
    pub fn predict_next_words(&self, max_count: usize) -> Vec<String> {
        const STARTERS: [&str; 10] = [
            "我", "我们", "不是", "怎么", "可以", "这个", "其实", "但是", "今天", "现在",
        ];
        let limit = max_count.clamp(1, 16);
        if self.recent_context.is_empty() {
            return STARTERS
                .iter()
                .take(limit)
                .map(|s| (*s).to_string())
                .collect();
        }

        let p1 = *self.recent_context.last().unwrap();
        let p2 = if self.recent_context.len() >= 2 {
            Some(self.recent_context[self.recent_context.len() - 2])
        } else {
            None
        };
        let prev2_str: Option<String> = p2.map(|a| {
            let mut s = String::with_capacity(8);
            s.push(a);
            s.push(p1);
            s
        });

        struct PredCand {
            text: String,
            first_ch: char,
            score: f32,
            is_single: bool,
            is_completion: bool,
            is_personal: bool,
        }

        let mut pool = Vec::<PredCand>::with_capacity(64);

        // 1. Personal word bigrams from user's own typing history
        for (w, sc) in self
            .user
            .next_words_after(&self.recent_context, &self.last_committed)
        {
            if let Some(first_ch) = w.chars().next() {
                if prev2_str.as_deref() == Some(w.as_str()) {
                    continue;
                }
                pool.push(PredCand {
                    is_single: w.chars().count() == 1,
                    text: w,
                    first_ch,
                    score: sc,
                    is_completion: false,
                    is_personal: true,
                });
            }
        }

        // 2. Corpus char N-gram + dictionary-validated 2-char word expansion
        if let Some(ref aux) = self.aux {
            let dict = self.active_dict();
            // Gather candidate first characters `c1` after `(p2, p1)`
            let mut first_map = HashMap::<char, (f32, f32, f32)>::with_capacity(128);
            for (c1, bi) in aux.bigrams_after(p1) {
                if !is_cjk_char(c1) {
                    continue;
                }
                let uni = aux.unigram_prior(c1).unwrap_or(0.0);
                first_map.insert(c1, (uni, bi, 0.0));
            }
            if let Some(prev2) = p2 {
                for (c1, tri) in aux.trigrams_after(prev2, p1) {
                    if !is_cjk_char(c1) {
                        continue;
                    }
                    let entry = first_map.entry(c1).or_insert_with(|| {
                        (
                            aux.unigram_prior(c1).unwrap_or(0.0),
                            aux.bigram_boost(p1, c1),
                            0.0,
                        )
                    });
                    if tri > entry.2 {
                        entry.2 = tri;
                    }
                }
            }

            let nv_opt = aux
                .neural_view()
                .filter(|_| self.neural_tracker.is_ready());
            if let Some(ref nv) = nv_opt {
                for (c1, _lp) in nv.top_next_chars(&self.neural_tracker.active, 24) {
                    if is_cjk_char(c1) {
                        first_map.entry(c1).or_insert_with(|| {
                            (
                                aux.unigram_prior(c1).unwrap_or(5.0),
                                aux.bigram_boost(p1, c1),
                                p2.map(|prev2| aux.trigram_boost(prev2, p1, c1))
                                    .unwrap_or(0.0),
                            )
                        });
                    }
                }
            }

            // Check if `(p2, p1)` itself is a cohesive 2-char word (e.g. 情绪, 什么, 今天, 这个)
            let prev2_is_word = prev2_str
                .as_deref()
                .and_then(|s| dict.and_then(|d| d.contains_exact_word(s)))
                .is_some();

            let mut first_ranked: Vec<(char, f32, f32, f32, f32, Option<f32>)> = first_map
                .into_iter()
                .filter_map(|(c1, (uni, bi, tri))| {
                    let rwkv_lp1 = nv_opt
                        .as_ref()
                        .map(|nv| nv.score_next_char(&self.neural_tracker.active, c1));
                    if uni < 4.0 && tri < 1.0 && !rwkv_lp1.map(|lp| lp > -4.5).unwrap_or(false) {
                        return None;
                    }
                    // Avoid repeating same char unless strong trigram / neural (e.g. 哈哈)
                    if c1 == p1
                        && tri < 1.5
                        && bi < 3.5
                        && !rwkv_lp1.map(|lp| lp > -1.8).unwrap_or(false)
                    {
                        return None;
                    }
                    // When `(p2, p1)` is a 2-char word (like 情绪), discount `p1->c1` pairs that have
                    // zero trigram AND weak neural support (avoids homograph bleed like 光绪->年间).
                    let context_discount = if prev2_is_word && tri == 0.0 {
                        if rwkv_lp1.map(|lp| lp > -4.8).unwrap_or(false) {
                            0.15
                        } else {
                            1.45
                        }
                    } else {
                        0.0
                    };
                    let rwkv_boost = rwkv_lp1
                        .map(|lp| (lp + 6.0).clamp(-2.5, 5.5) * 0.90)
                        .unwrap_or(0.0);
                    // Conditional log-prob proxy: log P(c1 | clause) ≈ N-gram + Micro-RWKV-7 state
                    let cond = 1.00 * uni + 0.78 * bi + 1.55 * tri + rwkv_boost - context_discount;
                    Some((c1, cond, uni, bi, tri, rwkv_lp1))
                })
                .collect();
            first_ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
            first_ranked.truncate(18);

            let mut strong_phrase_for_c1 = HashMap::<char, f32>::new();

            for &(c1, cond1, uni1, bi1, tri1, rwkv_lp1) in first_ranked.iter().take(14) {
                // Determine whether `c1` completes the current in-progress 2-char or 3-char word
                // (e.g. 不知->道, 小红->书, 大模->型, 没问->题, 性价->比, 绷不->住, 输入->法)
                let mut is_tri_comp = false;
                let mut is_bi_comp = false;
                let mut comp_bonus = 0.0f32;
                let strong_rwkv_comp = rwkv_lp1.map(|lp| lp > -1.75).unwrap_or(false);
                if let Some(prev2) = p2 {
                    let mut s3 = String::with_capacity(12);
                    s3.push(prev2);
                    s3.push(p1);
                    s3.push(c1);
                    let in_dict_3 = dict.and_then(|d| d.contains_exact_word(&s3)).is_some();
                    if in_dict_3 || strong_rwkv_comp || (tri1 >= 0.35 && bi1 >= 1.0) {
                        is_tri_comp = true;
                        comp_bonus = if in_dict_3 || strong_rwkv_comp {
                            3.45
                        } else {
                            (1.30 + 0.70 * tri1).min(2.90)
                        };
                    }
                } else if let Some(d) = dict {
                    let mut s2 = String::with_capacity(8);
                    s2.push(p1);
                    s2.push(c1);
                    if d.contains_exact_word(&s2).is_some() && (bi1 >= 2.6 || strong_rwkv_comp) {
                        is_bi_comp = true;
                        comp_bonus = 1.95;
                    }
                }
                let is_comp = is_tri_comp || is_bi_comp;

                // Single-char candidate
                pool.push(PredCand {
                    text: c1.to_string(),
                    first_ch: c1,
                    score: cond1 + comp_bonus,
                    is_single: true,
                    is_completion: is_comp,
                    is_personal: false,
                });

                // If `c1` strongly completes a 3-char word `(p2, p1, c1)` (like 不知->道, 小红->书,
                // 性价->比, 绷不->住), do NOT glue `c1` onto a new word (`道德`/`书记`/`比较`/`住房`)!
                if is_tri_comp && (tri1 >= 0.35 || comp_bonus >= 3.0) {
                    continue;
                }

                // Expand c1 -> c2 (only high-confidence real 2-char Chinese words)
                let mut sec_map = HashMap::<char, (f32, f32)>::with_capacity(48);
                for (c2, bi12) in aux.bigrams_after(c1) {
                    if !is_cjk_char(c2) || bi12 < 1.6 {
                        continue;
                    }
                    sec_map.insert(c2, (bi12, 0.0));
                }
                for (c2, tri12) in aux.trigrams_after(p1, c1) {
                    if !is_cjk_char(c2) {
                        continue;
                    }
                    let e = sec_map
                        .entry(c2)
                        .or_insert_with(|| (aux.bigram_boost(c1, c2), 0.0));
                    if tri12 > e.1 {
                        e.1 = tri12;
                    }
                }

                let mut sec_ranked: Vec<(char, f32, f32, f32, f32)> = sec_map
                    .into_iter()
                    .filter_map(|(c2, (bi12, tri12))| {
                        let uni2 = aux.unigram_prior(c2).unwrap_or(0.0);
                        if uni2 < 4.5 && tri12 < 1.5 {
                            return None;
                        }
                        if bi12 < 1.6 && tri12 < 1.3 {
                            return None;
                        }
                        let raw = 0.40 * bi12 + 0.70 * tri12 + 0.32 * uni2;
                        Some((c2, raw, uni2, bi12, tri12))
                    })
                    .collect();
                sec_ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));

                let mut emitted_for_c1 = 0usize;
                for (c2, raw2, _uni2, bi12, tri12) in sec_ranked.into_iter().take(8) {
                    let mut w2 = String::with_capacity(8);
                    w2.push(c1);
                    w2.push(c2);
                    // Never repeat the exact 2-char word just typed (e.g. 我们 -> 我们)
                    if prev2_str.as_deref() == Some(w2.as_str()) {
                        continue;
                    }
                    // Skip pure Chinese numeral pairs (二十/三十/十五 from historical dates) unless context is numeric
                    if is_numeral_pair(c1, c2) && !is_numeral_char(p1) {
                        continue;
                    }
                    let dict_wt = dict.and_then(|d| d.contains_exact_word(&w2));
                    let Some(wt) = dict_wt else {
                        // Allow modern non-dict 2-char slang ONLY with very strong trigram + bigram support
                        if tri12 >= 1.6 && bi12 >= 2.8 {
                            let sc2 = cond1 + raw2 - 3.10;
                            pool.push(PredCand {
                                text: w2,
                                first_ch: c1,
                                score: sc2,
                                is_single: false,
                                is_completion: false,
                                is_personal: false,
                            });
                            emitted_for_c1 += 1;
                            if emitted_for_c1 >= 2 {
                                break;
                            }
                        }
                        continue;
                    };
                    // Require genuinely common dictionary words (avoid obscure 0-weight compound entries)
                    if wt < 50_000 && tri12 < 1.0 {
                        continue;
                    }
                    let rwkv_w2 = nv_opt
                        .as_ref()
                        .map(|nv| nv.score_phrase_fast(&self.neural_tracker.active, &w2));
                    // If `c1` is a standalone common char (会/是/有/后/分) and `(p1,c1,c2)` has ZERO
                    // trigram evidence and weak neural phrase support, don't hallucinate unrelated compounds!
                    if is_standalone_common_char(c1)
                        && tri12 == 0.0
                        && !rwkv_w2.map(|lp| lp > -4.2).unwrap_or(false)
                    {
                        continue;
                    }

                    // Check if `p1+c1+c2` forms a 3-char word (e.g. `不` -> `知道`, `小` -> `红书`, `大` -> `模型`)
                    let mut comp3_bonus = 0.0f32;
                    if let Some(d) = dict {
                        let mut s3 = String::with_capacity(12);
                        s3.push(p1);
                        s3.push(c1);
                        s3.push(c2);
                        if d.contains_exact_word(&s3).is_some() {
                            comp3_bonus = 1.40;
                        }
                    }

                    let tri_ground_bonus = if tri12 > 0.0 {
                        0.85
                    } else if rwkv_w2.map(|lp| lp > -4.8).unwrap_or(false) {
                        0.35
                    } else {
                        -0.65
                    };
                    let rwkv_w2_boost = rwkv_w2
                        .map(|lp| (lp + 5.8).clamp(-1.8, 4.5) * 0.85)
                        .unwrap_or(0.0);
                    let freq_bonus = (((wt as f32) + 100.0).ln() - 11.0).clamp(-1.0, 2.2) * 0.40;
                    let split_penalty = if is_bi_comp && comp3_bonus == 0.0 && tri12 < 0.8 {
                        1.25
                    } else {
                        0.0
                    };
                    let sc2 = cond1 + raw2 + tri_ground_bonus + rwkv_w2_boost + freq_bonus + comp3_bonus - 3.10 - split_penalty;
                    if bi12 >= 3.4 && sc2 > cond1 {
                        let entry = strong_phrase_for_c1.entry(c1).or_insert(f32::NEG_INFINITY);
                        if sc2 > *entry {
                            *entry = sc2;
                        }
                    }
                    pool.push(PredCand {
                        text: w2,
                        first_ch: c1,
                        score: sc2,
                        is_single: false,
                        is_completion: comp3_bonus > 0.0,
                        is_personal: false,
                    });
                    emitted_for_c1 += 1;
                    if emitted_for_c1 >= 3 {
                        break;
                    }
                }
                let _ = uni1;
            }

            // Penalize bare orphan bound-morpheme chars (e.g. `觉` when `觉得` beat it strongly)
            for item in &mut pool {
                if item.is_single && !item.is_completion && !item.is_personal {
                    if let Some(&best_w2) = strong_phrase_for_c1.get(&item.first_ch) {
                        if best_w2 > item.score + 0.25 && !is_standalone_common_char(item.first_ch) {
                            item.score -= 3.2;
                        }
                    }
                }
            }
        }

        pool.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });

        let mut result = Vec::<String>::with_capacity(limit);
        let mut seen = HashSet::<String>::new();
        let mut per_first = HashMap::<char, usize>::new();

        for cand in pool {
            if seen.contains(&cand.text) {
                continue;
            }
            let fc = per_first.get(&cand.first_ch).copied().unwrap_or(0);
            if fc >= 2 && !cand.is_personal {
                continue;
            }
            seen.insert(cand.text.clone());
            *per_first.entry(cand.first_ch).or_insert(0) += 1;
            result.push(cand.text);
            if result.len() >= limit {
                break;
            }
        }

        if result.len() < limit {
            for &s in &STARTERS {
                if seen.insert(s.to_string()) {
                    result.push(s.to_string());
                    if result.len() >= limit {
                        break;
                    }
                }
            }
        }
        result
    }

    pub fn load_dict(&mut self, schema: WubiSchema, dict: DictView) {
        let slot = schema.slot_index();
        if slot >= self.dicts.len() {
            return;
        }
        self.dicts[slot] = Some(dict);
        if self.config.schema == schema {
            if let Some(ref d) = self.dicts[slot] {
                self.user.reindex_phrases(d);
            }
            if !self.raw_code.is_empty() {
                self.rebuild_candidates();
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

    pub fn set_aux(&mut self, aux: AuxView) {
        if let Some(nv) = aux.neural_view() {
            self.neural_tracker.init_with_model(&nv);
            self.lm_weights = LmWeights::default_neural();
        } else {
            self.neural_tracker = NeuralContextTracker::default();
            self.lm_weights = LmWeights::default_ngram();
        }
        self.aux = Some(aux);
        if !self.raw_code.is_empty() {
            self.rebuild_candidates();
        }
    }

    pub fn active_dict(&self) -> Option<&DictView> {
        let slot = self.config.schema.slot_index();
        self.dicts.get(slot).and_then(|d| d.as_ref())
    }

    pub fn set_config(&mut self, new_config: WubiConfig) {
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

    pub fn set_schema(&mut self, schema: WubiSchema) {
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
            let committed: Vec<&str> = self.committed_stream_codes.iter().map(String::as_str).collect();
            format!("{} │ {}", committed.join("·"), pending)
        }
    }

    #[inline]
    fn is_pinyin_mode(&self) -> bool {
        self.config.schema.is_pinyin() && self.pinyin_dict.is_some()
    }

    fn uses_spaceless(&self) -> bool {
        !self.config.schema.is_pinyin()
            && self.config.spaceless
            && self.active_dict().is_some()
            && self.aux.as_ref().is_some_and(AuxView::has_neural)
            && !(self.config.schema.is_wubi()
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
        self.committed_stream_codes.clear();
        self.observations.clear();
        self.candidates.clear();
        self.page = 0;
        self.hold_fourth_for_split = false;
        self.pending_empty_split = None;
        self.top_typo_spatial_logp = None;
    }

    /// Toggle between Chinese Wubi mode and English ASCII mode (e.g. Shift tap).
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

    /// Explicitly select candidate at index `rel_idx` (`0..page_size`) on the current page.
    pub fn select_page_candidate(&mut self, rel_idx: usize, timestamp_ms: u64) -> Option<EngineOutput> {
        let ps = (self.config.page_size as usize).max(1);
        let abs_idx = self.page * ps + rel_idx;
        self.select_abs_candidate(abs_idx, timestamp_ms)
    }

    pub fn select_abs_candidate(&mut self, abs_idx: usize, timestamp_ms: u64) -> Option<EngineOutput> {
        self.commit_candidate(abs_idx, timestamp_ms, true)
    }

    fn commit_candidate(
        &mut self,
        abs_idx: usize,
        timestamp_ms: u64,
        explicit: bool,
    ) -> Option<EngineOutput> {
        if self.is_pinyin_mode() {
            let pc = self.pinyin_candidates.get(abs_idx)?;
            let commit_text = pc.candidate.text.clone();
            let consumed = pc.consumed_keys.min(self.observations.len());
            let pc_boundaries = pc.boundaries.clone();
            let pc_is_exact = pc.candidate.is_exact;
            let multi_segment = pc_boundaries.len() >= 2;
            let learn = self.config.dynamic_word_freq && (explicit || pc_is_exact);
            let first_choice = (learn && explicit && abs_idx > 0)
                .then(|| self.candidates.first().map(|c| c.text.clone()))
                .flatten();
            // Compute RWKV surprisal BEFORE advancing neural_tracker so auto-phrase can gate on it
            let surprisal = self.compute_rwkv_surprisal(&commit_text);
            let mut notice = String::new();
            if learn {
                self.learn_continuation(&commit_text, explicit, first_choice.as_deref());
                let prev = self.last_committed.clone();
                let dict_idx = self.config.schema.slot_index();
                let dict_opt = self.dicts.get(dict_idx).and_then(|d| d.as_ref());
                let learned = self.user.record_commit_with_surprisal(
                    &prev,
                    &commit_text,
                    timestamp_ms,
                    self.config.auto_phrase,
                    self.config.auto_phrase_threshold,
                    dict_opt,
                    self.pinyin_dict.as_ref(),
                    surprisal,
                );
                if let Some(new_word) = learned {
                    notice = format!("自动成词：{}", new_word);
                } else if multi_segment {
                    // User assembled a multi-segment phrase in Pinyin; promote if surprising to RWKV
                    if let Some(new_word) = self.user.record_assembled_phrase(
                        &commit_text,
                        surprisal,
                        self.config.auto_phrase,
                        self.config.auto_phrase_threshold,
                        dict_opt,
                        self.pinyin_dict.as_ref(),
                    ) {
                        notice = format!("自动成词：{}", new_word);
                    }
                }
                // Also record individual CJK characters of a committed word so subsequent single-char lookup (e.g. ju -> 橘 after 橘子) boosts them!
                if commit_text.chars().count() >= 2 {
                    for ch in commit_text.chars() {
                        let s = ch.to_string();
                        let _ = self.user.record_commit("", &s, timestamp_ms, false, 2, None);
                    }
                }
            }
            self.last_committed = commit_text.clone();
            for ch in commit_text.chars() {
                if is_cjk_char(ch) {
                    self.recent_context.push(ch);
                } else {
                    self.recent_context.clear();
                }
            }
            if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
                if !commit_text.is_empty() {
                    self.neural_tracker.commit_cjk_str(&nv, &commit_text);
                } else {
                    self.neural_tracker.reset_to_bos(Some(&nv));
                }
            }
            if self.recent_context.len() > 8 {
                let drain_n = self.recent_context.len() - 8;
                self.recent_context.drain(..drain_n);
            }
            if self.config.typo.enabled && self.config.typo.touch_hitbox_adapt {
                if !pc_boundaries.is_empty() {
                    // Learn touch offsets against the *corrected* syllable letters (not raw mis-tapped keys!)
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
                                for (obs, &target_key) in seg_obs.iter().zip(target_letters.iter()) {
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
        let learn = self.config.dynamic_word_freq && (explicit || cand.is_exact);
        let first_choice =
            (learn && explicit && abs_idx > 0).then(|| self.candidates[0].text.clone());
        let mut notice = String::new();
        let commit_text = if self.uses_spaceless() {
            let segments = if cand.matched_code.is_empty() {
                let path = self.stream.paths().iter().find(|path| path.text == cand.text)?;
                path.segments(self.raw_code.len())
            } else {
                let start = self.raw_code.len() - cand.matched_code.len();
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
            if learn {
                let selected: String =
                    segments.iter().map(|segment| segment.text.as_str()).collect();
                self.learn_continuation(&selected, explicit, first_choice.as_deref());
            }
            self.finalize_stream_segments(segments, true, timestamp_ms, &mut notice)
        } else {
            if learn {
                self.learn_continuation(&cand.text, explicit, first_choice.as_deref());
            }
            self.finalize_commit(&cand.text, Some(cand.matched_code), timestamp_ms, &mut notice);
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

    /// 26-bit mask (`bit i` = `'a' + i`) of letters valid as the next Wubi keystroke.
    /// Used by iOS touch hit-testing to dynamically expand valid next-key boundaries.
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
        if bytes.is_empty()
            || bytes.len() >= 4
            || (self.config.schema.is_wubi() && bytes.contains(&b'z'))
        {
            return ALL_26;
        }
        let mut mask = self.user.valid_next_letters_mask(bytes);
        if let Some(dict) = self.active_dict() {
            mask |= dict.valid_next_letters_mask(bytes, self.config.enable_rare_charset);
            // Soften `validNextMask` when an earlier touch landed near a key boundary:
            // union valid continuations of top TouchBoundary prefix hypotheses so a Key #1 slip
            // does not mark the true Key #2 as a dead key!
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
        // If current prefix has no exact continuations at all (e.g. already a typo), stay neutral.
        if mask == 0 {
            return ALL_26;
        }
        if self.config.schema.is_wubi() && self.config.z_omnipotent {
            mask |= 1u32 << 25;
        }
        mask
    }

    /// Compute normalized 26-key probability mass (`0.0..=1.0`) for the next syllable's initial
    /// letter from `Micro-RWKV-7`'s top-32 predicted characters.
    fn rwkv_pinyin_start_priors(&self, pdict: &PinyinDictView) -> Option<[f32; 26]> {
        let nv = self.aux.as_ref()?.neural_view()?;
        if !self.neural_tracker.is_ready() {
            return None;
        }
        let state = self
            .pinyin_paths
            .first()
            .map(|p| &p.state)
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
            let cnt = mask.count_ones() as f32;
            let share = p / cnt;
            for idx in 0..26usize {
                if (mask & (1u32 << idx)) != 0 {
                    mass[idx] += share;
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

    /// Compute 26-key contextual priors (`54..=180`) for any shape/phonetic-shape schema
    /// (Wubi98/06/Xiaohe/spaceless) from `Micro-RWKV-7`'s `top_next_chars`.
    fn rwkv_dict_first_key_priors(&self, state: &RwkvState, dict: &DictView) -> Option<[u8; 26]> {
        let nv = self.aux.as_ref()?.neural_view()?;
        if !self.neural_tracker.is_ready() {
            return None;
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
            for idx in 0..26usize {
                if (mask & (1u32 << idx)) != 0 {
                    mass[idx] += share;
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

    /// 26-element continuous touch hitbox weight vector (`0` = dead key, `100` = neutral valid key,
    /// `54..=180` = `Micro-RWKV-7` context-weighted valid key).
    /// Powers Key #1 predictive gutter bias AND Keys #2..#4 valid-vs-valid gutter disambiguation
    /// across Wubi86/98/06, Xiaohe, Spaceless, and Full Pinyin!
    pub fn next_key_priors(&self) -> [u8; 26] {
        let mut out = [100u8; 26];
        if self.ascii_mode || !self.config.typo.enabled {
            return out;
        }
        if self.is_pinyin_mode() {
            if let Some(ref pdict) = self.pinyin_dict {
                let rwkv_starts = self.rwkv_pinyin_start_priors(pdict);
                let (_, priors) = if self.config.schema.is_shuangpin() {
                    shuangpin_next_key_priors(&self.raw_code, pdict, rwkv_starts.as_ref())
                } else {
                    pinyin_next_key_priors(&self.raw_code, pdict, rwkv_starts.as_ref())
                };
                return priors;
            }
            return out;
        }
        if self.uses_spaceless() {
            if let Some(dict) = self.active_dict() {
                let state = self
                    .stream
                    .best_state()
                    .unwrap_or(&self.neural_tracker.active);
                if let Some(priors) = self.rwkv_dict_first_key_priors(state, dict) {
                    return priors;
                }
            }
            return out;
        }
        let bytes = self.raw_code.as_bytes();
        if bytes.len() >= 4 || (self.config.schema.is_wubi() && bytes.contains(&b'z')) {
            return out;
        }

        // Case 1: Key #1 (`raw_code` is empty) — use `Micro-RWKV-7` first-key priors for ALL schemas!
        if bytes.is_empty() {
            if self.neural_tracker.is_ready() {
                if self.config.schema == WubiSchema::Wubi86 {
                    // Uniform log-prob across 25 Wubi keys ('a'..'y') is ln(1/25) = -3.2189
                    for idx in 0..25usize {
                        let lp = self.neural_tracker.active.key1_logp[idx];
                        let delta = lp - (-3.2189f32);
                        out[idx] = (100.0 + 20.0 * delta).round().clamp(72.0, 168.0) as u8;
                    }
                } else if let Some(dict) = self.active_dict() {
                    if let Some(priors) =
                        self.rwkv_dict_first_key_priors(&self.neural_tracker.active, dict)
                    {
                        return priors;
                    }
                }
            }
            return out;
        }

        // Case 2: Keys #2..#4 (`1 <= len <= 3`) — combine dictionary validity with RWKV context priors!
        let mask = self.valid_next_mask();
        if mask == 0x03FF_FFFF || mask == 0 {
            return out;
        }
        for idx in 0..26usize {
            if (mask & (1u32 << idx)) == 0 {
                out[idx] = 0;
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
        out
    }

    /// Push a touchscreen letter tap with view/normalized coordinates `(x, y)` (used by iOS keyboard & touch tests).
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
            // View-space UI hit-testing (`IOSKeyboardLayoutEngine.hitTest`) already resolved
            // rectangular keycaps, side margins, ergonomic thumb Y-bias, and `valid_next_mask` —
            // and showed the callout bubble for `reported_lower`. Always keep `reported_lower` as
            // the primary observation while still storing exact `(nx, ny)` in `KeyObs::touch` for
            // 2D Gaussian typo rescue (`generate_hypotheses`), so outer-margin/corner taps (e.g.
            // beside `z` or `m`) never silently mutate away from what the callout displayed.
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

    /// Unified macOS / physical keyboard event entry point.
    pub fn handle_key_event(
        &mut self,
        keycode: u16,
        ch: Option<char>,
        has_cmd_or_ctrl: bool,
        timestamp_ms: u64,
    ) -> EngineOutput {
        // Shortcut: Ctrl + '=' manually registers last 2..4 committed chars as a custom phrase!
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

        // Handle special navigation / editing keys when composing
        match keycode {
            KEY_BACKSPACE => {
                if self.raw_code.is_empty() {
                    self.user.break_single_burst();
                    self.recent_context.clear();
                    let nv = self.aux.as_ref().and_then(|a| a.neural_view());
                    self.neural_tracker.pop_commit_or_reset(nv.as_ref());
                    return EngineOutput::default();
                }
                self.raw_code.pop();
                self.observations.pop();
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
                let raw = self.raw_code.clone();
                self.clear_composition();
                self.user.break_single_burst();
                self.clear_context();
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
                // Empty code on Space: clear or bell
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
            _ => {}
        }

        let Some(c) = ch else {
            return EngineOutput::default();
        };

        // Lowercase letter 'a'..='z': core composition!
        if c.is_ascii_lowercase() {
            let obs = KeyObs::physical(c as u8, timestamp_ms);
            return self.push_code_observation(obs);
        }
        // Apostrophe syllable separator in Full Pinyin (e.g. xi'an)
        if c == '\'' && self.is_pinyin_mode() && !self.raw_code.is_empty() {
            let obs = KeyObs::physical(b'\'', timestamp_ms);
            return self.push_code_observation(obs);
        }

        // Uppercase letter 'A'..='Z': commit current candidate #1 (if composing) and pass/commit uppercase
        if c.is_ascii_uppercase() {
            if self.raw_code.is_empty() {
                self.user.break_single_burst();
                self.clear_context();
                return EngineOutput::default();
            }
            let mut out = if self.uses_spaceless() {
                self.finish_spaceless(timestamp_ms)
            } else {
                self.commit_candidate(0, timestamp_ms, false)
                    .unwrap_or_else(|| {
                        self.clear_composition();
                        EngineOutput::default()
                    })
            };
            self.user.break_single_burst();
            self.clear_context();
            out.consumed = true;
            out.commit_text.push(c);
            return out;
        }

        // While composing: digits 1..=9, paging `-`/`=`/`[`/`]`, quick `;`/`'` select
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
            if (c == '-' || c == '[') && !self.uses_spaceless() {
                self.page_prev();
                return EngineOutput {
                    consumed: true,
                    preedit: self.raw_code.clone(),
                    ..Default::default()
                };
            }
            if (c == '=' || c == ']') && !self.uses_spaceless() {
                self.page_next();
                return EngineOutput {
                    consumed: true,
                    preedit: self.raw_code.clone(),
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

        // Chinese smart punctuation (commits active candidate #1 first if composing!)
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
        if self.is_pinyin_mode() {
            self.raw_code.push(obs.ch as char);
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
            if obs.ch != b'z' || !self.config.z_omnipotent || !self.config.schema.is_wubi() {
                return self.push_spaceless(obs);
            }
            if self.is_composing() {
                let out = self.finish_spaceless(obs.timestamp_ms);
                commit_prefix = out.commit_text;
                notice = out.notice;
            }
        }

        let is_z_pinyin_mode = self.config.schema.is_wubi()
            && self.config.z_omnipotent
            && self.raw_code.starts_with('z')
            && !self.raw_code.starts_with("zz");
        let max_len = if is_z_pinyin_mode { 7 } else { 4 };

        // Fifth-code push-commit (第五码自动顶字上屏) + LM Missed-Space Key-#5 Disambiguation!
        let mut resolved_via_fifth_split = false;
        if self.raw_code.len() >= max_len {
            if !is_z_pinyin_mode && self.config.fifth_code_push_commit {
                if self.raw_code.len() == 4
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
                    preedit: self.raw_code.clone(),
                    bell: true,
                    ..Default::default()
                };
            }
        }

        if !resolved_via_fifth_split {
            self.raw_code.push(obs.ch as char);
            self.observations.push(obs);
            self.page = 0;
            self.rebuild_candidates();
        }

        // Check 4th-code behaviors (ONLY for standard 4-letter Wubi codes, not z+pinyin)!
        let now_z_pinyin = self.config.z_omnipotent
            && self.raw_code.starts_with('z')
            && !self.raw_code.starts_with("zz");

        if !now_z_pinyin && self.raw_code.len() == 4 {
            // Seamless smart push-commit for high-confidence missed-space on an empty 4-letter code!
            if self.config.fifth_code_push_commit {
                if let Some((cut, c1_text)) = self.pending_empty_split.take() {
                    if cut >= 1 && cut < self.raw_code.len() && self.observations.len() == self.raw_code.len() {
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
                            preedit: self.raw_code.clone(),
                            bell: false,
                            notice,
                        };
                    }
                }
            }

            if self.candidates.is_empty() {
                // Truly empty 4-letter code (even typo rescue & missed-space found no match)
                match self.config.fourth_empty_behavior {
                    FourthEmptyBehavior::Block => {
                        self.raw_code.pop();
                        self.observations.pop();
                        self.rebuild_candidates();
                        return EngineOutput {
                            consumed: true,
                            commit_text: commit_prefix,
                            preedit: self.raw_code.clone(),
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
            preedit: self.raw_code.clone(),
            bell: false,
            notice,
        }
    }

    fn should_auto_commit_fourth(&self) -> bool {
        if self.raw_code.contains('z') || self.hold_fourth_for_split {
            return false;
        }
        // Count exact candidates vs typo-corrected candidates
        let exact_cands: Vec<&Candidate> = self.candidates.iter().filter(|c| c.is_exact).collect();
        if exact_cands.len() == 1 {
            // Only auto-commit when #1 IS that unique exact candidate!
            if self.candidates[0].is_exact {
                return true;
            }
        }
        // If #1 is a typo-corrected candidate, respect `suppress_auto_commit_on_correction`!
        if !self.candidates[0].is_exact && !self.config.typo.suppress_auto_commit_on_correction {
            return self.candidates.len() == 1 || self.candidates[0].confidence >= 0.94;
        }
        false
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
        self.candidates = paths
            .iter()
            .zip(softmax_confidences(&scores))
            .map(|(path, confidence)| {
                let comment = path.correction_hint(self.raw_code.len());
                let is_exact = comment.is_empty();
                Candidate {
                    text: path.text.clone(),
                    matched_code: PackedCode::EMPTY,
                    comment,
                    badge: if is_exact { CandidateBadge::None } else { CandidateBadge::TypoCorrected },
                    confidence,
                    is_exact,
                }
            })
            .collect();
        let Some(dict) = self.dicts[self.config.schema.slot_index()].as_ref() else {
            return;
        };
        // Retain exact single-character choices independently of the sentence beam.
        // For longer input, these replace only the last segment of the leading path.
        let lookup_singles = |code| {
            dict.lookup_exact(code, self.config.enable_rare_charset)
                .into_iter()
                .filter(|hit| hit.is_single_char() && hit.flags & ENTRY_FLAG_SYMBOL == 0)
                .collect::<Vec<_>>()
        };
        let exact = PackedCode::from_ascii(&self.raw_code)
            .map(|code| (code, lookup_singles(code)))
            .filter(|(_, hits)| !hits.is_empty());
        let Some((code, hits)) = exact.or_else(|| {
            let start = paths.first()?.tail_start();
            if start == 0 {
                return None;
            }
            let code = PackedCode::from_ascii(&self.raw_code[start..])?;
            Some((code, lookup_singles(code)))
        }) else {
            return;
        };
        let start = self.raw_code.len() - code.len();
        let comment = paths.first().map_or_else(String::new, |path| path.correction_hint(start));
        let is_exact = comment.is_empty();
        let mut singles: Vec<_> = hits.into_iter()
            .filter(|hit| self.candidates.first().is_none_or(|first| first.text != hit.text))
            .map(|hit| Candidate {
                text: hit.text.to_owned(),
                matched_code: code,
                comment: comment.clone(),
                badge: if is_exact { CandidateBadge::None } else { CandidateBadge::TypoCorrected },
                confidence: 0.0,
                is_exact,
            })
            .collect();
        let lead = usize::from(!self.candidates.is_empty());
        let mut alternatives = self.candidates.split_off(lead);
        alternatives.retain(|candidate| !singles.iter().any(|single| single.text == candidate.text));
        let visible = singles.len().min(2);
        self.candidates.extend(singles.drain(..visible));
        self.candidates.extend(alternatives);
        self.candidates.extend(singles);
    }

    fn push_spaceless(&mut self, obs: KeyObs) -> EngineOutput {
        self.raw_code.push(obs.ch as char);
        self.observations.push(obs);
        self.page = 0;
        self.update_spaceless();
        let dict = self.dicts[self.config.schema.slot_index()].as_ref().unwrap();
        let stable = self.stream.take_stable_prefix(
            &self.raw_code,
            dict,
            &self.user,
            &self.config,
        );
        let mut out = EngineOutput {
            consumed: true,
            ..Default::default()
        };
        if let Some(prefix) = stable {
            out.commit_text = self.finalize_stream_segments(
                prefix.segments,
                false,
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
        timestamp_ms: u64,
        notice: &mut String,
    ) -> String {
        let mut text = String::new();
        let mut start = 0;
        for segment in segments {
            let end = start + segment.code.len();
            if !confirmed_by_user && segment.correction == CorrectionKind::Exact {
                self.learn_continuation(&segment.text, false, None);
            }
            if segment.correction == CorrectionKind::Exact
                || (confirmed_by_user && segment.correction != CorrectionKind::Transposition)
            {
                self.learn_touch_offsets(segment.code, start..end);
            }
            self.finalize_commit(&segment.text, None, timestamp_ms, notice);
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

    fn finalize_automatic_candidate(
        &mut self,
        candidate: &Candidate,
        timestamp_ms: u64,
        notice: &mut String,
    ) {
        if candidate.is_exact {
            self.learn_continuation(&candidate.text, false, None);
        }
        let code = (!candidate.matched_code.is_empty()).then_some(candidate.matched_code);
        self.finalize_commit(&candidate.text, code, timestamp_ms, notice);
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
                self.user.touch_profile.observe(key, obs.norm_x - cx, obs.norm_y - cy);
                self.user.dirty = true;
            }
        }
    }

    /// Compute mean per-character RWKV surprisal (`-log P_rwkv(c_i | ctx)`) before advancing context.
    fn compute_rwkv_surprisal(&self, text: &str) -> Option<f32> {
        let nv = self.aux.as_ref()?.neural_view()?;
        if !self.neural_tracker.is_ready() {
            return None;
        }
        let mut chars = text.chars().filter(|&c| is_cjk_char(c));
        let c1 = chars.next()?;
        let lp1 = nv.score_next_char(&self.neural_tracker.active, c1).clamp(-13.5, -0.01);
        let Some(c2) = chars.next() else {
            return Some(-lp1);
        };
        let mut st = self.neural_tracker.active.clone();
        nv.step_token(nv.char_to_token(c1), &mut st);
        let mut sum_surp = -nv.score_next_char(&st, c2).clamp(-13.5, -0.01);
        let mut count = 1.0f32;
        nv.step_token(nv.char_to_token(c2), &mut st);
        for ch in chars {
            sum_surp += -nv.score_next_char(&st, ch).clamp(-13.5, -0.01);
            count += 1.0;
            nv.step_token(nv.char_to_token(ch), &mut st);
        }
        Some(sum_surp / count)
    }

    fn finalize_commit(
        &mut self,
        text: &str,
        matched_code: Option<PackedCode>,
        timestamp_ms: u64,
        notice: &mut String,
    ) {
        // 1. Update personal touch-region adaptive offsets if touch observations match `matched_code`!
        if let Some(code) = matched_code {
            self.learn_touch_offsets(code, 0..self.observations.len());
        }

        // 2. Compute RWKV surprisal BEFORE advancing neural context, then update personal history
        //    & surprisal-guided auto-phrase (indexed for both active shape schema AND Full Pinyin!)
        let surprisal = self.compute_rwkv_surprisal(text);
        let dict_idx = self.config.schema.slot_index();
        let dict_opt = self.dicts.get(dict_idx).and_then(|d| d.as_ref());
        let prev = self.last_committed.clone();
        let learned = self.user.record_commit_with_surprisal(
            &prev,
            text,
            timestamp_ms,
            self.config.auto_phrase,
            self.config.auto_phrase_threshold,
            dict_opt,
            self.pinyin_dict.as_ref(),
            surprisal,
        );
        if let Some(new_word) = learned {
            if let Some(d) = dict_opt {
                if let Some(c) = d.encode_phrase(&new_word) {
                    *notice = format!("自动成词：{} ({})", new_word, c.as_string());
                }
            }
        }
        self.last_committed = text.to_string();
        for ch in text.chars() {
            if is_cjk_char(ch) {
                self.recent_context.push(ch);
            } else {
                self.recent_context.clear();
            }
        }
        if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
            if !text.is_empty() {
                self.neural_tracker.commit_cjk_str(&nv, text);
            } else {
                self.neural_tracker.reset_to_bos(Some(&nv));
            }
        }
        if self.recent_context.len() > 8 {
            let drain_n = self.recent_context.len() - 8;
            self.recent_context.drain(..drain_n);
        }
    }

    fn manual_create_phrase_from_recent(&mut self) -> Option<EngineOutput> {
        let dict_idx = self.config.schema.slot_index();
        let d = self.dicts[dict_idx].as_ref()?;
        if self.last_committed.chars().count() >= 2 {
            let phrase = self.last_committed.clone();
            let code =
                self.user
                    .add_phrase_with_pinyin(&phrase, false, Some(d), self.pinyin_dict.as_ref())?;
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

    // ========================================================================
    // Candidate Assembly & Bayesian Spatial + N-gram Ranking
    // ========================================================================

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
        let initial_state = if let Some(ref model) = nv {
            let _ = model;
            self.neural_tracker.active.clone()
        } else {
            RwkvState::default()
        };
        let decoder = PinyinDecoder {
            dict: pdict,
            aux: self.aux.as_ref(),
            model: nv,
            config: &self.config,
            user: &self.user,
            context: &self.recent_context,
            layout: &self.key_layout,
        };
        let (cands, paths) = self.pinyin_stream.update_and_decode(
            &self.observations,
            &self.raw_code,
            &decoder,
            &initial_state,
        );
        self.candidates = cands.iter().map(|pc| pc.candidate.clone()).collect();
        self.pinyin_candidates = cands;
        self.pinyin_paths = paths;
    }

    pub fn rebuild_candidates(&mut self) {
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

        // Branch 1: `z` omnipotent mode (Wubi only: z-prompt, zz-symbols, z+pinyin reverse lookup)
        if self.config.schema.is_wubi()
            && self.config.z_omnipotent
            && self.raw_code.starts_with('z')
        {
            self.build_z_mode_candidates(slot);
            return;
        }

        // Branch 2: `z` wildcard at positions 2..=4 (Wubi only)
        if self.config.schema.is_wubi()
            && self.config.z_omnipotent
            && self.raw_code.contains('z')
        {
            self.build_wildcard_candidates(slot);
            return;
        }

        // Branch 3: Standard 1..=4 letters (Wubi or Xiaohe) + Bayesian Spatial/Lexical/History Typo Decoder
        self.build_standard_and_typo_candidates(slot);
    }

    fn build_z_mode_candidates(&mut self, slot: usize) {
        let dict = self.dicts[slot].as_ref().unwrap();
        if self.raw_code == "z" {
            for (sym, desc) in [
                ("，", "输拼音反查五笔 · zz特殊符号"),
                ("。", "句号"),
                ("？", "问号"),
                ("！", "叹号"),
                ("……", "省略号"),
                ("——", "破折号"),
                ("、", "顿号"),
            ] {
                self.candidates.push(Candidate {
                    text: sym.to_string(),
                    matched_code: PackedCode::from_ascii("z").unwrap_or_default(),
                    comment: desc.to_string(),
                    badge: CandidateBadge::PinyinLookup,
                    confidence: 1.0,
                    is_exact: true,
                });
            }
            return;
        }

        let mut seen = HashSet::<String>::new();

        // Check `zz*` symbol dictionary entries first if code <= 4
        if self.raw_code.starts_with("zz") && self.raw_code.len() <= 4 {
            if let Some(pc) = PackedCode::from_ascii(&self.raw_code) {
                for e in dict.lookup_exact(pc, true) {
                    if seen.insert(e.text.to_string()) {
                        self.candidates.push(Candidate {
                            text: e.text.to_string(),
                            matched_code: pc,
                            comment: if (e.flags & ENTRY_FLAG_SYMBOL) != 0 {
                                "符号".to_string()
                            } else {
                                pc.as_string()
                            },
                            badge: CandidateBadge::None,
                            confidence: 1.0,
                            is_exact: true,
                        });
                    }
                }
                for e in dict.lookup_longer_prefix(pc, 24, true) {
                    if seen.insert(e.text.to_string()) {
                        self.candidates.push(Candidate {
                            text: e.text.to_string(),
                            matched_code: e.code,
                            comment: e.code.as_string(),
                            badge: CandidateBadge::None,
                            confidence: 0.9,
                            is_exact: true,
                        });
                    }
                }
            }
        }

        // Pinyin reverse lookup (`z` + pinyin)
        if let Some(ref aux) = self.aux {
            let py_query = &self.raw_code.as_bytes()[1..];
            if !py_query.is_empty() && py_query.len() <= 6 {
                for (ch, _wt, py_disp, _exact) in aux.lookup_pinyin(py_query, 200) {
                    let s = ch.to_string();
                    if !seen.insert(s.clone()) {
                        continue;
                    }
                    if let Some(meta) = dict.char_meta(ch) {
                        let short_s = meta.shortest_code.as_string();
                        let full_s = meta.full_code.as_string();
                        let code_desc = if !short_s.is_empty() && short_s != full_s {
                            if py_disp.is_empty() {
                                format!("{} · {}", short_s, full_s)
                            } else {
                                format!("{} · {} ({})", short_s, full_s, py_disp)
                            }
                        } else if py_disp.is_empty() {
                            full_s
                        } else {
                            format!("{} ({})", full_s, py_disp)
                        };
                        self.candidates.push(Candidate {
                            text: s,
                            matched_code: meta.full_code,
                            comment: code_desc,
                            badge: CandidateBadge::PinyinLookup,
                            confidence: 1.0,
                            is_exact: true,
                        });
                    }
                }
            }
        }
    }

    fn build_wildcard_candidates(&mut self, slot: usize) {
        let dict = self.dicts[slot].as_ref().unwrap();
        let codes = expand_wildcards(self.raw_code.as_bytes());
        let mut raw_hits = Vec::<(String, PackedCode, u32)>::new();
        for c in codes {
            for (utext, ucnt, _) in self.user.lookup_exact(c) {
                raw_hits.push((utext.to_string(), c, 900_000_000 + ucnt * 10_000));
            }
            for e in dict.lookup_exact(c, self.config.enable_rare_charset) {
                raw_hits.push((e.text.to_string(), c, e.weight));
            }
        }
        raw_hits.sort_by(|a, b| b.2.cmp(&a.2));
        let mut seen = HashSet::<String>::new();
        for (text, code, _) in raw_hits.into_iter().take(60) {
            if seen.insert(text.clone()) {
                self.candidates.push(Candidate {
                    text,
                    matched_code: code,
                    comment: code.as_string(),
                    badge: CandidateBadge::Wildcard,
                    confidence: 1.0,
                    is_exact: true,
                });
            }
        }
    }

    fn build_standard_and_typo_candidates(&mut self, slot: usize) {
        let dict = self.dicts[slot].as_ref().unwrap();
        let Some(exact_code) = PackedCode::from_ascii(&self.raw_code) else {
            return;
        };
        let lm_w = self.lm_weights;
        let neural_view = if lm_w.neural_scale > 0.0 && self.neural_tracker.is_ready() {
            self.aux.as_ref().and_then(|a| a.neural_view())
        } else {
            None
        };

        // 1. Collect Exact Candidates (preserving canonical Wubi ordering!)
        struct ExactItem {
            text: String,
            code: PackedCode,
            comment: String,
            badge: CandidateBadge,
            sort_bucket: u8, // 0 = exact primary, 1 = dynamic/dict exact, 2 = demoted strict_shortcode, 3 = prefix completion
            order_score: f64,
            base_score: f32,
            posterior_score: f32,
            dict_weight: u32,
            is_single: bool,
            is_user: bool,
        }

        let mut exact_items = Vec::<ExactItem>::new();
        let mut exact_seen = HashSet::<String>::new();

        // 1a. User phrases at exact_code
        let user_hits = self.user.lookup_exact(exact_code);
        for (idx, (utext, ucnt, _)) in user_hits.into_iter().enumerate() {
            if !exact_seen.insert(utext.to_string()) {
                continue;
            }
            let ctx = compute_context_boost(
                &self.user,
                self.aux.as_ref(),
                &self.last_committed,
                &self.recent_context,
                utext,
                &lm_w,
            );
            let n_fast = neural_view
                .as_ref()
                .map(|nv| {
                    lm_w.neural_scale
                        * smooth_neural_score(self.neural_cache_phrase_logp(
                            nv,
                            &self.neural_tracker.active,
                            &self.recent_context,
                            utext,
                            false,
                        ))
                })
                .unwrap_or(0.0);
            let base = 8.8 + ((ucnt as f32 + 1.0).ln() * 0.8) + ctx;
            let post = base + n_fast;
            let ord = if self.config.dynamic_word_freq {
                1_000_000.0 + (ucnt as f64) * 100.0 + ((ctx + n_fast) as f64) * 500.0 - (idx as f64)
            } else {
                500_000.0 - (idx as f64)
            };
            exact_items.push(ExactItem {
                text: utext.to_string(),
                code: exact_code,
                comment: String::new(),
                badge: CandidateBadge::UserPhrase,
                sort_bucket: 1,
                order_score: ord,
                base_score: base,
                posterior_score: post,
                dict_weight: 50_000_000,
                is_single: utext.chars().count() == 1,
                is_user: true,
            });
        }

        // 1b. Built-in dictionary exact matches
        let dict_exact = dict.lookup_exact(exact_code, self.config.enable_rare_charset);
        for (rank, entry) in dict_exact.iter().enumerate() {
            if !exact_seen.insert(entry.text.to_string()) {
                continue;
            }
            let mut badge = CandidateBadge::None;
            let mut comment = String::new();
            let mut demoted = false;

            if entry.is_single_char() {
                if let Some(ch) = entry.text.chars().next() {
                    if let Some(meta) = dict.char_meta(ch) {
                        // Short-code hint when user typed longer code than shortest_code
                        if self.config.show_shortcode_hint
                            && !meta.shortest_code.is_empty()
                            && meta.shortest_code.len() < exact_code.len()
                        {
                            badge = CandidateBadge::Shortcode;
                            comment = format!("⚡{}", meta.shortest_code.as_string());
                        }
                        // Strict shortcode ("出简不出全"): demote at 4-code if char has a primary short code
                        if self.config.strict_shortcode
                            && exact_code.len() == 4
                            && !meta.primary_short_code.is_empty()
                            && meta.primary_short_code.len() < 4
                        {
                            demoted = true;
                        }
                    }
                }
            }

            let ctx = compute_context_boost(
                &self.user,
                self.aux.as_ref(),
                &self.last_committed,
                &self.recent_context,
                entry.text,
                &lm_w,
            );
            let prior = compute_candidate_prior(
                self.aux.as_ref(),
                entry.weight,
                entry.text,
                entry.is_single_char(),
                exact_code.len(),
                &lm_w,
            );
            let n_fast = neural_view
                .as_ref()
                .map(|nv| {
                    lm_w.neural_scale
                        * smooth_neural_score(self.neural_cache_phrase_logp(
                            nv,
                            &self.neural_tracker.active,
                            &self.recent_context,
                            entry.text,
                            false,
                        ))
                })
                .unwrap_or(0.0);
            let base = prior + ctx;
            let post = base + n_fast;

            // Preserve #1 fixed muscle-memory slot unless dynamic freq is explicitly enabled for this kind!
            let allow_dyn = if entry.is_single_char() {
                self.config.dynamic_char_freq
            } else {
                self.config.dynamic_word_freq
            };

            let (bucket, ord) = if demoted {
                (2u8, 10_000.0 - rank as f64)
            } else if rank == 0 && !allow_dyn {
                // Sacred #1 slot for fixed-order single-char muscle memory!
                (0u8, 10_000_000.0)
            } else if allow_dyn {
                let dyn_bonus = ((ctx + n_fast * 0.5) as f64) * 1800.0
                    + (self.user.unigram_boost(entry.text) as f64) * 1200.0;
                let base_rank_score = 900_000.0 - (rank as f64) * 1500.0;
                (1u8, base_rank_score + dyn_bonus)
            } else {
                (1u8, 900_000.0 - (rank as f64) * 1500.0)
            };

            exact_items.push(ExactItem {
                text: entry.text.to_string(),
                code: exact_code,
                comment,
                badge,
                sort_bucket: bucket,
                order_score: ord,
                base_score: base,
                posterior_score: post,
                dict_weight: entry.weight,
                is_single: entry.is_single_char(),
                is_user: false,
            });
        }

        // 1c. Prefix completions when `raw_code.len() < 4` (placed AFTER exact shortcodes when `shortest_code_first` is true!)
        if exact_code.len() < 4 && exact_items.len() < 15 {
            let need = 15 - exact_items.len();
            for (pcode, utext, ucnt) in self.user.lookup_prefix(exact_code, 4) {
                if exact_seen.insert(utext.to_string()) {
                    let tail = &pcode.as_string()[exact_code.len()..];
                    exact_items.push(ExactItem {
                        text: utext.to_string(),
                        code: pcode,
                        comment: format!("…{}", tail),
                        badge: CandidateBadge::UserPhrase,
                        sort_bucket: if self.config.shortest_code_first { 3 } else { 1 },
                        order_score: 50_000.0 + ucnt as f64,
                        base_score: 7.5,
                        posterior_score: 7.5,
                        dict_weight: 50_000_000,
                        is_single: utext.chars().count() == 1,
                        is_user: true,
                    });
                }
            }
            for e in dict.lookup_longer_prefix(exact_code, need, false) {
                if exact_seen.insert(e.text.to_string()) {
                    let full_str = e.code.as_string();
                    let tail = if full_str.len() > exact_code.len() {
                        &full_str[exact_code.len()..]
                    } else {
                        ""
                    };
                    let bucket = if self.config.shortest_code_first { 3 } else { 1 };
                    let bs = 0.45 * ((e.weight as f32) + 50.0).ln();
                    exact_items.push(ExactItem {
                        text: e.text.to_string(),
                        code: e.code,
                        comment: if tail.is_empty() {
                            String::new()
                        } else {
                            format!("…{}", tail)
                        },
                        badge: CandidateBadge::None,
                        sort_bucket: bucket,
                        order_score: (e.weight as f64) * 0.0001,
                        base_score: bs,
                        posterior_score: bs,
                        dict_weight: e.weight,
                        is_single: e.is_single_char(),
                        is_user: false,
                    });
                }
            }
        }

        exact_items.sort_by(|a, b| {
            a.sort_bucket
                .cmp(&b.sort_bucket)
                .then_with(|| {
                    b.order_score
                        .partial_cmp(&a.order_score)
                        .unwrap_or(core::cmp::Ordering::Equal)
                })
        });

        // 2. Statistical + Touch-Region + Neural RWKV-7 Typo-Correction Decoder (for 2..=4 key codes)
        struct FuzzyItem {
            text: String,
            code: PackedCode,
            base_score: f32,
            posterior_score: f32,
            boundary_slip: bool,
            kind: CorrectionKind,
            dict_weight: u32,
            is_single: bool,
            spatial_logp: f32,
        }

        let mut fuzzy_items = Vec::<FuzzyItem>::new();
        if self.config.typo.enabled && self.observations.len() >= 2 {
            let hyps = generate_hypotheses_with_options(
                &self.observations,
                &self.key_layout,
                &self.user.touch_profile,
                &self.config.typo,
                self.config.schema.allows_z_code(),
            );

            let mut fuzzy_seen = HashSet::<String>::new();
            for item in &exact_items {
                if item.sort_bucket <= 2 {
                    fuzzy_seen.insert(item.text.clone());
                }
            }

            let spatial_scale = if self
                .aux
                .as_ref()
                .map(|a| a.has_unigrams())
                .unwrap_or(false)
            {
                lm_w.spatial_scale
            } else {
                2.05
            };

            for h in hyps {
                if h.kind == CorrectionKind::Exact || h.code == exact_code {
                    continue;
                }
                // Check user phrases on neighbor hypothesis
                for (utext, ucnt, _) in self.user.lookup_exact(h.code).into_iter().take(2) {
                    if !fuzzy_seen.insert(utext.to_string()) {
                        continue;
                    }
                    let ctx = compute_context_boost(
                        &self.user,
                        self.aux.as_ref(),
                        &self.last_committed,
                        &self.recent_context,
                        utext,
                        &lm_w,
                    );
                    let n_fast = neural_view
                        .as_ref()
                        .map(|nv| lm_w.neural_scale * smooth_neural_score(nv.score_phrase_fast(&self.neural_tracker.active, utext)))
                        .unwrap_or(0.0);
                    let prior = 9.0 + ((ucnt as f32 + 1.0).ln() * 0.75);
                    let base = spatial_scale * h.spatial_logp + prior + ctx;
                    fuzzy_items.push(FuzzyItem {
                        text: utext.to_string(),
                        code: h.code,
                        base_score: base,
                        posterior_score: base + n_fast,
                        boundary_slip: h.boundary_slip,
                        kind: h.kind,
                        dict_weight: 50_000_000,
                        is_single: utext.chars().count() == 1,
                        spatial_logp: h.spatial_logp,
                    });
                }
                // Check top dictionary entries on neighbor hypothesis
                for e in dict.lookup_exact(h.code, false).into_iter().take(4) {
                    if !fuzzy_seen.insert(e.text.to_string()) {
                        continue;
                    }
                    if e.weight < 5_000 {
                        let known_modern = self
                            .aux
                            .as_ref()
                            .and_then(|a| e.text.chars().next().and_then(|c| a.unigram_prior(c)))
                            .map(|u| u >= 3.5)
                            .unwrap_or(false);
                        if !known_modern {
                            continue;
                        }
                    }
                    let ctx = compute_context_boost(
                        &self.user,
                        self.aux.as_ref(),
                        &self.last_committed,
                        &self.recent_context,
                        e.text,
                        &lm_w,
                    );
                    let prior = compute_candidate_prior(
                        self.aux.as_ref(),
                        e.weight,
                        e.text,
                        e.is_single_char(),
                        h.code.len(),
                        &lm_w,
                    );
                    let n_fast = neural_view
                        .as_ref()
                        .map(|nv| {
                            lm_w.neural_scale
                                * smooth_neural_score(self.neural_cache_phrase_logp(
                                    nv,
                                    &self.neural_tracker.active,
                                    &self.recent_context,
                                    e.text,
                                    false,
                                ))
                        })
                        .unwrap_or(0.0);
                    let base = spatial_scale * h.spatial_logp + prior + ctx;
                    fuzzy_items.push(FuzzyItem {
                        text: e.text.to_string(),
                        code: h.code,
                        base_score: base,
                        posterior_score: base + n_fast,
                        boundary_slip: h.boundary_slip,
                        kind: h.kind,
                        dict_weight: e.weight,
                        is_single: e.is_single_char(),
                        spatial_logp: h.spatial_logp,
                    });
                }
            }

            fuzzy_items.sort_by(|a, b| {
                b.posterior_score
                    .partial_cmp(&a.posterior_score)
                    .unwrap_or(core::cmp::Ordering::Equal)
            });

            // Stage 2b: Exact multi-token RWKV-7 recurrent unroll only when fuzzy candidates are
            // competitive with exact (or exact is empty), keeping ordinary keystrokes < 15 µs!
            if let Some(ref nv) = neural_view {
                if !fuzzy_items.is_empty() {
                    let best_exact_fast = exact_items
                        .iter()
                        .filter(|x| x.sort_bucket <= 2)
                        .map(|x| x.posterior_score)
                        .fold(f32::NEG_INFINITY, f32::max);
                    let best_fuzzy_fast = fuzzy_items[0].posterior_score;
                    let needs_exact_unroll = !best_exact_fast.is_finite()
                        || (best_fuzzy_fast >= best_exact_fast - 5.0);
                    if needs_exact_unroll {
                        for item in exact_items.iter_mut().filter(|x| x.sort_bucket <= 2).take(2) {
                            if item.text.chars().nth(1).is_some() {
                                let n_ex = lm_w.neural_scale
                                    * smooth_neural_score(self.neural_cache_phrase_logp(
                                        nv,
                                        &self.neural_tracker.active,
                                        &self.recent_context,
                                        &item.text,
                                        true,
                                    ));
                                item.posterior_score = item.base_score + n_ex;
                            }
                        }
                        let refine_k = fuzzy_items.len().min(4);
                        for f in &mut fuzzy_items[..refine_k] {
                            if f.text.chars().nth(1).is_some() {
                                let n_ex = lm_w.neural_scale
                                    * smooth_neural_score(self.neural_cache_phrase_logp(
                                        nv,
                                        &self.neural_tracker.active,
                                        &self.recent_context,
                                        &f.text,
                                        true,
                                    ));
                                f.posterior_score = f.base_score + n_ex;
                            }
                        }
                        fuzzy_items[..refine_k].sort_by(|a, b| {
                            b.posterior_score
                                .partial_cmp(&a.posterior_score)
                                .unwrap_or(core::cmp::Ordering::Equal)
                        });
                    }
                }
            }

            fuzzy_items.truncate(4);
        }

        // 3. Compute calibrated Softmax confidences over top exact (bucket <= 2) + top fuzzy hypotheses!
        let has_direct_exact = exact_items.iter().any(|x| x.sort_bucket <= 2);
        let best_exact_meta = exact_items
            .iter()
            .find(|x| x.sort_bucket <= 2)
            .map(|x| (x.text.clone(), x.dict_weight, x.is_single, x.is_user));
        let top_fuzzy_meta = fuzzy_items.first().map(|f| {
            (
                f.text.clone(),
                f.dict_weight,
                f.is_single,
                f.code.len(),
                f.spatial_logp,
                f.boundary_slip,
            )
        });
        let best_exact_post = exact_items
            .iter()
            .filter(|x| x.sort_bucket <= 2)
            .map(|x| x.posterior_score)
            .fold(f32::NEG_INFINITY, f32::max);

        let mut pool_scores = Vec::<f32>::new();
        if has_direct_exact {
            pool_scores.push(best_exact_post);
        }
        for f in &fuzzy_items {
            pool_scores.push(f.posterior_score);
        }
        let probs = softmax_confidences(&pool_scores);
        let exact_conf = if has_direct_exact {
            probs.first().copied().unwrap_or(1.0)
        } else {
            0.0
        };
        let fuzzy_offset = usize::from(has_direct_exact);

        // Convert exact items into final `Candidate`s
        let mut final_exact: Vec<Candidate> = exact_items
            .into_iter()
            .map(|it| Candidate {
                text: it.text,
                matched_code: it.code,
                comment: it.comment,
                badge: it.badge,
                confidence: if it.sort_bucket <= 2 {
                    exact_conf
                } else {
                    exact_conf * 0.5
                },
                is_exact: true,
            })
            .collect();

        // Filter & format qualifying fuzzy corrections
        let mut promoted_fuzzy = Vec::<Candidate>::new();
        let mut secondary_fuzzy = Vec::<Candidate>::new();

        for (i, f) in fuzzy_items.into_iter().enumerate() {
            let conf = probs.get(fuzzy_offset + i).copied().unwrap_or(0.0);
            let tag_prefix = match f.kind {
                CorrectionKind::Transposition => "⇄",
                CorrectionKind::TouchBoundary => "⌖",
                _ => "⟲",
            };
            let cand = Candidate {
                text: f.text,
                matched_code: f.code,
                comment: format!("{}{} {:.0}%", tag_prefix, f.code.as_string(), conf * 100.0),
                badge: CandidateBadge::TypoCorrected,
                confidence: conf,
                is_exact: false,
            };

            // When should a typo-corrected candidate promote to #1?
            // 1) Direct exact code is EMPTY (空码救回) and confidence >= promote_threshold (or >= 0.65 when no other options)
            // 2) Touch landed on key boundary (`f.boundary_slip`) AND posterior confidence >= promote_threshold
            let rescue_empty = !has_direct_exact && conf >= (self.config.typo.promote_threshold * 0.78);
            let snap_boundary = f.boundary_slip && conf >= self.config.typo.promote_threshold;

            if promoted_fuzzy.is_empty() && (rescue_empty || snap_boundary) {
                promoted_fuzzy.push(cand);
            } else if conf >= self.config.typo.suggest_threshold
                || (!has_direct_exact && secondary_fuzzy.len() < 2 && conf >= 0.20)
            {
                secondary_fuzzy.push(cand);
            }
        }

        // Assemble final candidate list:
        // - Any promoted rescue/boundary-snap #1 goes first
        // - Exact primary candidate(s) stay right at the top
        // - Secondary fuzzy suggestions slot cleanly at position 1..=2 (right after #1 exact) so 1-key press '2' selects them!
        self.candidates.extend(promoted_fuzzy);
        if !final_exact.is_empty() {
            let first_exact = final_exact.remove(0);
            self.candidates.push(first_exact);
            for sf in secondary_fuzzy.drain(..) {
                self.candidates.push(sf);
            }
            self.candidates.extend(final_exact);
        } else {
            self.candidates.extend(secondary_fuzzy);
        }

        // 4. Language-Model Missed-Space Split Decoder (漏空格切分纠错)
        if self.config.typo.enabled && self.config.typo.missed_space_correction {
            self.evaluate_missed_space_on_rebuild(
                slot,
                exact_code,
                has_direct_exact,
                best_exact_meta.as_ref(),
                top_fuzzy_meta.as_ref(),
            );
        }
    }

    // ========================================================================
    // Language-Model Missed-Space Split Correction (漏空格切分纠错)
    // ========================================================================

    /// Return the #1 primary candidate `(text, weight, is_single_char)` that pressing Space
    /// would commit for a short code `code` (`len = 1..=3`).
    fn primary_exact_for_code(&self, slot: usize, code: PackedCode) -> Option<(String, u32, bool)> {
        let dict = self.dicts[slot].as_ref()?;
        let dict_first = dict
            .lookup_exact(code, false)
            .into_iter()
            .next();
        if let Some(d0) = dict_first {
            if d0.is_single_char() && !self.config.dynamic_char_freq {
                return Some((d0.text.to_string(), d0.weight, true));
            }
            if self.config.dynamic_word_freq {
                if let Some((utext, _, _)) = self.user.lookup_exact(code).into_iter().next() {
                    return Some((utext.to_string(), 50_000_000, utext.chars().count() == 1));
                }
            }
            return Some((d0.text.to_string(), d0.weight, d0.is_single_char()));
        }
        self.user
            .lookup_exact(code)
            .into_iter()
            .next()
            .map(|(utext, _, _)| (utext.to_string(), 50_000_000, utext.chars().count() == 1))
    }

    /// Unified RWKV-7 + Neural Cache LM phrase log-probability (in nats), shared by standard-mode
    /// exact/typo ranking and missed-space split decoding.
    fn neural_cache_phrase_logp(
        &self,
        nv: &NeuralView<'_>,
        state: &RwkvState,
        ctx_chars: &[char],
        text: &str,
        exact_unroll: bool,
    ) -> f32 {
        let mut raw_lp = if exact_unroll && text.chars().nth(1).is_some() {
            nv.score_phrase_exact(state, text)
        } else {
            nv.score_phrase_fast(state, text)
        }
        .clamp(-13.5, -0.02);

        if self.config.dynamic_word_freq && !self.user.continuations.is_empty() {
            if let Some(c1) = text.chars().next() {
                let raw_lp1 = nv.score_next_char(state, c1).clamp(-13.5, -0.02);
                let pctx = PersonalContext::with_chars(ctx_chars.iter().copied());
                let st_opt = self
                    .user
                    .continuations
                    .matches_model(nv)
                    .then_some(state);
                let interp_lp1 = self
                    .user
                    .continuations
                    .interpolate_logp(raw_lp1, pctx, c1, st_opt);
                raw_lp = (raw_lp - raw_lp1 + interp_lp1).clamp(-13.5, -0.02);
            }
        }
        raw_lp
    }

    /// Calibrated log-probability proxy (in nats) for committing `text` after `(state, ctx_chars, last_comm)`.
    fn lm_word_logp(
        &self,
        nv: Option<&NeuralView<'_>>,
        state: Option<&RwkvState>,
        ctx_chars: &[char],
        last_comm: &str,
        text: &str,
        dict_weight: u32,
        exact_unroll: bool,
    ) -> f32 {
        let ctx_ng = compute_context_boost(
            &self.user,
            self.aux.as_ref(),
            last_comm,
            ctx_chars,
            text,
            &self.lm_weights,
        );
        if let (Some(nv), Some(st)) = (nv, state) {
            let mut rwkv_lp =
                self.neural_cache_phrase_logp(nv, st, ctx_chars, text, exact_unroll);
            // Enforce probability monotonicity P(c1 c2 ...) <= P(c1) so long phrases in prefix
            // scans cannot be artificially inflated above 1-char words by sublinear length scaling.
            if let Some(c1) = text.chars().next() {
                if text.chars().nth(1).is_some() {
                    let lp1 = self.neural_cache_phrase_logp(
                        nv,
                        st,
                        ctx_chars,
                        &c1.to_string(),
                        false,
                    );
                    if rwkv_lp > lp1 {
                        rwkv_lp = lp1;
                    }
                }
            }
            let dict_adj = if dict_weight > 0 {
                0.08 * (((dict_weight as f32) + 50.0).ln() - 14.0).clamp(-3.5, 0.8)
            } else {
                0.0
            };
            rwkv_lp + 0.22 * ctx_ng.min(7.0) + dict_adj
        } else {
            // Graceful N-gram / dictionary fallback when `WBLM0001` neural weights are absent
            let has_uni = self.aux.as_ref().map(|a| a.has_unigrams()).unwrap_or(false);
            if has_uni {
                let mut uni_sum = 0.0f32;
                let mut n = 0usize;
                for ch in text.chars().take(4) {
                    let u = self
                        .aux
                        .as_ref()
                        .and_then(|a| a.unigram_prior(ch))
                        .map(|v| v - 14.0)
                        .unwrap_or(-8.5);
                    uni_sum += u;
                    n += 1;
                }
                let mean_uni = if n > 0 { uni_sum / (n as f32) } else { -9.0 };
                let dict_adj = 0.30 * (((dict_weight as f32) + 50.0).ln() - 16.0).clamp(-4.5, 1.5);
                mean_uni + 0.50 * ctx_ng.min(8.0) + dict_adj
            } else {
                // Legacy WBAUX001 / dict-only fallback calibrated onto the same nats scale
                let wt_score = -6.2 + 0.45 * (((dict_weight as f32) + 50.0).ln() - 12.0).clamp(-5.0, 6.0);
                wt_score + 0.50 * ctx_ng.min(8.0)
            }
        }
    }

    /// Log-probability (in nats) that the next word after `(state, ctx_chars, last_comm)` matches
    /// Wubi prefix `right_bytes` (`len = 1..=4`). Returns `None` if `right_bytes` has no valid
    /// exact or prefix continuations in the active dictionary/user store.
    /// Also returns `exact_c2` if `right_bytes` itself has a primary exact candidate `(text, lm_logp)`.
    fn lm_prefix_logp(
        &self,
        slot: usize,
        nv: Option<&NeuralView<'_>>,
        state: Option<&RwkvState>,
        ctx_chars: &[char],
        last_comm: &str,
        right_bytes: &[u8],
    ) -> Option<(f32, Option<(String, f32)>)> {
        let dict = self.dicts[slot].as_ref()?;
        let right_code = PackedCode::from_bytes(right_bytes)?;
        let rlen = right_bytes.len();

        let exact_c2 = self
            .primary_exact_for_code(slot, right_code)
            .map(|(t, wt, _)| {
                let lp = self.lm_word_logp(nv, state, ctx_chars, last_comm, &t, wt, true);
                (t, lp)
            });

        if rlen == 1 {
            let key_byte = right_bytes[0];
            if !(b'a'..=b'y').contains(&key_byte) {
                return None;
            }
            let key_idx = (key_byte - b'a') as usize;
            let c2_lp = exact_c2.as_ref().map(|x| x.1).unwrap_or(-13.5);
            let pref_lp = if let (Some(nv), Some(st)) = (nv, state) {
                if self.config.schema == WubiSchema::Wubi86 {
                    let k1 = st.key1_logp[key_idx].clamp(-9.0, -0.05);
                    k1.max(c2_lp).min(-0.05)
                } else {
                    // For 98 / 06 schemas, aggregate top-24 neural next chars whose schema code starts with `key_byte`
                    let mut acc = f32::NEG_INFINITY;
                    for (ch, lp) in nv.top_next_chars(st, 24) {
                        if let Some(meta) = dict.char_meta(ch) {
                            let s0 = meta.shortest_code.as_string();
                            let f0 = meta.full_code.as_string();
                            if s0.as_bytes().first() == Some(&key_byte)
                                || f0.as_bytes().first() == Some(&key_byte)
                            {
                                acc = logsumexp_pair(acc, lp.clamp(-13.5, -0.05));
                            }
                        }
                    }
                    let fallback = (st.key1_logp[key_idx] - 0.5).clamp(-8.0, -1.8);
                    acc.max(c2_lp).max(fallback).min(-0.05)
                }
            } else {
                // N-gram fallback: combine exact 1-code char with matching bigrams
                let mut best = c2_lp;
                if let (Some(aux), Some(&p1)) = (self.aux.as_ref(), ctx_chars.last()) {
                    for (ch, bi) in aux.bigrams_after(p1).into_iter().take(24) {
                        if let Some(meta) = dict.char_meta(ch) {
                            let s0 = meta.shortest_code.as_string();
                            let f0 = meta.full_code.as_string();
                            if s0.as_bytes().first() == Some(&key_byte)
                                || f0.as_bytes().first() == Some(&key_byte)
                            {
                                let u = aux.unigram_prior(ch).map(|v| v - 14.0).unwrap_or(-7.5);
                                best = best.max((u + 0.5 * bi).min(-0.5));
                            }
                        }
                    }
                }
                best.min(-0.05)
            };
            return Some((pref_lp, exact_c2));
        }

        // Length 2, 3, or 4: require at least one valid exact or prefix entry!
        let has_user = !self.user.lookup_exact(right_code).is_empty()
            || (rlen < 4 && !self.user.lookup_prefix(right_code, 1).is_empty());
        let has_dict = if rlen == 4 {
            dict.has_exact(right_code, false)
        } else {
            dict.has_prefix_or_exact(right_code, false)
        };
        if !has_user && !has_dict {
            return None;
        }

        let mut seen = HashSet::<String>::with_capacity(24);
        let mut lse = f32::NEG_INFINITY;

        // 1. User exact & prefix matches
        for (utext, _, _) in self.user.lookup_exact(right_code).into_iter().take(2) {
            if seen.insert(utext.to_string()) {
                let lp = self.lm_word_logp(nv, state, ctx_chars, last_comm, utext, 50_000_000, rlen == 4);
                lse = logsumexp_pair(lse, lp);
            }
        }
        if rlen < 4 {
            for (_, utext, _) in self.user.lookup_prefix(right_code, 3) {
                if seen.insert(utext.to_string()) {
                    let lp = self.lm_word_logp(nv, state, ctx_chars, last_comm, utext, 50_000_000, false);
                    lse = logsumexp_pair(lse, lp);
                }
            }
        }

        // 2. Dictionary exact matches
        for e in dict.lookup_exact(right_code, false).into_iter().take(4) {
            if seen.insert(e.text.to_string()) {
                let lp = self.lm_word_logp(nv, state, ctx_chars, last_comm, e.text, e.weight, rlen == 4);
                lse = logsumexp_pair(lse, lp);
            }
        }

        // 3. Top dictionary prefix continuations across the full prefix range (for len 2..=3)
        if rlen < 4 {
            for e in dict.top_prefix_entries(right_code, 14, false) {
                if seen.insert(e.text.to_string()) {
                    let lp = self.lm_word_logp(nv, state, ctx_chars, last_comm, e.text, e.weight, false);
                    lse = logsumexp_pair(lse, lp);
                }
            }
            // 4. Contextually hot next characters from Micro-RWKV-7 whose code starts with `right_bytes`
            if let (Some(nv), Some(st)) = (nv, state) {
                for (ch, raw_lp) in nv.top_next_chars(st, 16) {
                    if let Some(meta) = dict.char_meta(ch) {
                        let s0 = meta.shortest_code.as_string();
                        let f0 = meta.full_code.as_string();
                        if s0.as_bytes().starts_with(right_bytes)
                            || f0.as_bytes().starts_with(right_bytes)
                        {
                            let s = ch.to_string();
                            if seen.insert(s.clone()) {
                                let ctx_ng = compute_context_boost(
                                    &self.user,
                                    self.aux.as_ref(),
                                    last_comm,
                                    ctx_chars,
                                    &s,
                                    &self.lm_weights,
                                );
                                let lp = raw_lp.clamp(-13.5, -0.02) + 0.22 * ctx_ng.min(7.0);
                                lse = logsumexp_pair(lse, lp);
                            }
                        }
                    }
                }
            }
        }

        if !lse.is_finite() {
            return None;
        }
        Some((lse.min(-0.05), exact_c2))
    }

    fn evaluate_missed_space_on_rebuild(
        &mut self,
        slot: usize,
        exact_code: PackedCode,
        has_direct_exact: bool,
        best_exact_meta: Option<&(String, u32, bool, bool)>,
        top_fuzzy_meta: Option<&(String, u32, bool, usize, f32, bool)>,
    ) {
        let raw_bytes = self.raw_code.as_bytes();
        let len = raw_bytes.len();
        if (len != 3 && len != 4) || raw_bytes.contains(&b'z') {
            return;
        }

        let nv = self
            .aux
            .as_ref()
            .and_then(|a| a.neural_view())
            .filter(|_| self.neural_tracker.is_ready());
        let active_st = nv.as_ref().map(|_| &self.neural_tracker.active);

        // Case 1: Dead 3-letter code (no exact match AND no 4-letter prefix continuation!)
        // Offer complete-pair candidate (`ab+c` or `a+bc`) in candidate bar without auto-splitting
        // (so typing a 4th key for a 4-key typo like `yrwu -> 我们` still works cleanly).
        if len == 3 {
            if has_direct_exact {
                return;
            }
            let dict = match self.dicts[slot].as_ref() {
                Some(d) => d,
                None => return,
            };
            if dict.has_prefix_or_exact(exact_code, false)
                || !self.user.lookup_prefix(exact_code, 1).is_empty()
            {
                return;
            }
            let typo_nats = top_fuzzy_meta
                .map(|(t, wt, _, _, sp, _)| {
                    self.lm_word_logp(
                        nv.as_ref(),
                        active_st,
                        &self.recent_context,
                        &self.last_committed,
                        t,
                        *wt,
                        true,
                    ) + 1.35 * sp
                })
                .unwrap_or(f32::NEG_INFINITY);

            let mut best_pair: Option<(usize, String, f32)> = None;
            for cut in [2usize, 1usize] {
                let Some(left_code) = PackedCode::from_bytes(&raw_bytes[..cut]) else {
                    continue;
                };
                let Some(right_code) = PackedCode::from_bytes(&raw_bytes[cut..]) else {
                    continue;
                };
                let Some((c1, wt1, _)) = self.primary_exact_for_code(slot, left_code) else {
                    continue;
                };
                let Some((c2, wt2, _)) = self.primary_exact_for_code(slot, right_code) else {
                    continue;
                };
                let lp1 = self.lm_word_logp(
                    nv.as_ref(),
                    active_st,
                    &self.recent_context,
                    &self.last_committed,
                    &c1,
                    wt1,
                    true,
                );
                let st1 = nv
                    .as_ref()
                    .map(|v| step_rwkv_text(v, &self.neural_tracker.active, &c1));
                let ctx1 = ctx_push_text(&self.recent_context, &c1);
                let lp2 = self.lm_word_logp(nv.as_ref(), st1.as_ref(), &ctx1, &c1, &c2, wt2, true);
                let pair_nats = lp1 + lp2 - 1.8;
                if best_pair.as_ref().map(|x| pair_nats > x.2).unwrap_or(true) {
                    let mut combined = String::with_capacity(c1.len() + c2.len());
                    combined.push_str(&c1);
                    combined.push_str(&c2);
                    best_pair = Some((cut, combined, pair_nats));
                }
            }
            if let Some((cut, combined, pair_nats)) = best_pair {
                if pair_nats >= -13.5 && !self.candidates.iter().any(|c| c.text == combined) {
                    let left_s = &self.raw_code[..cut];
                    let right_s = &self.raw_code[cut..];
                    let conf = if typo_nats.is_finite() {
                        let p = softmax_confidences(&[pair_nats, typo_nats]);
                        p[0]
                    } else {
                        0.92
                    };
                    let cand = Candidate {
                        text: combined,
                        matched_code: PackedCode::EMPTY,
                        comment: format!("⎵{}+{} {:.0}%", left_s, right_s, conf * 100.0),
                        badge: CandidateBadge::TypoCorrected,
                        confidence: conf,
                        is_exact: false,
                    };
                    if !typo_nats.is_finite() || pair_nats >= typo_nats + 0.8 {
                        self.candidates.insert(0, cand);
                    } else if pair_nats >= typo_nats - 1.5 {
                        let pos = self.candidates.len().min(1);
                        self.candidates.insert(pos, cand);
                    }
                }
            }
            return;
        }

        // Case 2: Standard 4-letter code (`len == 4`)
        const FAST_PATH_EXACT_NATS: f32 = -4.2;
        const SPATIAL_NATS_SCALE: f32 = 1.65;
        const MIN_EMPTY_SPLIT_NATS: f32 = -13.5;
        const EMPTY_SPLIT_TYPO_MARGIN: f32 = 1.5;
        const HOLD_FOURTH_MARGIN: f32 = -0.3;
        const PAIR_PROMOTE_MARGIN: f32 = 2.6;
        const PAIR_SUGGEST_MARGIN: f32 = -0.8;

        if let Some((_, _, _, _, sp, _)) = top_fuzzy_meta {
            self.top_typo_spatial_logp = Some(*sp);
        }

        let exact_nats = if let Some((etext, ewt, _, is_user)) = best_exact_meta {
            if *is_user {
                return;
            }
            let sc = self.lm_word_logp(
                nv.as_ref(),
                active_st,
                &self.recent_context,
                &self.last_committed,
                etext,
                *ewt,
                true,
            );
            // Fast path: clearly plausible 4-code exact word -> 0 µs overhead, 0% false positive!
            if sc >= FAST_PATH_EXACT_NATS {
                return;
            }
            sc
        } else {
            f32::NEG_INFINITY
        };

        // Evaluate missed-space splits `k = 3` (`abc|d`), `k = 2` (`ab|cd`), `k = 1` (`a|bcd`)
        struct SplitEval {
            cut: usize,
            c1: String,
            split_nats: f32,
        }

        let mut best_split: Option<SplitEval> = None;
        let mut best_pair: Option<(usize, String, f32)> = None;

        for cut in [3usize, 2usize, 1usize] {
            let Some(left_code) = PackedCode::from_bytes(&raw_bytes[..cut]) else {
                continue;
            };
            let Some((c1, wt1, _)) = self.primary_exact_for_code(slot, left_code) else {
                continue;
            };
            let lp1 = self.lm_word_logp(
                nv.as_ref(),
                active_st,
                &self.recent_context,
                &self.last_committed,
                &c1,
                wt1,
                true,
            );
            // Quick prune if C_1 itself is completely implausible
            if lp1 < -11.0 {
                continue;
            }
            let st1 = nv
                .as_ref()
                .map(|v| step_rwkv_text(v, &self.neural_tracker.active, &c1));
            let ctx1 = ctx_push_text(&self.recent_context, &c1);
            let Some((pref_lp, exact_c2)) = self.lm_prefix_logp(
                slot,
                nv.as_ref(),
                st1.as_ref(),
                &ctx1,
                &c1,
                &raw_bytes[cut..],
            ) else {
                continue;
            };
            let miss_prior = match cut {
                3 => -1.5,
                2 => -1.8,
                _ => -2.1,
            };
            let split_nats = lp1 + pref_lp + miss_prior;
            if let Some((c2, lp2)) = exact_c2 {
                let mut comb = String::with_capacity(c1.len() + c2.len());
                comb.push_str(&c1);
                comb.push_str(&c2);
                let pair_nats = lp1 + lp2 + miss_prior;
                if best_pair.as_ref().map(|x| pair_nats > x.2).unwrap_or(true) {
                    best_pair = Some((cut, comb, pair_nats));
                }
            }
            if best_split
                .as_ref()
                .map(|x| split_nats > x.split_nats)
                .unwrap_or(true)
            {
                best_split = Some(SplitEval {
                    cut,
                    c1,
                    split_nats,
                });
            }
        }

        let Some(bs) = best_split else {
            return;
        };

        if !has_direct_exact {
            // Empty 4-letter code: compare best missed-space split vs best 4-key typo!
            let typo_nats = top_fuzzy_meta
                .map(|(t, wt, _, _, sp, _)| {
                    self.lm_word_logp(
                        nv.as_ref(),
                        active_st,
                        &self.recent_context,
                        &self.last_committed,
                        t,
                        *wt,
                        true,
                    ) + SPATIAL_NATS_SCALE * sp
                })
                .unwrap_or(f32::NEG_INFINITY);

            // Protect genuine touch boundary slips when they have high likelihood
            let strong_boundary_typo = top_fuzzy_meta
                .map(|(_, _, _, _, sp, is_b)| *is_b && *sp >= -1.25)
                .unwrap_or(false);
            let req_margin = if strong_boundary_typo {
                EMPTY_SPLIT_TYPO_MARGIN + 1.8
            } else {
                EMPTY_SPLIT_TYPO_MARGIN
            };

            let split_wins = bs.split_nats >= MIN_EMPTY_SPLIT_NATS
                && (!typo_nats.is_finite() || bs.split_nats >= typo_nats + req_margin);

            if split_wins {
                self.pending_empty_split = Some((bs.cut, bs.c1.clone()));
                if let Some((pcut, ptext, pnats)) = best_pair {
                    if !self.candidates.iter().any(|c| c.text == ptext) {
                        let conf = if typo_nats.is_finite() {
                            softmax_confidences(&[pnats.max(bs.split_nats), typo_nats])[0]
                        } else {
                            0.94
                        };
                        self.candidates.insert(
                            0,
                            Candidate {
                                text: ptext,
                                matched_code: PackedCode::EMPTY,
                                comment: format!(
                                    "⎵{}+{} {:.0}%",
                                    &self.raw_code[..pcut],
                                    &self.raw_code[pcut..],
                                    conf * 100.0
                                ),
                                badge: CandidateBadge::TypoCorrected,
                                confidence: conf,
                                is_exact: false,
                            },
                        );
                    }
                }
            } else if let Some((pcut, ptext, pnats)) = best_pair {
                if pnats >= typo_nats - 1.5
                    && pnats >= MIN_EMPTY_SPLIT_NATS
                    && !self.candidates.iter().any(|c| c.text == ptext)
                {
                    let conf = softmax_confidences(&[pnats, typo_nats])[0];
                    let pos = self.candidates.len().min(1);
                    self.candidates.insert(
                        pos,
                        Candidate {
                            text: ptext,
                            matched_code: PackedCode::EMPTY,
                            comment: format!(
                                "⎵{}+{} {:.0}%",
                                &self.raw_code[..pcut],
                                &self.raw_code[pcut..],
                                conf * 100.0
                            ),
                            badge: CandidateBadge::TypoCorrected,
                            confidence: conf,
                            is_exact: false,
                        },
                    );
                }
            }
        } else {
            // Colliding 4-letter exact code (`exact_nats < FAST_PATH_EXACT_NATS`):
            // Never displace an already-promoted touch-boundary typo rescue at #1!
            let has_promoted_typo = self.candidates.first().map(|c| !c.is_exact).unwrap_or(false);
            if has_promoted_typo {
                return;
            }
            // Hold 4th-code auto-commit open when a missed-space split is competitive so
            // Key #5 (`abc | de` vs `abcd | e`) or Space (`C1+C2` vs `X`) can disambiguate cleanly!
            if bs.split_nats >= exact_nats + HOLD_FOURTH_MARGIN {
                self.hold_fourth_for_split = true;
            }
            if let Some((pcut, ptext, pnats)) = best_pair {
                if !self.candidates.iter().any(|c| c.text == ptext) {
                    if pnats >= exact_nats + PAIR_PROMOTE_MARGIN {
                        self.hold_fourth_for_split = true;
                        let conf = softmax_confidences(&[pnats, exact_nats])[0];
                        self.candidates.insert(
                            0,
                            Candidate {
                                text: ptext,
                                matched_code: PackedCode::EMPTY,
                                comment: format!(
                                    "⎵{}+{} {:.0}%",
                                    &self.raw_code[..pcut],
                                    &self.raw_code[pcut..],
                                    conf * 100.0
                                ),
                                badge: CandidateBadge::TypoCorrected,
                                confidence: conf,
                                is_exact: false,
                            },
                        );
                    } else if pnats >= exact_nats + PAIR_SUGGEST_MARGIN {
                        let conf = softmax_confidences(&[pnats, exact_nats])[0];
                        let pos = self.candidates.len().min(1);
                        self.candidates.insert(
                            pos,
                            Candidate {
                                text: ptext,
                                matched_code: PackedCode::EMPTY,
                                comment: format!(
                                    "⎵{}+{} {:.0}%",
                                    &self.raw_code[..pcut],
                                    &self.raw_code[pcut..],
                                    conf * 100.0
                                ),
                                badge: CandidateBadge::TypoCorrected,
                                confidence: conf,
                                is_exact: false,
                            },
                        );
                    }
                }
            }
        }
    }

    /// Key-#5 Joint Language-Model Disambiguation (`abcd | e` vs `abc | de` vs `ab | cde` vs `a | bcde`).
    /// Returns `true` if a missed-space split `k ∈ {1, 2, 3}` won and updated `raw_code`/`observations`
    /// to the remaining tail keys (`de` / `cde` / `bcde`).
    fn try_resolve_fifth_key_split(
        &mut self,
        obs5: KeyObs,
        commit_prefix: &mut String,
        notice: &mut String,
    ) -> bool {
        if !self.config.typo.enabled
            || !self.config.typo.missed_space_correction
            || self.raw_code.len() != 4
            || self.observations.len() != 4
            || self.raw_code.contains('z')
            || !(b'a'..=b'y').contains(&obs5.ch)
        {
            return false;
        }
        let slot = self.config.schema.slot_index();
        if self.dicts[slot].is_none() {
            return false;
        }
        let Some(exact_code) = PackedCode::from_ascii(&self.raw_code) else {
            return false;
        };
        if !self.user.lookup_exact(exact_code).is_empty() {
            return false;
        }

        let nv = self
            .aux
            .as_ref()
            .and_then(|a| a.neural_view())
            .filter(|_| self.neural_tracker.is_ready());
        let active_st = nv.as_ref().map(|_| &self.neural_tracker.active);

        // Find primary exact candidate for `abcd`
        let exact_cand = self
            .candidates
            .iter()
            .find(|c| c.is_exact)
            .cloned();
        let exact_wt = exact_cand
            .as_ref()
            .and_then(|ec| {
                self.dicts[slot].as_ref().and_then(|d| {
                    d.lookup_exact(exact_code, false)
                        .into_iter()
                        .find(|e| e.text == ec.text)
                        .map(|e| e.weight)
                })
            })
            .unwrap_or(50_000);

        let exact_lp = exact_cand
            .as_ref()
            .map(|ec| {
                self.lm_word_logp(
                    nv.as_ref(),
                    active_st,
                    &self.recent_context,
                    &self.last_committed,
                    &ec.text,
                    exact_wt,
                    true,
                )
            })
            .unwrap_or(f32::NEG_INFINITY);

        // Fast path: if Key #4 was not held and #1 exact word is already natural, do normal instant 5th-code push!
        if !self.hold_fourth_for_split && exact_lp >= -4.8 {
            return false;
        }

        let mut k5 = [0u8; 5];
        k5[..4].copy_from_slice(self.raw_code.as_bytes());
        k5[4] = obs5.ch;

        // Score k = 4 (`abcd | e`) for exact candidate $X$
        let mut score_4 = if let Some(ref ec) = exact_cand {
            let st_x = nv
                .as_ref()
                .map(|v| step_rwkv_text(v, &self.neural_tracker.active, &ec.text));
            let ctx_x = ctx_push_text(&self.recent_context, &ec.text);
            let pref_e = self
                .lm_prefix_logp(slot, nv.as_ref(), st_x.as_ref(), &ctx_x, &ec.text, &k5[4..5])
                .map(|x| x.0)
                .unwrap_or(-4.0);
            exact_lp + pref_e
        } else {
            f32::NEG_INFINITY
        };

        // Also score k = 4 if candidates[0] was a promoted non-exact pair or 4-key typo rescue
        let mut pair4_better_than_exact = false;
        if let Some(first) = self.candidates.first() {
            if !first.is_exact {
                let first_lp = if first.matched_code.is_empty() {
                    // Promoted complete-pair (`C1 + C2`): use true un-divided autoregressive sum
                    // `sum_i log P(c_i | context, c_<i)` so it compares apples-to-apples with `k = 1..3`!
                    if let (Some(nv_ref), Some(st0)) = (nv.as_ref(), active_st) {
                        let mut tmp = (*st0).clone();
                        let mut sum_lp = 0.0f32;
                        for ch in first.text.chars() {
                            if is_cjk_char(ch) {
                                sum_lp += nv_ref.score_next_char(&tmp, ch).clamp(-13.5, -0.02);
                                nv_ref.step_token(nv_ref.char_to_token(ch), &mut tmp);
                            }
                        }
                        sum_lp
                    } else {
                        self.lm_word_logp(
                            nv.as_ref(),
                            active_st,
                            &self.recent_context,
                            &self.last_committed,
                            &first.text,
                            50_000,
                            true,
                        ) * 1.5
                    }
                } else {
                    self.lm_word_logp(
                        nv.as_ref(),
                        active_st,
                        &self.recent_context,
                        &self.last_committed,
                        &first.text,
                        50_000,
                        true,
                    )
                };
                let st_f = nv
                    .as_ref()
                    .map(|v| step_rwkv_text(v, &self.neural_tracker.active, &first.text));
                let ctx_f = ctx_push_text(&self.recent_context, &first.text);
                let pref_e = self
                    .lm_prefix_logp(slot, nv.as_ref(), st_f.as_ref(), &ctx_f, &first.text, &k5[4..5])
                    .map(|x| x.0)
                    .unwrap_or(-4.0);
                // Charge a real 4-key typo its actual spatial/transposition penalty, vs a complete-pair's miss prior!
                let prior_pen = if first.matched_code.is_empty() {
                    -1.8
                } else {
                    1.65 * self.top_typo_spatial_logp.unwrap_or(-3.2)
                };
                let s_first = first_lp + pref_e + prior_pen;
                if s_first > score_4 {
                    score_4 = s_first;
                    pair4_better_than_exact = true;
                }
            }
        }

        // Evaluate missed-space splits on 5 keys: `k = 3` (`abc|de`), `k = 2` (`ab|cde`), `k = 1` (`a|bcde`)
        let mut best_split5: Option<(usize, String, f32)> = None;
        for cut in [3usize, 2usize, 1usize] {
            let Some(left_code) = PackedCode::from_bytes(&k5[..cut]) else {
                continue;
            };
            let Some((c1, wt1, _)) = self.primary_exact_for_code(slot, left_code) else {
                continue;
            };
            let lp1 = self.lm_word_logp(
                nv.as_ref(),
                active_st,
                &self.recent_context,
                &self.last_committed,
                &c1,
                wt1,
                true,
            );
            if lp1 < -11.0 {
                continue;
            }
            let st1 = nv
                .as_ref()
                .map(|v| step_rwkv_text(v, &self.neural_tracker.active, &c1));
            let ctx1 = ctx_push_text(&self.recent_context, &c1);
            let Some((pref_lp, _)) = self.lm_prefix_logp(
                slot,
                nv.as_ref(),
                st1.as_ref(),
                &ctx1,
                &c1,
                &k5[cut..5],
            ) else {
                continue;
            };
            let miss_prior = match cut {
                3 => -1.8,
                2 => -2.0,
                _ => -2.2,
            };
            let sc = lp1 + pref_lp + miss_prior;
            if best_split5.as_ref().map(|x| sc > x.2).unwrap_or(true) {
                best_split5 = Some((cut, c1, sc));
            }
        }

        let req_split5_margin = if exact_cand.is_some() { 1.4 } else { 0.4 };
        if let Some((cut, c1_text, split_sc)) = best_split5 {
            if split_sc >= score_4 + req_split5_margin {
                let mut new_code = String::with_capacity(5 - cut);
                for &b in &k5[cut..5] {
                    new_code.push(b as char);
                }
                let mut new_obs = Vec::with_capacity(5 - cut);
                new_obs.extend_from_slice(&self.observations[cut..4]);
                new_obs.push(obs5);

                self.finalize_commit(&c1_text, None, obs5.timestamp_ms, notice);
                commit_prefix.push_str(&c1_text);
                self.raw_code = new_code;
                self.observations = new_obs;
                self.page = 0;
                self.hold_fourth_for_split = false;
                self.pending_empty_split = None;
                self.rebuild_candidates();
                return true;
            }
        }

        // If `k = 4` exact beat a promoted Key-#4 pair when Key #5 arrived, ensure we commit the exact candidate!
        if !pair4_better_than_exact {
            if let Some(ec) = exact_cand {
                if self.candidates.first().map(|c| !c.is_exact).unwrap_or(false) {
                    if let Some(idx) = self.candidates.iter().position(|c| c.is_exact && c.text == ec.text) {
                        let item = self.candidates.remove(idx);
                        self.candidates.insert(0, item);
                    }
                }
            }
        }
        false
    }
}

#[inline]
fn logsumexp_pair(a: f32, b: f32) -> f32 {
    if !a.is_finite() {
        return b;
    }
    if !b.is_finite() {
        return a;
    }
    let m = a.max(b);
    m + ((a - m).exp() + (b - m).exp()).ln()
}

fn step_rwkv_text(nv: &NeuralView<'_>, base: &RwkvState, text: &str) -> RwkvState {
    let mut st = base.clone();
    for ch in text.chars() {
        if is_cjk_char(ch) {
            let tok = nv.char_to_token(ch);
            nv.step_token(tok, &mut st);
        }
    }
    st
}

fn ctx_push_text(recent_context: &[char], text: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(recent_context.len() + 4);
    out.extend_from_slice(recent_context);
    for ch in text.chars() {
        if is_cjk_char(ch) {
            out.push(ch);
        } else {
            out.clear();
        }
    }
    if out.len() > 8 {
        let d = out.len() - 8;
        out.drain(..d);
    }
    out
}

#[inline]
fn smooth_neural_score(raw_lp: f32) -> f32 {
    // Soft floor (-13.5 nats ~ 1.4e-6) guards against extreme OOV outliers while keeping full
    // negative penalty against nonsensical colliding typos (-10..-13.5 nats).
    raw_lp.clamp(-13.5, -0.02) + 11.0
}

#[inline]
fn is_cjk_char(ch: char) -> bool {
    let u = ch as u32;
    (0x4E00..=0x9FFF).contains(&u) || (0x3400..=0x4DBF).contains(&u)
}

#[inline]
fn is_standalone_common_char(ch: char) -> bool {
    matches!(
        ch,
        '的' | '了' | '是' | '在' | '我' | '你' | '他' | '她' | '它' | '们'
            | '也' | '都' | '就' | '还' | '很' | '太' | '不' | '没' | '有' | '要'
            | '会' | '能' | '想' | '看' | '说' | '去' | '来' | '给' | '把' | '被'
            | '和' | '跟' | '与' | '吗' | '呢' | '吧' | '啊' | '呀' | '嘛' | '哦'
            | '这' | '那' | '个' | '一' | '上' | '下' | '里' | '中' | '人' | '大' | '小'
    )
}

#[inline]
fn is_numeral_char(ch: char) -> bool {
    matches!(ch, '零' | '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十' | '百' | '千' | '万' | '两')
}

#[inline]
fn is_numeral_pair(c1: char, c2: char) -> bool {
    is_numeral_char(c1) && is_numeral_char(c2) && !(c1 == '一' && c2 == '个')
}

#[inline]
fn compute_candidate_prior(
    aux: Option<&AuxView>,
    dict_weight: u32,
    cand_text: &str,
    is_single_char: bool,
    code_len: usize,
    w: &LmWeights,
) -> f32 {
    let phrase_bonus = if !is_single_char && code_len == 4 {
        w.phrase_bonus
    } else {
        0.0
    };
    let Some(aux_v) = aux.filter(|a| a.has_unigrams()) else {
        // Legacy WBAUX001 fallback (no corpus unigram section)
        return 0.52 * ((dict_weight as f32) + 50.0).ln() + phrase_bonus;
    };

    let dict_prior = w.dict_prior_scale * ((dict_weight as f32) + 50.0).ln();
    let mut chars_buf = ['\0'; 8];
    let mut n = 0usize;
    for ch in cand_text.chars().take(8) {
        chars_buf[n] = ch;
        n += 1;
    }
    if n == 0 {
        return dict_prior + phrase_bonus;
    }

    let mut uni_sum = 0.0f32;
    for &ch in &chars_buf[..n] {
        uni_sum += aux_v.unigram_prior(ch).unwrap_or(0.5);
    }
    let mean_uni = uni_sum / (n as f32);

    let mut internal_cohesion = 0.0f32;
    if n >= 2 {
        let mut bg_sum = 0.0f32;
        for i in 0..(n - 1) {
            bg_sum += aux_v.bigram_boost(chars_buf[i], chars_buf[i + 1]);
        }
        internal_cohesion = (bg_sum / ((n - 1) as f32)).min(4.5);
    }

    dict_prior
        + w.corpus_unigram_scale * mean_uni
        + w.internal_bigram_scale * internal_cohesion
        + phrase_bonus
}

#[inline]
fn compute_context_boost(
    user: &UserStore,
    aux: Option<&AuxView>,
    last_committed: &str,
    recent_context: &[char],
    cand_text: &str,
    w: &LmWeights,
) -> f32 {
    let mut boost = user.context_boost(last_committed, cand_text);
    let Some(aux_v) = aux else {
        return boost;
    };
    let Some(p1) = recent_context.last().copied() else {
        return boost;
    };
    let mut cand_chars = cand_text.chars();
    let Some(c0) = cand_chars.next() else {
        return boost;
    };

    let bg = aux_v.bigram_boost(p1, c0);
    if !aux_v.has_unigrams() {
        // Legacy WBAUX001 scale
        boost += 1.35 * bg;
        return boost;
    }

    let prev2 = if recent_context.len() >= 2 {
        Some(recent_context[recent_context.len() - 2])
    } else {
        None
    };
    let tg1 = prev2.map(|p2| aux_v.trigram_boost(p2, p1, c0)).unwrap_or(0.0);
    let tg_cross = cand_chars
        .next()
        .map(|c1| aux_v.trigram_boost(p1, c0, c1))
        .unwrap_or(0.0);

    boost += w.bigram_scale * bg + w.trigram_scale * tg1 + w.cross_trigram_scale * tg_cross;
    boost
}
