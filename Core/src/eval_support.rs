//! Shared deterministic PRNG, QWERTY geometry helpers, romanization map, and evaluation metrics
//! reused across `eval_pinyin`, `eval_missed_space`, `eval_touch_and_bmk`, `eval_typo`,
//! `eval_english_mix`, `eval_from_log`, and `eval_pure_neural`.

use crate::format::DictView;
use crate::pinyin_dict::PinyinDictView;
use crate::typo::{KeyLayout, KeyObs};
use std::collections::HashMap;

/// Canonical 26-key QWERTY centers in key-pitch units (`pitch_x = 1.0`, `pitch_y = 1.0`).
pub const QWERTY_KEY_CENTERS: &[(char, f32, f32)] = &[
    ('q', 0.0, 0.0),
    ('w', 1.0, 0.0),
    ('e', 2.0, 0.0),
    ('r', 3.0, 0.0),
    ('t', 4.0, 0.0),
    ('y', 5.0, 0.0),
    ('u', 6.0, 0.0),
    ('i', 7.0, 0.0),
    ('o', 8.0, 0.0),
    ('p', 9.0, 0.0),
    ('a', 0.5, 1.0),
    ('s', 1.5, 1.0),
    ('d', 2.5, 1.0),
    ('f', 3.5, 1.0),
    ('g', 4.5, 1.0),
    ('h', 5.5, 1.0),
    ('j', 6.5, 1.0),
    ('k', 7.5, 1.0),
    ('l', 8.5, 1.0),
    ('z', 1.5, 2.0),
    ('x', 2.5, 2.0),
    ('c', 3.5, 2.0),
    ('v', 4.5, 2.0),
    ('b', 5.5, 2.0),
    ('n', 6.5, 2.0),
    ('m', 7.5, 2.0),
];

/// Deterministic xorshift64* PRNG with Box-Muller 2D Gaussian sampling (zero external crates).
#[derive(Clone, Debug)]
pub struct XorshiftRng(u64);

impl XorshiftRng {
    #[inline]
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform float in `[0.0, 1.0)`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
    }

    /// Uniform float in `(0.0, 1.0)` avoiding exact `0.0` / `1.0` for Box-Muller log.
    #[inline]
    pub fn next_open_f32(&mut self) -> f32 {
        (((self.next_u64() >> 40) as f32) + 0.5) / ((1u32 << 24) as f32)
    }

    #[inline]
    pub fn range(&mut self, n: usize) -> usize {
        if n <= 1 {
            0
        } else {
            (self.next_u64() as usize) % n
        }
    }

    #[inline]
    pub fn next_gaussian(&mut self) -> (f32, f32) {
        let u1 = self.next_open_f32().clamp(1e-6, 1.0 - 1e-6);
        let u2 = self.next_open_f32();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f32::consts::PI * u2;
        (r * theta.cos(), r * theta.sin())
    }
}

/// Build physical `KeyObs` sequence with key-center coordinates for offline evaluation.
pub fn make_obs(keys: &str, layout: &KeyLayout) -> Vec<KeyObs> {
    keys.bytes()
        .enumerate()
        .map(|(i, b)| {
            let ch = b.to_ascii_lowercase();
            let (cx, cy) = if ch.is_ascii_lowercase() {
                layout.center_of(ch)
            } else {
                (0.0, 0.0)
            };
            KeyObs {
                ch,
                is_upper: b.is_ascii_uppercase(),
                norm_x: cx,
                norm_y: cy,
                has_touch: false,
                timestamp_ms: 1000 + (i as u64) * 65,
            }
        })
        .collect()
}

/// Physical 2D neighbors among Wubi code keys `'a'..='y'`.
pub fn wubi_neighbor_keys(layout: &KeyLayout, ch: u8) -> Vec<u8> {
    let (cx0, cy0) = layout.center_of(ch);
    let mut out = Vec::new();
    for d in 0..25u8 {
        let k = b'a' + d;
        if k == ch {
            continue;
        }
        let (cx, cy) = layout.center_of(k);
        let dx = (cx - cx0).abs();
        let dy = (cy - cy0).abs();
        if (dy < 0.1 && dx <= 1.05) || (dy <= 1.05 && dx <= 0.85) {
            out.push(k);
        }
    }
    out
}

/// Closest same-row horizontal neighbor among `'a'..='z'`.
pub fn horizontal_neighbor(layout: &KeyLayout, ch: u8) -> Option<u8> {
    let (cx0, cy0) = layout.center_of(ch);
    let mut best: Option<(f32, u8)> = None;
    for d in 0..26u8 {
        let k = b'a' + d;
        if k == ch {
            continue;
        }
        let (cx, cy) = layout.center_of(k);
        if (cy - cy0).abs() < 0.1 {
            let dx = (cx - cx0).abs();
            if dx <= 1.05 && best.is_none_or(|b| dx < b.0) {
                best = Some((dx, k));
            }
        }
    }
    best.map(|b| b.1)
}

/// Positional character accuracy `(matched_chars, expected_len)`.
pub fn char_accuracy(expected: &str, predicted: &str) -> (usize, usize) {
    let e: Vec<char> = expected.chars().collect();
    let p: Vec<char> = predicted.chars().collect();
    let hits = e
        .iter()
        .enumerate()
        .filter(|&(i, &ec)| p.get(i).copied() == Some(ec))
        .count();
    (hits, e.len())
}

#[inline]
pub fn pct(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        100.0 * (num as f64) / (den as f64)
    }
}

#[inline]
pub fn percentile_ms(sorted_us: &[u128], p: f64) -> f64 {
    if sorted_us.is_empty() {
        return 0.0;
    }
    let idx = (((sorted_us.len() - 1) as f64) * p).round() as usize;
    sorted_us[idx.min(sorted_us.len() - 1)] as f64 / 1000.0
}

/// Build reverse `char -> highest-weight syllable` map from `PinyinDictView`.
pub fn build_char_to_pinyin(dict: &PinyinDictView) -> HashMap<char, String> {
    let mut best: HashMap<char, (u32, String)> = HashMap::new();
    for sid in 0..dict.syllable_count() {
        let syl = dict.syllable_str(sid as u16);
        for e in dict.lookup_chars(sid as u16, false) {
            let mut cs = e.text.chars();
            if let (Some(ch), None) = (cs.next(), cs.next()) {
                let entry = best.entry(ch).or_insert_with(|| (0, String::new()));
                if e.weight > entry.0 {
                    *entry = (e.weight, syl.to_owned());
                }
            }
        }
    }
    best.into_iter().map(|(c, (_, s))| (c, s)).collect()
}

/// Segment a Chinese sentence into realistic Wubi `(text, code)` tokens.
pub fn segment_wubi_sentence(
    dict: &DictView,
    sent: &str,
    single_char_mode: bool,
) -> Vec<(String, String)> {
    let chars: Vec<char> = sent.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if !single_char_mode {
            let mut matched: Option<(usize, String, String)> = None;
            for len in (2..=4).rev() {
                if i + len > chars.len() {
                    continue;
                }
                let sub: String = chars[i..i + len].iter().collect();
                if let Some(code) = dict.encode_phrase(&sub) {
                    let hits = dict.lookup_exact(code, false);
                    if hits.first().is_some_and(|e| e.text == sub) {
                        matched = Some((len, sub, code.as_string()));
                        break;
                    }
                }
            }
            if let Some((len, text, code)) = matched {
                tokens.push((text, code));
                i += len;
                continue;
            }
        }
        let ch = chars[i];
        let mut pushed = false;
        if let Some(meta) = dict.char_meta(ch) {
            let candidate_codes = if single_char_mode && meta.full_code.len() == 3 {
                [meta.full_code, meta.primary_short_code, meta.shortest_code]
            } else {
                [meta.primary_short_code, meta.shortest_code, meta.full_code]
            };
            for code in candidate_codes {
                if code.is_empty() {
                    continue;
                }
                let cs = code.as_string();
                if !cs.is_empty() && cs.bytes().all(|b| (b'a'..=b'y').contains(&b)) {
                    let hits = dict.lookup_exact(code, false);
                    if hits.first().is_some_and(|e| e.text == ch.to_string()) {
                        tokens.push((ch.to_string(), cs));
                        pushed = true;
                        break;
                    }
                }
            }
        }
        if !pushed {
            tokens.push((ch.to_string(), String::new()));
        }
        i += 1;
    }
    tokens
}

/// Extract a flat string value `"key":"..."` from a single-line JSON log record.
pub fn extract_json_str(line: &str, key: &str) -> String {
    let pat = format!("\"{}\":\"", key);
    let Some(start) = line.find(&pat) else {
        return String::new();
    };
    let rest = &line[start + pat.len()..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => break,
            },
            _ => out.push(c),
        }
    }
    out
}

/// Extract a flat numeric value `"key":123.4` from a single-line JSON log record.
pub fn extract_json_num(line: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{}\":", key);
    let start = line.find(&pat)? + pat.len();
    let rest = &line[start..];
    let end = rest
        .find(|c: char| {
            !c.is_ascii_digit() && c != '.' && c != '-' && c != 'e' && c != 'E' && c != '+'
        })
        .unwrap_or(rest.len());
    rest[..end].parse::<f64>().ok()
}
