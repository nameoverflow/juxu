//! Full Pinyin sentence decoder with:
//! - Toneless syllable lattice + initial-only abbreviations (`srf`, `shrf`, `shurf`)
//! - Apostrophe syllable separator (`xi'an`)
//! - Touch-boundary, physical-neighbor, and adjacent-transposition typo correction
//! - `Micro-RWKV-7` + `AuxView` N-gram + `PersonalContext` beam search

use crate::code::PackedCode;
use crate::config::WubiConfig;
use crate::engine::{Candidate, CandidateBadge};
use crate::format::AuxView;
use crate::neural::{NeuralView, RwkvState};
use crate::personalization::Context as PersonalContext;
use crate::pinyin_dict::{base_initial_id, PinyinDictView, SylConstraint};
use crate::typo::{key_choices_for_obs, softmax_confidences, CorrectionKind, KeyLayout, KeyObs};
use crate::user_data::UserStore;
use std::collections::HashSet;

const BEAM_WIDTH: usize = 8;
const MAX_SYL_LEN: usize = 6;
const MAX_PHRASE_SYLS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SylMatch {
    Full(u16),
    Initial(u8, bool),
    Prefix(String),
}

#[derive(Clone, Debug)]
struct LatticeEdge {
    start: usize,
    end: usize,
    syl: SylMatch,
    spatial_logp: f32,
    kind_cost: f32,
    correction: CorrectionKind,
    display_syl: String,
}

#[derive(Clone)]
pub struct PinyinBoundary {
    pub keys: usize,
    pub text_bytes: usize,
    pub syl_display: String,
    pub correction: CorrectionKind,
}

#[derive(Clone)]
pub struct PinyinPath {
    pub text: String,
    pub score: f32,
    pub boundaries: Vec<PinyinBoundary>,
    pub state: RwkvState,
    pub(crate) personal_context: PersonalContext,
    pub any_corrected: bool,
}

impl PinyinPath {
    pub fn segmented_code(&self, raw: &str) -> String {
        let mut parts = Vec::with_capacity(self.boundaries.len() + 1);
        for b in &self.boundaries {
            parts.push(b.syl_display.as_str());
        }
        let covered = self.boundaries.last().map_or(0, |b| b.keys);
        if covered < raw.len() {
            parts.push(&raw[covered..]);
        }
        parts.join("'")
    }

    pub fn correction_hint(&self) -> String {
        let mut hints = Vec::new();
        let mut start = 0usize;
        for b in &self.boundaries {
            let seg_text = self.text.get(start..b.text_bytes).unwrap_or("");
            start = b.text_bytes;
            if b.correction != CorrectionKind::Exact && !seg_text.is_empty() {
                hints.push(format!("{}({})", seg_text, b.syl_display));
            }
        }
        hints.join(" ")
    }
}

pub struct PinyinCandidate {
    pub candidate: Candidate,
    pub consumed_keys: usize,
    pub boundaries: Vec<PinyinBoundary>,
}

pub struct PinyinDecoder<'a> {
    pub dict: &'a PinyinDictView,
    pub aux: Option<&'a AuxView>,
    pub model: Option<NeuralView<'a>>,
    pub config: &'a WubiConfig,
    pub user: &'a UserStore,
    pub context: &'a [char],
    pub layout: &'a KeyLayout,
}

fn build_lattice(
    observations: &[KeyObs],
    decoder: &PinyinDecoder<'_>,
) -> Vec<Vec<LatticeEdge>> {
    let n = observations.len();
    let mut edges_by_end = vec![Vec::<LatticeEdge>::new(); n + 1];
    if n == 0 {
        return edges_by_end;
    }

    let per_pos_choices: Vec<Vec<(u8, f32, bool, bool)>> = observations
        .iter()
        .map(|obs| {
            if obs.ch == b'\'' {
                Vec::new()
            } else {
                key_choices_for_obs(obs, decoder.layout, &decoder.user.touch_profile, &decoder.config.typo, true)
            }
        })
        .collect();

    for start in 0..n {
        // Skip leading apostrophes transparently
        if observations[start].ch == b'\'' {
            continue;
        }
        let mut effective_start = start;
        // If previous char was apostrophe, `start` is a fresh syllable start;
        // also allow edges starting before an apostrophe to end right after it.
        let _ = &mut effective_start;

        let max_len = (n - start).min(MAX_SYL_LEN + 1);
        for len in 1..=max_len {
            let end = start + len;
            if observations[start..end].iter().any(|o| o.ch == b'\'') {
                break;
            }
            let exact_bytes: Vec<u8> = observations[start..end].iter().map(|o| o.ch).collect();
            let exact_str = String::from_utf8_lossy(&exact_bytes).into_owned();

            // Generate candidate strings for `start..end`
            // (bytes_str, spatial_logp, correction_kind, extra_cost)
            let mut hyps: Vec<(String, f32, CorrectionKind, f32)> = Vec::new();
            let exact_lp: f32 = (start..end)
                .map(|i| {
                    per_pos_choices[i]
                        .iter()
                        .find(|c| c.2)
                        .map_or(0.0, |c| c.1)
                })
                .sum();
            if len <= MAX_SYL_LEN {
                hyps.push((exact_str.clone(), exact_lp, CorrectionKind::Exact, 0.0));
            }

            // Compatible v->u normalization after j/q/x/y (e.g. jv->ju, jve->jue, jvan->juan, jvn->jun)
            if (2..=4).contains(&len)
                && matches!(exact_bytes[0], b'j' | b'q' | b'x' | b'y')
                && exact_bytes[1] == b'v'
            {
                let mut norm = exact_bytes.clone();
                norm[1] = b'u';
                let norm_str = String::from_utf8_lossy(&norm).into_owned();
                hyps.push((norm_str, exact_lp, CorrectionKind::TouchBoundary, -0.35));
            }

            if decoder.config.typo.enabled {
                if len <= MAX_SYL_LEN {
                    // Single-position substitution
                    for pos in 0..len {
                        let obs_idx = start + pos;
                        for &(d, lp, is_exact, is_boundary) in &per_pos_choices[obs_idx] {
                            if is_exact {
                                continue;
                            }
                            if len == 1 && !is_boundary {
                                continue;
                            }
                            let mut b = exact_bytes.clone();
                            b[pos] = b'a' + d - 1;
                            let s = String::from_utf8_lossy(&b).into_owned();
                            let other_lp: f32 = (start..end)
                                .enumerate()
                                .filter(|&(j, _)| j != pos)
                                .map(|(_j, idx)| {
                                    per_pos_choices[idx]
                                        .iter()
                                        .find(|c| c.2)
                                        .map_or(0.0, |c| c.1)
                                })
                                .sum();
                            let kind = if is_boundary {
                                CorrectionKind::TouchBoundary
                            } else {
                                CorrectionKind::NeighborKey
                            };
                            hyps.push((s, other_lp + lp, kind, 0.0));
                        }
                    }
                    // Adjacent transposition within syllable (`len >= 2`)
                    if len >= 2 {
                        for pos in 0..(len - 1) {
                            if exact_bytes[pos] == exact_bytes[pos + 1] {
                                continue;
                            }
                            let mut b = exact_bytes.clone();
                            b.swap(pos, pos + 1);
                            let s = String::from_utf8_lossy(&b).into_owned();
                            let dt = observations[start + pos + 1]
                                .timestamp_ms
                                .saturating_sub(observations[start + pos].timestamp_ms);
                            let trans_lp =
                                if observations[start + pos + 1].timestamp_ms > 0 && dt <= 65 {
                                    -2.45
                                } else {
                                    -3.45
                                };
                            hyps.push((s, trans_lp, CorrectionKind::Transposition, 0.0));
                        }
                    }
                }

                // Missing letter (omission):
                // 1) Missing trailing 'g' in -ng finals (an->ang, en->eng, in->ing, ian->iang, uan->uang)
                if (2..=5).contains(&len)
                    && (exact_str.ends_with("an")
                        || exact_str.ends_with("en")
                        || exact_str.ends_with("in"))
                {
                    let mut with_g = exact_str.clone();
                    with_g.push('g');
                    if decoder.dict.lookup_syllable_id(&with_g).is_some() {
                        hyps.push((with_g, exact_lp, CorrectionKind::NeighborKey, -2.0));
                    }
                }
                // 2) Missing 1 letter anywhere inside a syllable (e.g. zhog->zhong, xio->xiao)
                if (2..=5).contains(&len) && decoder.dict.lookup_syllable_id(&exact_str).is_none() {
                    let target_len = len + 1;
                    let first_ch = exact_bytes[0];
                    for sid in 0..decoder.dict.syllable_count() {
                        let cand = decoder.dict.syllable_str(sid as u16);
                        let cb = cand.as_bytes();
                        if cb.len() == target_len && cb[0] == first_ch {
                            let mut i = 0usize;
                            for &b in cb {
                                if i < len && b == exact_bytes[i] {
                                    i += 1;
                                }
                            }
                            if i == len {
                                hyps.push((
                                    cand.to_owned(),
                                    exact_lp,
                                    CorrectionKind::NeighborKey,
                                    -4.4,
                                ));
                            }
                        }
                    }
                }

                // Extra letter (insertion / fat-finger double-tap, e.g. zhonng->zhong, womren->women)
                if (3..=7).contains(&len)
                    && (len > MAX_SYL_LEN || decoder.dict.lookup_syllable_id(&exact_str).is_none())
                {
                    for drop_pos in 0..len {
                        let mut b = Vec::with_capacity(len - 1);
                        b.extend_from_slice(&exact_bytes[..drop_pos]);
                        b.extend_from_slice(&exact_bytes[drop_pos + 1..]);
                        let s = String::from_utf8_lossy(&b).into_owned();
                        if decoder.dict.lookup_syllable_id(&s).is_some() {
                            let is_repeat = (drop_pos > 0
                                && exact_bytes[drop_pos] == exact_bytes[drop_pos - 1])
                                || (drop_pos + 1 < len
                                    && exact_bytes[drop_pos] == exact_bytes[drop_pos + 1]);
                            let cost = if is_repeat { -3.2 } else { -4.4 };
                            hyps.push((s, exact_lp, CorrectionKind::NeighborKey, cost));
                        }
                    }
                }
            }

            // Include trailing apostrophe in `end` if immediately followed by `'`
            let actual_end = if end < n && observations[end].ch == b'\'' {
                end + 1
            } else {
                end
            };

            for (s, spatial_logp, correction, extra_cost) in hyps {
                let typo_cost = if extra_cost < 0.0 {
                    extra_cost
                } else {
                    match correction {
                        CorrectionKind::Exact => 0.0,
                        CorrectionKind::TouchBoundary => -1.8,
                        CorrectionKind::NeighborKey => -4.8,
                        CorrectionKind::Transposition => -3.6,
                    }
                };
                if let Some(sid) = decoder.dict.lookup_syllable_id(&s) {
                    let full_cost = if correction == CorrectionKind::Exact {
                        1.35 * ((end - start) as f32)
                    } else {
                        typo_cost
                    };
                    edges_by_end[actual_end].push(LatticeEdge {
                        start,
                        end: actual_end,
                        syl: SylMatch::Full(sid),
                        spatial_logp,
                        kind_cost: full_cost,
                        correction,
                        display_syl: s,
                    });
                } else if decoder.config.pinyin_initial_abbr
                    && (len == 1 || (len == 2 && (s == "zh" || s == "ch" || s == "sh")))
                {
                    let init_id = base_initial_id(&s);
                    if init_id > 0 {
                        let init_pen = if len == 2 { -1.8 } else { -2.4 };
                        edges_by_end[actual_end].push(LatticeEdge {
                            start,
                            end: actual_end,
                            syl: SylMatch::Initial(init_id, len == 2),
                            spatial_logp,
                            kind_cost: typo_cost + init_pen,
                            correction,
                            display_syl: s,
                        });
                    }
                } else if actual_end == n
                    && (correction == CorrectionKind::Exact
                        || correction == CorrectionKind::TouchBoundary)
                    && !decoder.dict.syllables_with_prefix(&s).is_empty()
                {
                    edges_by_end[actual_end].push(LatticeEdge {
                        start,
                        end: actual_end,
                        syl: SylMatch::Prefix(s.clone()),
                        spatial_logp,
                        kind_cost: typo_cost - 3.5,
                        correction,
                        display_syl: s,
                    });
                }
            }
        }
    }

    // Prune and deduplicate edges per `end`
    for list in &mut edges_by_end {
        list.sort_by(|a, b| {
            (b.spatial_logp + b.kind_cost)
                .partial_cmp(&(a.spatial_logp + a.kind_cost))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut deduped: Vec<LatticeEdge> = Vec::with_capacity(16);
        for edge in list.drain(..) {
            if !deduped
                .iter()
                .any(|e| e.start == edge.start && e.syl == edge.syl)
            {
                deduped.push(edge);
                if deduped.len() >= 18 {
                    break;
                }
            }
        }
        *list = deduped;
    }

    edges_by_end
}

#[derive(Default)]
pub struct PinyinStream {
    layers: Vec<Vec<PinyinPath>>,
}

impl PinyinStream {
    pub fn clear(&mut self) {
        self.layers.clear();
    }

    pub fn update_and_decode(
        &mut self,
        observations: &[KeyObs],
        raw_code: &str,
        decoder: &PinyinDecoder<'_>,
        initial_state: &RwkvState,
    ) -> (Vec<PinyinCandidate>, Vec<PinyinPath>) {
        let n = observations.len();
        if n == 0 {
            self.layers.clear();
            return (Vec::new(), Vec::new());
        }
        // Keep cached layers up to n - 1 (layer n-1 may have had Prefix edges, so recompute from n-1 or n)
        while self.layers.len() > n {
            self.layers.pop();
        }
        if self.layers.is_empty() {
            self.layers.push(vec![PinyinPath {
                text: String::new(),
                score: 0.0,
                boundaries: Vec::new(),
                state: initial_state.clone(),
                personal_context: PersonalContext::with_chars(decoder.context.iter().copied()),
                any_corrected: false,
            }]);
        }
        let lattice = build_lattice(observations, decoder);
        // Recompute only the newly added layer(s) (and the last layer if it was previously at the end)
        let start_end = self.layers.len().max(1);
        for end in start_end..=n {
            let next_paths = extend_pinyin_layer(end, &lattice, &self.layers, decoder);
            if end < self.layers.len() {
                self.layers[end] = next_paths;
            } else {
                self.layers.push(next_paths);
            }
        }
        build_pinyin_candidates(n, raw_code, &lattice, &self.layers, decoder)
    }
}

pub fn decode_pinyin(
    observations: &[KeyObs],
    raw_code: &str,
    decoder: &PinyinDecoder<'_>,
    initial_state: &RwkvState,
) -> (Vec<PinyinCandidate>, Vec<PinyinPath>) {
    let mut stream = PinyinStream::default();
    stream.update_and_decode(observations, raw_code, decoder, initial_state)
}

fn extend_pinyin_layer(
    end: usize,
    lattice: &[Vec<LatticeEdge>],
    layers: &[Vec<PinyinPath>],
    decoder: &PinyinDecoder<'_>,
) -> Vec<PinyinPath> {
    if lattice[end].is_empty() {
        return Vec::new();
    }
    let personal = &decoder.user.continuations;
    let personalize = decoder.config.dynamic_word_freq && !personal.is_empty();
    let use_personal_states = personalize
        && decoder
            .model
            .as_ref()
            .is_some_and(|m| personal.matches_model(m));

    struct Step<'a> {
        parent: &'a PinyinPath,
        text: String,
        estimate: f32,
        bias: f32,
        syl_display: String,
        correction: CorrectionKind,
        end: usize,
    }

    let mut steps: Vec<Step<'_>> = Vec::new();
    let mut chains: Vec<Vec<&LatticeEdge>> = Vec::new();
    for e1 in &lattice[end] {
        chains.push(vec![e1]);
    }
    let mut frontier: Vec<Vec<&LatticeEdge>> = lattice[end].iter().take(6).map(|e| vec![e]).collect();
    for _k in 2..=MAX_PHRASE_SYLS {
        let mut next_frontier = Vec::new();
        for chain in &frontier {
            let head_start = chain.last().unwrap().start;
            if head_start == 0 {
                continue;
            }
            for prev_edge in lattice[head_start].iter().take(4) {
                let mut nc = chain.clone();
                nc.push(prev_edge);
                chains.push(nc.clone());
                next_frontier.push(nc);
            }
        }
        if next_frontier.is_empty() {
            break;
        }
        frontier = next_frontier;
    }

    for mut chain in chains {
        chain.reverse();
        let chain_start = chain[0].start;
        let parents = &layers[chain_start];
        if parents.is_empty() {
            continue;
        }

        let syl_score: f32 = chain
            .iter()
            .map(|e| 1.8 * e.spatial_logp + e.kind_cost)
            .sum();
        let any_corr = chain
            .iter()
            .find(|e| e.correction != CorrectionKind::Exact)
            .map_or(CorrectionKind::Exact, |e| e.correction);
        let syl_display = chain
            .iter()
            .map(|e| e.display_syl.as_str())
            .collect::<Vec<_>>()
            .join("'");

        let mut entries: Vec<(String, u32)> = Vec::new();
        if chain.len() == 1 {
            match &chain[0].syl {
                SylMatch::Full(sid) => {
                    for (rank, e) in decoder
                        .dict
                        .lookup_chars(*sid, decoder.config.enable_rare_charset)
                        .into_iter()
                        .take(60)
                        .enumerate()
                    {
                        if rank < 12
                            || (decoder.config.dynamic_word_freq
                                && decoder.user.unigram_boost(e.text) > 0.0)
                        {
                            entries.push((e.text.to_owned(), e.weight));
                        }
                    }
                }
                SylMatch::Initial(init_id, is_digraph) => {
                    let prefix = &chain[0].display_syl;
                    let sids = decoder.dict.syllables_with_prefix(prefix);
                    let mut pool = Vec::new();
                    for sid in sids {
                        let (sid_init, sid_dg) = decoder.dict.syllable_init(sid);
                        if sid_init == *init_id && (!*is_digraph || sid_dg) {
                            for e in decoder
                                .dict
                                .lookup_chars(sid, decoder.config.enable_rare_charset)
                                .into_iter()
                                .take(2)
                            {
                                pool.push((e.text.to_owned(), e.weight));
                            }
                        }
                    }
                    pool.sort_by(|a, b| b.1.cmp(&a.1));
                    pool.truncate(6);
                    entries.extend(pool);
                }
                SylMatch::Prefix(pref) => {
                    let sids = decoder.dict.syllables_with_prefix(pref);
                    let mut pool = Vec::new();
                    for sid in sids.into_iter().take(4) {
                        for e in decoder
                            .dict
                            .lookup_chars(sid, decoder.config.enable_rare_charset)
                            .into_iter()
                            .take(4)
                        {
                            pool.push((e.text.to_owned(), e.weight));
                        }
                    }
                    pool.sort_by(|a, b| b.1.cmp(&a.1));
                    pool.truncate(8);
                    entries.extend(pool);
                }
            }
        } else {
            let constraints: Vec<SylConstraint<'_>> = chain
                .iter()
                .map(|e| match &e.syl {
                    SylMatch::Full(sid) => SylConstraint::Full(*sid),
                    SylMatch::Initial(id, dg) => SylConstraint::Initial(*id, *dg),
                    SylMatch::Prefix(p) => SylConstraint::Prefix(p.as_str()),
                })
                .collect();
            for (rank, e) in decoder.dict.lookup_phrases(&constraints, 200).into_iter().enumerate() {
                if rank < 48
                    || (decoder.config.dynamic_word_freq
                        && decoder.user.unigram_boost(e.text) > 0.0)
                {
                    entries.push((e.text.to_owned(), e.weight));
                }
            }
        }

        if entries.is_empty() {
            continue;
        }

        for (text, weight) in entries {
            let mut chars = text.chars();
            let Some(first) = chars.next() else { continue };
            let second = chars.next();
            let n_chars = 1 + usize::from(second.is_some()) + chars.count();
            let base_freq_cold = if n_chars == 1 {
                0.24 * (((weight as f32) + 1.0).ln() - 15.0)
            } else {
                0.42 * (((weight as f32) + 1.0).ln() - 8.5)
            };
            let base_freq_ctx = if n_chars == 1 {
                0.18 * (((weight as f32) + 1.0).ln() - 15.0)
            } else {
                0.28 * (((weight as f32) + 1.0).ln() - 8.5)
            };
            let phrase_bonus_cold = if n_chars >= 2 { 1.35 * ((n_chars - 1) as f32) } else { 0.0 };
            let phrase_bonus_ctx = if n_chars >= 2 { 0.95 * ((n_chars - 1) as f32) } else { 0.0 };
            let mut shared_bias = syl_score;
            if let (Some(aux), Some(sec)) = (decoder.aux, second) {
                shared_bias += 0.04 * aux.bigram_boost(first, sec);
            }

            if decoder.config.dynamic_word_freq {
                shared_bias += 1.6 * decoder.user.unigram_boost(&text);
            }
            for parent in parents {
                let in_sentence = !parent.text.is_empty();
                let neural_w = if in_sentence { 0.95 } else { 0.72 };
                let mut contextual_bias = shared_bias
                    + if in_sentence {
                        base_freq_ctx + phrase_bonus_ctx
                    } else {
                        base_freq_cold + phrase_bonus_cold
                    };
                if let Some(aux) = decoder.aux {
                    let mut preceding = parent
                        .text
                        .chars()
                        .rev()
                        .chain(decoder.context.iter().rev().copied());
                    if let Some(prev) = preceding.next() {
                        contextual_bias += 0.14 * aux.bigram_boost(prev, first);
                        if let Some(prev2) = preceding.next() {
                            contextual_bias += 0.12 * aux.trigram_boost(prev2, prev, first);
                        }
                    }
                }
                if personalize {
                    contextual_bias += personal.score(parent.personal_context, &text);
                }
                let neural_est = if let Some(nv) = &decoder.model {
                    if n_chars == 1 {
                        neural_w * nv.score_next_char(&parent.state, first)
                    } else if n_chars == 2 {
                        neural_w * nv.score_phrase_fast(&parent.state, &text) * 2.0f32.powf(0.65)
                    } else {
                        // Avoid penalizing 3-4 char phrases in coarse pruning with unconditioned uni_bias:
                        // estimate first 2 chars via MTP-2 and remaining in-word chars via mild continuation prior.
                        let prefix2: String = text.chars().take(2).collect();
                        let lp12 = neural_w
                            * nv.score_phrase_fast(&parent.state, &prefix2)
                            * 2.0f32.powf(0.65);
                        lp12 - 2.1 * ((n_chars - 2) as f32)
                    }
                } else {
                    0.0
                };
                let personal_est = if use_personal_states {
                    personal.state_bonus(parent.personal_context, first, &parent.state)
                } else {
                    0.0
                };
                steps.push(Step {
                    parent,
                    text: text.clone(),
                    estimate: parent.score + contextual_bias + neural_est + personal_est,
                    bias: contextual_bias,
                    syl_display: syl_display.clone(),
                    correction: any_corr,
                    end,
                });
            }
        }
    }

    steps.sort_unstable_by(|a, b| b.estimate.total_cmp(&a.estimate));
    let mut next_paths: Vec<PinyinPath> = Vec::with_capacity(BEAM_WIDTH * 2);
    for step in steps.into_iter().take(BEAM_WIDTH * 2) {
        let mut state = step.parent.state.clone();
        let mut score = step.parent.score + step.bias;
        let mut personal_context = step.parent.personal_context;
        let in_sentence = !step.parent.text.is_empty();
        let neural_w = if in_sentence { 0.95 } else { 0.72 };
        if let Some(nv) = &decoder.model {
            for ch in step.text.chars() {
                score += neural_w * nv.score_next_char(&state, ch);
                if use_personal_states {
                    score += personal.state_bonus(personal_context, ch, &state);
                }
                nv.step_token(nv.char_to_token(ch), &mut state);
                personal_context = personal_context.push(ch);
            }
        }
        let mut text = step.parent.text.clone();
        text.push_str(&step.text);
        let mut boundaries = step.parent.boundaries.clone();
        boundaries.push(PinyinBoundary {
            keys: step.end,
            text_bytes: text.len(),
            syl_display: step.syl_display,
            correction: step.correction,
        });
        let path = PinyinPath {
            text,
            score,
            boundaries,
            state,
            personal_context,
            any_corrected: step.parent.any_corrected || step.correction != CorrectionKind::Exact,
        };
        if let Some(existing) = next_paths.iter_mut().find(|p| p.text == path.text) {
            if path.score > existing.score {
                *existing = path;
            }
        } else {
            next_paths.push(path);
        }
    }
    next_paths.sort_unstable_by(|a, b| b.score.total_cmp(&a.score));
    next_paths.truncate(BEAM_WIDTH);
    next_paths
}

fn build_pinyin_candidates(
    n: usize,
    raw_code: &str,
    lattice: &[Vec<LatticeEdge>],
    layers: &[Vec<PinyinPath>],
    decoder: &PinyinDecoder<'_>,
) -> (Vec<PinyinCandidate>, Vec<PinyinPath>) {

    // Build final candidate list:
    // 1. Full-sentence paths from `layers[n]` (or deepest non-empty layer)
    let mut out = Vec::<PinyinCandidate>::new();
    let mut seen = HashSet::<String>::new();

    let (best_end, full_paths) = (1..=n)
        .rev()
        .find(|&pos| !layers[pos].is_empty())
        .map_or((0, &[][..]), |pos| (pos, layers[pos].as_slice()));

    let mut raw_scores: Vec<f32> = full_paths.iter().map(|p| p.score).collect();
    let confs = softmax_confidences(&raw_scores);
    for (idx, path) in full_paths.iter().enumerate() {
        if seen.insert(path.text.clone()) {
            if let Some(hint_comment) = (!path.correction_hint().is_empty()).then(|| path.correction_hint()) {
                let _ = hint_comment;
            }
            let hint = path.correction_hint();
            out.push(PinyinCandidate {
                candidate: Candidate {
                    text: path.text.clone(),
                    matched_code: PackedCode::EMPTY,
                    comment: hint,
                    badge: if path.any_corrected {
                        CandidateBadge::TypoCorrected
                    } else {
                        CandidateBadge::None
                    },
                    confidence: confs.get(idx).copied().unwrap_or(0.0),
                    is_exact: !path.any_corrected,
                },
                consumed_keys: best_end,
                boundaries: path.boundaries.clone(),
            });
        }
    }

    // 1b. If `n <= 6` (e.g. initial abbr `sjb` / `shenjb` or 2-3 char word), also surface extra matching phrases
    //     beyond `BEAM_WIDTH = 8` so lower-ranked exact/abbr words like `神经病` are always reachable & learnable.
    if n <= 6 {
        let mut extra_phrases: Vec<(String, f32)> = Vec::new();
        // Check 2..=4 syllable chains ending at `n` starting at `0`
        let mut full_chains: Vec<Vec<&LatticeEdge>> = lattice[n]
            .iter()
            .filter(|e| e.correction == CorrectionKind::Exact)
            .take(4)
            .map(|e| vec![e])
            .collect();
        for _k in 2..=MAX_PHRASE_SYLS {
            let mut next_f = Vec::new();
            for ch in &full_chains {
                let hs = ch.last().unwrap().start;
                if hs == 0 {
                    let mut rev = ch.clone();
                    rev.reverse();
                    let constraints: Vec<SylConstraint<'_>> = rev
                        .iter()
                        .map(|e| match &e.syl {
                            SylMatch::Full(sid) => SylConstraint::Full(*sid),
                            SylMatch::Initial(id, dg) => SylConstraint::Initial(*id, *dg),
                            SylMatch::Prefix(p) => SylConstraint::Prefix(p.as_str()),
                        })
                        .collect();
                    for e in decoder.dict.lookup_phrases(&constraints, 80) {
                        let mut sc = 0.42 * (((e.weight as f32) + 1.0).ln() - 8.5);
                        if decoder.config.dynamic_word_freq {
                            sc += 1.6 * decoder.user.unigram_boost(e.text);
                        }
                        extra_phrases.push((e.text.to_owned(), sc));
                    }
                    continue;
                }
                for pe in lattice[hs]
                    .iter()
                    .filter(|e| e.correction == CorrectionKind::Exact)
                    .take(3)
                {
                    let mut nc = ch.clone();
                    nc.push(pe);
                    next_f.push(nc);
                }
            }
            if next_f.is_empty() {
                break;
            }
            full_chains = next_f;
        }
        for ch in &full_chains {
            if ch.last().unwrap().start == 0 {
                let mut rev = ch.clone();
                rev.reverse();
                let constraints: Vec<SylConstraint<'_>> = rev
                    .iter()
                    .map(|e| match &e.syl {
                        SylMatch::Full(sid) => SylConstraint::Full(*sid),
                        SylMatch::Initial(id, dg) => SylConstraint::Initial(*id, *dg),
                        SylMatch::Prefix(p) => SylConstraint::Prefix(p.as_str()),
                    })
                    .collect();
                for e in decoder.dict.lookup_phrases(&constraints, 80) {
                    let mut sc = 0.42 * (((e.weight as f32) + 1.0).ln() - 8.5);
                    if decoder.config.dynamic_word_freq {
                        sc += 1.6 * decoder.user.unigram_boost(e.text);
                    }
                    extra_phrases.push((e.text.to_owned(), sc));
                }
            }
        }
        extra_phrases.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (text, _) in extra_phrases {
            if seen.insert(text.clone()) {
                out.push(PinyinCandidate {
                    candidate: Candidate {
                        text,
                        matched_code: PackedCode::EMPTY,
                        comment: String::new(),
                        badge: CandidateBadge::None,
                        confidence: 0.10,
                        is_exact: true,
                    },
                    consumed_keys: n,
                    boundaries: Vec::new(),
                });
                if out.len() >= 48 {
                    break;
                }
            }
        }
    }

    // 2. Prefix word & single-char candidates at the start of the buffer (so user can select a prefix word)
    let mut prefix_items: Vec<(String, usize, f32, bool, String)> = Vec::new();
    for end in (1..n).rev() {
        for edge in lattice[end].iter().filter(|e| e.start == 0).take(2) {
            match &edge.syl {
                SylMatch::Full(sid) => {
                    let syl_s = decoder.dict.syllable_str(*sid);
                    for e in decoder
                        .dict
                        .lookup_chars(*sid, decoder.config.enable_rare_charset)
                        .into_iter()
                        .take(4)
                    {
                        let hint = if edge.correction != CorrectionKind::Exact {
                            format!("{}({})", e.text, syl_s)
                        } else {
                            String::new()
                        };
                        prefix_items.push((
                            e.text.to_owned(),
                            end,
                            ((e.weight as f32) + 1.0).ln() + edge.kind_cost,
                            edge.correction != CorrectionKind::Exact,
                            hint,
                        ));
                    }
                }
                _ => {}
            }
        }
        // Also take top phrase starting at 0 ending at `end`
        for path in layers[end].iter().take(2) {
            if path.boundaries.len() == 1 {
                prefix_items.push((
                    path.text.clone(),
                    end,
                    path.score,
                    path.any_corrected,
                    path.correction_hint(),
                ));
            }
        }
    }
    // Single-char candidates at position 0: prioritize longest exact Full syllables first (e.g. `jian` in `jianpin` before `jia`/`ji`)
    let mut head_full_edges: Vec<&LatticeEdge> = Vec::new();
    for end in (1..=n).rev() {
        for edge in &lattice[end] {
            if edge.start == 0 && matches!(edge.syl, SylMatch::Full(_)) {
                head_full_edges.push(edge);
            }
        }
    }
    head_full_edges.sort_by(|a, b| {
        (a.correction != CorrectionKind::Exact)
            .cmp(&(b.correction != CorrectionKind::Exact))
            .then_with(|| b.end.cmp(&a.end))
    });
    for edge in head_full_edges.into_iter().take(3) {
        if let SylMatch::Full(sid) = edge.syl {
            let syl_s = decoder.dict.syllable_str(sid);
            for e in decoder
                .dict
                .lookup_chars(sid, decoder.config.enable_rare_charset)
                .into_iter()
                .take(80)
            {
                let mut sc = 0.25 * (((e.weight as f32) + 1.0).ln() - 15.0) + edge.kind_cost;
                if decoder.config.dynamic_word_freq {
                    sc += 1.2 * decoder.user.unigram_boost(e.text);
                }
                let hint = if edge.correction != CorrectionKind::Exact {
                    format!("{}({})", e.text, syl_s)
                } else {
                    String::new()
                };
                prefix_items.push((
                    e.text.to_owned(),
                    edge.end,
                    sc,
                    edge.correction != CorrectionKind::Exact,
                    hint,
                ));
            }
        }
    }

    prefix_items.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    for (text, consumed, _, corr, hint) in prefix_items {
        if seen.insert(text.clone()) {
            out.push(PinyinCandidate {
                candidate: Candidate {
                    text,
                    matched_code: PackedCode::EMPTY,
                    comment: hint,
                    badge: if corr {
                        CandidateBadge::TypoCorrected
                    } else {
                        CandidateBadge::None
                    },
                    confidence: 0.15,
                    is_exact: !corr,
                },
                consumed_keys: consumed,
                boundaries: Vec::new(),
            });
            if out.len() >= 80 {
                break;
            }
        }
    }

    let _ = raw_code;
    let _ = &mut raw_scores;
    (out, full_paths.to_vec())
}

/// Compute 26-bit `validNextMask` and 26-key `nextKeyPriors` for Pinyin touch-keyboard缝裁决.
pub fn pinyin_next_key_priors(raw_code: &str, dict: &PinyinDictView) -> (u32, [u8; 26]) {
    let mut mask = 0u32;
    let mut scores = [0.0f32; 26];

    // Valid syllable-starting letters (all letters except 'i', 'u', 'v' in standard Mandarin)
    let syl_starts: &[u8] = b"abcdefghjklmnopqrstwxyz";

    // Find trailing letters after the last apostrophe or likely syllable tail
    let tail = raw_code.rsplit('\'').next().unwrap_or(raw_code);
    let max_tail = tail.len().min(6);
    let mut can_end_syllable = tail.is_empty();

    for l in 1..=max_tail {
        let sub = &tail[tail.len() - l..];
        if dict.lookup_syllable_id(sub).is_some() {
            can_end_syllable = true;
        }
        for s_id in dict.syllables_with_prefix(sub) {
            let s = dict.syllable_str(s_id);
            if let Some(&next_b) = s.as_bytes().get(sub.len()) {
                if next_b.is_ascii_lowercase() {
                    let idx = (next_b - b'a') as usize;
                    mask |= 1u32 << idx;
                    scores[idx] += 2.0;
                }
            }
        }
    }

    if can_end_syllable || mask == 0 {
        for &b in syl_starts {
            let idx = (b - b'a') as usize;
            mask |= 1u32 << idx;
            scores[idx] += 1.0;
        }
    }

    let max_s = scores.iter().copied().fold(0.0f32, f32::max);
    let mut priors = [0u8; 26];
    if max_s > 0.0 {
        for i in 0..26 {
            priors[i] = ((scores[i] / max_s) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    (mask, priors)
}
