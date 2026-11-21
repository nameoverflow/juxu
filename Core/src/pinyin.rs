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
use std::collections::{HashMap, HashSet};

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

/// Decode a 2-key Xiaohe Shuangpin pair `(k1, k2)` into matching full-pinyin syllable(s) present in `dict`.
pub fn xiaohe_sp_decode_pair(k1: u8, k2: u8, dict: &PinyinDictView) -> Vec<(u16, String)> {
    let mut out = Vec::new();
    if !k1.is_ascii_lowercase() || !k2.is_ascii_lowercase() {
        return out;
    }
    // 1. Zero-initial syllables (`a`, `e`, `o` leading key)
    let zero_syl: Option<&'static str> = match (k1, k2) {
        (b'a', b'a') => Some("a"),
        (b'a', b'i') => Some("ai"),
        (b'a', b'n') => Some("an"),
        (b'a', b'h') => Some("ang"),
        (b'a', b'o') => Some("ao"),
        (b'e', b'e') => Some("e"),
        (b'e', b'i') => Some("ei"),
        (b'e', b'n') => Some("en"),
        (b'e', b'g') => Some("eng"),
        (b'e', b'r') => Some("er"),
        (b'o', b'o') => Some("o"),
        (b'o', b'u') => Some("ou"),
        _ => None,
    };
    if let Some(syl) = zero_syl {
        if let Some(sid) = dict.lookup_syllable_id(syl) {
            out.push((sid, syl.to_owned()));
        }
        return out;
    }

    // 2. Consonant-initial syllables
    let initial: &str = match k1 {
        b'u' => "sh",
        b'i' => "ch",
        b'v' => "zh",
        b'a' | b'e' | b'o' => return out,
        _ => std::str::from_utf8(std::slice::from_ref(&k1)).unwrap_or(""),
    };

    let finals: &[&str] = match k2 {
        b'a' => &["a"],
        b'b' => &["in"],
        b'c' => &["ao"],
        b'd' => &["ai"],
        b'e' => &["e"],
        b'f' => &["en"],
        b'g' => &["eng"],
        b'h' => &["ang"],
        b'i' => &["i"],
        b'j' => &["an"],
        b'k' => &["ing", "uai"],
        b'l' => &["iang", "uang"],
        b'm' => &["ian"],
        b'n' => &["iao"],
        b'o' => {
            if matches!(k1, b'b' | b'p' | b'm' | b'f' | b'w' | b'y') {
                &["o"]
            } else {
                &["uo", "o"]
            }
        }
        b'p' => &["ie"],
        b'q' => &["iu"],
        b'r' => &["uan"],
        b's' => &["ong", "iong"],
        b't' => &["ue", "ve"],
        b'u' => &["u"],
        b'v' => &["ui", "v"],
        b'w' => &["ei"],
        b'x' => &["ia", "ua"],
        b'y' => &["un"],
        b'z' => &["ou"],
        _ => &[],
    };

    for &fin in finals {
        let mut s = String::with_capacity(initial.len() + fin.len());
        s.push_str(initial);
        s.push_str(fin);
        if let Some(sid) = dict.lookup_syllable_id(&s) {
            out.push((sid, s));
        }
    }
    out
}

fn xiaohe_sp_initial_for_key(k: u8) -> Option<(&'static str, bool)> {
    match k {
        b'u' => Some(("sh", true)),
        b'i' => Some(("ch", true)),
        b'v' => Some(("zh", true)),
        b'b' => Some(("b", false)),
        b'p' => Some(("p", false)),
        b'm' => Some(("m", false)),
        b'f' => Some(("f", false)),
        b'd' => Some(("d", false)),
        b't' => Some(("t", false)),
        b'n' => Some(("n", false)),
        b'l' => Some(("l", false)),
        b'g' => Some(("g", false)),
        b'k' => Some(("k", false)),
        b'h' => Some(("h", false)),
        b'j' => Some(("j", false)),
        b'q' => Some(("q", false)),
        b'x' => Some(("x", false)),
        b'r' => Some(("r", false)),
        b'z' => Some(("z", false)),
        b'c' => Some(("c", false)),
        b's' => Some(("s", false)),
        b'y' => Some(("y", false)),
        b'w' => Some(("w", false)),
        _ => None,
    }
}

fn build_shuangpin_lattice(
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
        if observations[start].ch == b'\'' {
            continue;
        }
        let k1 = observations[start].ch;
        let lp1_exact = per_pos_choices[start]
            .iter()
            .find(|c| c.2)
            .map_or(0.0, |c| c.1);

        // 1-key edge: initial abbreviation (or trailing partial key at end of buffer)
        let end1 = start + 1;
        let actual_end1 = if end1 < n && observations[end1].ch == b'\'' {
            end1 + 1
        } else {
            end1
        };
        if decoder.config.pinyin_initial_abbr || actual_end1 == n {
            let mut single_hyps: Vec<(u8, f32, CorrectionKind)> = vec![(k1, lp1_exact, CorrectionKind::Exact)];
            if decoder.config.typo.enabled && actual_end1 == n {
                for &(d, lp, is_exact, is_boundary) in &per_pos_choices[start] {
                    if !is_exact && is_boundary {
                        single_hyps.push((b'a' + d - 1, lp, CorrectionKind::TouchBoundary));
                    }
                }
            }
            for (ch, sp_lp, corr) in single_hyps {
                let typo_cost = match corr {
                    CorrectionKind::Exact => 0.0,
                    CorrectionKind::TouchBoundary => -1.8,
                    CorrectionKind::NeighborKey => -4.8,
                    CorrectionKind::Transposition => -3.6,
                };
                if let Some((init_str, is_digraph)) = xiaohe_sp_initial_for_key(ch) {
                    let init_id = base_initial_id(init_str);
                    if init_id > 0 {
                        let pen = if actual_end1 == n { -2.2 } else { -3.4 };
                        edges_by_end[actual_end1].push(LatticeEdge {
                            start,
                            end: actual_end1,
                            syl: SylMatch::Initial(init_id, is_digraph),
                            spatial_logp: sp_lp,
                            kind_cost: typo_cost + pen,
                            correction: corr,
                            display_syl: init_str.to_owned(),
                        });
                    }
                } else if actual_end1 == n && matches!(ch, b'a' | b'e' | b'o') {
                    let s = (ch as char).to_string();
                    if !decoder.dict.syllables_with_prefix(&s).is_empty() {
                        edges_by_end[actual_end1].push(LatticeEdge {
                            start,
                            end: actual_end1,
                            syl: SylMatch::Prefix(s.clone()),
                            spatial_logp: sp_lp,
                            kind_cost: typo_cost - 3.2,
                            correction: corr,
                            display_syl: s,
                        });
                    }
                }
            }
        }

        // 2-key full Shuangpin syllable edge
        if start + 1 < n && observations[start + 1].ch != b'\'' {
            let end2 = start + 2;
            let actual_end2 = if end2 < n && observations[end2].ch == b'\'' {
                end2 + 1
            } else {
                end2
            };
            let k2 = observations[start + 1].ch;
            let lp2_exact = per_pos_choices[start + 1]
                .iter()
                .find(|c| c.2)
                .map_or(0.0, |c| c.1);

            let mut pair_hyps: Vec<(u8, u8, f32, CorrectionKind)> =
                vec![(k1, k2, lp1_exact + lp2_exact, CorrectionKind::Exact)];

            if decoder.config.typo.enabled {
                for &(d, lp, is_exact, is_boundary) in &per_pos_choices[start] {
                    if !is_exact {
                        let kind = if is_boundary {
                            CorrectionKind::TouchBoundary
                        } else {
                            CorrectionKind::NeighborKey
                        };
                        pair_hyps.push((b'a' + d - 1, k2, lp + lp2_exact, kind));
                    }
                }
                for &(d, lp, is_exact, is_boundary) in &per_pos_choices[start + 1] {
                    if !is_exact {
                        let kind = if is_boundary {
                            CorrectionKind::TouchBoundary
                        } else {
                            CorrectionKind::NeighborKey
                        };
                        pair_hyps.push((k1, b'a' + d - 1, lp1_exact + lp, kind));
                    }
                }
                if k1 != k2 {
                    let dt = observations[start + 1]
                        .timestamp_ms
                        .saturating_sub(observations[start].timestamp_ms);
                    let trans_lp = if observations[start + 1].timestamp_ms > 0 && dt <= 65 {
                        -2.45
                    } else {
                        -3.45
                    };
                    pair_hyps.push((k2, k1, trans_lp, CorrectionKind::Transposition));
                }
            }

            for (c1, c2, sp_lp, corr) in pair_hyps {
                let matches = xiaohe_sp_decode_pair(c1, c2, decoder.dict);
                let kind_cost = match corr {
                    CorrectionKind::Exact => 2.7,
                    CorrectionKind::TouchBoundary => -1.8,
                    CorrectionKind::NeighborKey => -4.8,
                    CorrectionKind::Transposition => -3.6,
                };
                for (sid, syl_str) in matches {
                    edges_by_end[actual_end2].push(LatticeEdge {
                        start,
                        end: actual_end2,
                        syl: SylMatch::Full(sid),
                        spatial_logp: sp_lp,
                        kind_cost,
                        correction: corr,
                        display_syl: syl_str,
                    });
                }
            }
        }
    }

    for list in &mut edges_by_end {
        list.sort_by(|a, b| {
            (b.spatial_logp + b.kind_cost)
                .partial_cmp(&(a.spatial_logp + a.kind_cost))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut deduped: Vec<LatticeEdge> = Vec::with_capacity(24);
        for edge in list.drain(..) {
            if !deduped
                .iter()
                .any(|e| e.start == edge.start && e.syl == edge.syl)
            {
                deduped.push(edge);
                if deduped.len() >= 24 {
                    break;
                }
            }
        }
        *list = deduped;
    }

    edges_by_end
}

fn build_lattice(
    observations: &[KeyObs],
    decoder: &PinyinDecoder<'_>,
) -> Vec<Vec<LatticeEdge>> {
    if decoder.config.schema.is_shuangpin() {
        return build_shuangpin_lattice(observations, decoder);
    }
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
                        // Touch-boundary taps already pay continuous 2D Gaussian `spatial_logp`;
                        // keep step cost light so frontend `hitTest` priors aren't double-counted.
                        CorrectionKind::TouchBoundary => -0.45,
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
        let mut deduped: Vec<LatticeEdge> = Vec::with_capacity(24);
        for edge in list.drain(..) {
            if !deduped
                .iter()
                .any(|e| e.start == edge.start && e.syl == edge.syl)
            {
                deduped.push(edge);
                if deduped.len() >= 24 {
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
        parent_key: (usize, usize),
        text: String,
        first: char,
        n_chars: usize,
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

    let has_neural = decoder.model.is_some();
    let aux_bi_scale = if has_neural { 0.04 } else { 0.14 };
    let aux_tri_scale = if has_neural { 0.04 } else { 0.12 };

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

        // (text, weight, is_user_phrase)
        let mut entries: Vec<(String, u32, bool)> = Vec::new();
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
                            entries.push((e.text.to_owned(), e.weight, false));
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
                                pool.push((e.text.to_owned(), e.weight, false));
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
                            pool.push((e.text.to_owned(), e.weight, false));
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
            // 1. Cross-mode personal user phrases indexed by Pinyin syllables
            for (utext, ucnt) in decoder.user.lookup_pinyin_phrases(&constraints, decoder.dict, 12) {
                let w = 200_000u32.saturating_add(ucnt.saturating_mul(100_000));
                entries.push((utext.to_owned(), w, true));
            }
            // 2. Built-in Pinyin dictionary phrases
            for (rank, e) in decoder.dict.lookup_phrases(&constraints, 200).into_iter().enumerate() {
                if rank < 48
                    || (decoder.config.dynamic_word_freq
                        && decoder.user.unigram_boost(e.text) > 0.0)
                {
                    if !entries.iter().any(|x| x.0 == e.text) {
                        entries.push((e.text.to_owned(), e.weight, false));
                    }
                }
            }
        }

        if entries.is_empty() {
            continue;
        }

        for (text, weight, is_user_phrase) in entries {
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
            let user_phrase_bonus = if is_user_phrase { 4.5 } else { 0.0 };
            let mut shared_bias = syl_score + user_phrase_bonus;
            if let (Some(aux), Some(sec)) = (decoder.aux, second) {
                shared_bias += (if has_neural { 0.015 } else { 0.04 }) * aux.bigram_boost(first, sec);
            }

            if decoder.config.dynamic_word_freq {
                shared_bias += 1.2 * decoder.user.unigram_boost(&text);
            }
            for (parent_idx, parent) in parents.iter().enumerate() {
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
                        contextual_bias += aux_bi_scale * aux.bigram_boost(prev, first);
                        if let Some(prev2) = preceding.next() {
                            contextual_bias += aux_tri_scale * aux.trigram_boost(prev2, prev, first);
                        }
                    }
                }
                let neural_est = if let Some(nv) = &decoder.model {
                    let raw_lp1 = nv.score_next_char(&parent.state, first);
                    let lp1 = if personalize {
                        personal.interpolate_logp(
                            raw_lp1,
                            parent.personal_context,
                            first,
                            use_personal_states.then_some(&parent.state),
                        )
                    } else {
                        raw_lp1
                    };
                    if n_chars == 1 {
                        neural_w * lp1
                    } else if n_chars == 2 {
                        let fast_total = nv.score_phrase_fast(&parent.state, &text) * 2.0f32.powf(0.65);
                        neural_w * (fast_total - raw_lp1 + lp1)
                    } else {
                        let prefix2: String = text.chars().take(2).collect();
                        let fast12 = nv.score_phrase_fast(&parent.state, &prefix2) * 2.0f32.powf(0.65);
                        neural_w * (fast12 - raw_lp1 + lp1) - 2.1 * ((n_chars - 2) as f32)
                    }
                } else if personalize {
                    personal.score(parent.personal_context, &text)
                } else {
                    0.0
                };
                steps.push(Step {
                    parent,
                    parent_key: (chain_start, parent_idx),
                    text: text.clone(),
                    first,
                    n_chars,
                    estimate: parent.score + contextual_bias + neural_est,
                    bias: contextual_bias,
                    syl_display: syl_display.clone(),
                    correction: any_corr,
                    end,
                });
            }
        }
    }

    steps.sort_unstable_by(|a, b| b.estimate.total_cmp(&a.estimate));

    // Trie-shared prefix state refinement for top candidates when RWKV is active
    // Reuses `step_token(c1)` across both refinement and final beam unroll (zero wasted steps!)
    let refine_limit = (BEAM_WIDTH * 2).min(steps.len());
    let mut prefix_cache: HashMap<(usize, usize, char), RwkvState> =
        HashMap::with_capacity(refine_limit);
    if let Some(nv) = &decoder.model {
        for step in &mut steps[..refine_limit] {
            if step.n_chars >= 2 {
                let in_sentence = !step.parent.text.is_empty();
                let neural_w = if in_sentence { 0.95 } else { 0.72 };
                let cache_key = (step.parent_key.0, step.parent_key.1, step.first);
                let state_after_c1 = prefix_cache.entry(cache_key).or_insert_with(|| {
                    let mut st = step.parent.state.clone();
                    nv.step_token(nv.char_to_token(step.first), &mut st);
                    st
                });
                let raw_lp1 = nv.score_next_char(&step.parent.state, step.first);
                let lp1 = if personalize {
                    personal.interpolate_logp(
                        raw_lp1,
                        step.parent.personal_context,
                        step.first,
                        use_personal_states.then_some(&step.parent.state),
                    )
                } else {
                    raw_lp1
                };
                let rest = &step.text[step.first.len_utf8()..];
                let rest_chars = step.n_chars - 1;
                let rest_est = if rest_chars == 1 {
                    let c2 = rest.chars().next().unwrap();
                    let raw_lp2 = nv.score_next_char(state_after_c1, c2);
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
                    nv.score_phrase_fast(state_after_c1, rest)
                        * (rest_chars.min(4) as f32).powf(0.65)
                };
                step.estimate = step.parent.score + step.bias + neural_w * (lp1 + rest_est);
            }
        }
        steps[..refine_limit].sort_unstable_by(|a, b| b.estimate.total_cmp(&a.estimate));
    }

    let mut next_paths: Vec<PinyinPath> = Vec::with_capacity(BEAM_WIDTH * 2);
    for step in steps.into_iter().take(BEAM_WIDTH * 2) {
        let mut score = step.parent.score + step.bias;
        let mut personal_context = step.parent.personal_context;
        let in_sentence = !step.parent.text.is_empty();
        let neural_w = if in_sentence { 0.95 } else { 0.72 };
        let state = if let Some(nv) = &decoder.model {
            let raw_lp1 = nv.score_next_char(&step.parent.state, step.first);
            let lp1 = if personalize {
                personal.interpolate_logp(
                    raw_lp1,
                    personal_context,
                    step.first,
                    use_personal_states.then_some(&step.parent.state),
                )
            } else {
                raw_lp1
            };
            score += neural_w * lp1;
            let cache_key = (step.parent_key.0, step.parent_key.1, step.first);
            let mut st = if let Some(cached) = prefix_cache.get(&cache_key) {
                cached.clone()
            } else {
                let mut s = step.parent.state.clone();
                nv.step_token(nv.char_to_token(step.first), &mut s);
                s
            };
            personal_context = personal_context.push(step.first);
            for ch in step.text.chars().skip(1) {
                let raw_lp = nv.score_next_char(&st, ch);
                let lp = if personalize {
                    personal.interpolate_logp(
                        raw_lp,
                        personal_context,
                        ch,
                        use_personal_states.then_some(&st),
                    )
                } else {
                    raw_lp
                };
                score += neural_w * lp;
                nv.step_token(nv.char_to_token(ch), &mut st);
                personal_context = personal_context.push(ch);
            }
            st
        } else {
            if personalize {
                score += personal.score(personal_context, &step.text);
                for ch in step.text.chars() {
                    personal_context = personal_context.push(ch);
                }
            }
            step.parent.state.clone()
        };
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

    // Build final candidate list in 3 clean tiers:
    //   Tier 1: Top full-sentence paths (capped at 3 for multi-word sentences >= 4 chars so the ribbon
    //           and expanded grid aren't flooded with near-duplicate full sentences)
    //   Tier 2: High-confidence multi-character prefix words (2..=4 chars starting at 0, ranked by
    //           RWKV + lexical prior, longest/best first)
    //   Tier 3: Single-character candidates at position 0 (exact syllables strictly before typo fallbacks)
    let mut out = Vec::<PinyinCandidate>::new();
    let mut seen = HashSet::<String>::new();

    let (best_end, full_paths) = (1..=n)
        .rev()
        .find(|&pos| !layers[pos].is_empty())
        .map_or((0, &[][..]), |pos| (pos, layers[pos].as_slice()));

    let top_char_len = full_paths
        .first()
        .map_or(0, |p| p.text.chars().count());
    let is_long_sentence = top_char_len >= 4;
    let max_full_sentences = if is_long_sentence { 3usize } else { BEAM_WIDTH };
    let top_score = full_paths.first().map_or(f32::NEG_INFINITY, |p| p.score);

    let mut raw_scores: Vec<f32> = full_paths.iter().map(|p| p.score).collect();
    let confs = softmax_confidences(&raw_scores);
    let mut full_added = 0usize;
    for (idx, path) in full_paths.iter().enumerate() {
        if full_added >= max_full_sentences {
            break;
        }
        // For long sentences, only keep #2..#3 when they are genuinely competitive with #1
        if is_long_sentence && full_added >= 1 && path.score < top_score - 4.2 {
            break;
        }
        if seen.insert(path.text.clone()) {
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
            full_added += 1;
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
                    for (utext, ucnt) in decoder.user.lookup_pinyin_phrases(&constraints, decoder.dict, 8) {
                        let sc = 4.0 + ((ucnt as f32) + 1.0).ln();
                        extra_phrases.push((utext.to_owned(), sc));
                    }
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

    // 2. Tier 2: High-confidence multi-character prefix words (2..=4 chars starting at 0, ending < best_end)
    //    Kept in a dedicated pool strictly BEFORE single characters so users can commit a partial phrase cleanly!
    struct PrefixWordItem {
        text: String,
        consumed: usize,
        char_len: usize,
        score: f32,
        corr: bool,
        hint: String,
    }
    let mut prefix_words: Vec<PrefixWordItem> = Vec::new();
    let init_state = layers[0].first().map(|p| &p.state);

    // 2a. Multi-char paths already decoded in intermediate beam layers `layers[end]`
    for end in (2..best_end).rev() {
        let mut taken_at_end = 0usize;
        for path in layers[end].iter() {
            let clen = path.text.chars().count();
            if !(2..=4).contains(&clen) {
                continue;
            }
            // Prefer single-word phrases (`boundaries.len() == 1`) and high-confidence 2-3 char compounds
            let single_word_bonus = if path.boundaries.len() == 1 { 1.8 } else { 0.0 };
            prefix_words.push(PrefixWordItem {
                text: path.text.clone(),
                consumed: end,
                char_len: clen,
                score: path.score + single_word_bonus,
                corr: path.any_corrected,
                hint: path.correction_hint(),
            });
            taken_at_end += 1;
            if taken_at_end >= 5 {
                break;
            }
        }
    }

    // 2b. Direct dictionary + user phrase lookup on exact prefix syllable chains (2..=4 syllables)
    //     so high-frequency words like `试试`/`事实`/`实时`/`实施` always surface even if beam pruned them!
    for end in (2..best_end).rev() {
        let mut chains: Vec<Vec<&LatticeEdge>> = lattice[end]
            .iter()
            .filter(|e| e.correction == CorrectionKind::Exact && matches!(e.syl, SylMatch::Full(_)))
            .take(2)
            .map(|e| vec![e])
            .collect();
        for _k in 2..=MAX_PHRASE_SYLS {
            let mut next_f = Vec::new();
            for ch in &chains {
                let hs = ch.last().unwrap().start;
                if hs == 0 {
                    continue;
                }
                for pe in lattice[hs]
                    .iter()
                    .filter(|e| e.correction == CorrectionKind::Exact && matches!(e.syl, SylMatch::Full(_)))
                    .take(2)
                {
                    let mut nc = ch.clone();
                    nc.push(pe);
                    if nc.last().unwrap().start == 0 {
                        let mut rev = nc.clone();
                        rev.reverse();
                        let constraints: Vec<SylConstraint<'_>> = rev
                            .iter()
                            .map(|e| match &e.syl {
                                SylMatch::Full(sid) => SylConstraint::Full(*sid),
                                SylMatch::Initial(id, dg) => SylConstraint::Initial(*id, *dg),
                                SylMatch::Prefix(p) => SylConstraint::Prefix(p.as_str()),
                            })
                            .collect();
                        let clen = constraints.len();
                        for (utext, ucnt) in decoder.user.lookup_pinyin_phrases(&constraints, decoder.dict, 4) {
                            let sc = 6.0 + ((ucnt as f32) + 1.0).ln();
                            prefix_words.push(PrefixWordItem {
                                text: utext.to_owned(),
                                consumed: end,
                                char_len: clen,
                                score: sc,
                                corr: false,
                                hint: String::new(),
                            });
                        }
                        for e in decoder.dict.lookup_phrases(&constraints, 10) {
                            let freq = 0.45 * (((e.weight as f32) + 1.0).ln() - 8.5);
                            let rwkv = match (&decoder.model, init_state) {
                                (Some(nv), Some(st)) => {
                                    0.72 * nv.score_phrase_fast(st, e.text) * (clen.min(4) as f32).powf(0.65)
                                }
                                _ => 0.0,
                            };
                            let dyn_b = if decoder.config.dynamic_word_freq {
                                1.2 * decoder.user.unigram_boost(e.text)
                            } else {
                                0.0
                            };
                            prefix_words.push(PrefixWordItem {
                                text: e.text.to_owned(),
                                consumed: end,
                                char_len: clen,
                                score: freq + rwkv + dyn_b + 1.35 * ((clen - 1) as f32),
                                corr: false,
                                hint: String::new(),
                            });
                        }
                    } else {
                        next_f.push(nc);
                    }
                }
            }
            if next_f.is_empty() {
                break;
            }
            chains = next_f;
        }
    }

    // Deduplicate prefix words keeping best score, sort by (exact first, longer prefix preferred slightly, score DESC)
    prefix_words.sort_by(|a, b| {
        a.corr
            .cmp(&b.corr)
            .then_with(|| {
                let sa = a.score + 0.35 * (a.char_len as f32);
                let sb = b.score + 0.35 * (b.char_len as f32);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    let mut prefix_words_added = 0usize;
    let max_prefix_words = 16usize;
    for pw in prefix_words {
        if prefix_words_added >= max_prefix_words {
            break;
        }
        if seen.insert(pw.text.clone()) {
            out.push(PinyinCandidate {
                candidate: Candidate {
                    text: pw.text,
                    matched_code: PackedCode::EMPTY,
                    comment: pw.hint,
                    badge: if pw.corr {
                        CandidateBadge::TypoCorrected
                    } else {
                        CandidateBadge::None
                    },
                    confidence: 0.25,
                    is_exact: !pw.corr,
                },
                consumed_keys: pw.consumed,
                boundaries: Vec::new(),
            });
            prefix_words_added += 1;
        }
    }

    // 3. Tier 3: Single-char candidates at position 0 (longest exact Full syllables strictly BEFORE typo fallbacks)
    let mut single_items: Vec<(String, usize, f32, bool, String)> = Vec::new();
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
            let is_corr = edge.correction != CorrectionKind::Exact;
            let take_n = if is_corr { 6 } else { 80 };
            for e in decoder
                .dict
                .lookup_chars(sid, decoder.config.enable_rare_charset)
                .into_iter()
                .take(take_n)
            {
                let mut sc = 0.25 * (((e.weight as f32) + 1.0).ln() - 15.0) + edge.kind_cost;
                if let (Some(nv), Some(st)) = (&decoder.model, init_state) {
                    if let Some(ch) = e.text.chars().next() {
                        sc += 0.35 * nv.score_next_char(st, ch);
                    }
                }
                if decoder.config.dynamic_word_freq {
                    sc += 1.2 * decoder.user.unigram_boost(e.text);
                }
                let hint = if is_corr {
                    format!("{}({})", e.text, syl_s)
                } else {
                    String::new()
                };
                single_items.push((
                    e.text.to_owned(),
                    edge.end,
                    sc,
                    is_corr,
                    hint,
                ));
            }
        }
    }

    single_items.sort_by(|a, b| {
        a.3.cmp(&b.3)
            .then_with(|| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
    });
    for (text, consumed, _, corr, hint) in single_items {
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

/// Compute 26-bit `validNextMask` and 26-key `nextKeyPriors` for Xiaohe Shuangpin touch-keyboard键缝裁决.
pub fn shuangpin_next_key_priors(
    raw_code: &str,
    dict: &PinyinDictView,
    rwkv_start_priors: Option<&[f32; 26]>,
) -> (u32, [u8; 26]) {
    let mut mask = 0u32;
    let mut scores = [0.0f32; 26];
    let tail = raw_code.rsplit('\'').next().unwrap_or(raw_code);
    let expect_final = tail.len() % 2 == 1;

    if expect_final {
        let prev = tail.as_bytes()[tail.len() - 1];
        for b in b'a'..=b'z' {
            if !xiaohe_sp_decode_pair(prev, b, dict).is_empty() {
                let idx = (b - b'a') as usize;
                mask |= 1u32 << idx;
                scores[idx] += 2.0;
            }
        }
    }
    if !expect_final || mask == 0 {
        for b in b'a'..=b'z' {
            let idx = (b - b'a') as usize;
            mask |= 1u32 << idx;
            let rwkv_w = rwkv_start_priors.map_or(0.0, |p| match b {
                b'u' => p[(b's' - b'a') as usize] * 0.65,
                b'i' => p[(b'c' - b'a') as usize] * 0.65,
                b'v' => p[(b'z' - b'a') as usize] * 0.65,
                _ => p[idx],
            });
            scores[idx] += 1.0 + 4.5 * rwkv_w;
        }
    }

    let max_s = scores.iter().copied().fold(0.0f32, f32::max);
    let mut priors = [0u8; 26];
    if max_s > 0.0 {
        for i in 0..26 {
            if (mask & (1u32 << i)) != 0 {
                let norm = (scores[i] / max_s).clamp(0.0, 1.0);
                priors[i] = (65.0 + 115.0 * norm).round().clamp(54.0, 180.0) as u8;
            }
        }
    }
    (mask, priors)
}

/// Compute 26-bit `validNextMask` and 26-key `nextKeyPriors` for Pinyin touch-keyboard键缝裁决.
/// When `rwkv_start_priors` is provided (`0.0..=1.0` probability mass per letter from `Micro-RWKV-7`),
/// syllable-start keys are weighted by contextual neural probability rather than flat uniform priors.
pub fn pinyin_next_key_priors(
    raw_code: &str,
    dict: &PinyinDictView,
    rwkv_start_priors: Option<&[f32; 26]>,
) -> (u32, [u8; 26]) {
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
            let rwkv_w = rwkv_start_priors.map_or(0.0, |p| p[idx]);
            scores[idx] += 1.0 + 4.5 * rwkv_w;
        }
    }

    let max_s = scores.iter().copied().fold(0.0f32, f32::max);
    let mut priors = [0u8; 26];
    if max_s > 0.0 {
        for i in 0..26 {
            if (mask & (1u32 << i)) != 0 {
                // Keep calibrated in the same 54..=180 hitbox range used by Wubi/Xiaohe
                let norm = (scores[i] / max_s).clamp(0.0, 1.0);
                priors[i] = (65.0 + 115.0 * norm).round().clamp(54.0, 180.0) as u8;
            }
        }
    }
    (mask, priors)
}
