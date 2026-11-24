use crate::engine::Candidate;
use std::fmt::Write as _;

/// Append JSON-escaped inner characters of `s` (without surrounding quotes) to `out`.
pub fn push_json_escaped(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

/// Append a JSON string literal `"..."` with proper escaping to `out`.
#[inline]
pub fn push_json_str(out: &mut String, s: &str) {
    out.push('"');
    push_json_escaped(out, s);
    out.push('"');
}

/// Append a JSON array `[...]` of serialized `Candidate` objects (`index` is 1-based).
pub fn push_candidates_json(out: &mut String, candidates: &[Candidate]) {
    out.push('[');
    for (idx, c) in candidates.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let _ = write!(out, "{{\"index\":{},\"text\":", idx + 1);
        push_json_str(out, &c.text);
        out.push_str(",\"comment\":");
        push_json_str(out, &c.comment);
        let _ = write!(
            out,
            ",\"badge\":{},\"confidence\":{:.3},\"isExact\":{}}}",
            c.badge as u8, c.confidence, c.is_exact
        );
    }
    out.push(']');
}
