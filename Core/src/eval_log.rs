use crate::engine::{Candidate, CandidateBadge};
use crate::typo::{CorrectionKind, KeyObs};
use std::collections::VecDeque;

const MAX_QUEUED_EVENTS: usize = 256;
const MAX_LOG_CANDIDATES: usize = 5;

#[derive(Clone, Debug)]
pub struct EvalCandSnapshot {
    pub text: String,
    pub badge: u8,
    pub is_exact: bool,
    pub confidence: f32,
}

impl EvalCandSnapshot {
    pub fn from_candidate(c: &Candidate) -> Self {
        Self {
            text: c.text.clone(),
            badge: c.badge as u8,
            is_exact: c.is_exact,
            confidence: c.confidence,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EvalEvent {
    pub ts_ms: u64,
    pub schema: &'static str,
    /// `"commit"` | `"partial_commit"` | `"backspace_after_commit"` | `"abandon"`
    pub event: &'static str,
    pub ctx: String,
    pub keys: String,
    /// Normalized 2D touch coordinates `[[x, y], ...]` when entered via touchscreen.
    pub touches: Vec<[f32; 2]>,
    pub rank: usize,
    pub explicit: bool,
    pub chosen: String,
    pub top1: String,
    pub typo: &'static str,
    /// RWKV-7 mean surprisal on `chosen` (lower = higher model probability).
    pub lm_chosen: Option<f32>,
    /// RWKV-7 mean surprisal on `top1` (when `rank > 0` or `chosen != top1`).
    pub lm_top1: Option<f32>,
    /// True when `rank > 0` and RWKV-7 already gave `chosen` meaningfully better score than `top1`.
    pub lm_overruled: bool,
    pub cands: Vec<EvalCandSnapshot>,
}

#[derive(Clone, Debug)]
pub struct LastCommitSnapshot {
    pub ts_ms: u64,
    pub schema: &'static str,
    pub ctx: String,
    pub keys: String,
    pub chosen: String,
    pub top1: String,
    pub rank: usize,
    pub typo: &'static str,
}

#[derive(Default)]
pub struct EvalLogger {
    queue: VecDeque<String>,
    pub last_commit: Option<LastCommitSnapshot>,
}

impl EvalLogger {
    pub fn push(&mut self, ev: &EvalEvent) {
        if self.queue.len() >= MAX_QUEUED_EVENTS {
            self.queue.pop_front();
        }
        self.queue.push_back(ev.to_json_line());
    }

    pub fn drain_jsonl(&mut self) -> String {
        if self.queue.is_empty() {
            return String::new();
        }
        let mut out = String::with_capacity(self.queue.len() * 192);
        let mut first = true;
        while let Some(line) = self.queue.pop_front() {
            if !first {
                out.push('\n');
            }
            first = false;
            out.push_str(&line);
        }
        out
    }

    pub fn pending_len(&self) -> usize {
        self.queue.len()
    }
}

pub fn is_sensitive_numeric_text(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }
    let mut has_digit = false;
    let mut only_digit_or_sep = true;
    let mut consec_digits = 0usize;
    for ch in trimmed.chars() {
        if ch.is_ascii_digit() || ('０'..='９').contains(&ch) {
            has_digit = true;
            consec_digits += 1;
            if consec_digits >= 4 {
                // Skip 4+ consecutive digits (OTP codes, PINs, phone/card fragments)
                return true;
            }
        } else {
            consec_digits = 0;
            if !matches!(ch, '-' | ' ' | '.' | ',' | ':' | '/' | '+' | '(' | ')') {
                only_digit_or_sep = false;
            }
        }
    }
    has_digit && only_digit_or_sep
}

pub fn scrub_numeric_context(ctx: &str) -> String {
    let mut out = String::with_capacity(ctx.len());
    let mut digits = String::new();
    for ch in ctx.chars() {
        if ch.is_ascii_digit() || ('０'..='９').contains(&ch) {
            digits.push(ch);
        } else {
            if digits.len() >= 4 {
                out.push_str("[NUM]");
            } else {
                out.push_str(&digits);
            }
            digits.clear();
            out.push(ch);
        }
    }
    if digits.len() >= 4 {
        out.push_str("[NUM]");
    } else {
        out.push_str(&digits);
    }
    out
}

pub fn touches_from_obs(obs: &[KeyObs]) -> Vec<[f32; 2]> {
    if !obs.iter().any(|o| o.has_touch) {
        return Vec::new();
    }
    obs.iter()
        .map(|o| {
            if o.has_touch {
                [o.norm_x, o.norm_y]
            } else {
                [0.0, 0.0]
            }
        })
        .collect()
}

pub fn top_cand_snapshots(cands: &[Candidate]) -> Vec<EvalCandSnapshot> {
    cands
        .iter()
        .take(MAX_LOG_CANDIDATES)
        .map(EvalCandSnapshot::from_candidate)
        .collect()
}

pub fn correction_kind_tag(kind: CorrectionKind) -> &'static str {
    match kind {
        CorrectionKind::Exact => "none",
        CorrectionKind::TouchBoundary => "touch",
        CorrectionKind::NeighborKey => "neighbor",
        CorrectionKind::Transposition => "transpose",
    }
}

pub fn candidate_typo_tag(cand: &Candidate) -> &'static str {
    if cand.badge == CandidateBadge::TypoCorrected || !cand.is_exact {
        "typo_cand"
    } else {
        "none"
    }
}

fn json_escape(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl EvalEvent {
    pub fn to_json_line(&self) -> String {
        let mut j = String::with_capacity(256);
        j.push_str("{\"ts\":");
        j.push_str(&self.ts_ms.to_string());
        j.push_str(",\"schema\":");
        json_escape(self.schema, &mut j);
        j.push_str(",\"event\":");
        json_escape(self.event, &mut j);
        j.push_str(",\"ctx\":");
        json_escape(&self.ctx, &mut j);
        j.push_str(",\"keys\":");
        json_escape(&self.keys, &mut j);
        if !self.touches.is_empty() {
            j.push_str(",\"touches\":[");
            for (i, pt) in self.touches.iter().enumerate() {
                if i > 0 {
                    j.push(',');
                }
                j.push_str(&format!("[{:.3},{:.3}]", pt[0], pt[1]));
            }
            j.push(']');
        }
        j.push_str(&format!(
            ",\"rank\":{},\"explicit\":{},\"chosen\":",
            self.rank, self.explicit
        ));
        json_escape(&self.chosen, &mut j);
        j.push_str(",\"top1\":");
        json_escape(&self.top1, &mut j);
        j.push_str(",\"typo\":");
        json_escape(self.typo, &mut j);
        if let Some(v) = self.lm_chosen {
            if v.is_finite() {
                j.push_str(&format!(",\"lm_chosen\":{:.3}", v));
            }
        }
        if let Some(v) = self.lm_top1 {
            if v.is_finite() {
                j.push_str(&format!(",\"lm_top1\":{:.3}", v));
            }
        }
        if self.lm_overruled {
            j.push_str(",\"lm_overruled\":true");
        }
        if !self.cands.is_empty() {
            j.push_str(",\"cands\":[");
            for (i, c) in self.cands.iter().enumerate() {
                if i > 0 {
                    j.push(',');
                }
                j.push_str("{\"t\":");
                json_escape(&c.text, &mut j);
                j.push_str(&format!(
                    ",\"b\":{},\"x\":{},\"c\":{:.2}}}",
                    c.badge,
                    if c.is_exact { 1 } else { 0 },
                    c.confidence
                ));
            }
            j.push(']');
        }
        j.push('}');
        j
    }
}
