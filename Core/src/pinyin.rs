//! Full Pinyin sentence decoder with:
//! - Toneless syllable lattice + initial-only abbreviations (`srf`, `shrf`, `shurf`)
//! - Apostrophe syllable separator (`xi'an`)
//! - Touch-boundary, physical-neighbor, and adjacent-transposition typo correction
//! - `Micro-RWKV-7` + `AuxView` N-gram + `PersonalContext` beam search

use crate::code::PackedCode;
use crate::config::JuxuConfig;
use crate::engine::{Candidate, CandidateBadge};
use crate::english::{format_english_display, EnDictView};
use crate::format::AuxView;
use crate::neural::{NeuralView, RwkvState, TOK_UNK};
use crate::personalization::Context as PersonalContext;
use crate::pinyin_dict::{base_initial_id, PinyinDictView};
use crate::typo::{
    key_choices_for_obs, letter_to_t9_digit, softmax_confidences, t9_key_choices_for_obs,
    t9_letters_for_digit, CorrectionKind, KeyLayout, KeyObs,
};
use crate::user_data::UserStore;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

const BEAM_WIDTH: usize = 20;
const MAX_SYL_LEN: usize = 6;
const MAX_EN_LEN: usize = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SylMatch {
    Full(u16),
    Initial(u8, bool),
    Prefix(String),
    Latin {
        display: String,
        weight: u16,
        is_oov: bool,
        is_explicit_upper: bool,
    },
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
pub enum PathState {
    Ready(Rc<RwkvState>),
    PendingChar {
        parent: Rc<RwkvState>,
        tok: usize,
    },
    Evicted,
}

#[derive(Clone)]
pub struct PinyinPath {
    pub text: String,
    pub score: f32,
    pub boundaries: Vec<PinyinBoundary>,
    pub path_state: PathState,
    pub(crate) personal_context: PersonalContext,
    pub any_corrected: bool,
}

impl PinyinPath {
    #[inline]
    pub fn ready_state(&self) -> Option<&RwkvState> {
        match &self.path_state {
            PathState::Ready(rc) => Some(rc.as_ref()),
            _ => None,
        }
    }

    #[inline]
    pub fn ready_rc(&self) -> Option<&Rc<RwkvState>> {
        match &self.path_state {
            PathState::Ready(rc) => Some(rc),
            _ => None,
        }
    }

    #[inline]
    pub fn is_evicted(&self) -> bool {
        matches!(self.path_state, PathState::Evicted)
    }

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
    pub english: &'a EnDictView,
    pub aux: Option<&'a AuxView>,
    pub model: Option<NeuralView<'a>>,
    pub config: &'a JuxuConfig,
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
    raw_code: &str,
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
            if obs.ch == b'\'' || obs.is_upper {
                Vec::new()
            } else {
                key_choices_for_obs(obs, decoder.layout, &decoder.user.touch_profile, &decoder.config.typo, true)
            }
        })
        .collect();

    for start in 0..n {
        if observations[start].ch == b'\'' || observations[start].is_upper {
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
        if start + 1 < n && observations[start + 1].ch != b'\'' && !observations[start + 1].is_upper {
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

    add_latin_lattice_edges(observations, raw_code, decoder, &mut edges_by_end, true);

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

/// Standard Full Pinyin orthographic spelling aliases (Hanyu Pinyin + Rime-ice / iOS / Sogou conventions):
/// - `lue -> lve`, `nue -> nve` (`略/掠/虐/疟`, unambiguous `0.0` offset)
/// - `lu -> lv`, `nu -> nv` (`绿/率/旅/律/虑/女`, `-0.45` offset so bare `lu`/`nu` still prefers `路`/`努`)
/// - `j/q/x/y + v* -> u*` (`jv->ju`, `jve->jue`, `jvan->juan`, `jvn->jun`, `jven->jun`, `0.0` offset)
/// - Uncontracted finals `*iou -> *iu`, `*uei -> *ui`, `*uen -> *un` (`shuei->shui`, `liou->liu`, `luen->lun`, `0.0` offset)
fn pinyin_orthographic_variants(exact_bytes: &[u8], exact_str: &str) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    let len = exact_bytes.len();
    match exact_str {
        "lue" => out.push(("lve".to_owned(), 0.0)),
        "nue" => out.push(("nve".to_owned(), 0.0)),
        "lu" => out.push(("lv".to_owned(), -0.45)),
        "nu" => out.push(("nv".to_owned(), -0.45)),
        _ => {}
    }
    if (2..=4).contains(&len)
        && matches!(exact_bytes[0], b'j' | b'q' | b'x' | b'y')
        && exact_bytes[1] == b'v'
    {
        if &exact_bytes[1..] == b"ven" {
            let mut s = String::with_capacity(3);
            s.push(exact_bytes[0] as char);
            s.push_str("un");
            out.push((s, 0.0));
        } else {
            let mut norm = exact_bytes.to_vec();
            norm[1] = b'u';
            out.push((String::from_utf8_lossy(&norm).into_owned(), 0.0));
        }
    }
    if len >= 4 && !matches!(exact_bytes[0], b'a' | b'e' | b'i' | b'o' | b'u' | b'v') {
        if let Some(stem) = exact_str.strip_suffix("iou") {
            out.push((format!("{stem}iu"), 0.0));
        } else if let Some(stem) = exact_str.strip_suffix("uei") {
            out.push((format!("{stem}ui"), 0.0));
        } else if let Some(stem) = exact_str.strip_suffix("uen") {
            out.push((format!("{stem}un"), 0.0));
        }
    }
    out
}

const ORTHO_PINYIN_SYLLABLES: &[&str] = &[
    "lue", "nue", "jv", "jve", "jvan", "jvn", "jven", "qv", "qve", "qvan", "qvn", "qven", "xv",
    "xve", "xvan", "xvn", "xven", "yv", "yve", "yvan", "yvn", "yven", "diou", "miou", "niou",
    "liou", "jiou", "qiou", "xiou", "duei", "tuei", "guei", "kuei", "huei", "zhuei", "chuei",
    "shuei", "ruei", "zuei", "cuei", "suei", "duen", "tuen", "nuen", "luen", "guen", "kuen",
    "huen", "zhuen", "chuen", "shuen", "ruen", "zuen", "cuen", "suen", "juen", "quen", "xuen",
    "yuen",
];

fn build_lattice(
    observations: &[KeyObs],
    raw_code: &str,
    decoder: &PinyinDecoder<'_>,
) -> Vec<Vec<LatticeEdge>> {
    if decoder.config.schema.is_shuangpin() {
        return build_shuangpin_lattice(observations, raw_code, decoder);
    }
    let n = observations.len();
    let mut edges_by_end = vec![Vec::<LatticeEdge>::new(); n + 1];
    if n == 0 {
        return edges_by_end;
    }

    let per_pos_choices: Vec<Vec<(u8, f32, bool, bool)>> = observations
        .iter()
        .map(|obs| {
            if obs.ch == b'\'' || obs.is_upper {
                Vec::new()
            } else {
                key_choices_for_obs(obs, decoder.layout, &decoder.user.touch_profile, &decoder.config.typo, true)
            }
        })
        .collect();

    for start in 0..n {
        // Skip leading apostrophes and uppercase Latin anchors transparently
        if observations[start].ch == b'\'' || observations[start].is_upper {
            continue;
        }
        let mut effective_start = start;
        // If previous char was apostrophe, `start` is a fresh syllable start;
        // also allow edges starting before an apostrophe to end right after it.
        let _ = &mut effective_start;

        let max_len = (n - start).min(MAX_SYL_LEN + 1);
        for len in 1..=max_len {
            let end = start + len;
            if observations[start..end]
                .iter()
                .any(|o| o.ch == b'\'' || o.is_upper)
            {
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
                for (ortho_syl, ortho_offset) in pinyin_orthographic_variants(&exact_bytes, &exact_str) {
                    hyps.push((ortho_syl, exact_lp, CorrectionKind::Exact, ortho_offset));
                }
            }

            let is_exact_or_ortho_syl = hyps
                .iter()
                .any(|(cand, _, kind, _)| *kind == CorrectionKind::Exact && decoder.dict.lookup_syllable_id(cand).is_some());

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
                // 1) Missing trailing 'g' in -ng finals (an->ang, en->eng, in->ing, on->ong, ian->iang, uan->uang, ion->iong)
                if (2..=5).contains(&len)
                    && (exact_str.ends_with("an")
                        || exact_str.ends_with("en")
                        || exact_str.ends_with("in")
                        || exact_str.ends_with("on"))
                {
                    let mut with_g = exact_str.clone();
                    with_g.push('g');
                    if decoder.dict.lookup_syllable_id(&with_g).is_some() {
                        hyps.push((with_g, exact_lp, CorrectionKind::NeighborKey, -3.6));
                    }
                }
                // 2) Missing 1 letter anywhere inside a syllable (e.g. zhog->zhong, xio->xiao)
                if (2..=5).contains(&len) && !is_exact_or_ortho_syl {
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
                                    -4.6,
                                ));
                            }
                        }
                    }
                }

                // Extra letter (insertion / fat-finger double-tap, e.g. zhonng->zhong, womren->women)
                if (3..=7).contains(&len) && (len > MAX_SYL_LEN || !is_exact_or_ortho_syl) {
                    for drop_pos in 0..len {
                        let is_repeat = (drop_pos > 0
                            && exact_bytes[drop_pos] == exact_bytes[drop_pos - 1])
                            || (drop_pos + 1 < len
                                && exact_bytes[drop_pos] == exact_bytes[drop_pos + 1]);
                        // Never drop a trailing consonant initial as an "extra letter" when it can start
                        // the next syllable (e.g. `zhez` = `zhe'z`, `gaij` = `gai'j`, `tas` = `ta's`,
                        // `gangg` = `gang'g`, `shangp` = `shang'p`).
                        if drop_pos + 1 == len && (!is_repeat || end == n) {
                            continue;
                        }
                        if end == n
                            && drop_pos + 2 == len
                            && exact_bytes[len - 2] == exact_bytes[len - 1]
                        {
                            continue;
                        }
                        let mut b = Vec::with_capacity(len - 1);
                        b.extend_from_slice(&exact_bytes[..drop_pos]);
                        b.extend_from_slice(&exact_bytes[drop_pos + 1..]);
                        let s = String::from_utf8_lossy(&b).into_owned();
                        if decoder.dict.lookup_syllable_id(&s).is_some() {
                            let cost = if is_repeat { -3.6 } else { -5.2 };
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
                let typo_cost = if correction == CorrectionKind::Exact {
                    extra_cost
                } else if extra_cost < 0.0 {
                    extra_cost
                } else {
                    match correction {
                        CorrectionKind::Exact => 0.0,
                        // Touch-boundary taps already pay continuous 2D Gaussian `spatial_logp`;
                        // keep step cost light so frontend `hitTest` priors aren't double-counted.
                        CorrectionKind::TouchBoundary => -0.65,
                        CorrectionKind::NeighborKey => -4.2,
                        CorrectionKind::Transposition => -1.2,
                    }
                };
                let disp_syl = if correction == CorrectionKind::Exact {
                    exact_str.clone()
                } else {
                    s.clone()
                };
                if let Some(sid) = decoder.dict.lookup_syllable_id(&s) {
                    // Base per-key credit applies to ALL full syllables; typo edges pay only their
                    // calibrated relative penalty `typo_cost` (+ `spatial_logp`), so 4-5 letter typo
                    // syllables (e.g. `shanban` -> `上班`) aren't unfairly double-penalized by `-1.35*len`.
                    let full_cost = 1.35 * ((end - start) as f32) + typo_cost;
                    edges_by_end[actual_end].push(LatticeEdge {
                        start,
                        end: actual_end,
                        syl: SylMatch::Full(sid),
                        spatial_logp,
                        kind_cost: full_cost,
                        correction,
                        display_syl: disp_syl,
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
                        syl: SylMatch::Prefix(s),
                        spatial_logp,
                        kind_cost: typo_cost - 3.5,
                        correction,
                        display_syl: disp_syl,
                    });
                }
            }
        }
    }

    add_latin_lattice_edges(observations, raw_code, decoder, &mut edges_by_end, false);

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

fn build_t9_lattice(
    observations: &[KeyObs],
    t9_locked_syls: &[(usize, String)],
    decoder: &PinyinDecoder<'_>,
) -> Vec<Vec<LatticeEdge>> {
    let n = observations.len();
    let mut edges_by_end = vec![Vec::<LatticeEdge>::new(); n + 1];
    if n == 0 {
        return edges_by_end;
    }

    // 1. Emit fixed edges for locked prefix syllables selected from the T9 sidebar
    let mut unlock_start = 0usize;
    for (klen, syl) in t9_locked_syls {
        while unlock_start < n && observations[unlock_start].ch == b'\'' {
            unlock_start += 1;
        }
        if unlock_start >= n || *klen == 0 {
            break;
        }
        let end = (unlock_start + *klen).min(n);
        let actual_end = if end < n && observations[end].ch == b'\'' {
            end + 1
        } else {
            end
        };
        if let Some(sid) = decoder.dict.lookup_syllable_id(syl) {
            edges_by_end[actual_end].push(LatticeEdge {
                start: unlock_start,
                end: actual_end,
                syl: SylMatch::Full(sid),
                spatial_logp: 0.0,
                kind_cost: 1.35 * (*klen as f32) + 2.0,
                correction: CorrectionKind::Exact,
                display_syl: syl.clone(),
            });
            unlock_start = actual_end;
        } else {
            break;
        }
    }
    while unlock_start < n && observations[unlock_start].ch == b'\'' {
        unlock_start += 1;
    }

    let t9_map = decoder.dict.t9_map();
    let per_pos_choices: Vec<Vec<(u8, f32, bool, bool)>> = observations
        .iter()
        .map(|obs| {
            if (b'2'..=b'9').contains(&obs.ch) {
                t9_key_choices_for_obs(obs, decoder.layout, &decoder.user.touch_profile, &decoder.config.typo)
            } else {
                Vec::new()
            }
        })
        .collect();

    for start in unlock_start..n {
        if !(b'2'..=b'9').contains(&observations[start].ch) {
            continue;
        }
        let max_len = (n - start).min(MAX_SYL_LEN);
        for len in 1..=max_len {
            let end = start + len;
            if observations[start..end]
                .iter()
                .any(|o| !(b'2'..=b'9').contains(&o.ch))
            {
                break;
            }
            let exact_bytes: Vec<u8> = observations[start..end].iter().map(|o| o.ch).collect();
            let exact_str = String::from_utf8_lossy(&exact_bytes).into_owned();
            let exact_lp: f32 = (start..end)
                .map(|i| {
                    per_pos_choices[i]
                        .iter()
                        .find(|c| c.2)
                        .map_or(0.0, |c| c.1)
                })
                .sum();
            let actual_end = if end < n && observations[end].ch == b'\'' {
                end + 1
            } else {
                end
            };

            let mut had_exact_full = false;
            if let Some(sids) = t9_map.get(&exact_str) {
                had_exact_full = true;
                for &sid in sids {
                    let syl_str = decoder.dict.syllable_str(sid);
                    edges_by_end[actual_end].push(LatticeEdge {
                        start,
                        end: actual_end,
                        syl: SylMatch::Full(sid),
                        spatial_logp: exact_lp,
                        kind_cost: 1.35 * (len as f32),
                        correction: CorrectionKind::Exact,
                        display_syl: syl_str.to_owned(),
                    });
                }
            }

            if decoder.config.typo.enabled {
                // Single-position 2D Gaussian boundary / neighbor key substitution
                for pos in 0..len {
                    let obs_idx = start + pos;
                    for &(alt_digit, lp, is_exact, is_boundary) in &per_pos_choices[obs_idx] {
                        if is_exact {
                            continue;
                        }
                        // Require genuine key-boundary proximity for short 1..=2 key spans to prevent T9 ambiguity blowup
                        if len <= 2 && !is_boundary {
                            continue;
                        }
                        let mut b = exact_bytes.clone();
                        b[pos] = alt_digit;
                        let cand_digits = String::from_utf8_lossy(&b);
                        let Some(sids) = t9_map.get(cand_digits.as_ref()) else {
                            continue;
                        };
                        let other_lp: f32 = (start..end)
                            .enumerate()
                            .filter(|&(j, _)| j != pos)
                            .map(|(_, idx)| {
                                per_pos_choices[idx]
                                    .iter()
                                    .find(|c| c.2)
                                    .map_or(0.0, |c| c.1)
                            })
                            .sum();
                        let (kind, kind_cost) = if is_boundary {
                            (CorrectionKind::TouchBoundary, -0.85)
                        } else {
                            (CorrectionKind::NeighborKey, -5.2)
                        };
                        for &sid in sids {
                            let syl_str = decoder.dict.syllable_str(sid);
                            edges_by_end[actual_end].push(LatticeEdge {
                                start,
                                end: actual_end,
                                syl: SylMatch::Full(sid),
                                spatial_logp: other_lp + lp,
                                kind_cost,
                                correction: kind,
                                display_syl: syl_str.to_owned(),
                            });
                        }
                    }
                }

                // Conservative adjacent key transposition (higher penalty than 26-key due to T9 density)
                if len >= 2 {
                    for pos in 0..(len - 1) {
                        if exact_bytes[pos] == exact_bytes[pos + 1] {
                            continue;
                        }
                        let dt = observations[start + pos + 1]
                            .timestamp_ms
                            .saturating_sub(observations[start + pos].timestamp_ms);
                        if len == 2 && !(observations[start + pos + 1].timestamp_ms > 0 && dt <= 65) {
                            continue;
                        }
                        let mut b = exact_bytes.clone();
                        b.swap(pos, pos + 1);
                        let cand_digits = String::from_utf8_lossy(&b);
                        let Some(sids) = t9_map.get(cand_digits.as_ref()) else {
                            continue;
                        };
                        let trans_lp =
                            if observations[start + pos + 1].timestamp_ms > 0 && dt <= 65 {
                                -3.2
                            } else {
                                -4.2
                            };
                        for &sid in sids {
                            let syl_str = decoder.dict.syllable_str(sid);
                            edges_by_end[actual_end].push(LatticeEdge {
                                start,
                                end: actual_end,
                                syl: SylMatch::Full(sid),
                                spatial_logp: trans_lp,
                                kind_cost: -4.2,
                                correction: CorrectionKind::Transposition,
                                display_syl: syl_str.to_owned(),
                            });
                        }
                    }
                }
            }

            // Initial-only abbreviations (`len == 1` single initial, `len == 2` zh/ch/sh)
            if decoder.config.pinyin_initial_abbr {
                if len == 1 {
                    for &ch in t9_letters_for_digit(exact_bytes[0]) {
                        let s = (ch as char).to_string();
                        let init_id = base_initial_id(&s);
                        if init_id > 0 {
                            edges_by_end[actual_end].push(LatticeEdge {
                                start,
                                end: actual_end,
                                syl: SylMatch::Initial(init_id, false),
                                spatial_logp: exact_lp,
                                kind_cost: -2.8,
                                correction: CorrectionKind::Exact,
                                display_syl: s,
                            });
                        }
                    }
                } else if len == 2 {
                    let digraph: Option<&str> = match (exact_bytes[0], exact_bytes[1]) {
                        (b'9', b'4') => Some("zh"),
                        (b'2', b'4') => Some("ch"),
                        (b'7', b'4') => Some("sh"),
                        _ => None,
                    };
                    if let Some(dg) = digraph {
                        let init_id = base_initial_id(dg);
                        if init_id > 0 {
                            edges_by_end[actual_end].push(LatticeEdge {
                                start,
                                end: actual_end,
                                syl: SylMatch::Initial(init_id, true),
                                spatial_logp: exact_lp,
                                kind_cost: -2.2,
                                correction: CorrectionKind::Exact,
                                display_syl: dg.to_owned(),
                            });
                        }
                    }
                }
            }

            // Trailing partial-syllable prefix completion at end of input buffer
            if actual_end == n && !had_exact_full && len >= 2 {
                let mut prefixes: HashSet<String> = HashSet::new();
                for (digits_key, sids) in t9_map {
                    if digits_key.len() > len && digits_key.starts_with(&exact_str) {
                        for &sid in sids {
                            let syl_str = decoder.dict.syllable_str(sid);
                            if let Some(pref) = syl_str.get(..len) {
                                prefixes.insert(pref.to_owned());
                            }
                        }
                    }
                }
                for pref in prefixes.into_iter().take(6) {
                    edges_by_end[actual_end].push(LatticeEdge {
                        start,
                        end: actual_end,
                        syl: SylMatch::Prefix(pref.clone()),
                        spatial_logp: exact_lp,
                        kind_cost: -3.5,
                        correction: CorrectionKind::Exact,
                        display_syl: pref,
                    });
                }
            }
        }
    }

    // Prune and deduplicate edges per `end` (slightly wider cap than 26-key for T9 branching)
    for list in &mut edges_by_end {
        list.sort_by(|a, b| {
            (b.spatial_logp + b.kind_cost)
                .partial_cmp(&(a.spatial_logp + a.kind_cost))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut deduped: Vec<LatticeEdge> = Vec::with_capacity(32);
        for edge in list.drain(..) {
            if !deduped
                .iter()
                .any(|e| e.start == edge.start && e.syl == edge.syl)
            {
                deduped.push(edge);
                if deduped.len() >= 32 {
                    break;
                }
            }
        }
        *list = deduped;
    }

    edges_by_end
}

/// Inject English dictionary, personal English, uppercase-anchored, and impossible-pinyin OOV
/// `SylMatch::Latin` edges into the syllable lattice for mixed Chinese-English sentence decoding.
fn add_latin_lattice_edges(
    observations: &[KeyObs],
    raw_code: &str,
    decoder: &PinyinDecoder<'_>,
    edges_by_end: &mut [Vec<LatticeEdge>],
    is_shuangpin: bool,
) {
    let n = observations.len();
    if n == 0 || raw_code.len() != n {
        return;
    }

    // 1. Compute exact & any (including 1-edit typo) full-syllable reachability & coverage (`O(n^2)` for small `n`)
    let mut exact_full_edges: Vec<Vec<usize>> = vec![Vec::new(); n + 1];
    let mut covered_by_exact_full = vec![false; n];
    let mut covered_by_any_full = vec![false; n];
    for end in 1..=n {
        for e in &edges_by_end[end] {
            if matches!(e.syl, SylMatch::Full(_)) {
                for pos in e.start..end {
                    covered_by_any_full[pos] = true;
                }
                if e.correction == CorrectionKind::Exact {
                    exact_full_edges[e.start].push(end);
                    for pos in e.start..end {
                        covered_by_exact_full[pos] = true;
                    }
                }
            }
        }
    }
    let can_parse_exact_full = |start: usize, end: usize| -> bool {
        if start >= end {
            return true;
        }
        let len = end - start;
        let mut dp = vec![false; len + 1];
        dp[0] = true;
        for i in 0..len {
            if !dp[i] {
                continue;
            }
            let abs_i = start + i;
            for &next_end in &exact_full_edges[abs_i] {
                if next_end <= end {
                    dp[next_end - start] = true;
                }
            }
        }
        dp[len]
    };

    // 2. Scan candidate Latin spans `start..end`
    for start in 0..n {
        if observations[start].ch == b'\'' {
            continue;
        }
        let max_len = (n - start).min(MAX_EN_LEN);
        for len in 1..=max_len {
            let end = start + len;
            if observations[start..end].iter().any(|o| o.ch == b'\'') {
                break;
            }
            let raw_slice = &raw_code[start..end];
            if !raw_slice.bytes().all(|b| b.is_ascii_alphabetic()) {
                break;
            }
            let has_upper = observations[start..end].iter().any(|o| o.is_upper);
            let starts_upper = observations[start].is_upper;
            // Don't start an uppercase-containing span mid-word after another uppercase letter
            // unless `start` itself is uppercase (CamelCase) or previous was not uppercase.
            if has_upper && !starts_upper {
                // Allow mixed-case dictionary words like `iOS`, `macOS`, `iPhone`, `iPad`, `eBay`
                let lower = raw_slice.to_ascii_lowercase();
                let is_known_mixed = decoder.user.lookup_english_exact(&lower).is_some()
                    || decoder.english.lookup_exact(&lower).is_some();
                if !is_known_mixed {
                    continue;
                }
            }

            let actual_end = if end < n && observations[end].ch == b'\'' {
                end + 1
            } else {
                end
            };
            let lower = raw_slice.to_ascii_lowercase();

            // 2a. Explicit uppercase anchor (covers both dictionary words & OOV like `Juxu`, `StillFail`, `PR`, `Go`)
            if starts_upper {
                let user_hit = decoder.user.lookup_english_exact(&lower);
                let dict_hit = decoder.english.lookup_with_morphology(&lower);
                let is_known = user_hit.is_some() || dict_hit.is_some();
                let weight = user_hit
                    .map(|(_, c)| (52_000u32 + c.min(10) * 1000).min(65_535) as u16)
                    .or_else(|| dict_hit.as_ref().map(|x| x.1))
                    .unwrap_or(28_000);
                // Emit if known word, or at end of buffer, or `len >= 2` (so OOV `Juxu` + Chinese `xie` works)
                if is_known || actual_end == n || len >= 2 {
                    let kind_cost = if is_known {
                        1.42 * (len as f32) + 4.2
                    } else {
                        1.18 * (len as f32) + 2.0
                    };
                    edges_by_end[actual_end].push(LatticeEdge {
                        start,
                        end: actual_end,
                        syl: SylMatch::Latin {
                            display: raw_slice.to_owned(),
                            weight,
                            is_oov: !is_known,
                            is_explicit_upper: true,
                        },
                        spatial_logp: 0.0,
                        kind_cost,
                        correction: CorrectionKind::Exact,
                        display_syl: raw_slice.to_owned(),
                    });
                }
                continue;
            }

            if len < 2 {
                continue;
            }

            // 2b. All-lowercase (or known mixed-case like `iOS`/`macOS`) dictionary / user English word
            let raw_user_hit = decoder.user.lookup_english_exact(&lower);
            let dict_hit = decoder.english.lookup_with_morphology(&lower);
            let exact_py_span = !has_upper && can_parse_exact_full(start, end);
            // Guard against 1-off `Return` passthrough of raw Pinyin examples (e.g. `"celue"`, `"lve"`, `"ue"`):
            // an all-lowercase OOV string (`dict_hit.is_none()` and no uppercase in `disp`) must not hijack
            // short `<=3`-letter spans on `cnt == 1` or override valid exact Chinese Pinyin at #1.
            let user_hit = raw_user_hit.filter(|&(disp, cnt)| {
                let disp_has_upper = disp.bytes().any(|b| b.is_ascii_uppercase());
                if has_upper || disp_has_upper || dict_hit.is_some() {
                    true
                } else if exact_py_span {
                    cnt >= 3
                } else {
                    len > 3 || cnt >= 2
                }
            });
            if user_hit.is_some() || dict_hit.is_some() {
                let (canonical, weight, is_user) = if let Some((disp, cnt)) = user_hit {
                    (
                        disp.to_owned(),
                        (54_000u32 + cnt.min(10) * 1000).min(65_535) as u16,
                        true,
                    )
                } else {
                    let (disp, w, _) = dict_hit.unwrap();
                    (disp, w, false)
                };
                let display = format_english_display(raw_slice, &canonical);
                let has_vowel = lower
                    .bytes()
                    .any(|b| matches!(b, b'a' | b'e' | b'i' | b'o' | b'u' | b'y'));
                // For all-lowercase 2-letter English words, only emit ultra-common ones (or user-learned)
                // so Chinese 2..4-letter initial abbreviations (e.g. `bjsj` -> `北京时间`) aren't hijacked.
                if !has_upper && !is_user && len == 2 && weight < 48_000 {
                    continue;
                }
                let exact_py = !has_upper && can_parse_exact_full(start, end);
                let all_pos_covered = !has_upper && (start..end).all(|p| covered_by_exact_full[p]);
                let w_bonus = ((weight as f32) / 65535.0) * 1.1 + if is_user { 1.6 } else { 0.0 };
                let kind_cost = if has_upper {
                    1.42 * (len as f32) + 2.5
                } else if !exact_py && all_pos_covered {
                    // Cuts across valid Chinese full syllables (e.g. `chaos` + `hi` slicing through `chao'shi` = 超时)
                    0.95 * (len as f32) - 6.8 + if is_user { 1.5 } else { 0.0 }
                } else if !exact_py {
                    let prefix_is_exact_py = len >= 3 && can_parse_exact_full(start, end - 1);
                    if !is_user && len == 2 {
                        0.85 * (len as f32) - 5.2 + w_bonus * 0.5
                    } else if !is_user && len == 3 && !has_vowel {
                        // Consonant-only 3-letter acronym (e.g. `sql`, `ssh`, `pdf`, `cbd`): keep gentle so
                        // Chinese 3-letter initial abbreviations (`srf`, `jsj`, `cbd`) still win cleanly
                        0.90 * (len as f32) - 6.2 + w_bonus * 0.5
                    } else if !is_user && prefix_is_exact_py && weight < 42_000 {
                        // Low-frequency English word that is a valid Chinese syllable + trailing initial
                        // (e.g. `tas` = `ta's` 他是, `dans` = `dan's` 但是)
                        0.90 * (len as f32) - 5.8 + w_bonus * 0.5
                    } else if is_shuangpin {
                        1.05 * (len as f32) - 2.8 + w_bonus
                    } else {
                        1.15 * (len as f32) - 2.6 + w_bonus
                    }
                } else if !is_shuangpin && len <= 3 && decoder.dict.lookup_syllable_id(&lower).is_some() {
                    // Single exact Chinese syllable (e.g. `he`, `me`, `man`, `can`, `you`, `run`) -> keep Chinese strongly preferred
                    0.85 * (len as f32) - 6.5 + if is_user { 1.5 } else { 0.0 }
                } else if is_shuangpin {
                    // In Shuangpin almost every 2-letter pair is valid, so require >= 4 letters or user-learned for lowercase English
                    if len >= 4 || is_user {
                        0.95 * (len as f32) - 4.8 + w_bonus
                    } else {
                        continue;
                    }
                } else {
                    // Multi-syllable exact Chinese match (e.g. `women`, `fade`, `cheng`, `abi`, `data`, `api`)
                    1.02 * (len as f32) - 6.0 + w_bonus
                };
                edges_by_end[actual_end].push(LatticeEdge {
                    start,
                    end: actual_end,
                    syl: SylMatch::Latin {
                        display: display.clone(),
                        weight,
                        is_oov: false,
                        is_explicit_upper: has_upper,
                    },
                    spatial_logp: 0.0,
                    kind_cost,
                    correction: CorrectionKind::Exact,
                    display_syl: display,
                });
                continue;
            }

            // 2c. Open-vocabulary OOV `RawLatin` fallback for impossible-pinyin spans
            //     (e.g. `worktree`, `stillfail`, `wbdict`, `kqueue`, `tokio`, `serde`, `nomofu`).
            //     Require `len >= 4`, at least one vowel, and at least one position uncovered even by
            //     1-edit Pinyin typo syllables so spelling typos like `zhogwen` -> `中文` still rescue cleanly.
            if !is_shuangpin && !has_upper && len >= 4 {
                let has_vowel = lower
                    .bytes()
                    .any(|b| matches!(b, b'a' | b'e' | b'i' | b'o' | b'u' | b'y'));
                let has_impossible = has_vowel && (start..end).any(|p| !covered_by_any_full[p]);
                // Don't treat a valid Chinese full-syllable prefix + 1..2 trailing initials
                // (e.g. `shangp`, `woguor`, `heyiw`, `shurf`) or a span dominated by valid Chinese
                // full syllables (e.g. `nshitiganhuihaohend` = 17/19 Chinese) as an OOV English word.
                let is_chinese_plus_tail_init = (end >= start + 2 && can_parse_exact_full(start, end - 1))
                    || (end >= start + 3 && can_parse_exact_full(start, end - 2));
                let exact_covered_cnt = (start..end).filter(|&p| covered_by_exact_full[p]).count();
                if has_impossible
                    && !can_parse_exact_full(start, end)
                    && !is_chinese_plus_tail_init
                    && !covered_by_exact_full[end - 1]
                    && exact_covered_cnt * 2 <= len
                {
                    // Only start/end on clean boundaries (0, n, or adjacent to exact-full-covered positions)
                    let left_clean = start == 0 || covered_by_exact_full[start - 1] || !covered_by_exact_full[start];
                    let right_clean = end == n || covered_by_exact_full[end] || !covered_by_exact_full[end - 1];
                    if left_clean && right_clean {
                        let kind_cost = 0.05 * (len as f32) - 3.2;
                        edges_by_end[actual_end].push(LatticeEdge {
                            start,
                            end: actual_end,
                            syl: SylMatch::Latin {
                                display: raw_slice.to_owned(),
                                weight: 18_000,
                                is_oov: true,
                                is_explicit_upper: false,
                            },
                            spatial_logp: 0.0,
                            kind_cost,
                            correction: CorrectionKind::Exact,
                            display_syl: raw_slice.to_owned(),
                        });
                    }
                }
            }
        }
    }
}

/// Retain all `BEAM_WIDTH` states for the most recent `FULL_STATE_WINDOW` keystroke layers
/// (covering normal Pinyin syllables + 1-backspace lookback with zero recompute), top-2 states for
/// `TOP2_STATE_WINDOW` (covering long inline English words), and 4-path checkpoints every 12 keys
/// so even continuous hold-backspace across 60+ keys stays `< 25 MB` Dirty RAM and `< 2 ms` avg!
const FULL_STATE_WINDOW: usize = 22;
const TOP2_STATE_WINDOW: usize = 52;
const CHECKPOINT_STRIDE: usize = 12;

#[inline]
fn edge_parent_limit(edge: &LatticeEdge) -> usize {
    match &edge.syl {
        SylMatch::Full(_) => match edge.correction {
            CorrectionKind::Exact => {
                if edge.start <= 3 {
                    16
                } else {
                    8
                }
            }
            CorrectionKind::TouchBoundary => 4,
            _ => 2,
        },
        SylMatch::Initial(_, _) => {
            if edge.correction == CorrectionKind::Exact && edge.start <= 4 {
                8
            } else if edge.correction == CorrectionKind::Exact {
                4
            } else {
                2
            }
        }
        SylMatch::Prefix(_) => 3,
        SylMatch::Latin {
            is_explicit_upper, ..
        } => {
            if *is_explicit_upper {
                4
            } else {
                2
            }
        }
    }
}

fn non_prefix_edge_sig(edges: &[LatticeEdge]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    for e in edges {
        if matches!(e.syl, SylMatch::Prefix(_)) {
            continue;
        }
        // Ignore weak lowercase English background candidates (`kind_cost < -1.0`) in cache invalidation
        // so completing a subsequent Chinese syllable (`covered_by_exact_full`) never rewinds earlier Chinese layers.
        if let SylMatch::Latin {
            is_explicit_upper: false,
            ..
        } = e.syl
        {
            if e.kind_cost < -1.0 {
                continue;
            }
        }
        mix(e.start as u64);
        mix(e.spatial_logp.to_bits() as u64);
        mix(e.kind_cost.to_bits() as u64);
        mix(e.correction as u8 as u64);
        match &e.syl {
            SylMatch::Full(sid) => {
                mix(1);
                mix(*sid as u64);
            }
            SylMatch::Initial(id, dg) => {
                mix(2);
                mix((*id as u64) | (u64::from(*dg) << 16));
            }
            SylMatch::Latin {
                display,
                weight,
                is_oov,
                is_explicit_upper,
            } => {
                mix(3);
                mix((*weight as u64) | (u64::from(*is_oov) << 16) | (u64::from(*is_explicit_upper) << 17));
                for &b in display.as_bytes() {
                    mix(b as u64);
                }
            }
            SylMatch::Prefix(_) => {}
        }
    }
    h
}

#[inline]
fn obs_same(a: &KeyObs, b: &KeyObs) -> bool {
    a.ch == b.ch
        && a.is_upper == b.is_upper
        && a.has_touch == b.has_touch
        && a.norm_x.to_bits() == b.norm_x.to_bits()
        && a.norm_y.to_bits() == b.norm_y.to_bits()
}

/// Materialize any `PathState::PendingChar` items in `paths[..limit]` using a single batched
/// `step_tokens_batch` pass over the 9.8 MB RWKV-7 weights.
fn materialize_paths_batch(
    paths: &mut [PinyinPath],
    nv: Option<&NeuralView<'_>>,
    limit: usize,
) {
    let cap = limit.min(paths.len());
    let Some(model) = nv else {
        for p in &mut paths[..cap] {
            if let PathState::PendingChar { ref parent, .. } = p.path_state {
                p.path_state = PathState::Ready(Rc::clone(parent));
            }
        }
        return;
    };

    let mut indices: Vec<usize> = Vec::new();
    let mut toks: Vec<usize> = Vec::new();
    let mut states: Vec<RwkvState> = Vec::new();
    for (idx, p) in paths[..cap].iter().enumerate() {
        if let PathState::PendingChar { ref parent, tok } = p.path_state {
            indices.push(idx);
            toks.push(tok);
            states.push((**parent).clone());
        }
    }
    if indices.is_empty() {
        return;
    }

    {
        let mut batch: Vec<(usize, &mut RwkvState, bool, bool)> = indices
            .iter()
            .zip(toks.iter())
            .zip(states.iter_mut())
            .map(|((&path_idx, &tok), st)| (tok, st, path_idx < 2, false))
            .collect();
        model.step_tokens_batch(&mut batch);
    }

    for (path_idx, st) in indices.into_iter().zip(states.into_iter()) {
        paths[path_idx].path_state = PathState::Ready(Rc::new(st));
    }
}

/// Materialize `PendingChar` paths across multiple parent layers in a SINGLE batched `step_tokens_batch` sweep.
fn materialize_multi_layers_batch(
    layers: &mut [Vec<PinyinPath>],
    reqs: &[(usize, usize)],
    nv: Option<&NeuralView<'_>>,
) {
    if reqs.is_empty() {
        return;
    }
    let Some(model) = nv else {
        for &(pos, limit) in reqs {
            if let Some(paths) = layers.get_mut(pos) {
                let cap = limit.min(paths.len());
                for p in &mut paths[..cap] {
                    if let PathState::PendingChar { ref parent, .. } = p.path_state {
                        p.path_state = PathState::Ready(Rc::clone(parent));
                    }
                }
            }
        }
        return;
    };

    let mut coords: Vec<(usize, usize)> = Vec::new();
    let mut toks: Vec<usize> = Vec::new();
    let mut states: Vec<RwkvState> = Vec::new();
    for &(pos, limit) in reqs {
        let Some(paths) = layers.get(pos) else {
            continue;
        };
        let cap = limit.min(paths.len());
        for (idx, p) in paths[..cap].iter().enumerate() {
            if let PathState::PendingChar { ref parent, tok } = p.path_state {
                coords.push((pos, idx));
                toks.push(tok);
                states.push((**parent).clone());
            }
        }
    }
    if coords.is_empty() {
        return;
    }
    {
        let mut batch: Vec<(usize, &mut RwkvState, bool, bool)> = coords
            .iter()
            .zip(toks.iter())
            .zip(states.iter_mut())
            .map(|((&(_, path_idx), &tok), st)| (tok, st, path_idx < 2, false))
            .collect();
        model.step_tokens_batch(&mut batch);
    }
    for ((pos, idx), st) in coords.into_iter().zip(states.into_iter()) {
        layers[pos][idx].path_state = PathState::Ready(Rc::new(st));
    }
}

#[derive(Default)]
pub struct PinyinStream {
    layers: Vec<Vec<PinyinPath>>,
    layer_sigs: Vec<u64>,
    prev_obs: Vec<KeyObs>,
    prev_t9_locked: Vec<(usize, String)>,
    prev_is_t9: bool,
    prev_ctx: Vec<char>,
    prev_init_sig: u32,
}

impl PinyinStream {
    pub fn clear(&mut self) {
        self.layers.clear();
        self.layer_sigs.clear();
        self.prev_obs.clear();
        self.prev_t9_locked.clear();
    }

    fn run_incremental(
        &mut self,
        observations: &[KeyObs],
        raw_code: &str,
        t9_locked_syls: &[(usize, String)],
        is_t9: bool,
        decoder: &PinyinDecoder<'_>,
        initial_state: &RwkvState,
    ) -> (Vec<PinyinCandidate>, Vec<PinyinPath>) {
        let n = observations.len();
        if n == 0 {
            self.clear();
            return (Vec::new(), Vec::new());
        }

        let init_sig = initial_state.p1_scale.to_bits()
            ^ initial_state
                .p1_q
                .first()
                .map_or(0, |&b| (b as u8 as u32) << 16);

        if self.prev_is_t9 != is_t9
            || self.prev_t9_locked != t9_locked_syls
            || self.prev_ctx.as_slice() != decoder.context
            || self.prev_init_sig != init_sig
        {
            self.layers.clear();
            self.layer_sigs.clear();
            self.prev_obs.clear();
        }

        let lattice = if is_t9 {
            build_t9_lattice(observations, t9_locked_syls, decoder)
        } else {
            build_lattice(observations, raw_code, decoder)
        };

        let mut target_sigs = Vec::with_capacity(n + 1);
        target_sigs.push(0u64);
        for end in 1..=n {
            target_sigs.push(non_prefix_edge_sig(&lattice[end]));
        }

        let common_obs = self
            .prev_obs
            .iter()
            .zip(observations.iter())
            .take_while(|(a, b)| obs_same(a, b))
            .count();

        // Keep every cached non-Prefix layer `1..=max_valid` whose lattice edges are 100% identical!
        // Because `SylMatch::Prefix` is evaluated as an ephemeral terminal overlay (never stored in `self.layers`),
        // forward typing never invalidates `n - 1` after a partial syllable, and Backspace reuses `layers[0..=n]` directly!
        let mut keep_layers = if self.layers.is_empty() {
            0
        } else {
            let max_valid = common_obs.min(n).min(self.layers.len() - 1);
            let mut matched = 0usize;
            for pos in 1..=max_valid {
                if self.layer_sigs.get(pos).copied() == Some(target_sigs[pos]) {
                    matched = pos;
                } else {
                    break;
                }
            }
            matched + 1
        };

        // Check if any newly computed layer `keep_layers..=n` (or terminal `Prefix` overlay at `n`)
        // references a parent layer whose required states were evicted on ultra-long (>22 key) hold-backspace.
        if decoder.model.is_some() && keep_layers > 1 {
            let mut min_evicted_parent: Option<usize> = None;
            for end in keep_layers..=n {
                for e in &lattice[end] {
                    if matches!(e.syl, SylMatch::Prefix(_)) {
                        continue;
                    }
                    let req = edge_parent_limit(e);
                    if e.start < keep_layers {
                        let cap = req.min(self.layers[e.start].len());
                        if self.layers[e.start][..cap].iter().any(PinyinPath::is_evicted) {
                            min_evicted_parent =
                                Some(min_evicted_parent.map_or(e.start, |m| m.min(e.start)));
                        }
                    }
                }
            }
            for e in &lattice[n] {
                if matches!(e.syl, SylMatch::Prefix(_)) && e.start < keep_layers {
                    let cap = 2usize.min(self.layers[e.start].len());
                    if self.layers[e.start][..cap].iter().any(PinyinPath::is_evicted) {
                        min_evicted_parent =
                            Some(min_evicted_parent.map_or(e.start, |m| m.min(e.start)));
                    }
                }
            }
            if let Some(bad_pos) = min_evicted_parent {
                let mut cp = bad_pos.saturating_sub(1);
                while cp > 0 {
                    if !self.layers[cp].is_empty()
                        && !self.layers[cp].iter().any(PinyinPath::is_evicted)
                    {
                        break;
                    }
                    cp -= 1;
                }
                if cp == 0 && bad_pos >= CHECKPOINT_STRIDE {
                    let aligned = ((bad_pos - 1) / CHECKPOINT_STRIDE) * CHECKPOINT_STRIDE;
                    if aligned > 0
                        && aligned < self.layers.len()
                        && !self.layers[aligned].is_empty()
                        && !self.layers[aligned][0].is_evicted()
                    {
                        self.layers[aligned].retain(|p| !p.is_evicted());
                        cp = aligned;
                    }
                }
                keep_layers = cp + 1;
            }
        }

        self.layers.truncate(keep_layers);
        self.layer_sigs.truncate(keep_layers);

        if self.layers.is_empty() {
            self.layers.push(vec![PinyinPath {
                text: String::new(),
                score: 0.0,
                boundaries: Vec::new(),
                path_state: PathState::Ready(Rc::new(initial_state.clone())),
                personal_context: PersonalContext::with_chars(decoder.context.iter().copied()),
                any_corrected: false,
            }]);
            self.layer_sigs.push(0);
        }

        let start_end = self.layers.len().max(1);
        for end in start_end..=n {
            let next_paths = extend_pinyin_layer(end, &lattice, &mut self.layers, decoder, false);
            self.layers.push(next_paths);
            self.layer_sigs.push(target_sigs[end]);
        }

        // Bound Dirty RAM cleanly while preserving 22-key full window + 52-key top-2 window + checkpoints
        if decoder.model.is_some() && n > FULL_STATE_WINDOW {
            let full_cut = n - FULL_STATE_WINDOW;
            let mut evict_reqs: Vec<(usize, usize)> = Vec::new();
            for k in 1..full_cut {
                if self.layers[k].is_empty() {
                    continue;
                }
                let keep_n = if k % CHECKPOINT_STRIDE == 0 {
                    4usize.min(self.layers[k].len())
                } else if k + TOP2_STATE_WINDOW >= n {
                    2usize.min(self.layers[k].len())
                } else {
                    0usize
                };
                if keep_n > 0 {
                    evict_reqs.push((k, keep_n));
                }
            }
            materialize_multi_layers_batch(&mut self.layers, &evict_reqs, decoder.model.as_ref());
            for k in 1..full_cut {
                if self.layers[k].is_empty() {
                    continue;
                }
                let keep_n = if k % CHECKPOINT_STRIDE == 0 {
                    4usize.min(self.layers[k].len())
                } else if k + TOP2_STATE_WINDOW >= n {
                    2usize.min(self.layers[k].len())
                } else {
                    0usize
                };
                for p in &mut self.layers[k][keep_n..] {
                    p.path_state = PathState::Evicted;
                }
            }
        }

        self.prev_obs.clear();
        self.prev_obs.extend_from_slice(observations);
        self.prev_t9_locked = t9_locked_syls.to_vec();
        self.prev_is_t9 = is_t9;
        self.prev_ctx.clear();
        self.prev_ctx.extend_from_slice(decoder.context);
        self.prev_init_sig = init_sig;

        // Ephemeral terminal overlay if `lattice[n]` has trailing `SylMatch::Prefix` edges:
        // temporarily swap `self.layers[n]` to build candidates & Top-1 prior, then restore clean `self.layers[n]`!
        let has_terminal_prefix = lattice[n]
            .iter()
            .any(|e| matches!(e.syl, SylMatch::Prefix(_)));
        if has_terminal_prefix {
            let mut terminal_n =
                extend_pinyin_layer(n, &lattice, &mut self.layers[..n], decoder, true);
            materialize_paths_batch(&mut terminal_n, decoder.model.as_ref(), 1);
            let saved_n = std::mem::replace(&mut self.layers[n], terminal_n);
            if saved_n.is_empty() {
                if let Some(best_end) = (1..=n).rev().find(|&pos| !self.layers[pos].is_empty()) {
                    materialize_paths_batch(&mut self.layers[best_end], decoder.model.as_ref(), 1);
                }
            }
            let res = build_pinyin_candidates(n, raw_code, &lattice, &self.layers, decoder);
            self.layers[n] = saved_n;
            return res;
        }

        if let Some(best_end) = (1..=n).rev().find(|&pos| !self.layers[pos].is_empty()) {
            materialize_paths_batch(&mut self.layers[best_end], decoder.model.as_ref(), 1);
        }

        build_pinyin_candidates(n, raw_code, &lattice, &self.layers, decoder)
    }

    pub fn update_and_decode(
        &mut self,
        observations: &[KeyObs],
        raw_code: &str,
        decoder: &PinyinDecoder<'_>,
        initial_state: &RwkvState,
    ) -> (Vec<PinyinCandidate>, Vec<PinyinPath>) {
        self.run_incremental(observations, raw_code, &[], false, decoder, initial_state)
    }

    /// T9 9-key Pinyin decode entry point reusing incremental layer caching & locked prefix syllables.
    pub fn update_and_decode_t9(
        &mut self,
        observations: &[KeyObs],
        raw_code: &str,
        t9_locked_syls: &[(usize, String)],
        decoder: &PinyinDecoder<'_>,
        initial_state: &RwkvState,
    ) -> (Vec<PinyinCandidate>, Vec<PinyinPath>) {
        self.run_incremental(
            observations,
            raw_code,
            t9_locked_syls,
            true,
            decoder,
            initial_state,
        )
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

/// Score a single character under pure Bayesian neural + probability-layer personalization:
/// `log P(syllable | char) + log P_adapted(char | h)`
/// For single-pronunciation in-vocab characters, `log P(syllable | char) == 0.0`, relying 100% on
/// `Micro-RWKV-7` + `PersonalContinuations::interpolate_logp` with zero dictionary frequency weights
/// or phrase bonuses.
#[inline]
fn score_char_pure_neural(
    nv: &NeuralView<'_>,
    state: &RwkvState,
    ch: char,
    dict_weight: u32,
    dict: &PinyinDictView,
    personal: &crate::personalization::PersonalContinuations,
    personal_context: PersonalContext,
    personalize: bool,
    use_personal_states: bool,
) -> f32 {
    let tok = nv.char_to_token(ch);
    let raw_lp = if tok == TOK_UNK {
        -12.0 + 0.05 * ((dict_weight as f32) + 1.0).ln()
    } else {
        nv.score_next_token(state, tok)
    };
    let lm_lp = if personalize {
        personal.interpolate_logp(
            raw_lp,
            personal_context,
            ch,
            use_personal_states.then_some(state),
        )
    } else {
        raw_lp
    };
    lm_lp + dict.char_emission_logp(ch, dict_weight)
}

fn extend_pinyin_layer(
    end: usize,
    lattice: &[Vec<LatticeEdge>],
    layers: &mut [Vec<PinyinPath>],
    decoder: &PinyinDecoder<'_>,
    include_prefix_edges: bool,
) -> Vec<PinyinPath> {
    if lattice[end].is_empty() {
        return Vec::new();
    }
    if !include_prefix_edges
        && lattice[end]
            .iter()
            .all(|e| matches!(e.syl, SylMatch::Prefix(_)))
    {
        return Vec::new();
    }

    // Coalesce all required parent & grandparent state materializations into ONE single `step_tokens_batch` sweep!
    {
        let mut req_map: Vec<(usize, usize)> = Vec::with_capacity(10);
        let mut add_req = |pos: usize, limit: usize| {
            if pos >= layers.len() {
                return;
            }
            if let Some(existing) = req_map.iter_mut().find(|x| x.0 == pos) {
                if limit > existing.1 {
                    existing.1 = limit;
                }
            } else {
                req_map.push((pos, limit));
            }
        };
        for edge in &lattice[end] {
            if !include_prefix_edges && matches!(edge.syl, SylMatch::Prefix(_)) {
                continue;
            }
            add_req(edge.start, edge_parent_limit(edge));
        }
        if decoder.model.is_some() {
            for e2 in lattice[end]
                .iter()
                .filter(|e| {
                    (include_prefix_edges || !matches!(e.syl, SylMatch::Prefix(_)))
                        && !matches!(e.syl, SylMatch::Latin { .. })
                        && e.start > 0
                })
                .take(2)
            {
                for e1 in lattice[e2.start]
                    .iter()
                    .filter(|e| {
                        e.correction == CorrectionKind::Exact
                            && !matches!(e.syl, SylMatch::Latin { .. } | SylMatch::Prefix(_))
                    })
                    .take(2)
                {
                    add_req(e1.start, 2);
                }
            }
        }
        materialize_multi_layers_batch(layers, &req_map, decoder.model.as_ref());
    }

    let layers: &[Vec<PinyinPath>] = &*layers;
    let personal = &decoder.user.continuations;
    let personalize = decoder.config.dynamic_word_freq && !personal.is_empty();
    let use_personal_states = personalize
        && decoder
            .model
            .as_ref()
            .is_some_and(|m| personal.matches_model(m));

    struct Step<'a> {
        parent: &'a PinyinPath,
        parent_rc: &'a Rc<RwkvState>,
        text: String,
        ch: char,
        is_latin: bool,
        score: f32,
        syl_display: String,
        correction: CorrectionKind,
        end: usize,
    }

    let mut steps: Vec<Step<'_>> = Vec::with_capacity(128);

    // Pure character-level single-syllable expansion:
    // ZERO multi-syllable phrase chains, ZERO dictionary phrase lookup, ZERO phrase/user-phrase bonuses.
    for edge in &lattice[end] {
        if !include_prefix_edges && matches!(edge.syl, SylMatch::Prefix(_)) {
            continue;
        }
        let parent_cap = edge_parent_limit(edge);
        let parents = &layers[edge.start];
        if parents.is_empty() {
            continue;
        }
        let parents_slice = &parents[..parent_cap.min(parents.len())];
        let syl_score = 1.8 * edge.spatial_logp + edge.kind_cost;

        match &edge.syl {
            SylMatch::Latin {
                display,
                weight,
                is_oov,
                is_explicit_upper,
            } => {
                let Some(first) = display.chars().next() else {
                    continue;
                };
                let base_prior = if *is_explicit_upper {
                    1.0
                } else if *is_oov {
                    -0.5
                } else {
                    0.22 * (((*weight as f32) + 1.0).ln() - 9.0)
                };
                let mut shared_bias = syl_score + base_prior;
                if decoder.config.dynamic_word_freq {
                    shared_bias += 1.0 * decoder.user.unigram_boost(display);
                }
                for parent in parents_slice {
                    let Some(parent_rc) = parent.ready_rc() else {
                        continue;
                    };
                    let neural_est = if let Some(nv) = &decoder.model {
                        nv.estimate_latin_logp(parent_rc.as_ref(), display)
                    } else {
                        0.0
                    };
                    steps.push(Step {
                        parent,
                        parent_rc,
                        text: display.clone(),
                        ch: first,
                        is_latin: true,
                        score: parent.score + shared_bias + neural_est,
                        syl_display: edge.display_syl.clone(),
                        correction: edge.correction,
                        end,
                    });
                }
            }
            SylMatch::Full(sid) => {
                let chars = decoder
                    .dict
                    .lookup_chars(*sid, decoder.config.enable_rare_charset);
                if chars.is_empty() {
                    continue;
                }
                let scan_limit = chars.len().min(96);
                let per_parent_keep = if edge.correction == CorrectionKind::Exact { 16 } else { 5 };

                for parent in parents_slice {
                    let Some(parent_rc) = parent.ready_rc() else {
                        continue;
                    };
                    let parent_st = parent_rc.as_ref();
                    let mut top: Vec<(char, f32)> = Vec::with_capacity(per_parent_keep + 1);
                    let mut floor = f32::NEG_INFINITY;
                    for e in &chars[..scan_limit] {
                        let Some(ch) = e.text.chars().next() else {
                            continue;
                        };
                        let lp = if let Some(nv) = &decoder.model {
                            score_char_pure_neural(
                                nv,
                                parent_st,
                                ch,
                                e.weight,
                                decoder.dict,
                                personal,
                                parent.personal_context,
                                personalize,
                                use_personal_states,
                            )
                        } else {
                            0.25 * (((e.weight as f32) + 1.0).ln() - 15.0)
                                + if personalize {
                                    personal.score(parent.personal_context, e.text)
                                } else {
                                    0.0
                                }
                        };
                        if top.len() == per_parent_keep && lp <= floor {
                            continue;
                        }
                        let pos = top.partition_point(|x| x.1 > lp);
                        if pos < per_parent_keep {
                            top.insert(pos, (ch, lp));
                            if top.len() > per_parent_keep {
                                top.pop();
                            }
                            if top.len() == per_parent_keep {
                                floor = top.last().unwrap().1;
                            }
                        }
                    }
                    for (ch, lp) in top {
                        steps.push(Step {
                            parent,
                            parent_rc,
                            text: ch.to_string(),
                            ch,
                            is_latin: false,
                            score: parent.score + syl_score + lp,
                            syl_display: edge.display_syl.clone(),
                            correction: edge.correction,
                            end,
                        });
                    }
                }
            }
            SylMatch::Initial(init_id, is_digraph) => {
                let prefix = &edge.display_syl;
                let sids = decoder.dict.syllables_with_prefix(prefix);
                let mut cand_chars: Vec<(char, u32)> = Vec::with_capacity(160);
                let mut seen_ch: HashSet<char> = HashSet::with_capacity(160);
                for sid in sids {
                    let (sid_init, sid_dg) = decoder.dict.syllable_init(sid);
                    if sid_init == *init_id && (!*is_digraph || sid_dg) {
                        for e in decoder
                            .dict
                            .lookup_chars(sid, decoder.config.enable_rare_charset)
                            .into_iter()
                            .take(10)
                        {
                            if let Some(ch) = e.text.chars().next() {
                                if seen_ch.insert(ch) {
                                    cand_chars.push((ch, e.weight));
                                }
                            }
                        }
                    }
                }
                let per_parent_keep = 8usize;
                for parent in parents_slice {
                    let Some(parent_rc) = parent.ready_rc() else {
                        continue;
                    };
                    let parent_st = parent_rc.as_ref();
                    let mut top: Vec<(char, f32)> = Vec::with_capacity(per_parent_keep + 1);
                    let mut floor = f32::NEG_INFINITY;
                    for &(ch, w) in &cand_chars {
                        let lp = if let Some(nv) = &decoder.model {
                            score_char_pure_neural(
                                nv,
                                parent_st,
                                ch,
                                w,
                                decoder.dict,
                                personal,
                                parent.personal_context,
                                personalize,
                                use_personal_states,
                            )
                        } else {
                            0.25 * (((w as f32) + 1.0).ln() - 15.0)
                        };
                        if top.len() == per_parent_keep && lp <= floor {
                            continue;
                        }
                        let pos = top.partition_point(|x| x.1 > lp);
                        if pos < per_parent_keep {
                            top.insert(pos, (ch, lp));
                            if top.len() > per_parent_keep {
                                top.pop();
                            }
                            if top.len() == per_parent_keep {
                                floor = top.last().unwrap().1;
                            }
                        }
                    }
                    for (ch, lp) in top {
                        steps.push(Step {
                            parent,
                            parent_rc,
                            text: ch.to_string(),
                            ch,
                            is_latin: false,
                            score: parent.score + syl_score + lp,
                            syl_display: edge.display_syl.clone(),
                            correction: edge.correction,
                            end,
                        });
                    }
                }
            }
            SylMatch::Prefix(pref) => {
                let sids = decoder.dict.syllables_with_prefix(pref);
                let mut cand_chars: Vec<(char, u32)> = Vec::with_capacity(64);
                let mut seen_ch: HashSet<char> = HashSet::with_capacity(64);
                for sid in sids.into_iter().take(6) {
                    for e in decoder
                        .dict
                        .lookup_chars(sid, decoder.config.enable_rare_charset)
                        .into_iter()
                        .take(8)
                    {
                        if let Some(ch) = e.text.chars().next() {
                            if seen_ch.insert(ch) {
                                cand_chars.push((ch, e.weight));
                            }
                        }
                    }
                }
                let per_parent_keep = 6usize;
                for parent in parents_slice {
                    let Some(parent_rc) = parent.ready_rc() else {
                        continue;
                    };
                    let parent_st = parent_rc.as_ref();
                    let mut top: Vec<(char, f32)> = Vec::with_capacity(per_parent_keep + 1);
                    let mut floor = f32::NEG_INFINITY;
                    for &(ch, w) in &cand_chars {
                        let lp = if let Some(nv) = &decoder.model {
                            score_char_pure_neural(
                                nv,
                                parent_st,
                                ch,
                                w,
                                decoder.dict,
                                personal,
                                parent.personal_context,
                                personalize,
                                use_personal_states,
                            )
                        } else {
                            0.25 * (((w as f32) + 1.0).ln() - 15.0)
                        };
                        if top.len() == per_parent_keep && lp <= floor {
                            continue;
                        }
                        let pos = top.partition_point(|x| x.1 > lp);
                        if pos < per_parent_keep {
                            top.insert(pos, (ch, lp));
                            if top.len() > per_parent_keep {
                                top.pop();
                            }
                            if top.len() == per_parent_keep {
                                floor = top.last().unwrap().1;
                            }
                        }
                    }
                    for (ch, lp) in top {
                        steps.push(Step {
                            parent,
                            parent_rc,
                            text: ch.to_string(),
                            ch,
                            is_latin: false,
                            score: parent.score + syl_score + lp,
                            syl_display: edge.display_syl.clone(),
                            correction: edge.correction,
                            end,
                        });
                    }
                }
            }
        }
    }

    steps.sort_unstable_by(|a, b| b.score.total_cmp(&a.score));

    // Deduplicate by full resulting string and record lazy `PathState::PendingChar` transitions
    // (0 RWKV steps and 0 `280 KB` clones until a subsequent syllable actually branches from `end`!).
    let mut next_paths: Vec<PinyinPath> = Vec::with_capacity(BEAM_WIDTH);
    let mut seen_texts: HashSet<String> = HashSet::with_capacity(BEAM_WIDTH * 2);

    for step in steps {
        if next_paths.len() >= BEAM_WIDTH {
            break;
        }
        let mut full_text = String::with_capacity(step.parent.text.len() + step.text.len());
        full_text.push_str(&step.parent.text);
        full_text.push_str(&step.text);
        if !seen_texts.insert(full_text.clone()) {
            continue;
        }

        let mut score = step.score;
        let mut personal_context = step.parent.personal_context;
        let path_state = if let Some(nv) = &decoder.model {
            if step.is_latin {
                let est = nv.estimate_latin_logp(step.parent_rc.as_ref(), &step.text);
                let (lp, st) =
                    nv.score_and_step_latin(step.parent_rc.as_ref(), &step.text, true, false);
                score = score - est + lp;
                for ch in step.text.chars() {
                    personal_context = personal_context.push(ch);
                }
                PathState::Ready(Rc::new(st))
            } else {
                personal_context = personal_context.push(step.ch);
                PathState::PendingChar {
                    parent: Rc::clone(step.parent_rc),
                    tok: nv.char_to_token(step.ch),
                }
            }
        } else {
            for ch in step.text.chars() {
                personal_context = personal_context.push(ch);
            }
            PathState::Ready(Rc::clone(step.parent_rc))
        };

        let mut boundaries = step.parent.boundaries.clone();
        boundaries.push(PinyinBoundary {
            keys: step.end,
            text_bytes: full_text.len(),
            syl_display: step.syl_display,
            correction: step.correction,
        });
        next_paths.push(PinyinPath {
            text: full_text,
            score,
            boundaries,
            path_state,
            personal_context,
            any_corrected: step.parent.any_corrected || step.correction != CorrectionKind::Exact,
        });
    }

    // Pure-Neural MTP-2 2-Syllable Lookahead Rescue (zero phrase dictionary!):
    if let Some(nv) = &decoder.model {
        let beam_floor = if next_paths.len() >= BEAM_WIDTH {
            next_paths.last().map_or(f32::NEG_INFINITY, |p| p.score)
        } else {
            f32::NEG_INFINITY
        };

        let collect_edge_chars = |edge: &LatticeEdge, max_n: usize| -> Vec<(char, u32)> {
            match &edge.syl {
                SylMatch::Full(sid) => decoder
                    .dict
                    .lookup_chars(*sid, decoder.config.enable_rare_charset)
                    .into_iter()
                    .take(max_n)
                    .filter_map(|e| e.text.chars().next().map(|ch| (ch, e.weight)))
                    .collect(),
                SylMatch::Initial(init_id, is_digraph) => {
                    let sids = decoder.dict.syllables_with_prefix(&edge.display_syl);
                    let mut out = Vec::with_capacity(160);
                    let mut seen_c = HashSet::with_capacity(160);
                    for sid in sids {
                        let (sid_init, sid_dg) = decoder.dict.syllable_init(sid);
                        if sid_init == *init_id && (!*is_digraph || sid_dg) {
                            for e in decoder
                                .dict
                                .lookup_chars(sid, decoder.config.enable_rare_charset)
                                .into_iter()
                                .take(6)
                            {
                                if let Some(ch) = e.text.chars().next() {
                                    if seen_c.insert(ch) {
                                        out.push((ch, e.weight));
                                    }
                                }
                            }
                        }
                    }
                    out.sort_unstable_by(|a, b| b.1.cmp(&a.1));
                    out.truncate(max_n);
                    out
                }
                SylMatch::Prefix(pref) => {
                    let sids = decoder.dict.syllables_with_prefix(pref);
                    let mut out = Vec::with_capacity(max_n);
                    let mut seen_c = HashSet::with_capacity(max_n);
                    for sid in sids.into_iter().take(4) {
                        for e in decoder
                            .dict
                            .lookup_chars(sid, decoder.config.enable_rare_charset)
                            .into_iter()
                            .take(6)
                        {
                            if let Some(ch) = e.text.chars().next() {
                                if seen_c.insert(ch) {
                                    out.push((ch, e.weight));
                                    if out.len() >= max_n {
                                        return out;
                                    }
                                }
                            }
                        }
                    }
                    out
                }
                SylMatch::Latin { .. } => Vec::new(),
            }
        };

        struct RescueCand<'a> {
            gp: &'a PinyinPath,
            gp_rc: &'a Rc<RwkvState>,
            e1: &'a LatticeEdge,
            e2: &'a LatticeEdge,
            c1: char,
            w1: u32,
            c2: char,
            w2: u32,
            est: f32,
        }
        let mut rescues: Vec<RescueCand<'_>> = Vec::with_capacity(8);
        let mut p2_q = [0i8; 512];

        for e2 in lattice[end]
            .iter()
            .filter(|e| {
                (include_prefix_edges || !matches!(e.syl, SylMatch::Prefix(_)))
                    && !matches!(e.syl, SylMatch::Latin { .. })
                    && e.start > 0
            })
            .take(2)
        {
            let e2_is_init = matches!(e2.syl, SylMatch::Initial(_, _));
            let c2_raw = collect_edge_chars(e2, if e2_is_init { 64 } else { 48 });
            if c2_raw.is_empty() {
                continue;
            }
            let syl_score2 = 1.8 * e2.spatial_logp + e2.kind_cost;

            for e1 in lattice[e2.start]
                .iter()
                .filter(|e| {
                    e.correction == CorrectionKind::Exact
                        && !matches!(e.syl, SylMatch::Latin { .. } | SylMatch::Prefix(_))
                })
                .take(2)
            {
                let gps = &layers[e1.start];
                if gps.is_empty() {
                    continue;
                }
                let e1_is_init = matches!(e1.syl, SylMatch::Initial(_, _));
                let c1_raw = collect_edge_chars(e1, if e1_is_init { 120 } else { 80 });
                if c1_raw.is_empty() {
                    continue;
                }
                let syl_score1 = 1.8 * e1.spatial_logp + e1.kind_cost;
                let c1_keep = if e1_is_init { 40 } else { 28 };

                for gp in gps.iter().take(2) {
                    let Some(gp_rc) = gp.ready_rc() else {
                        continue;
                    };
                    let gp_st = gp_rc.as_ref();
                    if gp_st.mat_state.is_empty() {
                        continue;
                    }
                    let mut top_c1: Vec<(char, u32, f32)> = Vec::with_capacity(c1_keep + 1);
                    let mut c1_floor = f32::NEG_INFINITY;
                    for &(ch1, w1) in &c1_raw {
                        let lp1 = score_char_pure_neural(
                            nv,
                            gp_st,
                            ch1,
                            w1,
                            decoder.dict,
                            personal,
                            gp.personal_context,
                            personalize,
                            use_personal_states,
                        );
                        if top_c1.len() == c1_keep && lp1 <= c1_floor {
                            continue;
                        }
                        let pos = top_c1.partition_point(|x| x.2 > lp1);
                        if pos < c1_keep {
                            top_c1.insert(pos, (ch1, w1, lp1));
                            if top_c1.len() > c1_keep {
                                top_c1.pop();
                            }
                            if top_c1.len() == c1_keep {
                                c1_floor = top_c1.last().unwrap().2;
                            }
                        }
                    }

                    for &(c1, w1, lp1) in &top_c1 {
                        let p2_sc = nv.prepare_mtp2_vec(gp_st, c1, &mut p2_q);
                        let ctx_after_c1 = gp.personal_context.push(c1);
                        for &(c2, w2) in &c2_raw {
                            let raw_lp2 = if nv.char_to_token(c2) == TOK_UNK {
                                -12.0
                            } else {
                                nv.score_mtp2_char(&p2_q, p2_sc, c2)
                            };
                            let lm_lp2 = if personalize {
                                personal.interpolate_logp(raw_lp2, ctx_after_c1, c2, None)
                            } else {
                                raw_lp2
                            };
                            let lp2 = lm_lp2 + decoder.dict.char_emission_logp(c2, w2);
                            let est = gp.score + syl_score1 + syl_score2 + lp1 + lp2;
                            if est <= beam_floor - 1.5 {
                                continue;
                            }
                            let mut cand_text = String::with_capacity(gp.text.len() + 8);
                            cand_text.push_str(&gp.text);
                            cand_text.push(c1);
                            cand_text.push(c2);
                            if seen_texts.contains(&cand_text) {
                                continue;
                            }
                            if rescues.len() < 4 || est > rescues.last().unwrap().est {
                                let pos = rescues.partition_point(|r| r.est > est);
                                if pos < 4 {
                                    rescues.insert(
                                        pos,
                                        RescueCand {
                                            gp,
                                            gp_rc,
                                            e1,
                                            e2,
                                            c1,
                                            w1,
                                            c2,
                                            w2,
                                            est,
                                        },
                                    );
                                    if rescues.len() > 4 {
                                        rescues.pop();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Batch `c1` RWKV transitions across all rescued `(c1, c2)` pairs in a SINGLE weight pass,
        // and leave `c2` lazy (`PendingChar`) until a future syllable actually branches from `end`!
        let mut active_rescues: Vec<(RescueCand<'_>, String)> = Vec::with_capacity(4);
        for r in rescues {
            let mut full_text = String::with_capacity(r.gp.text.len() + 8);
            full_text.push_str(&r.gp.text);
            full_text.push(r.c1);
            full_text.push(r.c2);
            if seen_texts.insert(full_text.clone()) {
                active_rescues.push((r, full_text));
            }
        }
        if !active_rescues.is_empty() {
            let mut c1_states: Vec<RwkvState> = active_rescues
                .iter()
                .map(|(r, _)| (**r.gp_rc).clone())
                .collect();
            {
                let mut batch: Vec<(usize, &mut RwkvState, bool, bool)> = active_rescues
                    .iter()
                    .zip(c1_states.iter_mut())
                    .map(|((r, _), st)| (nv.char_to_token(r.c1), st, false, false))
                    .collect();
                nv.step_tokens_batch(&mut batch);
            }
            for ((r, full_text), st_after_c1) in active_rescues.into_iter().zip(c1_states.into_iter()) {
                let syl_score1 = 1.8 * r.e1.spatial_logp + r.e1.kind_cost;
                let syl_score2 = 1.8 * r.e2.spatial_logp + r.e2.kind_cost;
                let lp1 = score_char_pure_neural(
                    nv,
                    r.gp_rc.as_ref(),
                    r.c1,
                    r.w1,
                    decoder.dict,
                    personal,
                    r.gp.personal_context,
                    personalize,
                    use_personal_states,
                );
                let pctx1 = r.gp.personal_context.push(r.c1);
                let lp2 = score_char_pure_neural(
                    nv,
                    &st_after_c1,
                    r.c2,
                    r.w2,
                    decoder.dict,
                    personal,
                    pctx1,
                    personalize,
                    use_personal_states,
                );
                let pctx2 = pctx1.push(r.c2);
                let exact_score = r.gp.score + syl_score1 + syl_score2 + lp1 + lp2;
                let mut boundaries = r.gp.boundaries.clone();
                let mid_bytes = r.gp.text.len() + r.c1.len_utf8();
                boundaries.push(PinyinBoundary {
                    keys: r.e1.end,
                    text_bytes: mid_bytes,
                    syl_display: r.e1.display_syl.clone(),
                    correction: r.e1.correction,
                });
                boundaries.push(PinyinBoundary {
                    keys: r.e2.end,
                    text_bytes: full_text.len(),
                    syl_display: r.e2.display_syl.clone(),
                    correction: r.e2.correction,
                });
                next_paths.push(PinyinPath {
                    text: full_text,
                    score: exact_score,
                    boundaries,
                    path_state: PathState::PendingChar {
                        parent: Rc::new(st_after_c1),
                        tok: nv.char_to_token(r.c2),
                    },
                    personal_context: pctx2,
                    any_corrected: r.gp.any_corrected
                        || r.e1.correction != CorrectionKind::Exact
                        || r.e2.correction != CorrectionKind::Exact,
                });
            }
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
    // Pure Neural Candidate List (zero dictionary phrase lookup):
    //   Tier 1: Top full-sentence paths from the neural char beam
    //   Tier 2: Multi-character prefix paths (2..=4 chars) already decoded in intermediate neural beam layers
    //   Tier 3: Single-character candidates at position 0, ranked purely by RWKV + PersonalContinuations
    //   Tier 4: English prefix completions
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

    let raw_scores: Vec<f32> = full_paths.iter().map(|p| p.score).collect();
    let confs = softmax_confidences(&raw_scores);
    let mut full_added = 0usize;
    for (idx, path) in full_paths.iter().enumerate() {
        if full_added >= max_full_sentences {
            break;
        }
        if is_long_sentence && full_added >= 1 && path.score < top_score - 4.5 {
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

    // Tier 2: Multi-character prefix paths (2..=4 chars starting at 0, ending < best_end) from neural beam
    struct PrefixWordItem {
        text: String,
        consumed: usize,
        char_len: usize,
        score: f32,
        corr: bool,
        hint: String,
    }
    let mut prefix_words: Vec<PrefixWordItem> = Vec::new();
    for end in (2..best_end).rev() {
        let mut taken_at_end = 0usize;
        for path in layers[end].iter() {
            let clen = path.text.chars().count();
            if !(2..=4).contains(&clen) {
                continue;
            }
            prefix_words.push(PrefixWordItem {
                text: path.text.clone(),
                consumed: end,
                char_len: clen,
                score: path.score,
                corr: path.any_corrected,
                hint: path.correction_hint(),
            });
            taken_at_end += 1;
            if taken_at_end >= 5 {
                break;
            }
        }
    }

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
    let max_prefix_words = if is_long_sentence { 5usize } else { 8usize };
    let mut per_span_counts: HashMap<(usize, usize), usize> = HashMap::new();
    for pw in prefix_words {
        if prefix_words_added >= max_prefix_words {
            break;
        }
        let span_cnt = per_span_counts.entry((pw.consumed, pw.char_len)).or_insert(0);
        if *span_cnt >= 2 {
            continue;
        }
        if seen.insert(pw.text.clone()) {
            *span_cnt += 1;
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

    // Tier 3: Single-char candidates at position 0 ranked purely by RWKV + PersonalContinuations
    let init_path = layers[0].first();
    let init_state = init_path.and_then(PinyinPath::ready_state);
    let init_pctx = init_path.map_or_else(PersonalContext::default, |p| p.personal_context);
    let personal = &decoder.user.continuations;
    let personalize = decoder.config.dynamic_word_freq && !personal.is_empty();
    let use_personal_states = personalize
        && decoder
            .model
            .as_ref()
            .is_some_and(|m| personal.matches_model(m));

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
            let take_n = if is_corr { 6 } else { 96 };
            for e in decoder
                .dict
                .lookup_chars(sid, decoder.config.enable_rare_charset)
                .into_iter()
                .take(take_n)
            {
                let sc = if let (Some(nv), Some(st), Some(ch)) =
                    (&decoder.model, init_state, e.text.chars().next())
                {
                    edge.kind_cost
                        + score_char_pure_neural(
                            nv,
                            st,
                            ch,
                            e.weight,
                            decoder.dict,
                            personal,
                            init_pctx,
                            personalize,
                            use_personal_states,
                        )
                } else {
                    0.25 * (((e.weight as f32) + 1.0).ln() - 15.0) + edge.kind_cost
                };
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
            .then_with(|| b.1.cmp(&a.1))
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

    // Tier 4: English prefix completions when typing a single English word prefix (e.g. `pyth` -> `Python`)
    if n >= 2 && n <= MAX_EN_LEN && raw_code.bytes().all(|b| b.is_ascii_alphabetic()) {
        let lower = raw_code.to_ascii_lowercase();
        let mut en_cands: Vec<String> = Vec::new();
        for (disp, _) in decoder.user.lookup_english_prefix(&lower, 2) {
            en_cands.push(format_english_display(raw_code, disp));
        }
        for e in decoder.english.lookup_prefix(&lower, 3) {
            if e.weight >= 24_000 || raw_code.bytes().any(|b| b.is_ascii_uppercase()) {
                let disp = if e.key == lower {
                    format_english_display(raw_code, e.display)
                } else {
                    e.display.to_owned()
                };
                if !en_cands.contains(&disp) {
                    en_cands.push(disp);
                }
            }
        }
        let insert_at = out.len().min(3);
        let mut inserted = 0usize;
        for disp in en_cands.into_iter().take(3) {
            if seen.insert(disp.clone()) {
                let pos = (insert_at + inserted).min(out.len());
                out.insert(
                    pos,
                    PinyinCandidate {
                        candidate: Candidate {
                            text: disp,
                            matched_code: PackedCode::EMPTY,
                            comment: String::new(),
                            badge: CandidateBadge::None,
                            confidence: 0.20,
                            is_exact: true,
                        },
                        consumed_keys: n,
                        boundaries: Vec::new(),
                    },
                );
                inserted += 1;
            }
        }
    }

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
        if dict.lookup_syllable_id(sub).is_some() || ORTHO_PINYIN_SYLLABLES.contains(&sub) {
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
        for &os in ORTHO_PINYIN_SYLLABLES {
            if os.starts_with(sub) {
                if let Some(&next_b) = os.as_bytes().get(sub.len()) {
                    if next_b.is_ascii_lowercase() {
                        let idx = (next_b - b'a') as usize;
                        mask |= 1u32 << idx;
                        scores[idx] += 2.0;
                    }
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

/// Compute the candidate Pinyin syllables at the current unlocked T9 cursor position
/// for the iOS T9 left sidebar filter column (ordered by top-beam relevance, length, and char weight).
pub fn t9_syllable_options(
    observations: &[KeyObs],
    t9_locked_syls: &[(usize, String)],
    dict: &PinyinDictView,
    top_paths: &[PinyinPath],
) -> Vec<String> {
    let n = observations.len();
    if n == 0 {
        return Vec::new();
    }
    let mut start = 0usize;
    for (klen, _) in t9_locked_syls {
        while start < n && observations[start].ch == b'\'' {
            start += 1;
        }
        if start >= n || *klen == 0 {
            break;
        }
        start = (start + *klen).min(n);
        if start < n && observations[start].ch == b'\'' {
            start += 1;
        }
    }
    while start < n && observations[start].ch == b'\'' {
        start += 1;
    }
    if start >= n {
        return Vec::new();
    }

    let t9_map = dict.t9_map();

    // Collect top-beam preferred syllable at `start` to surface it first
    let mut beam_preferred: Vec<String> = Vec::new();
    for path in top_paths.iter().take(4) {
        let mut seg_s = 0usize;
        for b in &path.boundaries {
            while seg_s < n && observations[seg_s].ch == b'\'' {
                seg_s += 1;
            }
            if seg_s == start {
                if let Some(first_syl) = b.syl_display.split('\'').next() {
                    if dict.lookup_syllable_id(first_syl).is_some()
                        && !beam_preferred.iter().any(|x| x == first_syl)
                    {
                        beam_preferred.push(first_syl.to_owned());
                    }
                }
                break;
            }
            seg_s = b.keys;
        }
    }

    let max_len = (n - start).min(MAX_SYL_LEN);
    let mut items: Vec<(String, usize, u32, bool)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for len in (1..=max_len).rev() {
        let end = start + len;
        if observations[start..end]
            .iter()
            .any(|o| !(b'2'..=b'9').contains(&o.ch))
        {
            continue;
        }
        let digits: String = observations[start..end]
            .iter()
            .map(|o| o.ch as char)
            .collect();
        if let Some(sids) = t9_map.get(&digits) {
            for &sid in sids {
                let syl_str = dict.syllable_str(sid);
                if seen.insert(syl_str.to_owned()) {
                    let top_w = dict
                        .lookup_chars(sid, false)
                        .first()
                        .map_or(0, |e| e.weight);
                    let in_beam = beam_preferred.iter().any(|x| x == syl_str);
                    items.push((syl_str.to_owned(), len, top_w, in_beam));
                }
            }
        }
    }

    items.sort_by(|a, b| {
        b.3.cmp(&a.3)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.0.cmp(&b.0))
    });
    items.into_iter().take(12).map(|x| x.0).collect()
}

/// Compute 8-bit `t9ValidNextMask` (bits 0..7 => keys `'2'..='9'`) and 8-key `t9NextKeyPriors`
/// by aggregating valid T9 continuations and `Micro-RWKV-7` 26-letter syllable-start priors.
pub fn t9_next_key_priors(
    raw_code: &str,
    locked_keys: usize,
    dict: &PinyinDictView,
    rwkv_start_priors: Option<&[f32; 26]>,
) -> (u8, [u8; 8]) {
    let mut mask = 0u8;
    let mut scores = [0.0f32; 8];

    let unlocked = raw_code.get(locked_keys.min(raw_code.len())..).unwrap_or("");
    let tail = unlocked.rsplit('\'').next().unwrap_or(unlocked);
    let max_tail = tail.len().min(MAX_SYL_LEN);
    let t9_map = dict.t9_map();

    let mut can_end_syllable = tail.is_empty();
    for l in 1..=max_tail {
        let sub = &tail[tail.len() - l..];
        if !sub.bytes().all(|b| (b'2'..=b'9').contains(&b)) {
            continue;
        }
        if t9_map.contains_key(sub) {
            can_end_syllable = true;
        }
        for digits_key in t9_map.keys() {
            if digits_key.len() > sub.len() && digits_key.starts_with(sub) {
                if let Some(&next_b) = digits_key.as_bytes().get(sub.len()) {
                    if (b'2'..=b'9').contains(&next_b) {
                        let idx = (next_b - b'2') as usize;
                        mask |= 1u8 << idx;
                        scores[idx] += 2.0;
                    }
                }
            }
        }
    }

    if can_end_syllable || mask == 0 {
        let syl_starts: &[u8] = b"abcdefghjklmnopqrstwxyz";
        let mut rwkv_t9 = [0.0f32; 8];
        for &b in syl_starts {
            if let Some(d) = letter_to_t9_digit(b) {
                let idx = (d - b'2') as usize;
                mask |= 1u8 << idx;
                let w = rwkv_start_priors.map_or(0.0, |p| p[(b - b'a') as usize]);
                if w > rwkv_t9[idx] {
                    rwkv_t9[idx] = w;
                }
            }
        }
        for idx in 0..8 {
            if (mask & (1u8 << idx)) != 0 {
                scores[idx] += 1.0 + 4.5 * rwkv_t9[idx];
            }
        }
    }

    let max_s = scores.iter().copied().fold(0.0f32, f32::max);
    let mut priors = [100u8; 8];
    if max_s > 0.0 {
        for i in 0..8 {
            if (mask & (1u8 << i)) != 0 {
                let norm = (scores[i] / max_s).clamp(0.0, 1.0);
                priors[i] = (65.0 + 115.0 * norm).round().clamp(54.0, 180.0) as u8;
            } else {
                priors[i] = 0;
            }
        }
    }
    (if mask == 0 { 0xFF } else { mask }, priors)
}
