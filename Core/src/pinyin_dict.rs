//! Zero-copy `mmap` dictionary format (`PYDICT02`) for Full Pinyin single characters.
//!
//! Supports:
//! - Exact toneless syllable lookup (`"fang"`, `"an"`)
//! - Initial-abbreviation syllable matching (`"srf"`, `"shrf"`, `"shurf"`)
//! - Incomplete trailing prefix expansion (`"zho"` -> `"zhong"`, `"zhou"`)

use crate::mmap_buf::{read_u16, read_u32, ByteSource};
use std::collections::HashMap;
use std::io;
use std::path::Path;

pub const PYDICT_MAGIC: &[u8; 8] = b"PYDICT02";
const HEADER_LEN: usize = 32;
const SYL_STRIDE: usize = 12; // name[8], init_id: u8, is_digraph: u8, pad: u16
const CHAR_BUCKET_STRIDE: usize = 8; // start: u32, count: u16, pad: u16
const CHAR_ENTRY_STRIDE: usize = 12; // str_off: u32, weight: u32, str_len: u16, flags: u16

pub const PY_FLAG_EXT_RARE: u16 = 1 << 1;

/// Map a toneless syllable (or initial string) to its base initial ID (`0..=20`).
/// `zh/ch/sh` map to the same base ID as `z/c/s` so `srf` and `shrf` share one bucket.
#[inline]
pub fn base_initial_id(syl: &str) -> u8 {
    let b = syl.as_bytes().first().copied().unwrap_or(0);
    match b {
        b'b' => 1,
        b'p' => 2,
        b'm' => 3,
        b'f' => 4,
        b'd' => 5,
        b't' => 6,
        b'n' => 7,
        b'l' => 8,
        b'g' => 9,
        b'k' => 10,
        b'h' => 11,
        b'j' => 12,
        b'q' => 13,
        b'x' => 14,
        b'z' => 15,
        b'c' => 16,
        b's' => 17,
        b'r' => 18,
        b'y' => 19,
        b'w' => 20,
        _ => 0, // zero-initial (a, e, o)
    }
}

#[inline]
pub fn pack_init_key(inits: &[u8]) -> Option<u32> {
    if inits.is_empty() || inits.len() > 6 {
        return None;
    }
    let mut key = 0u32;
    for (i, &init) in inits.iter().enumerate() {
        key |= ((init as u32) & 0x1F) << (i * 5);
    }
    Some(key)
}

#[derive(Clone, Copy, Debug)]
pub struct PinyinEntryRef<'a> {
    pub text: &'a str,
    pub weight: u32,
    pub flags: u16,
}

pub struct PinyinDictBuildInput {
    /// (syllable, text, weight, flags)
    pub chars: Vec<(String, String, u32, u16)>,
}

pub fn serialize_pydict(input: PinyinDictBuildInput) -> Vec<u8> {
    // 1. Collect all unique syllables and assign sorted IDs
    let mut syl_set = std::collections::BTreeSet::<String>::new();
    for (syl, _, _, _) in &input.chars {
        if !syl.is_empty() && syl.len() <= 7 {
            syl_set.insert(syl.clone());
        }
    }
    let syllables: Vec<String> = syl_set.into_iter().collect();
    let syl_to_id: HashMap<&str, u16> = syllables
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i as u16))
        .collect();

    // 2. Build string pool
    let mut str_offsets = HashMap::<String, (u32, u16)>::new();
    let mut string_pool = Vec::<u8>::new();
    let mut intern = |s: &str| -> (u32, u16) {
        if let Some(&v) = str_offsets.get(s) {
            return v;
        }
        let off = string_pool.len() as u32;
        let bytes = s.as_bytes();
        let len = bytes.len().min(u16::MAX as usize) as u16;
        string_pool.extend_from_slice(&bytes[..len as usize]);
        str_offsets.insert(s.to_owned(), (off, len));
        (off, len)
    };

    // 3. Group chars by syl_id
    let mut chars_by_syl: Vec<Vec<(u32, u16, u32, u16)>> = vec![Vec::new(); syllables.len()];
    for (syl, text, weight, flags) in &input.chars {
        if let Some(&sid) = syl_to_id.get(syl.as_str()) {
            let (off, len) = intern(text);
            chars_by_syl[sid as usize].push((off, len, *weight, *flags));
        }
    }
    let mut char_buckets_buf = Vec::with_capacity(syllables.len() * CHAR_BUCKET_STRIDE);
    let mut char_entries_buf = Vec::new();
    let mut char_entry_count = 0u32;
    for list in &mut chars_by_syl {
        // First deduplicate identical `(str_off, str_len)` within each syllable bucket,
        // keeping the highest weight and clearing `PY_FLAG_EXT_RARE` if any source marks it standard.
        list.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        list.dedup_by(|dup, keep| {
            if dup.0 == keep.0 && dup.1 == keep.1 {
                keep.2 = keep.2.max(dup.2);
                keep.3 &= dup.3;
                true
            } else {
                false
            }
        });
        // Sort by (ext_rare ASC, weight DESC, str_off ASC)
        list.sort_by(|a, b| {
            let ra = (a.3 & PY_FLAG_EXT_RARE) != 0;
            let rb = (b.3 & PY_FLAG_EXT_RARE) != 0;
            ra.cmp(&rb)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| a.0.cmp(&b.0))
        });
        let start = char_entry_count;
        let count = list.len().min(u16::MAX as usize) as u16;
        for &(off, len, wt, flags) in list.iter().take(count as usize) {
            char_entries_buf.extend_from_slice(&off.to_le_bytes());
            char_entries_buf.extend_from_slice(&wt.to_le_bytes());
            char_entries_buf.extend_from_slice(&len.to_le_bytes());
            char_entries_buf.extend_from_slice(&flags.to_le_bytes());
            char_entry_count += 1;
        }
        char_buckets_buf.extend_from_slice(&start.to_le_bytes());
        char_buckets_buf.extend_from_slice(&count.to_le_bytes());
        char_buckets_buf.extend_from_slice(&0u16.to_le_bytes());
    }

    // 4. Serialize syllable table
    let mut syl_table_buf = Vec::with_capacity(syllables.len() * SYL_STRIDE);
    for s in &syllables {
        let mut name = [0u8; 8];
        let b = s.as_bytes();
        let copy_len = b.len().min(7);
        name[..copy_len].copy_from_slice(&b[..copy_len]);
        let init_id = base_initial_id(s);
        let is_digraph = u8::from(s.starts_with("zh") || s.starts_with("ch") || s.starts_with("sh"));
        syl_table_buf.extend_from_slice(&name);
        syl_table_buf.push(init_id);
        syl_table_buf.push(is_digraph);
        syl_table_buf.extend_from_slice(&0u16.to_le_bytes());
    }

    let syl_table_off = HEADER_LEN as u32;
    let char_buckets_off = syl_table_off + syl_table_buf.len() as u32;
    let char_entries_off = char_buckets_off + char_buckets_buf.len() as u32;
    let string_pool_off = char_entries_off + char_entries_buf.len() as u32;

    let mut out = Vec::with_capacity(string_pool_off as usize + string_pool.len());
    out.extend_from_slice(PYDICT_MAGIC);
    out.extend_from_slice(&(syllables.len() as u32).to_le_bytes());
    out.extend_from_slice(&char_entry_count.to_le_bytes());
    out.extend_from_slice(&syl_table_off.to_le_bytes());
    out.extend_from_slice(&char_buckets_off.to_le_bytes());
    out.extend_from_slice(&char_entries_off.to_le_bytes());
    out.extend_from_slice(&string_pool_off.to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);

    out.extend_from_slice(&syl_table_buf);
    out.extend_from_slice(&char_buckets_buf);
    out.extend_from_slice(&char_entries_buf);
    out.extend_from_slice(&string_pool);
    out
}

pub struct PinyinDictView {
    buf: ByteSource,
    syl_count: usize,
    char_entry_count: usize,
    syl_table_off: usize,
    char_buckets_off: usize,
    char_entries_off: usize,
    string_pool_off: usize,
    /// Compact sorted reverse index `(unicode_cp, syl_id)` for O(log N) char -> syllables lookup.
    char_syl_index: Box<[(u32, u16)]>,
    /// Sorted `(unicode_cp, max_weight)` table across syllables to suppress rare literary readings without HashMap heap bloat.
    char_max_weights: Box<[(u32, f32)]>,
    /// Prebuilt T9 digit sequence -> `syl_id` list (built once on dictionary load, zero per-keystroke alloc).
    t9_index: HashMap<String, Vec<u16>>,
}

impl PinyinDictView {
    pub fn from_file<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let buf = ByteSource::mmap_file(path)?;
        Self::from_source(buf)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> io::Result<Self> {
        Self::from_source(ByteSource::from_vec(bytes))
    }

    fn from_source(buf: ByteSource) -> io::Result<Self> {
        let b = buf.as_slice();
        if b.len() < HEADER_LEN || &b[0..8] != PYDICT_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid PYDICT02 magic",
            ));
        }
        let syl_count = read_u32(b, 8) as usize;
        let char_entry_count = read_u32(b, 12) as usize;
        let syl_table_off = read_u32(b, 16) as usize;
        let char_buckets_off = read_u32(b, 20) as usize;
        let char_entries_off = read_u32(b, 24) as usize;
        let string_pool_off = read_u32(b, 28) as usize;

        if syl_table_off + syl_count * SYL_STRIDE > b.len()
            || char_buckets_off + syl_count * CHAR_BUCKET_STRIDE > b.len()
            || char_entries_off + char_entry_count * CHAR_ENTRY_STRIDE > b.len()
            || string_pool_off > b.len()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated PYDICT02",
            ));
        }

        let mut char_syl_index = Vec::<(u32, u16)>::with_capacity(char_entry_count);
        let mut char_max_weights_vec = Vec::<(u32, f32)>::with_capacity(char_entry_count);
        for sid in 0..syl_count {
            let boff = char_buckets_off + sid * CHAR_BUCKET_STRIDE;
            let start = read_u32(b, boff) as usize;
            let count = read_u16(b, boff + 4) as usize;
            for i in 0..count {
                if start + i >= char_entry_count {
                    break;
                }
                let eoff = char_entries_off + (start + i) * CHAR_ENTRY_STRIDE;
                let str_off = read_u32(b, eoff) as usize;
                let str_len = read_u16(b, eoff + 8) as usize;
                let text_start = string_pool_off + str_off;
                if text_start + str_len <= b.len() {
                    if let Ok(text) = std::str::from_utf8(&b[text_start..text_start + str_len]) {
                        let mut chars = text.chars();
                        if let (Some(ch), None) = (chars.next(), chars.next()) {
                            let cp = ch as u32;
                            let w = (read_u32(b, eoff + 4) as f32).max(1.0);
                            char_syl_index.push((cp, sid as u16));
                            char_max_weights_vec.push((cp, w));
                        }
                    }
                }
            }
        }
        char_syl_index.sort_unstable();
        char_syl_index.dedup();
        let char_syl_index = char_syl_index.into_boxed_slice();

        char_max_weights_vec.sort_unstable_by_key(|&(cp, _)| cp);
        char_max_weights_vec.dedup_by(|dup, keep| {
            if dup.0 == keep.0 {
                keep.1 = keep.1.max(dup.1);
                true
            } else {
                false
            }
        });
        let char_max_weights = char_max_weights_vec.into_boxed_slice();

        let mut t9_index = HashMap::<String, Vec<u16>>::with_capacity(256);
        for sid in 0..syl_count {
            let off = syl_table_off + sid * SYL_STRIDE;
            let slice = &b[off..off + 8];
            let len = slice.iter().position(|&c| c == 0).unwrap_or(8);
            if let Ok(s) = std::str::from_utf8(&slice[..len]) {
                let digits = crate::typo::syllable_to_t9_digits(s);
                if !digits.is_empty() {
                    t9_index.entry(digits).or_default().push(sid as u16);
                }
            }
        }
        for v in t9_index.values_mut() {
            v.shrink_to_fit();
        }
        t9_index.shrink_to_fit();

        Ok(Self {
            buf,
            syl_count,
            char_entry_count,
            syl_table_off,
            char_buckets_off,
            char_entries_off,
            string_pool_off,
            char_syl_index,
            char_max_weights,
            t9_index,
        })
    }

    #[inline]
    pub fn t9_map(&self) -> &HashMap<String, Vec<u16>> {
        &self.t9_index
    }

    #[inline]
    fn max_weight_for(&self, ch: char, fallback: f32) -> f32 {
        let cp = ch as u32;
        match self.char_max_weights.binary_search_by_key(&cp, |&(c, _)| c) {
            Ok(idx) => self.char_max_weights[idx].1.max(fallback),
            Err(_) => fallback,
        }
    }

    #[inline]
    pub fn char_emission_logp(&self, ch: char, syl_weight: u32) -> f32 {
        let w = (syl_weight as f32).max(1.0);
        let max_w = self.max_weight_for(ch, w);
        let base = 0.38 * (w / max_w).clamp(0.002, 1.0).ln();
        if max_w >= 100.0 {
            if syl_weight == 0 {
                // Zero-weight suppressed readings (`8105.dict.yaml` weight=0 like `么=ma`) and
                // unattested archaic readings from `41448.dict.yaml` (`不=fou`, `说=yue`, `大=tai`):
                // prevent them from hijacking common words (`uima`->`是吗`, `shifou`->`是否`, `fouren`->`否认`).
                base - 3.5
            } else if syl_weight == 1 {
                // Rare attested secondary readings (`强=jiang` in `倔强`) stay reachable with a light extra prior.
                base - 1.2
            } else {
                base
            }
        } else {
            base
        }
    }

    /// Look up the dictionary weight of a single character `ch` under `syl_id` without allocating.
    pub fn lookup_char_weight(&self, syl_id: u16, ch: char) -> Option<u32> {
        let idx = syl_id as usize;
        if idx >= self.syl_count {
            return None;
        }
        let mut ch_buf = [0u8; 4];
        let ch_bytes = ch.encode_utf8(&mut ch_buf).as_bytes();
        let b = self.buf.as_slice();
        let boff = self.char_buckets_off + idx * CHAR_BUCKET_STRIDE;
        let start = read_u32(b, boff) as usize;
        let count = read_u16(b, boff + 4) as usize;
        for i in 0..count {
            if start + i >= self.char_entry_count {
                break;
            }
            let eoff = self.char_entries_off + (start + i) * CHAR_ENTRY_STRIDE;
            let str_len = read_u16(b, eoff + 8) as usize;
            if str_len != ch_bytes.len() {
                continue;
            }
            let str_off = read_u32(b, eoff) as usize;
            let text_start = self.string_pool_off + str_off;
            if text_start + str_len <= b.len() && &b[text_start..text_start + str_len] == ch_bytes {
                return Some(read_u32(b, eoff + 4));
            }
        }
        None
    }

    /// Return syllable IDs for `ch` suitable for user-phrase Pinyin indexing, sorted by descending
    /// weight and excluding archaic/zero-weight secondary readings (`weight <= 1` when `max_w >= 100`).
    pub fn char_syllables_for_phrase_index(&self, ch: char) -> Vec<u16> {
        let sids = self.char_syllables(ch);
        if sids.len() <= 1 {
            return sids;
        }
        let max_w = self.max_weight_for(ch, 1.0);
        let mut weighted: Vec<(u16, u32)> = sids
            .into_iter()
            .map(|sid| (sid, self.lookup_char_weight(sid, ch).unwrap_or(1)))
            .collect();
        weighted.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        if max_w >= 100.0 {
            let filtered: Vec<u16> = weighted
                .iter()
                .filter(|&&(_, w)| w > 1)
                .map(|&(sid, _)| sid)
                .collect();
            if !filtered.is_empty() {
                return filtered;
            }
        }
        weighted.into_iter().map(|(sid, _)| sid).collect()
    }

    /// Whether `syl_weight` is a viable modern reading for initial-only abbreviation expansion
    /// (filters out archaic dictionary glosses like `不=fou(1)` vs `bu(2.98M)` while keeping real polyphones).
    #[inline]
    pub fn is_viable_initial_reading(&self, ch: char, syl_weight: u32) -> bool {
        let w = (syl_weight as f32).max(1.0);
        let max_w = self.max_weight_for(ch, w);
        w >= 0.015 * max_w
    }

    #[inline]
    pub fn syllable_count(&self) -> usize {
        self.syl_count
    }

    #[inline]
    pub fn syllable_str(&self, syl_id: u16) -> &str {
        let idx = syl_id as usize;
        if idx >= self.syl_count {
            return "";
        }
        let b = self.buf.as_slice();
        let off = self.syl_table_off + idx * SYL_STRIDE;
        let slice = &b[off..off + 8];
        let len = slice.iter().position(|&c| c == 0).unwrap_or(8);
        std::str::from_utf8(&slice[..len]).unwrap_or("")
    }

    #[inline]
    pub fn syllable_init(&self, syl_id: u16) -> (u8, bool) {
        let idx = syl_id as usize;
        if idx >= self.syl_count {
            return (0, false);
        }
        let b = self.buf.as_slice();
        let off = self.syl_table_off + idx * SYL_STRIDE;
        (b[off + 8], b[off + 9] != 0)
    }

    pub fn lookup_syllable_id(&self, syl: &str) -> Option<u16> {
        let mut lo = 0usize;
        let mut hi = self.syl_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let s = self.syllable_str(mid as u16);
            match s.cmp(syl) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(mid as u16),
            }
        }
        None
    }

    /// Return all syllable IDs starting with `prefix` (e.g. `"zho"` -> `"zhong"`, `"zhou"`).
    pub fn syllables_with_prefix(&self, prefix: &str) -> Vec<u16> {
        if prefix.is_empty() {
            return Vec::new();
        }
        let mut lo = 0usize;
        let mut hi = self.syl_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.syllable_str(mid as u16) < prefix {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut out = Vec::new();
        for idx in lo..self.syl_count {
            let s = self.syllable_str(idx as u16);
            if s.starts_with(prefix) {
                out.push(idx as u16);
            } else {
                break;
            }
        }
        out
    }

    pub fn lookup_chars(&self, syl_id: u16, include_rare: bool) -> Vec<PinyinEntryRef<'_>> {
        let idx = syl_id as usize;
        if idx >= self.syl_count {
            return Vec::new();
        }
        let b = self.buf.as_slice();
        let boff = self.char_buckets_off + idx * CHAR_BUCKET_STRIDE;
        let start = read_u32(b, boff) as usize;
        let count = read_u16(b, boff + 4) as usize;
        let mut out = Vec::with_capacity(count.min(32));
        for i in 0..count {
            if start + i >= self.char_entry_count {
                break;
            }
            let eoff = self.char_entries_off + (start + i) * CHAR_ENTRY_STRIDE;
            let str_off = read_u32(b, eoff) as usize;
            let weight = read_u32(b, eoff + 4);
            let str_len = read_u16(b, eoff + 8) as usize;
            let flags = read_u16(b, eoff + 10);
            if !include_rare && (flags & PY_FLAG_EXT_RARE) != 0 {
                continue;
            }
            let text_start = self.string_pool_off + str_off;
            if text_start + str_len <= b.len() {
                if let Ok(text) = std::str::from_utf8(&b[text_start..text_start + str_len]) {
                    out.push(PinyinEntryRef { text, weight, flags });
                }
            }
        }
        out
    }

    pub fn stats(&self) -> (usize, usize) {
        (self.syl_count, self.char_entry_count)
    }

    /// Return all syllable IDs for a single character via O(log N) binary search.
    pub fn char_syllables(&self, ch: char) -> Vec<u16> {
        let target = ch as u32;
        let start = self.char_syl_index.partition_point(|&(cp, _)| cp < target);
        let mut out = Vec::new();
        for &(cp, sid) in &self.char_syl_index[start..] {
            if cp != target {
                break;
            }
            out.push(sid);
        }
        out
    }

    /// Return a 26-bit mask of valid syllable-starting letters (`'a'..='z'`) for `ch`.
    pub fn char_first_letters_mask(&self, ch: char) -> u32 {
        let target = ch as u32;
        let start = self.char_syl_index.partition_point(|&(cp, _)| cp < target);
        let mut mask = 0u32;
        for &(cp, sid) in &self.char_syl_index[start..] {
            if cp != target {
                break;
            }
            if let Some(&b) = self.syllable_str(sid).as_bytes().first() {
                if b.is_ascii_lowercase() {
                    mask |= 1u32 << (b - b'a');
                }
            }
        }
        mask
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SylConstraint<'a> {
    Full(u16),
    Initial(u8, bool),
    Prefix(&'a str),
}
