use crate::code::{cangjie_radicals_str, expand_wildcards, PackedCode};
use crate::engine::{Candidate, CandidateBadge, JuxuEngine, LmWeights};
use crate::english::{format_english_display, resolve_english_display};
use crate::format::{AuxView, ENTRY_FLAG_SYMBOL};
use crate::typo::{generate_hypotheses_with_options, softmax_confidences, CorrectionKind};
use crate::user_data::UserStore;
use std::collections::HashSet;

impl JuxuEngine {
    pub(crate) fn looks_like_english_word_or_prefix(&self, code: &str) -> bool {
        if code.len() < 2 || !code.bytes().all(|b| b.is_ascii_alphabetic()) {
            return false;
        }
        let lower = code.to_ascii_lowercase();
        if self.user.lookup_english_exact(&lower).is_some() || self.user.has_english_prefix(&lower)
        {
            return true;
        }
        if self.english_dict.lookup_with_morphology(&lower).is_some() {
            return true;
        }
        self.english_dict
            .prefix_info(&lower)
            .is_some_and(|(w, _)| w >= 22_000)
    }

    pub(crate) fn should_extend_fixed_as_english(&self, combined: &str) -> bool {
        if combined.len() < 5
            || combined.len() > 32
            || !combined.bytes().all(|b| b.is_ascii_alphabetic())
        {
            return false;
        }
        let lower = combined.to_ascii_lowercase();
        if self
            .candidates
            .first()
            .is_some_and(|c| !c.text.is_empty() && c.text.bytes().all(|b| b.is_ascii_graphic()))
        {
            return true;
        }
        if self.user.lookup_english_exact(&lower).is_some() || self.user.has_english_prefix(&lower)
        {
            return true;
        }
        if self.english_dict.lookup_with_morphology(&lower).is_some() {
            return true;
        }
        self.english_dict
            .prefix_info(&lower)
            .is_some_and(|(w, _)| w >= 28_000)
    }

    pub(crate) fn should_suppress_fourth_for_english(&self) -> bool {
        if self.raw_code.bytes().any(|b| b.is_ascii_uppercase()) {
            return true;
        }
        let lower = self.raw_code.to_ascii_lowercase();
        let is_user_en = self.user.lookup_english_exact(&lower).is_some()
            || self.user.has_english_prefix(&lower);
        let is_dict_en = self
            .english_dict
            .lookup_exact(&lower)
            .is_some_and(|e| e.weight >= 24_000);
        let is_dict_prefix = self
            .english_dict
            .prefix_info(&lower)
            .is_some_and(|(w, has_longer)| has_longer && w >= 34_000);
        if !is_user_en && !is_dict_en && !is_dict_prefix {
            return false;
        }
        if !self.recent_context.is_empty() {
            if let Some(first) = self.candidates.first() {
                if first.is_exact && !first.text.is_ascii() {
                    if let Some(nv) = self.aux.as_ref().and_then(|a| a.neural_view()) {
                        let lp = nv.score_phrase_fast(&self.neural_tracker.active, &first.text);
                        if lp > -3.2 {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    pub(crate) fn build_english_buffer_candidates(&mut self) {
        self.candidates.clear();
        if self.raw_code.is_empty() {
            return;
        }
        let raw = self.raw_code.clone();
        let lower = raw.to_ascii_lowercase();
        let has_upper = raw.bytes().any(|b| b.is_ascii_uppercase());
        let mut seen = HashSet::<String>::new();

        let user_hit = self.user.lookup_english_exact(&lower);
        let dict_hit = self.english_dict.lookup_with_morphology(&lower);
        let primary = resolve_english_display(
            &raw,
            user_hit.map(|x| x.0),
            dict_hit.as_ref().map(|x| x.0.as_str()),
        );
        if seen.insert(primary.clone()) {
            self.candidates.push(Candidate {
                text: primary,
                matched_code: PackedCode::EMPTY,
                comment: String::new(),
                badge: CandidateBadge::None,
                confidence: 0.95,
                is_exact: true,
            });
        }
        if seen.insert(raw.clone()) {
            self.candidates.push(Candidate {
                text: raw.clone(),
                matched_code: PackedCode::EMPTY,
                comment: String::new(),
                badge: CandidateBadge::None,
                confidence: 0.80,
                is_exact: true,
            });
        }

        for (disp, _) in self.user.lookup_english_prefix(&lower, 3) {
            let text = if has_upper {
                format_english_display(&raw, disp)
            } else {
                disp.to_owned()
            };
            if seen.insert(text.clone()) {
                self.candidates.push(Candidate {
                    text,
                    matched_code: PackedCode::EMPTY,
                    comment: String::new(),
                    badge: CandidateBadge::None,
                    confidence: 0.60,
                    is_exact: true,
                });
            }
        }
        for e in self.english_dict.lookup_prefix(&lower, 5) {
            let text = if e.key == lower && has_upper {
                raw.clone()
            } else {
                e.display.to_owned()
            };
            if seen.insert(text.clone()) {
                self.candidates.push(Candidate {
                    text,
                    matched_code: PackedCode::EMPTY,
                    comment: String::new(),
                    badge: CandidateBadge::None,
                    confidence: 0.40,
                    is_exact: true,
                });
            }
            if self.candidates.len() >= 8 {
                break;
            }
        }
    }

    pub(crate) fn should_auto_commit_fourth(&self) -> bool {
        if self.raw_code.contains('z')
            || self.hold_fourth_for_split
            || self.should_suppress_fourth_for_english()
        {
            return false;
        }
        let exact_cands: Vec<&Candidate> = self
            .candidates
            .iter()
            .filter(|c| c.is_exact && c.badge != CandidateBadge::Emoji)
            .collect();
        if exact_cands.len() == 1 && self.candidates[0].is_exact {
            return true;
        }
        let non_emoji_count = self
            .candidates
            .iter()
            .filter(|c| c.badge != CandidateBadge::Emoji)
            .count();
        if !self.candidates[0].is_exact && !self.config.typo.suppress_auto_commit_on_correction {
            return non_emoji_count == 1 || self.candidates[0].confidence >= 0.94;
        }
        false
    }

    pub(crate) fn build_z_mode_candidates(&mut self, slot: usize) {
        let dict = self.dicts[slot].as_ref().unwrap();
        if self.raw_code == "z" {
            let prompt = if self.config.schema.is_cangjie() {
                "输拼音反查仓颉 · zz特殊符号"
            } else {
                "输拼音反查五笔 · zz特殊符号"
            };
            for (sym, desc) in [
                ("，", prompt),
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
                        let code_desc = if self.config.schema.is_cangjie() {
                            let rad_full = cangjie_radicals_str(&full_s);
                            if py_disp.is_empty() {
                                format!("{} {}", rad_full, full_s)
                            } else {
                                format!("{} {} ({})", rad_full, full_s, py_disp)
                            }
                        } else if !short_s.is_empty() && short_s != full_s {
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

    pub(crate) fn build_wildcard_candidates(&mut self, slot: usize) {
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

    pub(crate) fn build_standard_and_typo_candidates(&mut self, slot: usize) {
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

        struct ExactItem {
            text: String,
            code: PackedCode,
            comment: String,
            badge: CandidateBadge,
            sort_bucket: u8,
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
                1_000_000.0 + (ucnt as f64) * 100.0 + ((ctx + n_fast) as f64) * 500.0
                    - (idx as f64)
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
                        if self.config.show_shortcode_hint
                            && !meta.shortest_code.is_empty()
                            && meta.shortest_code.len() < exact_code.len()
                        {
                            badge = CandidateBadge::Shortcode;
                            comment = format!("⚡{}", meta.shortest_code.as_string());
                        }
                        let schema_max_len = self.config.schema.max_code_len();
                        if self.config.strict_shortcode
                            && exact_code.len() == schema_max_len
                            && !meta.primary_short_code.is_empty()
                            && meta.primary_short_code.len() < schema_max_len
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

            let allow_dyn = if entry.is_single_char() {
                self.config.dynamic_char_freq
            } else {
                self.config.dynamic_word_freq
            };

            let (bucket, ord) = if demoted {
                (2u8, 10_000.0 - rank as f64)
            } else if rank == 0 && !allow_dyn {
                (0u8, 10_000_000.0)
            } else if allow_dyn {
                let dyn_bonus = ((ctx + n_fast * 0.5) as f64) * 1800.0
                    + (self.user.unigram_boost(entry.text) as f64) * 1200.0;
                let freq_score = ((entry.weight as f64) + 50.0).ln() * 900.0;
                let base_rank_score = 900_000.0 + freq_score - (rank as f64) * 1200.0;
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

        // 1c. Prefix completions when `raw_code.len() < schema_max_len`
        let schema_max_len = self.config.schema.max_code_len();
        if exact_code.len() < schema_max_len && exact_items.len() < 15 {
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
            a.sort_bucket.cmp(&b.sort_bucket).then_with(|| {
                b.order_score
                    .partial_cmp(&a.order_score)
                    .unwrap_or(core::cmp::Ordering::Equal)
            })
        });

        // 2. Statistical + Touch-Region + Neural RWKV-7 Typo-Correction Decoder
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

            let spatial_scale = if self.aux.as_ref().is_some_and(AuxView::has_unigrams) {
                lm_w.spatial_scale
            } else {
                2.05
            };

            for h in hyps {
                if h.kind == CorrectionKind::Exact || h.code == exact_code {
                    continue;
                }
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
                        .map(|nv| {
                            lm_w.neural_scale
                                * smooth_neural_score(
                                    nv.score_phrase_fast(&self.neural_tracker.active, utext),
                                )
                        })
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
                for e in dict.lookup_exact(h.code, false).into_iter().take(4) {
                    if !fuzzy_seen.insert(e.text.to_string()) {
                        continue;
                    }
                    if e.weight < 5_000 {
                        let known_modern = self
                            .aux
                            .as_ref()
                            .and_then(|a| e.text.chars().next().and_then(|c| a.unigram_prior(c)))
                            .is_some_and(|u| u >= 3.5);
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

            if let Some(ref nv) = neural_view {
                if !fuzzy_items.is_empty() {
                    let best_exact_fast = exact_items
                        .iter()
                        .filter(|x| x.sort_bucket <= 2)
                        .map(|x| x.posterior_score)
                        .fold(f32::NEG_INFINITY, f32::max);
                    let best_fuzzy_fast = fuzzy_items[0].posterior_score;
                    let needs_exact_unroll =
                        !best_exact_fast.is_finite() || (best_fuzzy_fast >= best_exact_fast - 5.0);
                    if needs_exact_unroll {
                        for item in exact_items
                            .iter_mut()
                            .filter(|x| x.sort_bucket <= 2)
                            .take(2)
                        {
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

            let rescue_empty =
                !has_direct_exact && conf >= (self.config.typo.promote_threshold * 0.78);
            let snap_boundary = f.boundary_slip && conf >= self.config.typo.promote_threshold;

            if promoted_fuzzy.is_empty() && (rescue_empty || snap_boundary) {
                promoted_fuzzy.push(cand);
            } else if conf >= self.config.typo.suggest_threshold
                || (!has_direct_exact && secondary_fuzzy.len() < 2 && conf >= 0.20)
            {
                secondary_fuzzy.push(cand);
            }
        }

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

        // 4. Language-Model Missed-Space Split Decoder
        if self.config.typo.enabled && self.config.typo.missed_space_correction {
            self.evaluate_missed_space_on_rebuild(
                slot,
                exact_code,
                has_direct_exact,
                best_exact_meta.as_ref(),
                top_fuzzy_meta.as_ref(),
            );
        }

        // 5. Append matching English word candidate when a 3..=5 letter code matches an English word
        if self.raw_code.len() >= 3 && self.raw_code.bytes().all(|b| b.is_ascii_alphabetic()) {
            let lower = self.raw_code.to_ascii_lowercase();
            let en_disp = self
                .user
                .lookup_english_exact(&lower)
                .map(|(d, _)| d.to_owned())
                .or_else(|| {
                    self.english_dict
                        .lookup_exact(&lower)
                        .filter(|e| e.weight >= 24_000)
                        .map(|e| e.display.to_owned())
                });
            if let Some(disp) = en_disp {
                if !self.candidates.iter().any(|c| c.text == disp) {
                    let insert_pos = self.candidates.len().min(2);
                    self.candidates.insert(
                        insert_pos,
                        Candidate {
                            text: disp,
                            matched_code: PackedCode::EMPTY,
                            comment: String::new(),
                            badge: CandidateBadge::None,
                            confidence: 0.35,
                            is_exact: true,
                        },
                    );
                }
            }
        }
    }
}

#[inline]
pub(crate) fn smooth_neural_score(raw_lp: f32) -> f32 {
    raw_lp.clamp(-13.5, -0.02) + 11.0
}

#[inline]
pub(crate) fn compute_candidate_prior(
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
pub(crate) fn compute_context_boost(
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
        boost += 1.35 * bg;
        return boost;
    }

    let prev2 = if recent_context.len() >= 2 {
        Some(recent_context[recent_context.len() - 2])
    } else {
        None
    };
    let tg1 = prev2
        .map(|p2| aux_v.trigram_boost(p2, p1, c0))
        .unwrap_or(0.0);
    let tg_cross = cand_chars
        .next()
        .map(|c1| aux_v.trigram_boost(p1, c0, c1))
        .unwrap_or(0.0);

    boost += w.bigram_scale * bg + w.trigram_scale * tg1 + w.cross_trigram_scale * tg_cross;
    boost
}
