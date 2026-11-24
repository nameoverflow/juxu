//! Emoji secondary candidates for Pinyin schemas.
//!
//! The word-to-emoji table comes from rime-ice (`opencc/emoji.txt`, GPL-3.0). Like the other GPL
//! dictionaries it ships as a separate resource (`Resources/Dicts/emoji.tsv`) and is never compiled into
//! the core library. Lines are sorted by key bytes, so lookups binary-search the read-only mapping and
//! need no index on the heap. The file is only mapped when the feature is on.

use crate::code::PackedCode;
use crate::engine::{Candidate, CandidateBadge};
use crate::mmap_buf::ByteSource;
use crate::pinyin::PinyinCandidate;
use std::cmp::Ordering;
use std::io;
use std::path::Path;

/// Emoji shown for one matched word.
const MAX_EMOJI_PER_WORD: usize = 2;
/// Only the leading candidates are considered, so emoji never appear for words deep in the list.
const SCAN_CANDIDATES: usize = 3;

pub struct EmojiTable {
    buf: ByteSource,
}

fn is_emoji_value(value: &str) -> bool {
    value.chars().next().is_some_and(|c| {
        !c.is_ascii() && !c.is_alphanumeric()
    })
}

impl EmojiTable {
    pub fn open_mmap<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Ok(Self { buf: ByteSource::mmap_file(path)? })
    }

    /// The `key<TAB>values` line holding `word`, found by bisecting over line boundaries.
    fn find_line(&self, word: &[u8]) -> Option<&[u8]> {
        let bytes = self.buf.as_slice();
        let (mut low, mut high) = (0usize, bytes.len());
        while low < high {
            let mid = low + (high - low) / 2;
            let start = bytes[low..mid].iter().rposition(|&b| b == b'\n').map_or(low, |i| low + i + 1);
            let end = bytes[mid..high].iter().position(|&b| b == b'\n').map_or(high, |i| mid + i);
            let line = &bytes[start..end];
            let key_len = line.iter().position(|&b| b == b'\t').unwrap_or(line.len());
            match line[..key_len].cmp(word) {
                Ordering::Less => low = end + 1,
                Ordering::Greater => high = start,
                Ordering::Equal => return Some(line),
            }
        }
        None
    }

    /// Emoji for an exact word, most relevant first.
    pub fn lookup(&self, word: &str) -> Vec<&str> {
        let Some(line) = self.find_line(word.as_bytes()) else {
            return Vec::new();
        };
        let Some(line) = std::str::from_utf8(line).ok() else {
            return Vec::new();
        };
        let Some((_, values)) = line.split_once('\t') else {
            return Vec::new();
        };
        let mut out: Vec<&str> = Vec::with_capacity(MAX_EMOJI_PER_WORD);
        for value in values.split(' ') {
            if is_emoji_value(value) && !out.contains(&value) {
                out.push(value);
                if out.len() == MAX_EMOJI_PER_WORD {
                    break;
                }
            }
        }
        out
    }
}

fn build_emoji_candidates(emoji: Vec<&str>) -> Vec<Candidate> {
    emoji
        .into_iter()
        .map(|text| Candidate {
            text: text.to_owned(),
            matched_code: PackedCode::EMPTY,
            comment: String::new(),
            badge: CandidateBadge::Emoji,
            confidence: 0.10,
            is_exact: true,
        })
        .collect()
}

/// Inserts emoji right after the first leading candidate that covers all typed keys and has a match.
/// The first candidate always stays first, so space-to-commit behaves exactly as before.
pub fn inject(table: &EmojiTable, candidates: &mut Vec<PinyinCandidate>, total_keys: usize) {
    for idx in 0..candidates.len().min(SCAN_CANDIDATES) {
        let source = &candidates[idx];
        if source.consumed_keys != total_keys {
            continue;
        }
        let emoji = table.lookup(&source.candidate.text);
        if emoji.is_empty() {
            continue;
        }
        let items: Vec<PinyinCandidate> = build_emoji_candidates(emoji)
            .into_iter()
            .map(|candidate| PinyinCandidate {
                candidate,
                consumed_keys: total_keys,
                boundaries: Vec::new(),
            })
            .collect();
        candidates.splice(idx + 1..idx + 1, items);
        return;
    }
}

/// Inserts emoji right after the first leading candidate with a match (for fixed-code schemas).
/// The first candidate always stays first.
pub fn inject_candidates(table: &EmojiTable, candidates: &mut Vec<Candidate>) {
    for idx in 0..candidates.len().min(SCAN_CANDIDATES) {
        let emoji = table.lookup(&candidates[idx].text);
        if emoji.is_empty() {
            continue;
        }
        let items = build_emoji_candidates(emoji);
        candidates.splice(idx + 1..idx + 1, items);
        return;
    }
}

/// Inserts emoji into spaceless candidate lists while keeping `starts` aligned.
pub fn inject_spaceless_candidates(
    table: &EmojiTable,
    candidates: &mut Vec<Candidate>,
    starts: &mut Vec<Option<usize>>,
) {
    for idx in 0..candidates.len().min(SCAN_CANDIDATES) {
        let emoji = table.lookup(&candidates[idx].text);
        if emoji.is_empty() {
            continue;
        }
        let items = build_emoji_candidates(emoji);
        let count = items.len();
        candidates.splice(idx + 1..idx + 1, items);
        starts.splice(idx + 1..idx + 1, std::iter::repeat_n(None, count));
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InputSchema, JuxuConfig};
    use crate::engine::JuxuEngine;
    use crate::pinyin_dict::{serialize_pydict, PinyinDictBuildInput, PinyinDictView};

    fn engine(emoji_candidates: bool, schema: InputSchema) -> JuxuEngine {
        let bytes = serialize_pydict(PinyinDictBuildInput {
            chars: vec![
                ("si".into(), "思".into(), 9000, 0),
                ("kao".into(), "考".into(), 8000, 0),
                ("hao".into(), "好".into(), 9500, 0),
            ],
        });
        let mut cfg = JuxuConfig::default();
        cfg.schema = schema;
        cfg.emoji_candidates = emoji_candidates;
        cfg.eval_logging = false;
        let mut e = JuxuEngine::new(cfg);
        e.load_pinyin_dict(PinyinDictView::from_bytes(bytes).unwrap());
        e.load_emoji_table(table());
        e
    }

    fn table() -> EmojiTable {
        EmojiTable::open_mmap(concat!(env!("CARGO_MANIFEST_DIR"), "/../Resources/Dicts/emoji.tsv")).unwrap()
    }

    fn type_keys(e: &mut JuxuEngine, keys: &str) {
        for (i, ch) in keys.chars().enumerate() {
            let _ = e.handle_key_event(0, Some(ch), false, 1000 + i as u64 * 40);
        }
    }

    fn texts(e: &JuxuEngine) -> Vec<String> {
        e.all_candidates().iter().map(|c| c.text.clone()).collect()
    }

    #[test]
    fn table_lookup() {
        let t = table();
        assert_eq!(t.lookup("思考"), vec!["🤔"]);
        assert_eq!(t.lookup("笑"), vec!["😄", "😊"]);
        assert!(t.lookup("不存在的词语").is_empty());
        assert!(t.lookup("").is_empty());
    }

    #[test]
    fn every_key_is_sorted_and_reachable() {
        let t = table();
        let text = std::str::from_utf8(t.buf.as_slice()).unwrap();
        let keys: Vec<&str> = text.lines().map(|l| l.split_once('\t').unwrap().0).collect();
        assert!(keys.len() > 4_000);
        assert!(keys.windows(2).all(|w| w[0].as_bytes() < w[1].as_bytes()));
        for key in keys {
            assert!(t.find_line(key.as_bytes()).is_some(), "{key}");
            assert!(t.lookup(key).iter().all(|e| is_emoji_value(e)));
        }
    }

    #[test]
    fn no_table_means_no_emoji() {
        let mut e = engine(true, InputSchema::Pinyin);
        e.emoji_table = None;
        type_keys(&mut e, "sikao");
        assert!(!texts(&e).iter().any(|t| t == "🤔"));
    }

    #[test]
    fn emoji_follows_word_and_keeps_first_candidate() {
        let mut e = engine(true, InputSchema::Pinyin);
        type_keys(&mut e, "sikao");
        let all = texts(&e);
        assert_eq!(all[0], "思考");
        assert_eq!(all[1], "🤔");
    }

    #[test]
    fn double_pinyin_gets_emoji_too() {
        let mut e = engine(true, InputSchema::XiaoheShuangpin);
        type_keys(&mut e, "sikc");
        let all = texts(&e);
        assert_eq!(all[0], "思考");
        assert_eq!(all[1], "🤔");
    }

    #[test]
    fn switch_off_shows_no_emoji() {
        let mut e = engine(false, InputSchema::Pinyin);
        type_keys(&mut e, "sikao");
        assert!(!texts(&e).iter().any(|t| t == "🤔"));
    }

    #[test]
    fn committing_emoji_ends_composition_without_learning_it() {
        let mut e = engine(true, InputSchema::Pinyin);
        e.config.dynamic_word_freq = true;
        type_keys(&mut e, "sikao");
        let out = e.select_abs_candidate(1, 2000).expect("emoji commit");
        assert_eq!(out.commit_text, "🤔");
        assert!(e.raw_code().is_empty());
        assert_eq!(e.user.unigram_boost("🤔"), 0.0);
    }

    #[test]
    fn config_round_trip() {
        let mut cfg = JuxuConfig::default();
        assert!(!cfg.emoji_candidates);
        cfg.emoji_candidates = true;
        assert!(JuxuConfig::from_toml_str(&cfg.to_toml_string()).emoji_candidates);
    }

    #[test]
    fn fixed_code_wubi_gets_emoji_without_breaking_fourth_code_auto_commit() {
        use crate::format::{serialize_wbdict, DictBuildInput, DictView};
        let bytes = serialize_wbdict(DictBuildInput {
            schema_id: 86,
            entries: vec![
                (PackedCode::from_ascii("lf").unwrap(), "思考".into(), 5000, 0),
                (PackedCode::from_ascii("lfft").unwrap(), "思考".into(), 5000, 0),
            ],
            char_metas: vec![],
        });
        let mut cfg = JuxuConfig::default();
        cfg.schema = InputSchema::Wubi86;
        cfg.emoji_candidates = true;
        cfg.fourth_code_auto_commit = true;
        cfg.eval_logging = false;
        let mut e = JuxuEngine::new(cfg);
        e.load_dict(InputSchema::Wubi86, DictView::from_bytes(bytes).unwrap());
        e.load_emoji_table(table());

        // 2-code prefix shows word first, emoji second, and committing emoji works cleanly
        type_keys(&mut e, "lf");
        let all = texts(&e);
        assert_eq!(all[0], "思考");
        assert_eq!(all[1], "🤔");
        let out = e.select_abs_candidate(1, 2000).expect("wubi emoji commit");
        assert_eq!(out.commit_text, "🤔");
        assert!(e.raw_code().is_empty());

        // Unique 4-code word still auto-commits despite emoji secondary candidate
        let mut committed = String::new();
        for (i, ch) in "lfft".chars().enumerate() {
            let o = e.handle_key_event(0, Some(ch), false, 3000 + i as u64 * 40);
            committed.push_str(&o.commit_text);
        }
        assert_eq!(committed, "思考");
    }
}
