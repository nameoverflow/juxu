use crate::code::is_cjk_ideograph;
use crate::engine::JuxuEngine;
use std::collections::{HashMap, HashSet};

impl JuxuEngine {
    /// Lightweight idle next-word / next-char prediction reusing `WBAUX003` char N-grams,
    /// `Micro-RWKV-7` state, active dictionary phrase verification, and personal `UserStore` word bigrams.
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
            let mut first_map = HashMap::<char, (f32, f32, f32)>::with_capacity(128);
            for (c1, bi) in aux.bigrams_after(p1) {
                if !is_cjk_ideograph(c1) {
                    continue;
                }
                let uni = aux.unigram_prior(c1).unwrap_or(0.0);
                first_map.insert(c1, (uni, bi, 0.0));
            }
            if let Some(prev2) = p2 {
                for (c1, tri) in aux.trigrams_after(prev2, p1) {
                    if !is_cjk_ideograph(c1) {
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
                    if is_cjk_ideograph(c1) {
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
                    if uni < 4.0 && tri < 1.0 && !rwkv_lp1.is_some_and(|lp| lp > -4.5) {
                        return None;
                    }
                    if c1 == p1
                        && tri < 1.5
                        && bi < 3.5
                        && !rwkv_lp1.is_some_and(|lp| lp > -1.8)
                    {
                        return None;
                    }
                    let context_discount = if prev2_is_word && tri == 0.0 {
                        if rwkv_lp1.is_some_and(|lp| lp > -4.8) {
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
                    let cond = 1.00 * uni + 0.78 * bi + 1.55 * tri + rwkv_boost - context_discount;
                    Some((c1, cond, uni, bi, tri, rwkv_lp1))
                })
                .collect();
            first_ranked
                .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
            first_ranked.truncate(18);

            let mut strong_phrase_for_c1 = HashMap::<char, f32>::new();

            for &(c1, cond1, _uni1, bi1, tri1, rwkv_lp1) in first_ranked.iter().take(14) {
                let mut is_tri_comp = false;
                let mut is_bi_comp = false;
                let mut comp_bonus = 0.0f32;
                let strong_rwkv_comp = rwkv_lp1.is_some_and(|lp| lp > -1.75);
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

                pool.push(PredCand {
                    text: c1.to_string(),
                    first_ch: c1,
                    score: cond1 + comp_bonus,
                    is_single: true,
                    is_completion: is_comp,
                    is_personal: false,
                });

                if is_tri_comp && (tri1 >= 0.35 || comp_bonus >= 3.0) {
                    continue;
                }

                let mut sec_map = HashMap::<char, (f32, f32)>::with_capacity(48);
                for (c2, bi12) in aux.bigrams_after(c1) {
                    if !is_cjk_ideograph(c2) || bi12 < 1.6 {
                        continue;
                    }
                    sec_map.insert(c2, (bi12, 0.0));
                }
                for (c2, tri12) in aux.trigrams_after(p1, c1) {
                    if !is_cjk_ideograph(c2) {
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
                sec_ranked
                    .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));

                let mut emitted_for_c1 = 0usize;
                for (c2, raw2, _uni2, bi12, tri12) in sec_ranked.into_iter().take(8) {
                    let mut w2 = String::with_capacity(8);
                    w2.push(c1);
                    w2.push(c2);
                    if prev2_str.as_deref() == Some(w2.as_str()) {
                        continue;
                    }
                    if is_numeral_pair(c1, c2) && !is_numeral_char(p1) {
                        continue;
                    }
                    let dict_wt = dict.and_then(|d| d.contains_exact_word(&w2));
                    let Some(wt) = dict_wt else {
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
                    if wt < 50_000 && tri12 < 1.0 {
                        continue;
                    }
                    let rwkv_w2 = nv_opt
                        .as_ref()
                        .map(|nv| nv.score_phrase_fast(&self.neural_tracker.active, &w2));
                    if is_standalone_common_char(c1)
                        && tri12 == 0.0
                        && !rwkv_w2.is_some_and(|lp| lp > -4.2)
                    {
                        continue;
                    }

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
                    } else if rwkv_w2.is_some_and(|lp| lp > -4.8) {
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
                    let sc2 = cond1
                        + raw2
                        + tri_ground_bonus
                        + rwkv_w2_boost
                        + freq_bonus
                        + comp3_bonus
                        - 3.10
                        - split_penalty;
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
            }

            // Penalize bare orphan bound-morpheme chars (e.g. `觉` when `觉得` beat it strongly)
            for item in &mut pool {
                if item.is_single && !item.is_completion && !item.is_personal {
                    if let Some(&best_w2) = strong_phrase_for_c1.get(&item.first_ch) {
                        if best_w2 > item.score + 0.25 && !is_standalone_common_char(item.first_ch)
                        {
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
}

#[inline]
fn is_standalone_common_char(ch: char) -> bool {
    matches!(
        ch,
        '的' | '了'
            | '是'
            | '在'
            | '我'
            | '你'
            | '他'
            | '她'
            | '它'
            | '们'
            | '也'
            | '都'
            | '就'
            | '还'
            | '很'
            | '太'
            | '不'
            | '没'
            | '有'
            | '要'
            | '会'
            | '能'
            | '想'
            | '看'
            | '说'
            | '去'
            | '来'
            | '给'
            | '把'
            | '被'
            | '和'
            | '跟'
            | '与'
            | '吗'
            | '呢'
            | '吧'
            | '啊'
            | '呀'
            | '嘛'
            | '哦'
            | '这'
            | '那'
            | '个'
            | '一'
            | '上'
            | '下'
            | '里'
            | '中'
            | '人'
            | '大'
            | '小'
    )
}

#[inline]
fn is_numeral_char(ch: char) -> bool {
    matches!(
        ch,
        '零' | '一'
            | '二'
            | '三'
            | '四'
            | '五'
            | '六'
            | '七'
            | '八'
            | '九'
            | '十'
            | '百'
            | '千'
            | '万'
            | '两'
    )
}

#[inline]
fn is_numeral_pair(c1: char, c2: char) -> bool {
    is_numeral_char(c1) && is_numeral_char(c2) && !(c1 == '一' && c2 == '个')
}
