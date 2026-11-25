use crate::code::PackedCode;
use crate::config::JuxuConfig;
use crate::english::{resolve_english_display, EnDictView};
use crate::format::{AuxView, DictView};
use crate::neural::{NeuralView, RwkvState};
use crate::personalization::Context as PersonalContext;
use crate::typo::{
    generate_hypotheses_with_options, CorrectionKind, KeyLayout, KeyObs,
};
use crate::user_data::UserStore;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

const BEAM_WIDTH: usize = 8;
const MAX_CODE_LEN: usize = 5;
const MAX_EN_LEN: usize = 24;
const RETAINED_LAYERS: usize = 24;
const FULL_STATE_AGE: usize = 5;
const TOP1_STATE_AGE: usize = 16;
const COMMIT_DELAY: usize = 8;
const COMMIT_SCORE_MARGIN: f32 = 5.0;

pub(crate) struct Decoder<'a> {
    pub dict: &'a DictView,
    pub english: &'a EnDictView,
    pub raw_code: &'a str,
    pub aux: &'a AuxView,
    pub model: NeuralView<'a>,
    pub config: &'a JuxuConfig,
    pub user: &'a UserStore,
    pub context: &'a [char],
    pub layout: &'a KeyLayout,
}

#[derive(Clone, Copy)]
struct Boundary {
    keys: usize,
    text_bytes: usize,
    code: PackedCode,
    correction: CorrectionKind,
}

pub(crate) struct Segment {
    pub text: String,
    pub code: PackedCode,
    pub correction: CorrectionKind,
}

pub(crate) struct Path {
    pub text: String,
    pub score: f32,
    boundaries: Vec<Boundary>,
    state: Option<Rc<RwkvState>>,
    personal_context: PersonalContext,
}

impl Path {
    fn boundary_index(&self, boundary: Boundary) -> Option<usize> {
        let i = self
            .boundaries
            .binary_search_by_key(&boundary.keys, |b| b.keys)
            .ok()?;
        (self.boundaries[i].text_bytes == boundary.text_bytes).then_some(i)
    }

    pub fn tail_start(&self) -> usize {
        self.boundaries.iter().rev().nth(1).map_or(0, |b| b.keys)
    }

    pub fn segments(&self, keys: usize) -> Vec<Segment> {
        let mut start = 0;
        self.boundaries
            .iter()
            .take_while(|boundary| boundary.keys <= keys)
            .map(|boundary| {
                let segment = Segment {
                    text: self.text[start..boundary.text_bytes].to_owned(),
                    code: boundary.code,
                    correction: boundary.correction,
                };
                start = boundary.text_bytes;
                segment
            })
            .collect()
    }

    pub fn correction_hint(&self, keys: usize) -> String {
        self.boundaries
            .iter()
            .take_while(|boundary| boundary.keys <= keys)
            .filter(|boundary| boundary.correction != CorrectionKind::Exact)
            .map(|boundary| boundary.code.as_string())
            .collect::<Vec<_>>()
            .join("·")
    }

    fn code_segments<'a>(&self, code: &'a str) -> Vec<&'a str> {
        let mut start = 0;
        let mut groups = Vec::new();
        for boundary in self.boundaries.iter().take_while(|b| b.keys <= code.len()) {
            groups.push(&code[start..boundary.keys]);
            start = boundary.keys;
        }
        if start < code.len() {
            groups.push(&code[start..]);
        }
        groups
    }
}

pub(crate) struct StablePrefix {
    pub keys: usize,
    pub segments: Vec<Segment>,
    pub codes: Vec<String>,
}

#[derive(Default)]
pub(crate) struct Stream {
    layers: VecDeque<Vec<Path>>,
    // Number of keys represented by the oldest retained layer, relative to the pending code.
    first: usize,
}

impl Stream {
    pub fn update(&mut self, observations: &[KeyObs], decoder: &Decoder<'_>, initial: &RwkvState) {
        while !self.layers.is_empty() && self.first + self.layers.len() > observations.len() + 1 {
            self.layers.pop_back();
        }
        let need_tail_rebuild = self.first == 0
            && self.layers.len() > 1
            && self.first + self.layers.len() <= observations.len()
            && self
                .layers
                .iter()
                .rev()
                .take(MAX_CODE_LEN)
                .any(|layer| layer.iter().skip(1).any(|p| p.state.is_none()));
        if self.layers.is_empty()
            || (self.first > 0 && self.layers.len() < MAX_CODE_LEN)
            || need_tail_rebuild
        {
            self.first = 0;
            self.layers.clear();
            self.layers.push_back(vec![Path {
                text: String::new(),
                score: 0.0,
                boundaries: Vec::new(),
                state: Some(Rc::new(initial.clone())),
                personal_context: PersonalContext::with_chars(decoder.context.iter().copied()),
            }]);
        }
        for end in self.first + self.layers.len()..=observations.len() {
            let next = self.extend(decoder, &observations[..end]);
            self.layers.push_back(next);
            if self.layers.len() > RETAINED_LAYERS {
                self.layers.pop_front();
                self.first += 1;
            }
        }
        self.evict_old_states(FULL_STATE_AGE, TOP1_STATE_AGE);
    }

    fn evict_old_states(&mut self, full_age: usize, top1_age: usize) {
        let len = self.layers.len();
        for (idx, layer) in self.layers.iter_mut().enumerate() {
            let age = len.saturating_sub(1 + idx);
            if age > top1_age {
                for p in layer.iter_mut() {
                    p.state = None;
                }
            } else if age > full_age {
                for p in layer.iter_mut().skip(1) {
                    p.state = None;
                }
            }
        }
    }

    pub fn trim_memory(&mut self) {
        if self.layers.len() <= 1 {
            self.layers = VecDeque::new();
            self.first = 0;
        } else {
            self.evict_old_states(4, 8);
            self.layers.shrink_to_fit();
        }
    }

    pub fn live_state_count(&self) -> usize {
        let mut ptrs = Vec::new();
        for layer in &self.layers {
            for p in layer {
                if let Some(ref rc) = p.state {
                    ptrs.push(Rc::as_ptr(rc) as usize);
                }
            }
        }
        ptrs.sort_unstable();
        ptrs.dedup();
        ptrs.len()
    }

    fn layer(&self, pos: usize) -> &[Path] {
        pos.checked_sub(self.first)
            .and_then(|index| self.layers.get(index))
            .map_or(&[], Vec::as_slice)
    }

    pub fn paths(&self) -> &[Path] {
        self.layers.back().map_or(&[], Vec::as_slice)
    }

    pub fn best_state(&self) -> Option<&RwkvState> {
        self.layers
            .iter()
            .rev()
            .find_map(|layer| layer.first().and_then(|p| p.state.as_deref()))
    }

    pub fn preview(&self, code: &str) -> String {
        for (index, layer) in self.layers.iter().enumerate().rev() {
            if let Some(best) = layer.first() {
                let mut text = best.text.clone();
                text.push_str(&code[self.first + index..]);
                return text;
            }
        }
        code.to_owned()
    }

    pub fn segmented_code(&self, code: &str) -> String {
        self.layers
            .iter()
            .rev()
            .find_map(|layer| layer.first())
            .map_or_else(
                || code.to_owned(),
                |best| best.code_segments(code).join("·"),
            )
    }

    /// Include hypotheses that end up to three strokes earlier, if their remaining strokes
    /// can still be the beginning of a longer code.
    pub fn take_stable_prefix(
        &mut self,
        code: &str,
        dict: &DictView,
        english: &EnDictView,
        user: &UserStore,
        config: &JuxuConfig,
    ) -> Option<StablePrefix> {
        let limit = code.len().checked_sub(COMMIT_DELAY)?;
        // Protect in-progress long English words from being chopped mid-word by COMMIT_DELAY.
        let tail_start = code.len().saturating_sub(MAX_EN_LEN);
        for start in tail_start..=limit {
            let slice = &code[start..];
            if slice.len() >= 3 {
                let lower = slice.to_ascii_lowercase();
                if slice.bytes().any(|b| b.is_ascii_uppercase())
                    || user.has_english_prefix(&lower)
                    || english
                        .prefix_info(&lower)
                        .is_some_and(|(w, _)| w >= 24_000)
                {
                    return None;
                }
            }
        }
        let best = self.paths().first()?;
        let mut cuts: Vec<Boundary> = best
            .boundaries
            .iter()
            .copied()
            .take_while(|b| b.keys <= limit)
            .collect();
        let max_code_len = config.schema.max_code_len();
        for tail_len in 0..max_code_len {
            if cuts.is_empty() {
                return None;
            }
            let end = code.len() - tail_len;
            // An invalid raw tail can still grow into a corrected code on the next key.
            if tail_len > 0
                && !config.typo.enabled
                && dict.valid_next_letters_mask(&code.as_bytes()[end..], config.enable_rare_charset)
                    == 0
                && user.valid_next_letters_mask(&code.as_bytes()[end..]) == 0
            {
                continue;
            }
            let layer = self.layer(end);
            if let Some(leader) = layer.first() {
                for path in layer
                    .iter()
                    .take_while(|p| p.score >= leader.score - COMMIT_SCORE_MARGIN)
                {
                    // Different word boundaries can still arrive at the same text and code cut.
                    cuts.retain(|cut| {
                        path.text.starts_with(&best.text[..cut.text_bytes])
                            && path.boundary_index(*cut).is_some()
                    });
                }
            }
        }
        let boundary = *cuts.last()?;
        let prefix = best.text[..boundary.text_bytes].to_owned();
        let segments = best.segments(boundary.keys);
        let codes = best
            .code_segments(&code[..boundary.keys])
            .into_iter()
            .map(str::to_owned)
            .collect();
        let score_base = best.score;
        while self.first < boundary.keys {
            self.layers.pop_front();
            self.first += 1;
        }
        self.first -= boundary.keys;
        for layer in &mut self.layers {
            layer.retain_mut(|path| {
                let Some(boundary_index) = path.boundary_index(boundary) else {
                    return false;
                };
                if !path.text.starts_with(&prefix) {
                    return false;
                }
                path.text.drain(..boundary.text_bytes);
                path.boundaries.drain(..=boundary_index);
                for b in &mut path.boundaries {
                    b.keys -= boundary.keys;
                    b.text_bytes -= boundary.text_bytes;
                }
                path.score -= score_base;
                true
            });
        }
        Some(StablePrefix {
            keys: boundary.keys,
            segments,
            codes,
        })
    }

    fn extend(&self, decoder: &Decoder<'_>, observations: &[KeyObs]) -> Vec<Path> {
        struct Step<'a> {
            parent: &'a Path,
            parent_key: (usize, usize), // (layer_pos, parent_idx) for trie-shared prefix state cache
            text: String,
            first: char,
            n_chars: usize,
            is_latin: bool,
            estimate: f32,
            bias: f32,
            code: PackedCode,
            correction: CorrectionKind,
        }

        let mut steps = Vec::new();
        let end = observations.len();
        let personal = &decoder.user.continuations;
        let personalize = decoder.config.dynamic_word_freq && !personal.is_empty();
        let use_personal_states = personalize && personal.matches_model(&decoder.model);
        let max_code_len = decoder.config.schema.max_code_len();
        for len in 1..=end.min(max_code_len) {
            let parent_pos = end - len;
            let parents = self.layer(parent_pos);
            if parents.is_empty() {
                continue;
            }
            let window = &observations[parent_pos..];
            if window.iter().any(|o| o.is_upper) {
                continue;
            }
            let hypotheses = generate_hypotheses_with_options(
                window,
                decoder.layout,
                &decoder.user.touch_profile,
                &decoder.config.typo,
                decoder.config.schema.allows_z_code(),
            );
            let exact_logp = hypotheses
                .iter()
                .find(|hypothesis| hypothesis.kind == CorrectionKind::Exact)
                .map_or(0.0, |hypothesis| hypothesis.spatial_logp);
            let entries = hypotheses
                .into_iter()
                .filter(|hypothesis| {
                    // With a one-letter shortcut, require touch evidence before trying another key.
                    len > 1 || hypothesis.kind != CorrectionKind::NeighborKey
                })
                .flat_map(|hypothesis| {
                    let key = hypothesis.code;
                    let correction = hypothesis.kind;
                    let exact = correction == CorrectionKind::Exact;
                    let relative_logp = if correction == CorrectionKind::Transposition {
                        hypothesis.spatial_logp
                    } else {
                        hypothesis.spatial_logp - exact_logp
                    };
                    let cost = match correction {
                        CorrectionKind::Exact => 0.0,
                        // Touch-boundary taps already pay continuous 2D Gaussian `relative_logp`;
                        // avoid double-counting frontend `hitTest` priors with a steep step penalty.
                        CorrectionKind::TouchBoundary if len > 1 => 1.1,
                        _ => 4.5,
                    };
                    let penalty = cost - 1.65 * relative_logp.min(0.0);
                    let hits = decoder
                        .dict
                        .lookup_exact(key, decoder.config.enable_rare_charset);
                    let top_weight = hits.first().map_or(1, |e| e.weight.max(1));
                    let users = decoder.user.lookup_exact_detailed(key);
                    let limit = if exact { [1, 2, 4, 6][len - 1] } else { 2 };
                    hits.into_iter()
                        .enumerate()
                        .take(if exact && personalize { 24 } else { limit })
                        .filter(move |(rank, entry)| {
                            *rank < limit
                                || parents.iter().any(|parent| {
                                    let score = personal.score(parent.personal_context, entry.text);
                                    score >= 0.6
                                        || (use_personal_states
                                            && parent.state.as_deref().is_some_and(|pst| {
                                                entry.text.chars().next().is_some_and(|first| {
                                                    personal.state_bonus(
                                                        parent.personal_context,
                                                        first,
                                                        pst,
                                                    ) + score
                                                        >= 0.6
                                                })
                                            }))
                                })
                        })
                        .take(limit + 2)
                        .map(move |(rank, entry)| {
                            (rank, entry.text, entry.weight, top_weight, false, 0u32, false)
                        })
                        .chain(
                            users
                                .into_iter()
                                .take(if exact { 4 } else { 2 })
                                .enumerate()
                                .map(|(rank, (text, cnt, _, auto_comp))| {
                                    (rank, text, 500, 500, true, cnt, auto_comp)
                                }),
                        )
                        .map(move |entry| (entry, key, correction, penalty))
                });
            for ((rank, text, weight, top_weight, custom, user_cnt, user_auto), code, correction, penalty) in entries {
                let mut chars = text.chars();
                let Some(first) = chars.next() else { continue };
                let second = chars.next();
                let n_chars = 1 + usize::from(second.is_some()) + chars.count();
                let mut bias = (n_chars.min(2) as f32) * 0.32 - 0.40 - penalty;
                let mut has_shorter_primary = false;
                if n_chars == 1 {
                    if let Some(meta) = decoder.dict.char_meta(first) {
                        let short_len = meta.primary_short_code.len();
                        if short_len > 0 && short_len < len {
                            has_shorter_primary = true;
                            if decoder.config.strict_shortcode {
                                // Keep longer codes available, but prefer the user's normal short-code path.
                                bias -= 2.5 + 0.5 * (len - short_len - 1) as f32;
                            } else {
                                // Mild preference for primary shortcode over redundant full code in spaceless segmentation.
                                bias -= 0.75 + 0.35 * (len - short_len - 1) as f32;
                            }
                        }
                    }
                }
                let log_gap = if !custom && top_weight >= 10_000 {
                    (((top_weight as f32) + 1_000.0) / ((weight as f32) + 1_000.0))
                        .ln()
                        .max(0.0)
                } else {
                    0.0
                };
                bias -= if len <= 2 {
                    1.6 * rank as f32
                } else if top_weight >= 10_000 && log_gap <= 1.8 {
                    0.68 * (rank as f32).ln_1p()
                } else {
                    0.8 * (rank as f32).ln_1p()
                };
                bias -= if custom {
                    0.0
                } else if top_weight < 10_000 {
                    if weight < 50 {
                        2.5
                    } else if weight < 500 {
                        0.8
                    } else {
                        0.0
                    }
                } else {
                    let cliff = 0.72 * (log_gap - 0.9).max(0.0).min(6.5);
                    let rare_tail = if weight < 50_000 { 1.2 } else { 0.0 };
                    cliff + rare_tail
                };
                bias -= 6.0
                    * text
                        .chars()
                        .filter(|&c| decoder.model.char_to_token(c) <= 1)
                        .count() as f32;
                if let Some(second) = second {
                    // Light tie-breaking cohesion since RWKV V2 provides primary contextual LM in nats
                    bias += 0.02 * decoder.aux.bigram_boost(first, second);
                    if len == max_code_len && (rank == 0 || log_gap <= 1.5) {
                        bias += if rank == 0 { 0.72 } else { 0.32 };
                    }
                }
                if custom {
                    bias += if !user_auto || user_cnt >= 3 {
                        2.5
                    } else if user_cnt == 2 {
                        1.6
                    } else {
                        0.85
                    };
                }
                for (parent_idx, parent) in parents.iter().enumerate() {
                    let Some(parent_state) = parent.state.as_deref() else {
                        continue;
                    };
                    let mut preceding = parent
                        .text
                        .chars()
                        .rev()
                        .chain(decoder.context.iter().rev().copied());
                    let mut contextual_bias = bias;
                    if let Some(prev) = preceding.next() {
                        // Reduced static N-gram weight because RWKV V2 is active in nats
                        contextual_bias += 0.05 * decoder.aux.bigram_boost(prev, first);
                        if let Some(prev2) = preceding.next() {
                            contextual_bias += 0.05 * decoder.aux.trigram_boost(prev2, prev, first);
                        }
                    }
                    // Anti-redundant split in spaceless shape typing: when `first` already has a shorter
                    // primary shortcode (e.g. `ftjf` "者" vs 3-key `ftj`), spending > max_code_len keys across
                    // two single-char segments (`ak` + `ftjf` = 6 keys) to spell a common 4-key phrase ("或者" = `akft`)
                    // is strictly redundant and steals keys from the following word (`jfdw` "时期").
                    if n_chars == 1 && has_shorter_primary {
                        if let Some(b_last) = parent.boundaries.last() {
                            let prev_k0 = parent
                                .boundaries
                                .iter()
                                .rev()
                                .nth(1)
                                .map_or(0, |b| b.keys);
                            let prev_b0 = parent
                                .boundaries
                                .iter()
                                .rev()
                                .nth(1)
                                .map_or(0, |b| b.text_bytes);
                            let prev_keys = b_last.keys.saturating_sub(prev_k0);
                            if prev_keys + len > max_code_len {
                                let prev_seg = &parent.text[prev_b0..b_last.text_bytes];
                                let mut pc = prev_seg.chars();
                                if let (Some(c0), None) = (pc.next(), pc.next()) {
                                    let mut pair_buf = [0u8; 8];
                                    let n0 = c0.encode_utf8(&mut pair_buf).len();
                                    let n1 = first.encode_utf8(&mut pair_buf[n0..]).len();
                                    if let Ok(pair_s) = core::str::from_utf8(&pair_buf[..n0 + n1]) {
                                        if decoder
                                            .dict
                                            .contains_exact_word(pair_s)
                                            .is_some_and(|w| w >= 500_000)
                                        {
                                            contextual_bias -= 5.0;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let raw_lp1 = decoder.model.score_next_char(parent_state, first);
                    let lp1 = if personalize {
                        personal.interpolate_logp(
                            raw_lp1,
                            parent.personal_context,
                            first,
                            use_personal_states.then_some(parent_state),
                        )
                    } else {
                        raw_lp1
                    };
                    let estimate = if n_chars == 1 {
                        lp1
                    } else {
                        let len_scale = (n_chars.min(MAX_CODE_LEN) as f32).powf(0.65);
                        let fast_total = decoder.model.score_phrase_fast(parent_state, text) * len_scale;
                        // Replace c1's scaled raw logp with Neural-Cache-interpolated lp1
                        fast_total + (lp1 - raw_lp1) * len_scale
                    };
                    steps.push(Step {
                        parent,
                        parent_key: (parent_pos, parent_idx),
                        text: text.to_owned(),
                        first,
                        n_chars,
                        is_latin: false,
                        estimate: parent.score + contextual_bias + estimate,
                        bias: contextual_bias,
                        code,
                        correction,
                    });
                }
            }
        }

        // Mixed Chinese-English: allow dictionary, user-learned, uppercase-anchored, and empty-code OOV Latin words
        if decoder.raw_code.len() >= end {
            for len in 1..=end.min(MAX_EN_LEN) {
                let parent_pos = end - len;
                let parents = self.layer(parent_pos);
                if parents.is_empty() {
                    continue;
                }
                let raw_slice = &decoder.raw_code[parent_pos..end];
                if !raw_slice.bytes().all(|b| b.is_ascii_alphabetic()) {
                    continue;
                }
                let has_upper = observations[parent_pos..end].iter().any(|o| o.is_upper);
                let starts_upper = observations[parent_pos].is_upper;
                let lower = raw_slice.to_ascii_lowercase();
                if has_upper && !starts_upper {
                    let known = decoder.user.lookup_english_exact(&lower).is_some()
                        || decoder.english.lookup_exact(&lower).is_some();
                    if !known {
                        continue;
                    }
                }
                let user_hit = decoder.user.lookup_english_exact(&lower);
                let dict_hit = if len >= 2 || starts_upper {
                    decoder.english.lookup_with_morphology(&lower)
                } else {
                    None
                };

                let latin_cand: Option<(String, f32)> = if starts_upper {
                    let is_known = user_hit.is_some() || dict_hit.is_some();
                    let bonus = if is_known { 3.5 } else { 1.8 };
                    if is_known || len >= 2 || end == decoder.raw_code.len() {
                        Some((raw_slice.to_owned(), 1.2 * (len as f32) + bonus))
                    } else {
                        None
                    }
                } else if len >= 2 && (user_hit.is_some() || dict_hit.is_some()) {
                    let is_user = user_hit.is_some();
                    let weight = if let Some((_, cnt)) = user_hit {
                        let dict_w = dict_hit.as_ref().map_or(46_000u32, |x| x.1 as u32);
                        (dict_w.max(52_000) + cnt.min(10) * 1_200).min(65_535) as u16
                    } else {
                        dict_hit.as_ref().unwrap().1
                    };
                    // Zero-hardcode probabilistic + code-space boundary rule:
                    // - Cold-start (`!is_user && !has_upper`):
                    //   * Never hijack valid Wubi `#1` blind typing on `<= max_code_len` codes!
                    //     When `len <= max_code_len` (`2..=4`), only enter the `#1`-competing beam if
                    //     `parent_pos == 0` AND the Wubi dictionary has NO exact entry for `lower`
                    //     (truly empty Wubi code; colliding short English words surface in `#2/#3` of
                    //     `show_spaceless_candidates`, and promote into `user_hit` after 1 selection!).
                    //   * For `len > max_code_len` (`>= 5` letters, impossible single Wubi code), any
                    //     sufficiently common corpus word (`weight >= 36_000`) competes directly via RWKV + prior.
                    // - User-learned (`is_user == true`, 1-shot learned across any schema):
                    //   * Competes directly in the beam for `len >= 2` via unified RWKV logp + personal prior!
                    let wubi_has_exact = len <= max_code_len
                        && PackedCode::from_ascii(&lower)
                            .is_some_and(|c| !decoder.dict.lookup_exact(c, false).is_empty());
                    let cold_allowed = if len > max_code_len {
                        weight >= 36_000
                    } else {
                        len >= 3 && parent_pos == 0 && !wubi_has_exact && weight >= 42_000
                    };
                    if !has_upper && !is_user && !cold_allowed {
                        None
                    } else {
                        let display = resolve_english_display(
                            raw_slice,
                            user_hit.map(|x| x.0),
                            dict_hit.as_ref().map(|x| x.0.as_str()),
                        );
                        let user_cnt = user_hit.map_or(0, |x| x.1);
                        let w_prior = ((weight as f32) / 65535.0) * 1.2
                            + if is_user {
                                1.35 + 0.45 * ((user_cnt as f32) + 1.0).ln()
                            } else {
                                0.0
                            };
                        let len_prior = if len > max_code_len {
                            0.55 * (len as f32)
                        } else {
                            0.25 * (len as f32)
                        };
                        let switch_cost = if has_upper {
                            0.0
                        } else if is_user && len <= max_code_len && wubi_has_exact {
                            2.55
                        } else if parent_pos > 0 && !is_user {
                            2.25
                        } else {
                            1.85
                        };
                        Some((display, len_prior + w_prior - switch_cost))
                    }
                } else {
                    None
                };

                if let Some((display, bias)) = latin_cand {
                    let Some(first) = display.chars().next() else {
                        continue;
                    };
                    let n_chars = display.chars().count();
                    for (parent_idx, parent) in parents.iter().enumerate() {
                        let Some(parent_state) = parent.state.as_deref() else {
                            continue;
                        };
                        let est = decoder.model.estimate_latin_logp(parent_state, &display);
                        steps.push(Step {
                            parent,
                            parent_key: (parent_pos, parent_idx),
                            text: display.clone(),
                            first,
                            n_chars,
                            is_latin: true,
                            estimate: parent.score + bias + est,
                            bias,
                            code: PackedCode::EMPTY,
                            correction: CorrectionKind::Exact,
                        });
                    }
                }
            }
        }

        steps.sort_unstable_by(|a, b| b.estimate.total_cmp(&a.estimate));

        // Trie-shared prefix state refinement for top candidates:
        // Share `step_token(c1)` across multi-char candidates sharing `(parent_key, first)`
        // so 2-char phrases get 100% exact RWKV probability and 3-char phrases get exact c1,c2 + MTP-2 c3!
        let refine_limit = (BEAM_WIDTH * 2).min(steps.len());
        let mut prefix_cache: HashMap<(usize, usize, char), RwkvState> =
            HashMap::with_capacity(refine_limit);
        for step in &mut steps[..refine_limit] {
            if !step.is_latin && step.n_chars >= 2 {
                let Some(parent_state) = step.parent.state.as_deref() else {
                    continue;
                };
                let cache_key = (step.parent_key.0, step.parent_key.1, step.first);
                let state_after_c1 = prefix_cache.entry(cache_key).or_insert_with(|| {
                    let mut st = parent_state.clone();
                    decoder
                        .model
                        .step_token(decoder.model.char_to_token(step.first), &mut st);
                    st
                });
                let raw_lp1 = decoder.model.score_next_char(parent_state, step.first);
                let lp1 = if personalize {
                    personal.interpolate_logp(
                        raw_lp1,
                        step.parent.personal_context,
                        step.first,
                        use_personal_states.then_some(parent_state),
                    )
                } else {
                    raw_lp1
                };
                let rest = &step.text[step.first.len_utf8()..];
                let rest_chars = step.n_chars - 1;
                let rest_est = if rest_chars == 1 {
                    let c2 = rest.chars().next().unwrap();
                    let raw_lp2 = decoder.model.score_next_char(state_after_c1, c2);
                    if personalize {
                        let ctx2 = step.parent.personal_context.push(step.first);
                        personal.interpolate_logp(
                            raw_lp2,
                            ctx2,
                            c2,
                            use_personal_states.then_some(state_after_c1),
                        )
                    } else {
                        raw_lp2
                    }
                } else {
                    decoder.model.score_phrase_fast(state_after_c1, rest)
                        * (rest_chars.min(MAX_CODE_LEN) as f32).powf(0.65)
                };
                step.estimate = step.parent.score + step.bias + lp1 + rest_est;
            }
        }
        steps[..refine_limit].sort_unstable_by(|a, b| b.estimate.total_cmp(&a.estimate));

        let mut paths: Vec<Path> = Vec::with_capacity(BEAM_WIDTH * 2);
        for step in steps.into_iter().take(BEAM_WIDTH * 2) {
            let Some(parent_state) = step.parent.state.as_deref() else {
                continue;
            };
            let mut score = step.parent.score + step.bias;
            let mut personal_context = step.parent.personal_context;
            let state = if step.is_latin {
                let (lp, st) = decoder
                    .model
                    .score_and_step_latin(parent_state, &step.text, true);
                score += lp;
                for ch in step.text.chars() {
                    personal_context = personal_context.push(ch);
                }
                st
            } else {
                let raw_lp1 = decoder.model.score_next_char(parent_state, step.first);
                score += if personalize {
                    personal.interpolate_logp(
                        raw_lp1,
                        personal_context,
                        step.first,
                        use_personal_states.then_some(parent_state),
                    )
                } else {
                    raw_lp1
                };
                let cache_key = (step.parent_key.0, step.parent_key.1, step.first);
                let mut st = if let Some(cached) = prefix_cache.get(&cache_key) {
                    cached.clone()
                } else {
                    let mut s = parent_state.clone();
                    decoder
                        .model
                        .step_token(decoder.model.char_to_token(step.first), &mut s);
                    s
                };
                personal_context = personal_context.push(step.first);
                for ch in step.text.chars().skip(1) {
                    let raw_lp = decoder.model.score_next_char(&st, ch);
                    score += if personalize {
                        personal.interpolate_logp(
                            raw_lp,
                            personal_context,
                            ch,
                            use_personal_states.then_some(&st),
                        )
                    } else {
                        raw_lp
                    };
                    decoder
                        .model
                        .step_token(decoder.model.char_to_token(ch), &mut st);
                    personal_context = personal_context.push(ch);
                }
                st
            };
            let mut text = step.parent.text.clone();
            text.push_str(&step.text);
            let mut boundaries = step.parent.boundaries.clone();
            boundaries.push(Boundary {
                keys: end,
                text_bytes: text.len(),
                code: step.code,
                correction: step.correction,
            });
            let path = Path {
                text,
                score,
                boundaries,
                state: Some(Rc::new(state)),
                personal_context,
            };
            if let Some(existing) = paths.iter_mut().find(|p| p.text == path.text) {
                if path.score > existing.score {
                    *existing = path;
                }
            } else {
                paths.push(path);
            }
        }
        paths.sort_unstable_by(|a, b| b.score.total_cmp(&a.score));
        paths.truncate(BEAM_WIDTH);
        paths
    }
}
