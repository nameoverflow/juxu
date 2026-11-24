use crate::code::{is_cjk_ideograph, PackedCode};
use crate::engine::{Candidate, CandidateBadge, JuxuEngine};
use crate::fixed_decode::compute_context_boost;
use crate::format::AuxView;
use crate::neural::{NeuralView, RwkvState};
use crate::personalization::Context as PersonalContext;
use crate::typo::{softmax_confidences, KeyObs};
use std::collections::HashSet;

impl JuxuEngine {
    /// Return the #1 primary candidate `(text, weight, is_single_char)` that pressing Space
    /// would commit for a short code `code` (`len = 1..=3`).
    pub(crate) fn primary_exact_for_code(
        &self,
        slot: usize,
        code: PackedCode,
    ) -> Option<(String, u32, bool)> {
        let dict = self.dicts[slot].as_ref()?;
        let dict_first = dict.lookup_exact(code, false).into_iter().next();
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
    pub(crate) fn neural_cache_phrase_logp(
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lm_word_logp(
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
            if let Some(c1) = text.chars().next() {
                if text.chars().nth(1).is_some() {
                    let lp1 =
                        self.neural_cache_phrase_logp(nv, st, ctx_chars, &c1.to_string(), false);
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
            let has_uni = self.aux.as_ref().is_some_and(AuxView::has_unigrams);
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
                let wt_score =
                    -6.2 + 0.45 * (((dict_weight as f32) + 50.0).ln() - 12.0).clamp(-5.0, 6.0);
                wt_score + 0.50 * ctx_ng.min(8.0)
            }
        }
    }

    /// Log-probability (in nats) that the next word after `(state, ctx_chars, last_comm)` matches
    /// prefix `right_bytes` (`len = 1..=4`).
    pub(crate) fn lm_prefix_logp(
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
            if !key_byte.is_ascii_lowercase() {
                return None;
            }
            let c2_lp = exact_c2.as_ref().map_or(-13.5, |x| x.1);
            let pref_lp = if let (Some(nv), Some(st)) = (nv, state) {
                let mut acc = f32::NEG_INFINITY;
                for (ch, lp) in nv.top_next_chars(st, 32) {
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
                acc.max(c2_lp).clamp(-4.2, -0.05)
            } else {
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

        for (utext, _, _) in self.user.lookup_exact(right_code).into_iter().take(2) {
            if seen.insert(utext.to_string()) {
                let lp = self.lm_word_logp(
                    nv,
                    state,
                    ctx_chars,
                    last_comm,
                    utext,
                    50_000_000,
                    rlen == 4,
                );
                lse = logsumexp_pair(lse, lp);
            }
        }
        if rlen < 4 {
            for (_, utext, _) in self.user.lookup_prefix(right_code, 3) {
                if seen.insert(utext.to_string()) {
                    let lp = self.lm_word_logp(
                        nv,
                        state,
                        ctx_chars,
                        last_comm,
                        utext,
                        50_000_000,
                        false,
                    );
                    lse = logsumexp_pair(lse, lp);
                }
            }
        }

        for e in dict.lookup_exact(right_code, false).into_iter().take(4) {
            if seen.insert(e.text.to_string()) {
                let lp =
                    self.lm_word_logp(nv, state, ctx_chars, last_comm, e.text, e.weight, rlen == 4);
                lse = logsumexp_pair(lse, lp);
            }
        }

        if rlen < 4 {
            for e in dict.top_prefix_entries(right_code, 14, false) {
                if seen.insert(e.text.to_string()) {
                    let lp =
                        self.lm_word_logp(nv, state, ctx_chars, last_comm, e.text, e.weight, false);
                    lse = logsumexp_pair(lse, lp);
                }
            }
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

    pub(crate) fn evaluate_missed_space_on_rebuild(
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

        // Case 1: Dead 3-letter code
        if len == 3 {
            if has_direct_exact {
                return;
            }
            let Some(dict) = self.dicts[slot].as_ref() else {
                return;
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
                let lp2 =
                    self.lm_word_logp(nv.as_ref(), st1.as_ref(), &ctx1, &c1, &c2, wt2, true);
                let pair_nats = lp1 + lp2 - 1.8;
                if best_pair.as_ref().is_none_or(|x| pair_nats > x.2) {
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
                        softmax_confidences(&[pair_nats, typo_nats])[0]
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
            if sc >= FAST_PATH_EXACT_NATS {
                return;
            }
            sc
        } else {
            f32::NEG_INFINITY
        };

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
                if best_pair.as_ref().is_none_or(|x| pair_nats > x.2) {
                    best_pair = Some((cut, comb, pair_nats));
                }
            }
            if best_split.as_ref().is_none_or(|x| split_nats > x.split_nats) {
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

            let strong_boundary_typo =
                top_fuzzy_meta.is_some_and(|(_, _, _, _, sp, is_b)| *is_b && *sp >= -1.25);
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
            let has_promoted_typo = self.candidates.first().is_some_and(|c| !c.is_exact);
            if has_promoted_typo {
                return;
            }
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
    pub(crate) fn try_resolve_fifth_key_split(
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

        let exact_cand = self.candidates.iter().find(|c| c.is_exact).cloned();
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

        if !self.hold_fourth_for_split && exact_lp >= -4.8 {
            return false;
        }

        let mut k5 = [0u8; 5];
        k5[..4].copy_from_slice(self.raw_code.as_bytes());
        k5[4] = obs5.ch;

        let mut score_4 = if let Some(ref ec) = exact_cand {
            let st_x = nv
                .as_ref()
                .map(|v| step_rwkv_text(v, &self.neural_tracker.active, &ec.text));
            let ctx_x = ctx_push_text(&self.recent_context, &ec.text);
            let pref_e = self
                .lm_prefix_logp(
                    slot,
                    nv.as_ref(),
                    st_x.as_ref(),
                    &ctx_x,
                    &ec.text,
                    &k5[4..5],
                )
                .map_or(-4.0, |x| x.0);
            exact_lp + pref_e
        } else {
            f32::NEG_INFINITY
        };

        let mut pair4_better_than_exact = false;
        if let Some(first) = self.candidates.first() {
            if !first.is_exact {
                let first_lp = if first.matched_code.is_empty() {
                    if let (Some(nv_ref), Some(st0)) = (nv.as_ref(), active_st) {
                        let mut tmp = (*st0).clone();
                        let mut sum_lp = 0.0f32;
                        for ch in first.text.chars() {
                            if is_cjk_ideograph(ch) {
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
                    .lm_prefix_logp(
                        slot,
                        nv.as_ref(),
                        st_f.as_ref(),
                        &ctx_f,
                        &first.text,
                        &k5[4..5],
                    )
                    .map_or(-4.0, |x| x.0);
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
            if best_split5.as_ref().is_none_or(|x| sc > x.2) {
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

        if !pair4_better_than_exact {
            if let Some(ec) = exact_cand {
                if self.candidates.first().is_some_and(|c| !c.is_exact) {
                    if let Some(idx) = self
                        .candidates
                        .iter()
                        .position(|c| c.is_exact && c.text == ec.text)
                    {
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
        if is_cjk_ideograph(ch) {
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
        if is_cjk_ideograph(ch) {
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
