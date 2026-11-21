//! Zero-copy `mmap` dictionary format (`PYDICT01`) for Full Pinyin single characters and phrases.
//!
//! Supports:
//! - Exact toneless syllable lookup (`"fang"`, `"an"`)
//! - Initial-abbreviation phrase lookup (`"srf"`, `"shrf"`, `"shurf"` -> `"输入法"`)
//! - Incomplete trailing prefix expansion (`"zho"` -> `"zhong"`, `"zhou"`)

use crate::mmap_buf::ByteSource;
use std::collections::HashMap;
use std::io;
use std::path::Path;

pub const PYDICT_MAGIC: &[u8; 8] = b"PYDICT01";
const HEADER_LEN: usize = 64;
const SYL_STRIDE: usize = 12; // name[8], init_id: u8, is_digraph: u8, pad: u16
const CHAR_BUCKET_STRIDE: usize = 8; // start: u32, count: u16, pad: u16
const CHAR_ENTRY_STRIDE: usize = 12; // str_off: u32, weight: u32, str_len: u16, flags: u16
const PHRASE_BUCKET_STRIDE: usize = 12; // init_key: u32, start: u32, count: u16, len: u8, pad: u8
const PHRASE_ENTRY_STRIDE: usize = 16; // str_off: u32, weight: u32, syl_off: u32, str_len: u8, syl_count: u8, flags: u16

pub const PY_FLAG_EXT_RARE: u16 = 1 << 1;

#[inline]
fn read_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

#[inline]
fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

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
    /// (syllables, text, weight, flags)
    pub phrases: Vec<(Vec<String>, String, u32, u16)>,
}

pub fn serialize_pydict(input: PinyinDictBuildInput) -> Vec<u8> {
    // 1. Collect all unique syllables and assign sorted IDs
    let mut syl_set = std::collections::BTreeSet::<String>::new();
    for (syl, _, _, _) in &input.chars {
        if !syl.is_empty() && syl.len() <= 7 {
            syl_set.insert(syl.clone());
        }
    }
    for (syls, _, _, _) in &input.phrases {
        for s in syls {
            if !s.is_empty() && s.len() <= 7 {
                syl_set.insert(s.clone());
            }
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
        // Sort by (ext_rare ASC, weight DESC, str_off ASC)
        list.sort_by(|a, b| {
            let ra = (a.3 & PY_FLAG_EXT_RARE) != 0;
            let rb = (b.3 & PY_FLAG_EXT_RARE) != 0;
            ra.cmp(&rb).then_with(|| b.2.cmp(&a.2)).then_with(|| a.0.cmp(&b.0))
        });
        list.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
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

    // 4. Group phrases by (syl_count, init_key)
    struct PhraseRow {
        syl_ids: Vec<u16>,
        str_off: u32,
        str_len: u8,
        weight: u32,
        flags: u16,
    }
    let mut phrase_groups: std::collections::BTreeMap<(u8, u32), Vec<PhraseRow>> =
        std::collections::BTreeMap::new();
    for (syls, text, weight, flags) in &input.phrases {
        if syls.len() < 2 || syls.len() > 6 || text.len() > 255 {
            continue;
        }
        let mut sids = Vec::with_capacity(syls.len());
        let mut inits = Vec::with_capacity(syls.len());
        let mut valid = true;
        for s in syls {
            if let Some(&sid) = syl_to_id.get(s.as_str()) {
                sids.push(sid);
                inits.push(base_initial_id(s));
            } else {
                valid = false;
                break;
            }
        }
        if !valid {
            continue;
        }
        let Some(init_key) = pack_init_key(&inits) else {
            continue;
        };
        let (off, len) = intern(text);
        phrase_groups
            .entry((syls.len() as u8, init_key))
            .or_default()
            .push(PhraseRow {
                syl_ids: sids,
                str_off: off,
                str_len: len as u8,
                weight: *weight,
                flags: *flags,
            });
    }

    let mut phrase_buckets_buf = Vec::with_capacity(phrase_groups.len() * PHRASE_BUCKET_STRIDE);
    let mut phrase_entries_buf = Vec::new();
    let mut syl_seq_pool_buf = Vec::new();
    let mut phrase_bucket_count = 0u32;
    let mut phrase_entry_count = 0u32;
    let mut syl_seq_count = 0u32;

    for ((len, init_key), mut rows) in phrase_groups {
        rows.sort_by(|a, b| {
            b.weight
                .cmp(&a.weight)
                .then_with(|| a.syl_ids.cmp(&b.syl_ids))
                .then_with(|| a.str_off.cmp(&b.str_off))
        });
        rows.dedup_by(|a, b| a.str_off == b.str_off && a.syl_ids == b.syl_ids);
        let start = phrase_entry_count;
        let count = rows.len().min(u16::MAX as usize) as u16;
        for row in rows.into_iter().take(count as usize) {
            let syl_off = syl_seq_count;
            for sid in row.syl_ids {
                syl_seq_pool_buf.extend_from_slice(&sid.to_le_bytes());
                syl_seq_count += 1;
            }
            phrase_entries_buf.extend_from_slice(&row.str_off.to_le_bytes());
            phrase_entries_buf.extend_from_slice(&row.weight.to_le_bytes());
            phrase_entries_buf.extend_from_slice(&syl_off.to_le_bytes());
            phrase_entries_buf.push(row.str_len);
            phrase_entries_buf.push(len);
            phrase_entries_buf.extend_from_slice(&row.flags.to_le_bytes());
            phrase_entry_count += 1;
        }
        phrase_buckets_buf.extend_from_slice(&init_key.to_le_bytes());
        phrase_buckets_buf.extend_from_slice(&start.to_le_bytes());
        phrase_buckets_buf.extend_from_slice(&count.to_le_bytes());
        phrase_buckets_buf.push(len);
        phrase_buckets_buf.push(0u8);
        phrase_bucket_count += 1;
    }

    // 5. Serialize syllable table
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
    let phrase_buckets_off = char_entries_off + char_entries_buf.len() as u32;
    let phrase_entries_off = phrase_buckets_off + phrase_buckets_buf.len() as u32;
    let syl_seq_pool_off = phrase_entries_off + phrase_entries_buf.len() as u32;
    let string_pool_off = syl_seq_pool_off + syl_seq_pool_buf.len() as u32;

    let mut out = Vec::with_capacity(string_pool_off as usize + string_pool.len());
    out.extend_from_slice(PYDICT_MAGIC);
    out.extend_from_slice(&(syllables.len() as u32).to_le_bytes());
    out.extend_from_slice(&(syllables.len() as u32).to_le_bytes());
    out.extend_from_slice(&char_entry_count.to_le_bytes());
    out.extend_from_slice(&phrase_bucket_count.to_le_bytes());
    out.extend_from_slice(&phrase_entry_count.to_le_bytes());
    out.extend_from_slice(&syl_table_off.to_le_bytes());
    out.extend_from_slice(&char_buckets_off.to_le_bytes());
    out.extend_from_slice(&char_entries_off.to_le_bytes());
    out.extend_from_slice(&phrase_buckets_off.to_le_bytes());
    out.extend_from_slice(&phrase_entries_off.to_le_bytes());
    out.extend_from_slice(&syl_seq_pool_off.to_le_bytes());
    out.extend_from_slice(&string_pool_off.to_le_bytes());
    out.extend_from_slice(&[0u8; 8]);
    debug_assert_eq!(out.len(), HEADER_LEN);

    out.extend_from_slice(&syl_table_buf);
    out.extend_from_slice(&char_buckets_buf);
    out.extend_from_slice(&char_entries_buf);
    out.extend_from_slice(&phrase_buckets_buf);
    out.extend_from_slice(&phrase_entries_buf);
    out.extend_from_slice(&syl_seq_pool_buf);
    out.extend_from_slice(&string_pool);
    out
}

pub struct PinyinDictView {
    buf: ByteSource,
    syl_count: usize,
    char_entry_count: usize,
    phrase_bucket_count: usize,
    phrase_entry_count: usize,
    syl_table_off: usize,
    char_buckets_off: usize,
    char_entries_off: usize,
    phrase_buckets_off: usize,
    phrase_entries_off: usize,
    syl_seq_pool_off: usize,
    string_pool_off: usize,
    /// Compact sorted reverse index `(unicode_cp, syl_id)` for O(log N) char -> syllables lookup.
    char_syl_index: Vec<(u32, u16)>,
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
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid PYDICT01 magic"));
        }
        let syl_count = read_u32(b, 8) as usize;
        let char_entry_count = read_u32(b, 16) as usize;
        let phrase_bucket_count = read_u32(b, 20) as usize;
        let phrase_entry_count = read_u32(b, 24) as usize;
        let syl_table_off = read_u32(b, 28) as usize;
        let char_buckets_off = read_u32(b, 32) as usize;
        let char_entries_off = read_u32(b, 36) as usize;
        let phrase_buckets_off = read_u32(b, 40) as usize;
        let phrase_entries_off = read_u32(b, 44) as usize;
        let syl_seq_pool_off = read_u32(b, 48) as usize;
        let string_pool_off = read_u32(b, 52) as usize;

        if string_pool_off > b.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated PYDICT01"));
        }

        let mut char_syl_index = Vec::<(u32, u16)>::with_capacity(char_entry_count);
        for sid in 0..syl_count {
            let boff = char_buckets_off + sid * CHAR_BUCKET_STRIDE;
            if boff + CHAR_BUCKET_STRIDE > b.len() {
                break;
            }
            let start = read_u32(b, boff) as usize;
            let count = read_u16(b, boff + 4) as usize;
            for i in 0..count {
                if start + i >= char_entry_count {
                    break;
                }
                let eoff = char_entries_off + (start + i) * CHAR_ENTRY_STRIDE;
                if eoff + CHAR_ENTRY_STRIDE > b.len() {
                    break;
                }
                let str_off = read_u32(b, eoff) as usize;
                let str_len = read_u16(b, eoff + 8) as usize;
                let text_start = string_pool_off + str_off;
                if text_start + str_len <= b.len() {
                    if let Ok(text) = std::str::from_utf8(&b[text_start..text_start + str_len]) {
                        let mut chars = text.chars();
                        if let (Some(ch), None) = (chars.next(), chars.next()) {
                            char_syl_index.push((ch as u32, sid as u16));
                        }
                    }
                }
            }
        }
        char_syl_index.sort_unstable();
        char_syl_index.dedup();

        Ok(Self {
            buf,
            syl_count,
            char_entry_count,
            phrase_bucket_count,
            phrase_entry_count,
            syl_table_off,
            char_buckets_off,
            char_entries_off,
            phrase_buckets_off,
            phrase_entries_off,
            syl_seq_pool_off,
            string_pool_off,
            char_syl_index,
        })
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
            if let Ok(text) = std::str::from_utf8(&b[text_start..text_start + str_len]) {
                out.push(PinyinEntryRef { text, weight, flags });
            }
        }
        out
    }

    /// Look up phrases matching `constraints` (each element is either `ExactSyl(id)`, `Prefix(str)`, or `Initial(init_id, digraph)`).
    pub fn lookup_phrases(&self, constraints: &[SylConstraint<'_>], limit: usize) -> Vec<PinyinEntryRef<'_>> {
        let len = constraints.len();
        if !(2..=6).contains(&len) {
            return Vec::new();
        }
        let mut inits = [0u8; 6];
        for (i, c) in constraints.iter().enumerate() {
            inits[i] = match c {
                SylConstraint::Full(sid) => self.syllable_init(*sid).0,
                SylConstraint::Initial(init_id, _) => *init_id,
                SylConstraint::Prefix(p) => base_initial_id(p),
            };
        }
        let Some(init_key) = pack_init_key(&inits[..len]) else {
            return Vec::new();
        };

        // Binary search in phrase_buckets sorted by (len, init_key)
        let b = self.buf.as_slice();
        let target = (len as u8, init_key);
        let mut lo = 0usize;
        let mut hi = self.phrase_bucket_count;
        let mut found = None;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let boff = self.phrase_buckets_off + mid * PHRASE_BUCKET_STRIDE;
            let b_init_key = read_u32(b, boff);
            let b_len = b[boff + 10];
            match (b_len, b_init_key).cmp(&target) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    found = Some(boff);
                    break;
                }
            }
        }
        let Some(boff) = found else {
            return Vec::new();
        };
        let start = read_u32(b, boff + 4) as usize;
        let count = read_u16(b, boff + 8) as usize;
        let mut out = Vec::with_capacity(limit.min(count));

        for i in 0..count {
            if start + i >= self.phrase_entry_count {
                break;
            }
            let eoff = self.phrase_entries_off + (start + i) * PHRASE_ENTRY_STRIDE;
            let syl_off = read_u32(b, eoff + 8) as usize;
            let syl_count = b[eoff + 13] as usize;
            if syl_count != len {
                continue;
            }
            let mut matched = true;
            for j in 0..len {
                let sid = read_u16(b, self.syl_seq_pool_off + (syl_off + j) * 2);
                match constraints[j] {
                    SylConstraint::Full(req_sid) => {
                        if sid != req_sid {
                            matched = false;
                            break;
                        }
                    }
                    SylConstraint::Initial(_, req_digraph) => {
                        if req_digraph && !self.syllable_init(sid).1 {
                            matched = false;
                            break;
                        }
                    }
                    SylConstraint::Prefix(pref) => {
                        if !self.syllable_str(sid).starts_with(pref) {
                            matched = false;
                            break;
                        }
                    }
                }
            }
            if !matched {
                continue;
            }
            let str_off = read_u32(b, eoff) as usize;
            let weight = read_u32(b, eoff + 4);
            let str_len = b[eoff + 12] as usize;
            let flags = read_u16(b, eoff + 14);
            let text_start = self.string_pool_off + str_off;
            if let Ok(text) = std::str::from_utf8(&b[text_start..text_start + str_len]) {
                out.push(PinyinEntryRef { text, weight, flags });
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        (self.syl_count, self.char_entry_count, self.phrase_entry_count)
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
