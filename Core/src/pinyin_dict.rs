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

pub const PYDICT_MAGIC: &[u8; 8] = b"PYDICT03";
const PYDICT_MAGIC_V2: &[u8; 8] = b"PYDICT02";
const HEADER_LEN_V2: usize = 32;
const HEADER_LEN: usize = 56;
const SYL_STRIDE: usize = 12; // name[8], init_id: u8, is_digraph: u8, pad: u16
const CHAR_BUCKET_STRIDE: usize = 8; // start: u32, count: u16, pad: u16
const CHAR_ENTRY_STRIDE_V2: usize = 12; // str_off: u32, weight: u32, str_len: u16, flags: u16
const CHAR_ENTRY_STRIDE: usize = 8; // packed_str_flags: u32, weight: u32
const PHRASE_BUCKET_STRIDE: usize = 10; // bucket_key: u32, start: u32, count: u16
const PHRASE_ENTRY_STRIDE: usize = 10; // cidx: [u16; 4], log_wt_q8: u16
const HETERO_ENTRY_STRIDE: usize = 10; // pair_key: u32, packed40: [u8; 6] (4 * 10-bit specs)

pub const PY_FLAG_EXT_RARE: u16 = 1 << 1;
const PHRASE_CHAR_NONE: u16 = 0xFFFF;
const HETERO_NONE: u16 = 0xFFFF;
const HETERO_RESCUE_BIT: u16 = 0x8000;
const HETERO10_NONE: u64 = 0x3FF;
const HETERO10_RESCUE: u64 = 0x200;

#[inline]
fn encode_hetero10(spec: u16) -> u64 {
    if spec == HETERO_NONE {
        HETERO10_NONE
    } else {
        let sid = (spec & !HETERO_RESCUE_BIT) as u64;
        if sid >= HETERO10_RESCUE {
            HETERO10_NONE
        } else if (spec & HETERO_RESCUE_BIT) != 0 {
            sid | HETERO10_RESCUE
        } else {
            sid
        }
    }
}

#[inline]
fn decode_hetero10(bits: u64) -> u16 {
    let v = (bits & 0x3FF) as u16;
    if v == (HETERO10_NONE as u16) {
        HETERO_NONE
    } else if (v & (HETERO10_RESCUE as u16)) != 0 {
        (v & 0x1FF) | HETERO_RESCUE_BIT
    } else {
        v
    }
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

#[derive(Clone, Debug)]
pub struct PinyinPhrase {
    pub text: String,
    pub log_wt: f32,
    pub syl_ids: [u16; 4],
    pub syl_len: u8,
}

pub struct PinyinDictBuildInput {
    /// (syllable, text, weight, flags)
    pub chars: Vec<(String, String, u32, u16)>,
}

pub fn serialize_pydict(input: PinyinDictBuildInput) -> Vec<u8> {
    serialize_pydict_with_phrases(input.chars, Vec::new(), Vec::new())
}

pub fn serialize_pydict_with_phrases(
    chars_in: Vec<(String, String, u32, u16)>,
    phrases_in: Vec<(String, Vec<String>, u32)>,
    heteros_in: Vec<(char, char, String, String)>,
) -> Vec<u8> {
    // 1. Collect all unique syllables from both single chars and phrase readings
    let mut syl_set = std::collections::BTreeSet::<String>::new();
    for (syl, _, _, _) in &chars_in {
        if !syl.is_empty() && syl.len() <= 7 {
            syl_set.insert(syl.clone());
        }
    }
    for (_, syls, _) in &phrases_in {
        for syl in syls {
            if !syl.is_empty() && syl.len() <= 7 {
                syl_set.insert(syl.clone());
            }
        }
    }
    let syllables: Vec<String> = syl_set.into_iter().collect();
    let syl_to_id: HashMap<&str, u16> = syllables
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i as u16))
        .collect();

    // 2. Build single-character string pool (phrases reference `char_entries` via u16 indices -> 0 phrase string pool bytes!)
    let mut str_offsets = HashMap::<String, (u32, u16)>::new();
    let mut string_pool = Vec::<u8>::new();
    let mut intern = |s: &str| -> (u32, u16) {
        if let Some(&v) = str_offsets.get(s) {
            return v;
        }
        let off = string_pool.len() as u32;
        let bytes = s.as_bytes();
        let len = bytes.len().min(7) as u16;
        string_pool.extend_from_slice(&bytes[..len as usize]);
        str_offsets.insert(s.to_owned(), (off, len));
        (off, len)
    };

    // 3. Group chars by syl_id (and ensure every `(syl, ch)` attested in `phrases_in` exists in `chars_by_syl`)
    let mut chars_by_syl: Vec<Vec<(u32, u16, u32, u16)>> = vec![Vec::new(); syllables.len()];
    for (syl, text, weight, flags) in &chars_in {
        if let Some(&sid) = syl_to_id.get(syl.as_str()) {
            let (off, len) = intern(text);
            chars_by_syl[sid as usize].push((off, len, *weight, *flags));
        }
    }
    for (text, syls, _) in &phrases_in {
        if !(2..=4).contains(&syls.len()) || text.chars().count() != syls.len() {
            continue;
        }
        let mut ch_buf = [0u8; 4];
        for (ch, syl) in text.chars().zip(syls.iter()) {
            if let Some(&sid) = syl_to_id.get(syl.as_str()) {
                let s = ch.encode_utf8(&mut ch_buf);
                let (off, len) = intern(s);
                chars_by_syl[sid as usize].push((off, len, 1, 0));
            }
        }
    }

    let mut char_buckets_buf = Vec::with_capacity(syllables.len() * CHAR_BUCKET_STRIDE);
    let mut char_entries_buf = Vec::new();
    let mut char_entry_count = 0u32;
    let mut sid_off_to_cidx = HashMap::<(u16, u32), u16>::with_capacity(chars_in.len());
    for (sid_idx, list) in chars_by_syl.iter_mut().enumerate() {
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
        list.sort_by(|a, b| {
            let ra = (a.3 & PY_FLAG_EXT_RARE) != 0;
            let rb = (b.3 & PY_FLAG_EXT_RARE) != 0;
            ra.cmp(&rb)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| a.0.cmp(&b.0))
        });
        let start = char_entry_count;
        let max_add = (PHRASE_CHAR_NONE as u32).saturating_sub(char_entry_count) as usize;
        let count = list.len().min(u16::MAX as usize).min(max_add) as u16;
        for &(off, len, wt, flags) in list.iter().take(count as usize) {
            let packed_str_flags = (off & 0x1F_FFFF)
                | (((len as u32) & 0x7) << 21)
                | (((flags as u32) & 0xFF) << 24);
            char_entries_buf.extend_from_slice(&packed_str_flags.to_le_bytes());
            char_entries_buf.extend_from_slice(&wt.to_le_bytes());
            sid_off_to_cidx.insert((sid_idx as u16, off), char_entry_count as u16);
            char_entry_count += 1;
        }
        char_buckets_buf.extend_from_slice(&start.to_le_bytes());
        char_buckets_buf.extend_from_slice(&count.to_le_bytes());
        char_buckets_buf.extend_from_slice(&0u16.to_le_bytes());
    }

    // 4. Serialize compact core phrases (`2..=4` syllables) as `[cidx: u16; 4] + log_wt_q8: u16` (10 bytes/phrase!)
    let mut phrases_by_bucket: std::collections::BTreeMap<u32, Vec<([u16; 4], u16, u32)>> =
        std::collections::BTreeMap::new();
    for (text, syls, wt) in &phrases_in {
        let slen = syls.len();
        if !(2..=4).contains(&slen) || text.chars().count() != slen {
            continue;
        }
        let mut cidxs = [PHRASE_CHAR_NONE; 4];
        let mut inits = [0u8; 4];
        let mut ok = true;
        let mut ch_buf = [0u8; 4];
        for (i, (ch, s)) in text.chars().zip(syls.iter()).enumerate() {
            let ch_s = ch.encode_utf8(&mut ch_buf);
            if let (Some(&sid), Some(&(off, _))) = (syl_to_id.get(s.as_str()), str_offsets.get(ch_s))
            {
                if let Some(&cidx) = sid_off_to_cidx.get(&(sid, off)) {
                    cidxs[i] = cidx;
                    inits[i] = base_initial_id(s);
                    continue;
                }
            }
            ok = false;
            break;
        }
        if !ok {
            continue;
        }
        let Some(init_key) = pack_init_key(&inits[..slen]) else {
            continue;
        };
        let bucket_key = ((slen as u32) << 24) | init_key;
        let log_wt_q8 = (((*wt as f32 + 1.0).ln() * 256.0).round().clamp(0.0, 65535.0)) as u16;
        phrases_by_bucket
            .entry(bucket_key)
            .or_default()
            .push((cidxs, log_wt_q8, *wt));
    }

    let mut phrase_buckets_buf = Vec::with_capacity(phrases_by_bucket.len() * PHRASE_BUCKET_STRIDE);
    let mut phrase_entries_buf = Vec::new();
    let mut phrase_bucket_count = 0u32;
    let mut phrase_entry_count = 0u32;
    for (bkey, mut list) in phrases_by_bucket {
        list.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.2.cmp(&a.2)));
        list.dedup_by(|dup, keep| {
            if dup.0 == keep.0 {
                if dup.2 > keep.2 {
                    keep.2 = dup.2;
                    keep.1 = dup.1;
                }
                true
            } else {
                false
            }
        });
        list.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        let start = phrase_entry_count;
        let count = list.len().min(u16::MAX as usize) as u16;
        for &(cidxs, log_wt_q8, _) in list.iter().take(count as usize) {
            for c in cidxs {
                phrase_entries_buf.extend_from_slice(&c.to_le_bytes());
            }
            phrase_entries_buf.extend_from_slice(&log_wt_q8.to_le_bytes());
            phrase_entry_count += 1;
        }
        phrase_buckets_buf.extend_from_slice(&bkey.to_le_bytes());
        phrase_buckets_buf.extend_from_slice(&start.to_le_bytes());
        phrase_buckets_buf.extend_from_slice(&count.to_le_bytes());
        phrase_bucket_count += 1;
    }

    // 5. Serialize heteronym pronunciation-pair table (`pair_key: u32` + 4x10-bit packed specs in 6 bytes = 10B)
    let parse_hetero_spec = |spec: &str| -> (u16, u16) {
        let trimmed = spec.trim();
        if trimmed.is_empty() || trimmed == "*" {
            return (HETERO_NONE, HETERO_NONE);
        }
        let mut out = [HETERO_NONE; 2];
        for (idx, raw_part) in trimmed.split(',').take(2).enumerate() {
            let (rescue, s_name) = if let Some(rest) = raw_part.strip_prefix('+') {
                (true, rest)
            } else {
                (false, raw_part)
            };
            if let Some(&sid) = syl_to_id.get(s_name) {
                out[idx] = if idx == 0 && rescue {
                    sid | HETERO_RESCUE_BIT
                } else {
                    sid
                };
            }
        }
        (out[0], out[1])
    };

    let mut hetero_list: Vec<(u32, u16, u16, u16, u16)> = Vec::with_capacity(heteros_in.len());
    for (c1, c2, sp1, sp2) in &heteros_in {
        let u1 = *c1 as u32;
        let u2 = *c2 as u32;
        if u1 > 0xFFFF || u2 > 0xFFFF {
            continue;
        }
        let (s1_a, s1_b) = parse_hetero_spec(sp1);
        let (s2_a, s2_b) = parse_hetero_spec(sp2);
        if s1_a == HETERO_NONE && s2_a == HETERO_NONE {
            continue;
        }
        hetero_list.push(((u1 << 16) | u2, s1_a, s1_b, s2_a, s2_b));
    }
    hetero_list.sort_by_key(|x| x.0);
    hetero_list.dedup_by_key(|x| x.0);
    let hetero_count = hetero_list.len() as u32;
    let mut hetero_entries_buf = Vec::with_capacity(hetero_list.len() * HETERO_ENTRY_STRIDE);
    for (pkey, s1_a, s1_b, s2_a, s2_b) in hetero_list {
        let packed40: u64 = encode_hetero10(s1_a)
            | (encode_hetero10(s1_b) << 10)
            | (encode_hetero10(s2_a) << 20)
            | (encode_hetero10(s2_b) << 30);
        let pbytes = packed40.to_le_bytes();
        hetero_entries_buf.extend_from_slice(&pkey.to_le_bytes());
        hetero_entries_buf.extend_from_slice(&pbytes[..6]);
    }

    // 6. Serialize syllable table
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

    // Align 8-byte `phrase_entries_buf` cleanly (all preceding sections are multiples of 4; pad if needed)
    let syl_table_off = HEADER_LEN as u32;
    let char_buckets_off = syl_table_off + syl_table_buf.len() as u32;
    let char_entries_off = char_buckets_off + char_buckets_buf.len() as u32;
    let phrase_buckets_off = char_entries_off + char_entries_buf.len() as u32;
    let phrase_entries_off = phrase_buckets_off + phrase_buckets_buf.len() as u32;
    let hetero_entries_off = phrase_entries_off + phrase_entries_buf.len() as u32;
    let string_pool_off = hetero_entries_off + hetero_entries_buf.len() as u32;

    let mut out = Vec::with_capacity(string_pool_off as usize + string_pool.len());
    out.extend_from_slice(PYDICT_MAGIC);
    out.extend_from_slice(&(syllables.len() as u32).to_le_bytes());
    out.extend_from_slice(&char_entry_count.to_le_bytes());
    out.extend_from_slice(&syl_table_off.to_le_bytes());
    out.extend_from_slice(&char_buckets_off.to_le_bytes());
    out.extend_from_slice(&char_entries_off.to_le_bytes());
    out.extend_from_slice(&string_pool_off.to_le_bytes());
    out.extend_from_slice(&phrase_bucket_count.to_le_bytes());
    out.extend_from_slice(&phrase_entry_count.to_le_bytes());
    out.extend_from_slice(&phrase_buckets_off.to_le_bytes());
    out.extend_from_slice(&phrase_entries_off.to_le_bytes());
    out.extend_from_slice(&hetero_count.to_le_bytes());
    out.extend_from_slice(&hetero_entries_off.to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);

    out.extend_from_slice(&syl_table_buf);
    out.extend_from_slice(&char_buckets_buf);
    out.extend_from_slice(&char_entries_buf);
    out.extend_from_slice(&phrase_buckets_buf);
    out.extend_from_slice(&phrase_entries_buf);
    out.extend_from_slice(&hetero_entries_buf);
    out.extend_from_slice(&string_pool);
    out
}

pub struct PinyinDictView {
    buf: ByteSource,
    is_v3: bool,
    syl_count: usize,
    char_entry_count: usize,
    syl_table_off: usize,
    char_buckets_off: usize,
    char_entries_off: usize,
    string_pool_off: usize,
    phrase_bucket_count: usize,
    phrase_entry_count: usize,
    phrase_buckets_off: usize,
    phrase_entries_off: usize,
    hetero_count: usize,
    hetero_entries_off: usize,
    /// O(1) mapping from `char_entry` index `cidx` (`0..char_entry_count`) -> `syl_id` for phrase decoding.
    char_entry_syl: Box<[u16]>,
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
        let is_v3 = if b.len() >= HEADER_LEN && &b[0..8] == PYDICT_MAGIC {
            true
        } else if b.len() >= HEADER_LEN_V2 && &b[0..8] == PYDICT_MAGIC_V2 {
            false
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid PYDICT02/PYDICT03 magic",
            ));
        };
        let syl_count = read_u32(b, 8) as usize;
        let char_entry_count = read_u32(b, 12) as usize;
        let syl_table_off = read_u32(b, 16) as usize;
        let char_buckets_off = read_u32(b, 20) as usize;
        let char_entries_off = read_u32(b, 24) as usize;
        let string_pool_off = read_u32(b, 28) as usize;
        let (
            phrase_bucket_count,
            phrase_entry_count,
            phrase_buckets_off,
            phrase_entries_off,
            hetero_count,
            hetero_entries_off,
        ) = if is_v3 {
            (
                read_u32(b, 32) as usize,
                read_u32(b, 36) as usize,
                read_u32(b, 40) as usize,
                read_u32(b, 44) as usize,
                read_u32(b, 48) as usize,
                read_u32(b, 52) as usize,
            )
        } else {
            (0, 0, 0, 0, 0, 0)
        };

        let ce_stride = if is_v3 {
            CHAR_ENTRY_STRIDE
        } else {
            CHAR_ENTRY_STRIDE_V2
        };
        if syl_table_off + syl_count * SYL_STRIDE > b.len()
            || char_buckets_off + syl_count * CHAR_BUCKET_STRIDE > b.len()
            || char_entries_off + char_entry_count * ce_stride > b.len()
            || phrase_buckets_off + phrase_bucket_count * PHRASE_BUCKET_STRIDE > b.len()
            || phrase_entries_off + phrase_entry_count * PHRASE_ENTRY_STRIDE > b.len()
            || hetero_entries_off + hetero_count * HETERO_ENTRY_STRIDE > b.len()
            || string_pool_off > b.len()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated PYDICT02/PYDICT03",
            ));
        }

        let mut char_entry_syl_vec = if is_v3 && phrase_entry_count > 0 {
            vec![0u16; char_entry_count]
        } else {
            Vec::new()
        };
        let mut char_syl_index = Vec::<(u32, u16)>::with_capacity(char_entry_count);
        let mut char_max_weights_vec = Vec::<(u32, f32)>::with_capacity(char_entry_count);
        for sid in 0..syl_count {
            let boff = char_buckets_off + sid * CHAR_BUCKET_STRIDE;
            let start = read_u32(b, boff) as usize;
            let count = read_u16(b, boff + 4) as usize;
            for i in 0..count {
                let cidx = start + i;
                if cidx >= char_entry_count {
                    break;
                }
                if cidx < char_entry_syl_vec.len() {
                    char_entry_syl_vec[cidx] = sid as u16;
                }
                let eoff = char_entries_off + cidx * ce_stride;
                let (str_off, str_len, wt) = if is_v3 {
                    let p = read_u32(b, eoff);
                    (
                        (p & 0x1F_FFFF) as usize,
                        ((p >> 21) & 0x7) as usize,
                        read_u32(b, eoff + 4),
                    )
                } else {
                    (
                        read_u32(b, eoff) as usize,
                        read_u16(b, eoff + 8) as usize,
                        read_u32(b, eoff + 4),
                    )
                };
                let text_start = string_pool_off + str_off;
                if text_start + str_len <= b.len() {
                    if let Ok(text) = std::str::from_utf8(&b[text_start..text_start + str_len]) {
                        let mut chars = text.chars();
                        if let (Some(ch), None) = (chars.next(), chars.next()) {
                            let cp = ch as u32;
                            let w = (wt as f32).max(1.0);
                            char_syl_index.push((cp, sid as u16));
                            char_max_weights_vec.push((cp, w));
                        }
                    }
                }
            }
        }
        let char_entry_syl = char_entry_syl_vec.into_boxed_slice();
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
            is_v3,
            syl_count,
            char_entry_count,
            syl_table_off,
            char_buckets_off,
            char_entries_off,
            string_pool_off,
            phrase_bucket_count,
            phrase_entry_count,
            phrase_buckets_off,
            phrase_entries_off,
            hetero_count,
            hetero_entries_off,
            char_entry_syl,
            char_syl_index,
            char_max_weights,
            t9_index,
        })
    }

    #[inline]
    fn read_char_entry<'a>(&'a self, b: &'a [u8], cidx: usize) -> Option<(&'a str, u32, u16)> {
        if cidx >= self.char_entry_count {
            return None;
        }
        let (str_off, str_len, weight, flags) = if self.is_v3 {
            let eoff = self.char_entries_off + cidx * CHAR_ENTRY_STRIDE;
            let p = read_u32(b, eoff);
            (
                (p & 0x1F_FFFF) as usize,
                ((p >> 21) & 0x7) as usize,
                read_u32(b, eoff + 4),
                ((p >> 24) & 0xFF) as u16,
            )
        } else {
            let eoff = self.char_entries_off + cidx * CHAR_ENTRY_STRIDE_V2;
            (
                read_u32(b, eoff) as usize,
                read_u16(b, eoff + 8) as usize,
                read_u32(b, eoff + 4),
                read_u16(b, eoff + 10),
            )
        };
        let text_start = self.string_pool_off + str_off;
        if text_start + str_len > b.len() {
            return None;
        }
        let text = std::str::from_utf8(&b[text_start..text_start + str_len]).ok()?;
        Some((text, weight, flags))
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
        let ch_s = ch.encode_utf8(&mut ch_buf);
        let b = self.buf.as_slice();
        let boff = self.char_buckets_off + idx * CHAR_BUCKET_STRIDE;
        let start = read_u32(b, boff) as usize;
        let count = read_u16(b, boff + 4) as usize;
        for i in 0..count {
            let Some((text, wt, _)) = self.read_char_entry(b, start + i) else {
                break;
            };
            if text == ch_s {
                return Some(wt);
            }
        }
        None
    }

    /// Return syllable IDs for `ch` suitable for user-phrase Pinyin indexing, sorted by descending
    /// weight and excluding zero-weight suppressed / archaic glosses (`weight == 0` when `max_w >= 100`),
    /// while keeping attested secondary readings (`weight >= 1` in `8105`, e.g. `哪=ne`, `单=shan/chan`, `绿=lu`).
    pub fn char_syllables_for_phrase_index(&self, ch: char) -> Vec<u16> {
        let sids = self.char_syllables(ch);
        if sids.len() <= 1 {
            return sids;
        }
        let max_w = self.max_weight_for(ch, 1.0);
        let mut weighted: Vec<(u16, u32)> = sids
            .into_iter()
            .map(|sid| (sid, self.lookup_char_weight(sid, ch).unwrap_or(0)))
            .collect();
        weighted.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if max_w >= 100.0 {
            let filtered: Vec<u16> = weighted
                .iter()
                .filter(|&&(_, w)| w > 0)
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
            let Some((text, weight, flags)) = self.read_char_entry(b, start + i) else {
                break;
            };
            if !include_rare && (flags & PY_FLAG_EXT_RARE) != 0 {
                continue;
            }
            out.push(PinyinEntryRef { text, weight, flags });
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

    #[inline]
    pub fn has_phrases(&self) -> bool {
        self.phrase_bucket_count > 0
    }

    fn find_phrase_bucket(&self, bucket_key: u32) -> Option<(usize, usize)> {
        if self.phrase_bucket_count == 0 {
            return None;
        }
        let b = self.buf.as_slice();
        let mut lo = 0usize;
        let mut hi = self.phrase_bucket_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let off = self.phrase_buckets_off + mid * PHRASE_BUCKET_STRIDE;
            let k = read_u32(b, off);
            match k.cmp(&bucket_key) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    let start = read_u32(b, off + 4) as usize;
                    let count = read_u16(b, off + 8) as usize;
                    return Some((start, count));
                }
            }
        }
        None
    }

    /// Look up compact core phrases (`2..=4` syllables) matching `constraints` in descending weight order.
    pub fn lookup_phrases(
        &self,
        constraints: &[SylConstraint<'_>],
        limit: usize,
    ) -> Vec<PinyinPhrase> {
        let slen = constraints.len();
        if !(2..=4).contains(&slen) || limit == 0 || self.phrase_bucket_count == 0 {
            return Vec::new();
        }
        let mut inits = [0u8; 4];
        for (i, c) in constraints.iter().enumerate() {
            inits[i] = match *c {
                SylConstraint::Full(sid) => self.syllable_init(sid).0,
                SylConstraint::Initial(id, _) => id,
                SylConstraint::Prefix(pref) => base_initial_id(pref),
            };
        }
        let Some(init_key) = pack_init_key(&inits[..slen]) else {
            return Vec::new();
        };
        let bucket_key = ((slen as u32) << 24) | init_key;
        let Some((start, count)) = self.find_phrase_bucket(bucket_key) else {
            return Vec::new();
        };
        let b = self.buf.as_slice();
        let mut out = Vec::with_capacity(limit.min(8));
        for i in 0..count {
            if start + i >= self.phrase_entry_count {
                break;
            }
            let eoff = self.phrase_entries_off + (start + i) * PHRASE_ENTRY_STRIDE;
            let mut cidxs = [0u16; 4];
            let mut sids = [0u16; 4];
            let mut matched = true;
            for (ci, cons) in constraints.iter().enumerate() {
                let cidx = read_u16(b, eoff + ci * 2);
                if cidx == PHRASE_CHAR_NONE || (cidx as usize) >= self.char_entry_syl.len() {
                    matched = false;
                    break;
                }
                let sid = self.char_entry_syl[cidx as usize];
                let ok = match *cons {
                    SylConstraint::Full(req_sid) => sid == req_sid,
                    SylConstraint::Initial(req_id, req_dg) => {
                        let (si, dg) = self.syllable_init(sid);
                        si == req_id && (!req_dg || dg)
                    }
                    SylConstraint::Prefix(pref) => self.syllable_str(sid).starts_with(pref),
                };
                if !ok {
                    matched = false;
                    break;
                }
                cidxs[ci] = cidx;
                sids[ci] = sid;
            }
            if !matched {
                continue;
            }
            let mut text = String::with_capacity(slen * 3);
            let mut text_ok = true;
            for &cidx in &cidxs[..slen] {
                if let Some((cs, _, _)) = self.read_char_entry(b, cidx as usize) {
                    text.push_str(cs);
                } else {
                    text_ok = false;
                    break;
                }
            }
            if !text_ok {
                continue;
            }
            let log_wt_q8 = read_u16(b, eoff + 8);
            out.push(PinyinPhrase {
                text,
                log_wt: (log_wt_q8 as f32) / 256.0,
                syl_ids: sids,
                syl_len: slen as u8,
            });
            if out.len() >= limit {
                break;
            }
        }
        out
    }

    /// Check if `(text, constraints)` matches a syllable-verified entry in the compact core phrase table,
    /// returning its `ln(weight + 1)` if attested (zero heap allocation).
    pub fn lookup_matching_phrase_log_wt(
        &self,
        text: &str,
        constraints: &[SylConstraint<'_>],
    ) -> Option<f32> {
        let slen = constraints.len();
        if !(2..=4).contains(&slen) || self.phrase_bucket_count == 0 {
            return None;
        }
        let mut target_chars: [&str; 4] = [""; 4];
        let mut char_cnt = 0usize;
        for (idx, ch) in text.char_indices() {
            if char_cnt >= 4 {
                return None;
            }
            target_chars[char_cnt] = &text[idx..idx + ch.len_utf8()];
            char_cnt += 1;
        }
        if char_cnt != slen {
            return None;
        }
        let mut inits = [0u8; 4];
        for (i, c) in constraints.iter().enumerate() {
            inits[i] = match *c {
                SylConstraint::Full(sid) => self.syllable_init(sid).0,
                SylConstraint::Initial(id, _) => id,
                SylConstraint::Prefix(pref) => base_initial_id(pref),
            };
        }
        let init_key = pack_init_key(&inits[..slen])?;
        let bucket_key = ((slen as u32) << 24) | init_key;
        let (start, count) = self.find_phrase_bucket(bucket_key)?;
        let b = self.buf.as_slice();
        for i in 0..count {
            if start + i >= self.phrase_entry_count {
                break;
            }
            let eoff = self.phrase_entries_off + (start + i) * PHRASE_ENTRY_STRIDE;
            let mut matched = true;
            for (ci, cons) in constraints.iter().enumerate() {
                let cidx = read_u16(b, eoff + ci * 2);
                if cidx == PHRASE_CHAR_NONE || (cidx as usize) >= self.char_entry_syl.len() {
                    matched = false;
                    break;
                }
                let sid = self.char_entry_syl[cidx as usize];
                let syl_ok = match *cons {
                    SylConstraint::Full(req_sid) => sid == req_sid,
                    SylConstraint::Initial(req_id, req_dg) => {
                        let (si, dg) = self.syllable_init(sid);
                        si == req_id && (!req_dg || dg)
                    }
                    SylConstraint::Prefix(pref) => self.syllable_str(sid).starts_with(pref),
                };
                if !syl_ok {
                    matched = false;
                    break;
                }
                let Some((cs, _, _)) = self.read_char_entry(b, cidx as usize) else {
                    matched = false;
                    break;
                };
                if cs != target_chars[ci] {
                    matched = false;
                    break;
                }
            }
            if matched {
                let log_wt_q8 = read_u16(b, eoff + 8);
                return Some((log_wt_q8 as f32) / 256.0);
            }
        }
        None
    }

    fn find_hetero_entry(&self, c1: char, c2: char) -> Option<(u16, u16, u16, u16)> {
        if self.hetero_count == 0 {
            return None;
        }
        let u1 = c1 as u32;
        let u2 = c2 as u32;
        if u1 > 0xFFFF || u2 > 0xFFFF {
            return None;
        }
        let target = (u1 << 16) | u2;
        let b = self.buf.as_slice();
        let mut lo = 0usize;
        let mut hi = self.hetero_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let off = self.hetero_entries_off + mid * HETERO_ENTRY_STRIDE;
            let k = read_u32(b, off);
            match k.cmp(&target) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    let mut pbuf = [0u8; 8];
                    pbuf[..6].copy_from_slice(&b[off + 4..off + 10]);
                    let packed40 = u64::from_le_bytes(pbuf);
                    return Some((
                        decode_hetero10(packed40),
                        decode_hetero10(packed40 >> 10),
                        decode_hetero10(packed40 >> 20),
                        decode_hetero10(packed40 >> 30),
                    ));
                }
            }
        }
        None
    }

    /// True if `(c1, c2)` specifically validates `sid2` for `c2` (so `c2` in `(c1, c2, c3)` won't be
    /// falsely penalized by a cross-word `(c2, c3)` pair such as `淹没人`).
    #[inline]
    pub fn heteronym_c2_licensed(&self, c1: char, c2: char, sid2: u16) -> bool {
        let Some((_, _, s2_a, s2_b)) = self.find_hetero_entry(c1, c2) else {
            return false;
        };
        if s2_a == HETERO_NONE {
            return false;
        }
        let a2 = s2_a & !HETERO_RESCUE_BIT;
        sid2 == a2 || (s2_b != HETERO_NONE && sid2 == s2_b)
    }

    /// Contextual pronunciation log-probability adjustment for transitioning `(c1, sid1_opt) -> (c2, sid2)`.
    /// Penalizes wrong heteronym readings (`moshi -> 没事`, `xingye -> 行业`, `chongyao -> 重要`, `shuijue -> 睡觉`)
    /// while refunding context-free secondary-reading penalties on valid words (`yanmo -> 淹没`, `mudi -> 目的`).
    #[inline]
    pub fn heteronym_pair_delta(
        &self,
        c1: char,
        sid1_opt: Option<u16>,
        c1_already_licensed: bool,
        c2: char,
        sid2: u16,
        c2_char_wt: u32,
    ) -> f32 {
        let Some((s1_a, s1_b, s2_a, s2_b)) = self.find_hetero_entry(c1, c2) else {
            return 0.0;
        };
        let mut delta = 0.0f32;
        if s1_a != HETERO_NONE {
            if let Some(sid1) = sid1_opt {
                let a1 = s1_a & !HETERO_RESCUE_BIT;
                let ok1 = sid1 == a1 || (s1_b != HETERO_NONE && sid1 == s1_b);
                if !ok1 {
                    if !c1_already_licensed {
                        delta -= 8.5;
                    }
                } else if (s1_a & HETERO_RESCUE_BIT) != 0 && sid1 == a1 {
                    delta += 2.2;
                }
            }
        }
        if s2_a != HETERO_NONE {
            let a2 = s2_a & !HETERO_RESCUE_BIT;
            let ok2 = sid2 == a2 || (s2_b != HETERO_NONE && sid2 == s2_b);
            if !ok2 {
                delta -= 8.5;
            } else if (s2_a & HETERO_RESCUE_BIT) != 0 && sid2 == a2 {
                let refund = (-self.char_emission_logp(c2, c2_char_wt)).clamp(0.0, 2.5);
                delta += refund;
            }
        }
        delta
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SylConstraint<'a> {
    Full(u16),
    Initial(u8, bool),
    Prefix(&'a str),
}
