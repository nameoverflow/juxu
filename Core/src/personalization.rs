use crate::neural::{NeuralView, RwkvState};
use std::collections::HashMap;
use std::fmt::Write;

const CONTEXT_MASK: u64 = 0xFFFF_FFFF_FFFF;
const MAX_CONTEXTS: usize = 4_096;
const CONTEXTS_AFTER_PRUNE: usize = 3_072;
const HALF_LIFE: f32 = 2_048.0;
const OPTIONS_PER_CONTEXT: usize = 4;
const STATE_SAMPLES_PER_CONTEXT: usize = 4;
const STATE_NEIGHBORHOOD: u32 = 5;
const MAX_STATE_DISTANCE: u32 = 22;

fn han(c: char) -> bool {
    ('\u{3400}'..='\u{9fff}').contains(&c)
}

/// Up to three preceding Han characters, independent of the decoder's word boundaries.
#[derive(Clone, Copy, Default)]
pub(crate) struct Context(u64);

impl Context {
    pub fn with_chars(chars: impl IntoIterator<Item = char>) -> Self {
        chars.into_iter().fold(Self::default(), Self::push)
    }

    pub fn push(self, c: char) -> Self {
        if han(c) {
            Self(((self.0 << 16) | c as u64) & CONTEXT_MASK)
        } else {
            Self::default()
        }
    }

    fn suffixes(self) -> impl Iterator<Item = (u64, f32)> {
        [0.50, 1.10, 1.65]
            .into_iter()
            .enumerate()
            .zip([0xFFFF, 0xFFFF_FFFF, CONTEXT_MASK])
            .take_while(move |&((i, _), _)| self.0 >> (16 * i) != 0)
            .map(move |((_, limit), mask)| (self.0 & mask, limit))
    }

    fn three_chars(self) -> Option<u64> {
        (self.0 > u32::MAX as u64).then_some(self.0)
    }
}

#[derive(Clone, Copy, Default)]
struct Continuation {
    ch: u16,
    weight: f32,
}

#[derive(Clone, Copy, Default)]
struct StateSample {
    signature: u128,
    continuation: Continuation,
}

fn state_signature(state: &RwkvState) -> u128 {
    // Signs of the next-token projection retain similarities between RWKV states.
    // The projection has already seen the whole available prefix.
    state
        .p1_q
        .iter()
        .take(128)
        .enumerate()
        .fold(0, |bits, (i, &q)| bits | (u128::from(q > 0) << i))
}

#[derive(Default)]
struct Counts {
    tick: u64,
    total: f32,
    options: [Continuation; OPTIONS_PER_CONTEXT],
    states: [StateSample; STATE_SAMPLES_PER_CONTEXT],
}

impl Counts {
    fn decay(&self, now: u64) -> f32 {
        (-(now.saturating_sub(self.tick) as f32) / HALF_LIFE).exp2()
    }

    fn observe(&mut self, ch: u16, weight: f32, now: u64) {
        let decay = self.decay(now);
        self.tick = now;
        let scale = if self.total * decay > 64.0 {
            32.0 / self.total
        } else {
            decay
        };
        self.total *= scale;
        for option in self
            .options
            .iter_mut()
            .chain(self.states.iter_mut().map(|s| &mut s.continuation))
        {
            option.weight *= scale;
        }
        self.total += weight;
        let index = self
            .options
            .iter()
            .position(|option| option.ch == ch)
            .unwrap_or_else(|| {
                self.options
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.weight.total_cmp(&b.weight))
                    .map_or(0, |(index, _)| index)
            });
        let option = &mut self.options[index];
        if option.ch != ch {
            *option = Continuation { ch, weight: 0.0 };
        }
        option.weight += weight;
    }

    fn observe_state(&mut self, ch: u16, signature: u128, weight: f32) {
        if let Some(sample) = self.states.iter_mut().find(|sample| {
            sample.continuation.ch == ch
                && (sample.signature ^ signature).count_ones() <= STATE_NEIGHBORHOOD
        }) {
            sample.continuation.weight += weight;
            return;
        }
        if let Some(slot) = self
            .states
            .iter_mut()
            .min_by(|a, b| a.continuation.weight.total_cmp(&b.continuation.weight))
        {
            *slot = StateSample {
                signature,
                continuation: Continuation { ch, weight },
            };
        }
    }

    fn score(&self, ch: u16, limit: f32, now: u64) -> f32 {
        let Some(option) = self.options.iter().find(|option| option.ch == ch) else {
            return 0.0;
        };
        let decay = self.decay(now);
        let weight = option.weight * decay;
        let support = weight / (weight + 3.0);
        let preference = weight / (self.total * decay + 1.0);
        limit * support * preference
    }

    fn state_bonus(&self, ch: u16, state: &RwkvState, now: u64) -> f32 {
        if !self
            .states
            .iter()
            .any(|sample| sample.continuation.ch == ch)
        {
            return 0.0;
        }
        let decay = self.decay(now);
        let signature = state_signature(state);
        let samples = self.states.iter().filter_map(|sample| {
            let distance = (sample.signature ^ signature).count_ones();
            let weight = sample.continuation.weight * decay;
            (distance <= MAX_STATE_DISTANCE && weight >= 0.1).then_some((
                distance,
                sample.continuation.ch,
                weight,
            ))
        });
        let Some(nearest) = samples.clone().map(|(distance, _, _)| distance).min() else {
            return 0.0;
        };
        let mut support = 0.0;
        let mut matches = 0.0;
        for (_, candidate, weight) in
            samples.filter(|(distance, _, _)| *distance <= nearest + STATE_NEIGHBORHOOD)
        {
            support += weight;
            if candidate == ch {
                matches += weight;
            }
        }
        let similarity = 1.0 - nearest as f32 / 32.0;
        2.25 * similarity * (support / (support + 3.0)) * (matches / (support + 1.0))
    }

    /// Estimate `(p_cache, lambda)` from suffix n-gram counts at this context row.
    fn cache_suffix_prob(&self, ch: u16, max_lambda: f32, now: u64) -> (f32, f32) {
        let decay = self.decay(now);
        let total = self.total * decay;
        if total < 0.25 {
            return (0.0, 0.0);
        }
        let support = total / (total + 3.0);
        let lambda = max_lambda * support;
        let w = self
            .options
            .iter()
            .find(|o| o.ch == ch)
            .map_or(0.0, |o| o.weight * decay);
        let p_cache = (w / (total + 1.0)).clamp(0.0, 0.99);
        (p_cache, lambda)
    }

    /// Estimate `(p_cache, lambda)` from RWKV `p1_q` 128-bit state-signature kNN samples.
    fn cache_state_prob(&self, ch: u16, state: &RwkvState, now: u64) -> (f32, f32) {
        let decay = self.decay(now);
        let signature = state_signature(state);
        let mut nearest = u32::MAX;
        for sample in &self.states {
            if sample.continuation.ch == 0 {
                continue;
            }
            let w = sample.continuation.weight * decay;
            if w < 0.1 {
                continue;
            }
            let dist = (sample.signature ^ signature).count_ones();
            if dist <= MAX_STATE_DISTANCE && dist < nearest {
                nearest = dist;
            }
        }
        if nearest == u32::MAX {
            return (0.0, 0.0);
        }
        let mut support = 0.0f32;
        let mut matches = 0.0f32;
        for sample in &self.states {
            if sample.continuation.ch == 0 {
                continue;
            }
            let w = sample.continuation.weight * decay;
            if w < 0.1 {
                continue;
            }
            let dist = (sample.signature ^ signature).count_ones();
            if dist <= nearest + STATE_NEIGHBORHOOD {
                support += w;
                if sample.continuation.ch == ch {
                    matches += w;
                }
            }
        }
        if support <= 0.0 {
            return (0.0, 0.0);
        }
        let similarity = (1.0 - nearest as f32 / 32.0).clamp(0.0, 1.0);
        let lambda = 0.55 * similarity * (support / (support + 3.0));
        let p_cache = (matches / (support + 1.0)).clamp(0.0, 0.99);
        (p_cache, lambda)
    }
}

#[derive(Default)]
pub(crate) struct PersonalContinuations {
    tick: u64,
    rows: HashMap<u64, Counts>,
    state_model: Option<u64>,
}

impl PersonalContinuations {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn matches_model(&self, view: &NeuralView<'_>) -> bool {
        self.state_model == Some(view.offs.profile_id)
    }

    /// A rejected first choice increases evidence at the first differing character.
    pub fn observe(
        &mut self,
        prefix: &[char],
        text: &str,
        confirmed: bool,
        first_choice: Option<&str>,
        mut neural: Option<(NeuralView<'_>, RwkvState)>,
    ) {
        if let Some((view, _)) = &neural {
            if !self.matches_model(view) {
                for row in self.rows.values_mut() {
                    row.states = [StateSample::default(); STATE_SAMPLES_PER_CONTEXT];
                }
                self.state_model = Some(view.offs.profile_id);
            }
        }
        let weight = if confirmed { 1.0 } else { 0.25 };
        let mut context = Context::with_chars(prefix.iter().copied());
        let mut other = first_choice.map(str::chars);
        for ch in text.chars() {
            let different = other
                .as_mut()
                .and_then(Iterator::next)
                .is_some_and(|a| a != ch);
            if different {
                other = None;
            }
            if han(ch) {
                self.tick = self.tick.saturating_add(1);
                let weight = if different { weight + 2.0 } else { weight };
                for (key, _) in context.suffixes() {
                    self.rows
                        .entry(key)
                        .or_default()
                        .observe(ch as u16, weight, self.tick);
                }
                if let Some((model, state)) = neural.as_mut() {
                    if let Some(key) = context.three_chars() {
                        if let Some(row) = self.rows.get_mut(&key) {
                            row.observe_state(ch as u16, state_signature(state), weight);
                        }
                    }
                    model.step_token(model.char_to_token(ch), state);
                }
            } else {
                neural = None;
            }
            context = context.push(ch);
        }
        if self.rows.len() > MAX_CONTEXTS {
            let mut by_weight: Vec<_> = self
                .rows
                .iter()
                .map(|(&key, row)| (key, row.total * row.decay(self.tick)))
                .collect();
            let remove = by_weight.len() - CONTEXTS_AFTER_PRUNE;
            by_weight.select_nth_unstable_by(remove, |a, b| {
                a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0))
            });
            for (key, _) in by_weight.into_iter().take(remove) {
                self.rows.remove(&key);
            }
        }
    }

    pub fn score(&self, mut context: Context, text: &str) -> f32 {
        let mut score = 0.0;
        for ch in text.chars() {
            if han(ch) {
                let mut best = 0.0f32;
                for (key, limit) in context.suffixes() {
                    if let Some(row) = self.rows.get(&key) {
                        best = best.max(row.score(ch as u16, limit, self.tick));
                    }
                }
                score += best;
            }
            context = context.push(ch);
        }
        score
    }

    pub fn state_bonus(&self, context: Context, ch: char, state: &RwkvState) -> f32 {
        let Some(row) = context.three_chars().and_then(|key| self.rows.get(&key)) else {
            return 0.0;
        };
        if !han(ch) {
            return 0.0;
        }
        row.state_bonus(ch as u16, state, self.tick)
    }

    /// Unified Neural Cache LM probability-space interpolation:
    /// `P_final(ch | h) = (1 - lambda) * exp(rwkv_logp) + lambda * P_cache(ch | h_sig, suffix)`
    /// Returns the calibrated log-probability in nats.
    pub fn interpolate_logp(
        &self,
        rwkv_logp: f32,
        context: Context,
        ch: char,
        state: Option<&RwkvState>,
    ) -> f32 {
        if self.rows.is_empty() || !han(ch) {
            return rwkv_logp;
        }
        let ch_u16 = ch as u16;
        let mut best_p = 0.0f32;
        let mut best_lambda = 0.0f32;

        // 1. Prefer RWKV state-signature kNN distribution at 3-char context when available
        if let Some(st) = state {
            if let Some(row) = context.three_chars().and_then(|key| self.rows.get(&key)) {
                let (p_st, l_st) = row.cache_state_prob(ch_u16, st, self.tick);
                if l_st > 0.04 {
                    best_p = p_st;
                    best_lambda = l_st;
                }
            }
        }

        // 2. Fall back to or blend with longest matching character suffix (1..=3 chars)
        if best_lambda < 0.04 {
            let max_lambdas = [0.18f32, 0.30, 0.42];
            for (idx, (key, _)) in context.suffixes().enumerate() {
                if let Some(row) = self.rows.get(&key) {
                    let (p_suf, l_suf) =
                        row.cache_suffix_prob(ch_u16, max_lambdas[idx.min(2)], self.tick);
                    if l_suf > best_lambda || (l_suf > 0.04 && p_suf > best_p) {
                        best_p = p_suf;
                        best_lambda = l_suf;
                    }
                }
            }
        }

        if best_lambda <= 1e-4 {
            return rwkv_logp;
        }
        let p_rwkv = rwkv_logp.clamp(-13.5, -0.001).exp();
        let p_final = ((1.0 - best_lambda) * p_rwkv + best_lambda * best_p).clamp(1e-6, 0.999);
        p_final.ln()
    }

    pub fn write_to(&self, out: &mut String) {
        if self.rows.is_empty() {
            return;
        }
        let _ = writeln!(out, "CONT_SEQ\t{}", self.tick);
        if let Some(model) = self.state_model {
            let _ = writeln!(out, "CONT_MODEL\t{model:x}");
        }
        for (&key, row) in &self.rows {
            let _ = write!(out, "CONT\t{key:x}\t{}\t{:.4}", row.tick, row.total);
            for option in row.options.iter().filter(|option| option.ch != 0) {
                let _ = write!(out, "\t{:x}:{:.4}", option.ch, option.weight);
            }
            for sample in row
                .states
                .iter()
                .filter(|sample| sample.continuation.ch != 0)
            {
                let _ = write!(
                    out,
                    "\t{:x}:{:x}:{:.4}",
                    sample.continuation.ch, sample.signature, sample.continuation.weight
                );
            }
            out.push('\n');
        }
    }

    pub fn load_clock(&mut self, value: &str) {
        if let Ok(tick) = value.parse::<u64>() {
            self.tick = self.tick.max(tick);
        }
    }

    pub fn load_model(&mut self, value: &str) {
        self.state_model = u64::from_str_radix(value, 16).ok();
    }

    pub fn load_row(&mut self, fields: &[&str]) {
        if self.rows.len() >= MAX_CONTEXTS {
            return;
        }
        let [key, tick, total, options @ ..] = fields else {
            return;
        };
        let (Ok(key), Ok(tick), Ok(total)) = (
            u64::from_str_radix(key, 16),
            tick.parse::<u64>(),
            total.parse::<f32>(),
        ) else {
            return;
        };
        if key == 0 || key > CONTEXT_MASK || !total.is_finite() || total <= 0.0 {
            return;
        }
        let mut row = Counts {
            tick,
            total,
            ..Counts::default()
        };
        let mut text_slots = row.options.iter_mut();
        let mut state_slots = row.states.iter_mut();
        for field in options {
            let Some((ch, value)) = field.split_once(':') else {
                return;
            };
            let (signature, weight) = value
                .split_once(':')
                .map_or((None, value), |(signature, weight)| {
                    (Some(signature), weight)
                });
            let (Ok(ch), Ok(weight)) = (u16::from_str_radix(ch, 16), weight.parse::<f32>()) else {
                return;
            };
            if !char::from_u32(ch as u32).is_some_and(han)
                || !weight.is_finite()
                || weight < 0.0
                || weight > total + 0.0001
            {
                return;
            }
            let continuation = Continuation { ch, weight };
            if let Some(signature) = signature {
                let (Some(slot), Ok(signature)) =
                    (state_slots.next(), u128::from_str_radix(signature, 16))
                else {
                    return;
                };
                *slot = StateSample {
                    signature,
                    continuation,
                };
            } else {
                let Some(slot) = text_slots.next() else {
                    return;
                };
                *slot = continuation;
            }
        }
        self.tick = self.tick.max(tick);
        self.rows.insert(key, row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_favors_reselection_and_decays() {
        let prefix = ['审', '核'];
        let context = Context::with_chars(prefix);
        let mut confirmed = PersonalContinuations::default();
        confirmed.observe(&prefix, "安全材料", true, None, None);
        let mut chosen = PersonalContinuations::default();
        chosen.observe(&prefix, "安全材料", true, Some("案例材料"), None);
        let mut automatic = PersonalContinuations::default();
        automatic.observe(&prefix, "安全材料", false, None, None);

        let score = chosen.score(context, "安全资料");
        assert!(score > confirmed.score(context, "安全资料"));
        assert!(confirmed.score(context, "安全资料") > automatic.score(context, "安全资料"));
        let split_context = "安全".chars().fold(context, Context::push);
        let split_score = chosen.score(context, "安全") + chosen.score(split_context, "资料");
        assert!((score - split_score).abs() < 1e-6);
        assert_eq!(chosen.score(context, "案例资料"), 0.0);

        chosen.tick += 8 * HALF_LIFE as u64;
        assert!(chosen.score(context, "安全资料") < score * 0.01);
    }
}
