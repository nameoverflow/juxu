use crate::code::{compose_phrase_code, PackedCode, PackedPinyin, Stem2};
use crate::config::InputSchema;
use crate::mmap_buf::{read_u16, read_u32, read_u64, ByteSource};
use std::io;
use std::path::Path;

pub const WBDICT_MAGIC: &[u8; 8] = b"WBDICT03";
const WBDICT_MAGIC_V2: &[u8; 8] = b"WBDICT02";
pub const WBAUX_MAGIC: &[u8; 8] = b"WBAUX003";

pub const ENTRY_FLAG_SINGLE_CHAR: u16 = 1 << 0;
pub const ENTRY_FLAG_EXT_RARE: u16 = 1 << 1;
pub const ENTRY_FLAG_SYMBOL: u16 = 1 << 2;

const HEADER_LEN: usize = 64;
const BUCKET_STRIDE_V2: usize = 12; // packed_code: u32, start: u32, count: u16, reserved: u16
const ENTRY_STRIDE_V2: usize = 12; // str_off: u32, weight: u32, str_len: u16, flags: u16
const CHAR_STRIDE_V2: usize = 20; // cp: u32, stem2: u16, flags: u16, shortest: u32, primary_short: u32, full: u32
const BUCKET_STRIDE_V3: usize = 8; // packed_code: u32, packed_start_count: u32 ((start << 12) | count)
const ENTRY_STRIDE_V3: usize = 8; // packed_str_flags: u32 (str_off:21 | str_len:7 | flags:4), weight: u32
const CHAR_STRIDE_V3: usize = 16; // packed_cp_stem: u32 (cp:21 | stem10:10 | rare:1), shortest: u32, primary_short: u32, full: u32

#[derive(Clone, Copy, Debug)]
pub struct DictEntryRef<'a> {
    pub text: &'a str,
    pub weight: u32,
    pub flags: u16,
    pub code: PackedCode,
}

impl<'a> DictEntryRef<'a> {
    #[inline]
    pub fn is_single_char(&self) -> bool {
        (self.flags & ENTRY_FLAG_SINGLE_CHAR) != 0
    }
    #[inline]
    pub fn is_ext_rare(&self) -> bool {
        (self.flags & ENTRY_FLAG_EXT_RARE) != 0
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CharMeta {
    pub codepoint: u32,
    pub stem2: Stem2,
    pub flags: u16,
    pub shortest_code: PackedCode,
    /// Non-empty only when this character is the #1 primary candidate at `primary_short_code`.
    pub primary_short_code: PackedCode,
    pub full_code: PackedCode,
}

pub struct DictBuildInput {
    pub schema_id: u32,
    pub entries: Vec<(PackedCode, String, u32, u16)>,
    pub char_metas: Vec<CharMeta>,
}

pub fn serialize_wbdict(mut input: DictBuildInput) -> Vec<u8> {
    // Sort entries by (packed_code ASC, is_ext_rare ASC, weight DESC, text_char_len ASC, text ASC)
    input.entries.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| {
                let ra = (a.3 & ENTRY_FLAG_EXT_RARE) != 0;
                let rb = (b.3 & ENTRY_FLAG_EXT_RARE) != 0;
                ra.cmp(&rb)
            })
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.1.chars().count().cmp(&b.1.chars().count()))
            .then_with(|| a.1.cmp(&b.1))
    });
    // Deduplicate identical (code, text) keeping first (highest weight)
    let mut deduped: Vec<(PackedCode, String, u32, u16)> = Vec::with_capacity(input.entries.len());
    for item in input.entries {
        if let Some(last) = deduped.last() {
            if last.0 == item.0 && last.1 == item.1 {
                continue;
            }
        }
        deduped.push(item);
    }

    input.char_metas.sort_by_key(|m| m.codepoint);
    input.char_metas.dedup_by_key(|m| m.codepoint);

    // Compute primary_short_code accurately from sorted buckets!
    let max_code_len = InputSchema::from_id(input.schema_id).max_code_len();
    let mut primary_short_by_cp = std::collections::HashMap::<u32, PackedCode>::new();
    {
        let mut i = 0;
        while i < deduped.len() {
            let code = deduped[i].0;
            let code_len = code.len();
            if code_len < max_code_len {
                // First non-rare entry in this bucket is #1 primary candidate at `code`
                let first = &deduped[i];
                if (first.3 & ENTRY_FLAG_SINGLE_CHAR) != 0
                    && (first.3 & ENTRY_FLAG_EXT_RARE) == 0
                {
                    if let Some(ch) = first.1.chars().next() {
                        let cp = ch as u32;
                        primary_short_by_cp
                            .entry(cp)
                            .and_modify(|prev| {
                                if code.len() < prev.len() {
                                    *prev = code;
                                }
                            })
                            .or_insert(code);
                    }
                }
            }
            while i < deduped.len() && deduped[i].0 == code {
                i += 1;
            }
        }
    }
    for m in &mut input.char_metas {
        if let Some(&psc) = primary_short_by_cp.get(&m.codepoint) {
            if psc.len() < m.full_code.len() {
                m.primary_short_code = psc;
            }
        }
    }

    // Build string pool (deduplicated strings)
    let mut str_offsets = std::collections::HashMap::<String, (u32, u16)>::new();
    let mut string_pool = Vec::<u8>::new();
    let mut max_str_len = 0usize;
    let mut max_flags = 0u16;
    for (_, text, _, flags) in &deduped {
        max_flags |= *flags;
        if !str_offsets.contains_key(text) {
            let off = string_pool.len() as u32;
            let bytes = text.as_bytes();
            let len = bytes.len().min(u16::MAX as usize) as u16;
            max_str_len = max_str_len.max(len as usize);
            string_pool.extend_from_slice(&bytes[..len as usize]);
            str_offsets.insert(text.clone(), (off, len));
        }
    }

    // Omit redundant non-BMP `EXT_RARE` `CharMeta` rows that carry no shortcode (`shortest == full && primary_short == EMPTY`)
    // (their lookup entries in `deduped` remain 100% intact for `enable_rare_charset = true`).
    input.char_metas.retain(|m| {
        !(m.codepoint > 0xFFFF
            && (m.flags & ENTRY_FLAG_EXT_RARE) != 0
            && m.primary_short_code.is_empty()
            && m.shortest_code == m.full_code)
    });

    let mut max_bucket_count = 0usize;
    {
        let mut i = 0usize;
        while i < deduped.len() {
            let c = deduped[i].0;
            let mut j = i + 1;
            while j < deduped.len() && deduped[j].0 == c {
                j += 1;
            }
            max_bucket_count = max_bucket_count.max(j - i);
            i = j;
        }
    }
    let use_v3 = deduped.len() < (1 << 20)
        && string_pool.len() < (1 << 21)
        && max_str_len < (1 << 7)
        && max_bucket_count < (1 << 12)
        && max_flags < (1 << 4);

    // Build buckets + entry table
    let mut buckets_buf = Vec::<u8>::new();
    let mut entries_buf = Vec::<u8>::with_capacity(
        deduped.len() * if use_v3 { ENTRY_STRIDE_V3 } else { ENTRY_STRIDE_V2 },
    );
    let mut bucket_count = 0u32;

    let mut idx = 0usize;
    while idx < deduped.len() {
        let code = deduped[idx].0;
        let start = idx as u32;
        let mut end = idx;
        while end < deduped.len() && deduped[end].0 == code {
            let (_, ref text, weight, flags) = deduped[end];
            let (str_off, str_len) = str_offsets[text];
            if use_v3 {
                let packed_str_flags = (str_off & 0x1F_FFFF)
                    | (((str_len as u32) & 0x7F) << 21)
                    | (((flags as u32) & 0xF) << 28);
                entries_buf.extend_from_slice(&packed_str_flags.to_le_bytes());
                entries_buf.extend_from_slice(&weight.to_le_bytes());
            } else {
                entries_buf.extend_from_slice(&str_off.to_le_bytes());
                entries_buf.extend_from_slice(&weight.to_le_bytes());
                entries_buf.extend_from_slice(&str_len.to_le_bytes());
                entries_buf.extend_from_slice(&flags.to_le_bytes());
            }
            end += 1;
        }
        let count = (end - idx).min(u16::MAX as usize) as u16;
        if use_v3 {
            let packed_start_count = ((start & 0xF_FFFF) << 12) | ((count as u32) & 0xFFF);
            buckets_buf.extend_from_slice(&code.0.to_le_bytes());
            buckets_buf.extend_from_slice(&packed_start_count.to_le_bytes());
        } else {
            buckets_buf.extend_from_slice(&code.0.to_le_bytes());
            buckets_buf.extend_from_slice(&start.to_le_bytes());
            buckets_buf.extend_from_slice(&count.to_le_bytes());
            buckets_buf.extend_from_slice(&0u16.to_le_bytes());
        }
        bucket_count += 1;
        idx = end;
    }

    let mut chars_buf = Vec::<u8>::with_capacity(
        input.char_metas.len() * if use_v3 { CHAR_STRIDE_V3 } else { CHAR_STRIDE_V2 },
    );
    for m in &input.char_metas {
        if use_v3 {
            let stem10 = (((m.stem2.first() as u32) & 0x1F) << 5)
                | ((m.stem2.second() as u32) & 0x1F);
            let rare_bit = u32::from((m.flags & ENTRY_FLAG_EXT_RARE) != 0) << 31;
            let packed_cp_stem = (m.codepoint & 0x1F_FFFF) | (stem10 << 21) | rare_bit;
            chars_buf.extend_from_slice(&packed_cp_stem.to_le_bytes());
            chars_buf.extend_from_slice(&m.shortest_code.0.to_le_bytes());
            chars_buf.extend_from_slice(&m.primary_short_code.0.to_le_bytes());
            chars_buf.extend_from_slice(&m.full_code.0.to_le_bytes());
        } else {
            chars_buf.extend_from_slice(&m.codepoint.to_le_bytes());
            chars_buf.extend_from_slice(&m.stem2.0.to_le_bytes());
            chars_buf.extend_from_slice(&m.flags.to_le_bytes());
            chars_buf.extend_from_slice(&m.shortest_code.0.to_le_bytes());
            chars_buf.extend_from_slice(&m.primary_short_code.0.to_le_bytes());
            chars_buf.extend_from_slice(&m.full_code.0.to_le_bytes());
        }
    }

    let buckets_off = HEADER_LEN as u32;
    let entries_off = buckets_off + buckets_buf.len() as u32;
    let chars_off = entries_off + entries_buf.len() as u32;
    let strings_off = chars_off + chars_buf.len() as u32;
    let total_len = strings_off as usize + string_pool.len();

    let mut out = Vec::<u8>::with_capacity(total_len);
    out.extend_from_slice(if use_v3 { WBDICT_MAGIC } else { WBDICT_MAGIC_V2 });
    out.extend_from_slice(&input.schema_id.to_le_bytes());
    out.extend_from_slice(&bucket_count.to_le_bytes());
    out.extend_from_slice(&(deduped.len() as u32).to_le_bytes());
    out.extend_from_slice(&(input.char_metas.len() as u32).to_le_bytes());
    out.extend_from_slice(&buckets_off.to_le_bytes());
    out.extend_from_slice(&entries_off.to_le_bytes());
    out.extend_from_slice(&chars_off.to_le_bytes());
    out.extend_from_slice(&strings_off.to_le_bytes());
    out.extend_from_slice(&(string_pool.len() as u32).to_le_bytes());
    while out.len() < HEADER_LEN {
        out.push(0);
    }
    out.extend_from_slice(&buckets_buf);
    out.extend_from_slice(&entries_buf);
    out.extend_from_slice(&chars_buf);
    out.extend_from_slice(&string_pool);
    out
}

/// Zero-copy view over a `.wbdict` binary image (`WBDICT03` or `WBDICT02`).
pub struct DictView {
    buf: ByteSource,
    is_v3: bool,
    pub schema_id: u32,
    bucket_count: usize,
    entry_count: usize,
    char_count: usize,
    buckets_off: usize,
    entries_off: usize,
    chars_off: usize,
    strings_off: usize,
    strings_len: usize,
}

impl DictView {
    pub fn open_mmap<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let src = ByteSource::mmap_file(path)?;
        Self::from_source(src)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> io::Result<Self> {
        Self::from_source(ByteSource::from_vec(bytes))
    }

    pub fn from_source(buf: ByteSource) -> io::Result<Self> {
        let b = buf.as_slice();
        let is_v3 = if b.len() >= HEADER_LEN && &b[0..8] == WBDICT_MAGIC {
            true
        } else if b.len() >= HEADER_LEN && &b[0..8] == WBDICT_MAGIC_V2 {
            false
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid WBDICT header",
            ));
        };
        let schema_id = read_u32(b, 8);
        let bucket_count = read_u32(b, 12) as usize;
        let entry_count = read_u32(b, 16) as usize;
        let char_count = read_u32(b, 20) as usize;
        let buckets_off = read_u32(b, 24) as usize;
        let entries_off = read_u32(b, 28) as usize;
        let chars_off = read_u32(b, 32) as usize;
        let strings_off = read_u32(b, 36) as usize;
        let strings_len = read_u32(b, 40) as usize;

        let (b_stride, e_stride, c_stride) = if is_v3 {
            (BUCKET_STRIDE_V3, ENTRY_STRIDE_V3, CHAR_STRIDE_V3)
        } else {
            (BUCKET_STRIDE_V2, ENTRY_STRIDE_V2, CHAR_STRIDE_V2)
        };
        if buckets_off + bucket_count * b_stride > b.len()
            || entries_off + entry_count * e_stride > b.len()
            || chars_off + char_count * c_stride > b.len()
            || strings_off + strings_len > b.len()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated WBDICT section",
            ));
        }

        Ok(Self {
            buf,
            is_v3,
            schema_id,
            bucket_count,
            entry_count,
            char_count,
            buckets_off,
            entries_off,
            chars_off,
            strings_off,
            strings_len,
        })
    }

    #[inline]
    fn bucket_at(&self, i: usize) -> (PackedCode, usize, usize) {
        let b = self.buf.as_slice();
        if self.is_v3 {
            let off = self.buckets_off + i * BUCKET_STRIDE_V3;
            let code = PackedCode(read_u32(b, off));
            let packed = read_u32(b, off + 4);
            (code, (packed >> 12) as usize, (packed & 0xFFF) as usize)
        } else {
            let off = self.buckets_off + i * BUCKET_STRIDE_V2;
            let code = PackedCode(read_u32(b, off));
            let start = read_u32(b, off + 4) as usize;
            let count = read_u16(b, off + 8) as usize;
            (code, start, count)
        }
    }

    #[inline]
    fn entry_at(&self, idx: usize, code: PackedCode) -> Option<DictEntryRef<'_>> {
        if idx >= self.entry_count {
            return None;
        }
        let b = self.buf.as_slice();
        let (str_off, str_len, weight, flags) = if self.is_v3 {
            let off = self.entries_off + idx * ENTRY_STRIDE_V3;
            let p = read_u32(b, off);
            (
                (p & 0x1F_FFFF) as usize,
                ((p >> 21) & 0x7F) as usize,
                read_u32(b, off + 4),
                ((p >> 28) & 0xF) as u16,
            )
        } else {
            let off = self.entries_off + idx * ENTRY_STRIDE_V2;
            (
                read_u32(b, off) as usize,
                read_u16(b, off + 8) as usize,
                read_u32(b, off + 4),
                read_u16(b, off + 10),
            )
        };
        if str_off + str_len > self.strings_len {
            return None;
        }
        let s_start = self.strings_off + str_off;
        let text = core::str::from_utf8(&b[s_start..s_start + str_len]).ok()?;
        Some(DictEntryRef {
            text,
            weight,
            flags,
            code,
        })
    }

    fn lower_bound_bucket(&self, target: PackedCode) -> usize {
        let mut lo = 0usize;
        let mut hi = self.bucket_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let (c, _, _) = self.bucket_at(mid);
            if c < target {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    pub fn lookup_exact(&self, code: PackedCode, include_rare: bool) -> Vec<DictEntryRef<'_>> {
        let idx = self.lower_bound_bucket(code);
        if idx >= self.bucket_count {
            return Vec::new();
        }
        let (c, start, count) = self.bucket_at(idx);
        if c != code {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(count);
        for i in start..start + count {
            if let Some(e) = self.entry_at(i, code) {
                if !include_rare && e.is_ext_rare() {
                    continue;
                }
                out.push(e);
            }
        }
        out
    }

    pub fn has_exact(&self, code: PackedCode, include_rare: bool) -> bool {
        let idx = self.lower_bound_bucket(code);
        if idx >= self.bucket_count {
            return false;
        }
        let (c, start, count) = self.bucket_at(idx);
        if c != code {
            return false;
        }
        if include_rare {
            return count > 0;
        }
        for i in start..start + count {
            if let Some(e) = self.entry_at(i, code) {
                if !e.is_ext_rare() {
                    return true;
                }
            }
        }
        false
    }

    /// Return a 26-bit mask (`bit i` = `'a' + i`) of letters that yield at least one valid code
    /// starting with `[prefix_bytes..., letter]` (`1 <= prefix_bytes.len() <= 4`).
    pub fn valid_next_letters_mask(&self, prefix_bytes: &[u8], include_rare: bool) -> u32 {
        let schema = InputSchema::from_id(self.schema_id);
        let max_len = schema.max_code_len();
        if prefix_bytes.is_empty() || prefix_bytes.len() >= max_len {
            return 0x03FF_FFFF;
        }
        let mut buf = [0u8; 5];
        let n = prefix_bytes.len();
        buf[..n].copy_from_slice(prefix_bytes);
        let letter_limit = if !schema.allows_z_code() { 25u8 } else { 26u8 };
        let mut mask = 0u32;
        for idx in 0..letter_limit {
            buf[n] = b'a' + idx;
            let Some(ext) = PackedCode::from_bytes(&buf[..=n]) else {
                continue;
            };
            let (lo, hi) = ext.prefix_range();
            let mut b_idx = self.lower_bound_bucket(lo);
            while b_idx < self.bucket_count {
                let (c, start, count) = self.bucket_at(b_idx);
                if c > hi {
                    break;
                }
                if include_rare {
                    if count > 0 {
                        mask |= 1u32 << idx;
                        break;
                    }
                } else {
                    let mut found = false;
                    for i in start..start + count {
                        if let Some(e) = self.entry_at(i, c) {
                            if !e.is_ext_rare() {
                                found = true;
                                break;
                            }
                        }
                    }
                    if found {
                        mask |= 1u32 << idx;
                        break;
                    }
                }
                b_idx += 1;
            }
        }
        mask
    }

    /// Return the highest-weight non-rare entry for each next letter `'a'..='z'` (`0..26`)
    /// continuing `prefix_bytes` (`1 <= len < max_len`). Used for sub-microsecond neural gutter priors.
    pub fn best_continuation_per_letter(
        &self,
        prefix_bytes: &[u8],
        include_rare: bool,
    ) -> [Option<DictEntryRef<'_>>; 26] {
        let mut out: [Option<DictEntryRef<'_>>; 26] = [None; 26];
        let max_len = InputSchema::from_id(self.schema_id).max_code_len();
        if prefix_bytes.is_empty() || prefix_bytes.len() >= max_len {
            return out;
        }
        let mut buf = [0u8; 5];
        let n = prefix_bytes.len();
        buf[..n].copy_from_slice(prefix_bytes);

        for idx in 0..26u8 {
            buf[n] = b'a' + idx;
            let Some(ext) = PackedCode::from_bytes(&buf[..=n]) else {
                continue;
            };
            let (lo, hi) = ext.prefix_range();
            let mut b_idx = self.lower_bound_bucket(lo);
            let mut best_e: Option<DictEntryRef<'_>> = None;
            let mut scanned_buckets = 0usize;
            while b_idx < self.bucket_count && scanned_buckets < 24 {
                let (c, start, count) = self.bucket_at(b_idx);
                if c > hi {
                    break;
                }
                scanned_buckets += 1;
                for i in start..(start + count).min(start + 2) {
                    if let Some(e) = self.entry_at(i, c) {
                        if !include_rare && e.is_ext_rare() {
                            continue;
                        }
                        let replace = match best_e {
                            None => true,
                            Some(prev) => {
                                if c == ext && prev.code != ext {
                                    true
                                } else if c != ext && prev.code == ext {
                                    false
                                } else {
                                    e.weight > prev.weight
                                }
                            }
                        };
                        if replace {
                            best_e = Some(e);
                        }
                        break;
                    }
                }
                if best_e.is_some_and(|x| x.code == ext) && n == 3 {
                    break;
                }
                b_idx += 1;
            }
            out[idx as usize] = best_e;
        }
        out
    }

    /// Prefix completion lookup for codes strictly longer than `prefix`.
    pub fn lookup_longer_prefix(
        &self,
        prefix: PackedCode,
        limit: usize,
        include_rare: bool,
    ) -> Vec<DictEntryRef<'_>> {
        let (lo_code, hi_code) = prefix.prefix_range();
        let mut b_idx = self.lower_bound_bucket(lo_code);
        let mut collected = Vec::new();
        while b_idx < self.bucket_count {
            let (c, start, count) = self.bucket_at(b_idx);
            if c > hi_code {
                break;
            }
            if c != prefix {
                for i in start..start + count {
                    if let Some(e) = self.entry_at(i, c) {
                        if !include_rare && e.is_ext_rare() {
                            continue;
                        }
                        collected.push(e);
                        if collected.len() >= limit * 8 {
                            break;
                        }
                    }
                }
            }
            if collected.len() >= limit * 8 {
                break;
            }
            b_idx += 1;
        }
        collected.sort_by(|a, b| {
            a.code
                .len()
                .cmp(&b.code.len())
                .then_with(|| b.weight.cmp(&a.weight))
        });
        collected.truncate(limit);
        collected
    }

    /// Fast O(log B) check for whether `prefix` has at least one exact or longer-prefix match.
    pub fn has_prefix_or_exact(&self, prefix: PackedCode, include_rare: bool) -> bool {
        let (lo_code, hi_code) = prefix.prefix_range();
        let mut b_idx = self.lower_bound_bucket(lo_code);
        while b_idx < self.bucket_count {
            let (c, start, count) = self.bucket_at(b_idx);
            if c > hi_code {
                break;
            }
            if include_rare {
                if count > 0 {
                    return true;
                }
            } else {
                for i in start..start + count {
                    if let Some(e) = self.entry_at(i, c) {
                        if !e.is_ext_rare() {
                            return true;
                        }
                    }
                }
            }
            b_idx += 1;
        }
        false
    }

    /// Scan all buckets under `prefix` (including exact matches) and return the top `limit`
    /// highest-weight entries across the entire prefix range (never truncated alphabetically).
    pub fn top_prefix_entries(
        &self,
        prefix: PackedCode,
        limit: usize,
        include_rare: bool,
    ) -> Vec<DictEntryRef<'_>> {
        if limit == 0 {
            return Vec::new();
        }
        let (lo_code, hi_code) = prefix.prefix_range();
        let mut b_idx = self.lower_bound_bucket(lo_code);
        let mut best: Vec<DictEntryRef<'_>> = Vec::with_capacity(limit + 1);
        let mut floor_wt: u32 = 0;

        while b_idx < self.bucket_count {
            let (c, start, count) = self.bucket_at(b_idx);
            if c > hi_code {
                break;
            }
            let scan_end = (start + count).min(start + 2);
            for i in start..scan_end {
                if let Some(e) = self.entry_at(i, c) {
                    if !include_rare && e.is_ext_rare() {
                        continue;
                    }
                    if best.len() == limit && e.weight <= floor_wt {
                        continue;
                    }
                    let pos = best.partition_point(|x| x.weight > e.weight);
                    if pos < limit {
                        best.insert(pos, e);
                        if best.len() > limit {
                            best.pop();
                        }
                        if best.len() == limit {
                            floor_wt = best.last().unwrap().weight;
                        }
                    }
                }
            }
            b_idx += 1;
        }
        best
    }

    pub fn char_meta(&self, ch: char) -> Option<CharMeta> {
        let target = ch as u32;
        let b = self.buf.as_slice();
        let mut lo = 0usize;
        let mut hi = self.char_count;
        if self.is_v3 {
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let off = self.chars_off + mid * CHAR_STRIDE_V3;
                let packed = read_u32(b, off);
                let cp = packed & 0x1F_FFFF;
                if cp < target {
                    lo = mid + 1;
                } else if cp > target {
                    hi = mid;
                } else {
                    let s10 = (packed >> 21) & 0x3FF;
                    let stem2 = Stem2((((s10 >> 5) & 0x1F) as u16) << 8 | ((s10 & 0x1F) as u16));
                    let flags = if (packed >> 31) != 0 {
                        ENTRY_FLAG_EXT_RARE
                    } else {
                        0
                    };
                    return Some(CharMeta {
                        codepoint: cp,
                        stem2,
                        flags,
                        shortest_code: PackedCode(read_u32(b, off + 4)),
                        primary_short_code: PackedCode(read_u32(b, off + 8)),
                        full_code: PackedCode(read_u32(b, off + 12)),
                    });
                }
            }
        } else {
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let off = self.chars_off + mid * CHAR_STRIDE_V2;
                let cp = read_u32(b, off);
                if cp < target {
                    lo = mid + 1;
                } else if cp > target {
                    hi = mid;
                } else {
                    return Some(CharMeta {
                        codepoint: cp,
                        stem2: Stem2(read_u16(b, off + 4)),
                        flags: read_u16(b, off + 6),
                        shortest_code: PackedCode(read_u32(b, off + 8)),
                        primary_short_code: PackedCode(read_u32(b, off + 12)),
                        full_code: PackedCode(read_u32(b, off + 16)),
                    });
                }
            }
        }
        None
    }

    /// Dynamically compute the 4-letter Wubi code for any Chinese phrase (2+ chars)
    /// under this schema (`86` / `98` / `06`), enabling cross-schema user phrases!
    pub fn encode_phrase(&self, text: &str) -> Option<PackedCode> {
        let mut stems = Vec::with_capacity(4);
        for ch in text.chars() {
            let meta = self.char_meta(ch)?;
            if !meta.stem2.is_valid() {
                return None;
            }
            stems.push(meta.stem2);
        }
        compose_phrase_code(&stems)
    }

    /// Check whether `text` (2+ chars) exists as a multi-char entry in this dictionary, returning its weight.
    pub fn contains_exact_word(&self, text: &str) -> Option<u32> {
        let code = self.encode_phrase(text)?;
        for e in self.lookup_exact(code, false) {
            if e.text == text {
                return Some(e.weight);
            }
        }
        None
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        (self.bucket_count, self.entry_count, self.char_count)
    }
}

// ============================================================================
// Shared Auxiliary Index (.wbaux `WBAUX003`): Pinyin Reverse Lookup + LM + Neural
// ============================================================================

const AUX_HEADER_LEN: usize = 64;
const PY_BUCKET_STRIDE: usize = 12; // py_packed: u32, start: u32, count: u16, pad: u16
const PY_ITEM_STRIDE: usize = 12; // cp: u32, weight: u32, py_str_off: u16, py_str_len: u16
const BIGRAM_STRIDE: usize = 10; // pair_key: u64, log_boost_q8: u16
const UNIGRAM_STRIDE: usize = 6; // cp: u32, log_prior_q8: u16
const TRIGRAM_STRIDE: usize = 8; // packed u64: (c0<<48)|(c1<<32)|(c2<<16)|boost_q8

/// Pack a BMP character trigram + Q8 boost into a single 64-bit integer.
/// Returns `None` if any character is outside the Basic Multilingual Plane (`> 0xFFFF`).
#[inline]
pub fn pack_trigram(c0: char, c1: char, c2: char, boost_q8: u16) -> Option<u64> {
    let u0 = c0 as u32;
    let u1 = c1 as u32;
    let u2 = c2 as u32;
    if u0 > 0xFFFF || u1 > 0xFFFF || u2 > 0xFFFF {
        return None;
    }
    Some(((u0 as u64) << 48) | ((u1 as u64) << 32) | ((u2 as u64) << 16) | (boost_q8 as u64))
}

#[derive(Default)]
pub struct AuxBuildInput {
    /// `(pinyin_ascii, pinyin_display, codepoint, weight)`
    pub pinyin_items: Vec<(PackedPinyin, String, u32, u32)>,
    /// `(((prev_cp as u64) << 32) | (next_cp as u64), log_boost_q8)`
    pub bigrams: Vec<(u64, u16)>,
    /// `(codepoint, shifted_log_prior_q8)` where `score = (ln P(c) + 14.0).clamp(0.0, 14.0)`
    pub unigrams: Vec<(u32, u16)>,
    /// Packed BMP trigrams from `pack_trigram(c0, c1, c2, residual_boost_q8)`
    pub trigrams: Vec<u64>,
    /// Optional embedded `WBLM0002` / `WBLM0003` (`Micro-RWKV-7` INT8 weights) zero-copy section.
    pub neural_blob: Vec<u8>,
}

pub fn serialize_wbaux(mut input: AuxBuildInput) -> Vec<u8> {
    input
        .pinyin_items
        .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.3.cmp(&a.3)).then_with(|| a.2.cmp(&b.2)));
    input.pinyin_items.dedup_by(|a, b| a.0 == b.0 && a.2 == b.2);

    // Sort bigrams by key ASC, keeping highest boost on duplicate keys
    input
        .bigrams
        .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    input.bigrams.dedup_by_key(|x| x.0);

    // Sort unigrams by codepoint ASC, keeping highest prior on duplicate codepoints
    input
        .unigrams
        .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    input.unigrams.dedup_by_key(|x| x.0);

    // Sort trigrams by 48-bit key ASC, keeping highest boost on duplicate keys
    input.trigrams.sort_by(|a, b| {
        (a >> 16)
            .cmp(&(b >> 16))
            .then_with(|| (b & 0xFFFF).cmp(&(a & 0xFFFF)))
    });
    input.trigrams.dedup_by_key(|x| *x >> 16);

    let mut py_str_map = std::collections::HashMap::<String, (u16, u16)>::new();
    let mut py_pool = Vec::<u8>::new();
    for (_, disp, _, _) in &input.pinyin_items {
        if !py_str_map.contains_key(disp) && py_pool.len() + disp.len() < u16::MAX as usize {
            let off = py_pool.len() as u16;
            let len = disp.len() as u16;
            py_pool.extend_from_slice(disp.as_bytes());
            py_str_map.insert(disp.clone(), (off, len));
        }
    }

    let mut py_buckets = Vec::<u8>::new();
    let mut py_items = Vec::<u8>::with_capacity(input.pinyin_items.len() * PY_ITEM_STRIDE);
    let mut py_bucket_count = 0u32;

    let mut idx = 0usize;
    while idx < input.pinyin_items.len() {
        let py = input.pinyin_items[idx].0;
        let start = idx as u32;
        let mut end = idx;
        while end < input.pinyin_items.len() && input.pinyin_items[end].0 == py {
            let (_, ref disp, cp, wt) = input.pinyin_items[end];
            let (so, sl) = py_str_map.get(disp).copied().unwrap_or((0, 0));
            py_items.extend_from_slice(&cp.to_le_bytes());
            py_items.extend_from_slice(&wt.to_le_bytes());
            py_items.extend_from_slice(&so.to_le_bytes());
            py_items.extend_from_slice(&sl.to_le_bytes());
            end += 1;
        }
        let count = (end - idx).min(u16::MAX as usize) as u16;
        py_buckets.extend_from_slice(&py.0.to_le_bytes());
        py_buckets.extend_from_slice(&start.to_le_bytes());
        py_buckets.extend_from_slice(&count.to_le_bytes());
        py_buckets.extend_from_slice(&0u16.to_le_bytes());
        py_bucket_count += 1;
        idx = end;
    }

    let mut bg_buf = Vec::<u8>::with_capacity(input.bigrams.len() * BIGRAM_STRIDE);
    for (pair, boost) in &input.bigrams {
        bg_buf.extend_from_slice(&pair.to_le_bytes());
        bg_buf.extend_from_slice(&boost.to_le_bytes());
    }

    let mut uni_buf = Vec::<u8>::with_capacity(input.unigrams.len() * UNIGRAM_STRIDE);
    for (cp, prior) in &input.unigrams {
        uni_buf.extend_from_slice(&cp.to_le_bytes());
        uni_buf.extend_from_slice(&prior.to_le_bytes());
    }

    let mut tri_buf = Vec::<u8>::with_capacity(input.trigrams.len() * TRIGRAM_STRIDE);
    for packed in &input.trigrams {
        tri_buf.extend_from_slice(&packed.to_le_bytes());
    }

    let py_buckets_off = AUX_HEADER_LEN as u32;
    let py_items_off = py_buckets_off + py_buckets.len() as u32;
    let bg_off = py_items_off + py_items.len() as u32;
    let pool_off = bg_off + bg_buf.len() as u32;
    let unigram_off = pool_off + py_pool.len() as u32;
    let trigram_off = unigram_off + uni_buf.len() as u32;
    let after_tri = trigram_off as usize + tri_buf.len();

    let has_neural = !input.neural_blob.is_empty();
    let neural_off = if has_neural {
        ((after_tri + 63) & !63) as u32 // 64-byte cache-line align neural section
    } else {
        0u32
    };
    let neural_len = input.neural_blob.len() as u32;
    let total_cap = if has_neural {
        neural_off as usize + input.neural_blob.len()
    } else {
        after_tri
    };

    let mut out = Vec::<u8>::with_capacity(total_cap);
    out.extend_from_slice(WBAUX_MAGIC);
    out.extend_from_slice(&py_bucket_count.to_le_bytes());
    out.extend_from_slice(&(input.pinyin_items.len() as u32).to_le_bytes());
    out.extend_from_slice(&(input.bigrams.len() as u32).to_le_bytes());
    out.extend_from_slice(&py_buckets_off.to_le_bytes());
    out.extend_from_slice(&py_items_off.to_le_bytes());
    out.extend_from_slice(&bg_off.to_le_bytes());
    out.extend_from_slice(&pool_off.to_le_bytes());
    out.extend_from_slice(&(py_pool.len() as u32).to_le_bytes());
    out.extend_from_slice(&(input.unigrams.len() as u32).to_le_bytes());
    out.extend_from_slice(&unigram_off.to_le_bytes());
    out.extend_from_slice(&(input.trigrams.len() as u32).to_le_bytes());
    out.extend_from_slice(&trigram_off.to_le_bytes());
    out.extend_from_slice(&neural_off.to_le_bytes());
    out.extend_from_slice(&neural_len.to_le_bytes());
    debug_assert_eq!(out.len(), AUX_HEADER_LEN);
    out.extend_from_slice(&py_buckets);
    out.extend_from_slice(&py_items);
    out.extend_from_slice(&bg_buf);
    out.extend_from_slice(&py_pool);
    out.extend_from_slice(&uni_buf);
    out.extend_from_slice(&tri_buf);
    if has_neural {
        while out.len() < neural_off as usize {
            out.push(0);
        }
        out.extend_from_slice(&input.neural_blob);
    }
    out
}

pub struct AuxView {
    buf: ByteSource,
    py_bucket_count: usize,
    py_item_count: usize,
    bigram_count: usize,
    unigram_count: usize,
    trigram_count: usize,
    py_buckets_off: usize,
    py_items_off: usize,
    bg_off: usize,
    pool_off: usize,
    pool_len: usize,
    unigram_off: usize,
    trigram_off: usize,
    neural_off: usize,
    neural_len: usize,
    neural_offs: Option<crate::neural::NeuralOffsets>,
}

impl AuxView {
    pub fn open_mmap<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Self::from_source(ByteSource::mmap_file(path)?)
    }

    #[inline]
    pub fn from_mmap<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Self::open_mmap(path)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> io::Result<Self> {
        Self::from_source(ByteSource::from_vec(bytes))
    }

    pub fn from_source(buf: ByteSource) -> io::Result<Self> {
        let b = buf.as_slice();
        if b.len() < AUX_HEADER_LEN || &b[0..8] != WBAUX_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid WBAUX003 header",
            ));
        }
        let py_bucket_count = read_u32(b, 8) as usize;
        let py_item_count = read_u32(b, 12) as usize;
        let bigram_count = read_u32(b, 16) as usize;
        let py_buckets_off = read_u32(b, 20) as usize;
        let py_items_off = read_u32(b, 24) as usize;
        let bg_off = read_u32(b, 28) as usize;
        let pool_off = read_u32(b, 32) as usize;
        let pool_len = read_u32(b, 36) as usize;
        let unigram_count = read_u32(b, 40) as usize;
        let unigram_off = read_u32(b, 44) as usize;
        let trigram_count = read_u32(b, 48) as usize;
        let trigram_off = read_u32(b, 52) as usize;
        let neural_off = read_u32(b, 56) as usize;
        let neural_len = read_u32(b, 60) as usize;

        if py_buckets_off + py_bucket_count * PY_BUCKET_STRIDE > b.len()
            || py_items_off + py_item_count * PY_ITEM_STRIDE > b.len()
            || bg_off + bigram_count * BIGRAM_STRIDE > b.len()
            || pool_off + pool_len > b.len()
            || unigram_off + unigram_count * UNIGRAM_STRIDE > b.len()
            || trigram_off + trigram_count * TRIGRAM_STRIDE > b.len()
            || neural_off + neural_len > b.len()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated WBAUX003 section",
            ));
        }

        let neural_offs = if neural_len > 0 {
            Some(crate::neural::NeuralOffsets::parse(
                &b[neural_off..neural_off + neural_len],
            )?)
        } else {
            None
        };

        Ok(Self {
            buf,
            py_bucket_count,
            py_item_count,
            bigram_count,
            unigram_count,
            trigram_count,
            py_buckets_off,
            py_items_off,
            bg_off,
            pool_off,
            pool_len,
            unigram_off,
            trigram_off,
            neural_off,
            neural_len,
            neural_offs,
        })
    }

    /// Export `(unigrams, bigrams, trigrams, raw_neural_blob)` so `build-dicts` can rebuild
    /// reverse-Pinyin tables without wiping an already embedded corpus N-gram & `WBLM` neural LM.
    pub fn export_lm_sections(&self) -> (Vec<(u32, u16)>, Vec<(u64, u16)>, Vec<u64>, Vec<u8>) {
        let b = self.buf.as_slice();
        let mut unigrams = Vec::with_capacity(self.unigram_count);
        for i in 0..self.unigram_count {
            let off = self.unigram_off + i * UNIGRAM_STRIDE;
            unigrams.push((read_u32(b, off), read_u16(b, off + 4)));
        }
        let mut bigrams = Vec::with_capacity(self.bigram_count);
        for i in 0..self.bigram_count {
            let off = self.bg_off + i * BIGRAM_STRIDE;
            bigrams.push((read_u64(b, off), read_u16(b, off + 8)));
        }
        let mut trigrams = Vec::with_capacity(self.trigram_count);
        for i in 0..self.trigram_count {
            let off = self.trigram_off + i * TRIGRAM_STRIDE;
            trigrams.push(read_u64(b, off));
        }
        let neural_blob = if self.neural_len > 0 {
            b[self.neural_off..self.neural_off + self.neural_len].to_vec()
        } else {
            Vec::new()
        };
        (unigrams, bigrams, trigrams, neural_blob)
    }

    #[inline]
    pub fn has_neural(&self) -> bool {
        self.neural_offs.is_some()
    }

    #[inline]
    pub fn neural_view(&self) -> Option<crate::neural::NeuralView<'_>> {
        let offs = self.neural_offs.as_ref()?;
        let b = self.buf.as_slice();
        Some(crate::neural::NeuralView {
            blob: &b[self.neural_off..self.neural_off + self.neural_len],
            offs,
        })
    }

    #[inline]
    pub fn has_unigrams(&self) -> bool {
        self.unigram_count > 0
    }

    #[inline]
    pub fn stats(&self) -> (usize, usize, usize, usize) {
        (
            self.py_item_count,
            self.unigram_count,
            self.bigram_count,
            self.trigram_count,
        )
    }

    /// Look up characters by Pinyin prefix (1..=6 ASCII letters), returning `(char, weight, pinyin_str, exact_match)`.
    pub fn lookup_pinyin(&self, query: &[u8], limit: usize) -> Vec<(char, u32, &str, bool)> {
        let Some((lo_py, hi_py)) = PackedPinyin::prefix_range(query) else {
            return Vec::new();
        };
        let b = self.buf.as_slice();
        let mut lo = 0usize;
        let mut hi = self.py_bucket_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let py = PackedPinyin(read_u32(b, self.py_buckets_off + mid * PY_BUCKET_STRIDE));
            if py < lo_py {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::<u32>::new();
        let mut idx = lo;
        while idx < self.py_bucket_count {
            let boff = self.py_buckets_off + idx * PY_BUCKET_STRIDE;
            let py = PackedPinyin(read_u32(b, boff));
            if py > hi_py {
                break;
            }
            let exact = py == lo_py;
            let start = read_u32(b, boff + 4) as usize;
            let count = read_u16(b, boff + 8) as usize;
            for i in start..(start + count).min(self.py_item_count) {
                let ioff = self.py_items_off + i * PY_ITEM_STRIDE;
                let cp = read_u32(b, ioff);
                if !seen.insert(cp) {
                    continue;
                }
                let wt = read_u32(b, ioff + 4);
                let so = read_u16(b, ioff + 8) as usize;
                let sl = read_u16(b, ioff + 10) as usize;
                let py_str = if so + sl <= self.pool_len {
                    core::str::from_utf8(&b[self.pool_off + so..self.pool_off + so + sl])
                        .unwrap_or("")
                } else {
                    ""
                };
                if let Some(ch) = char::from_u32(cp) {
                    results.push((ch, wt, py_str, exact));
                }
                if results.len() >= limit * 6 {
                    break;
                }
            }
            if results.len() >= limit * 6 {
                break;
            }
            idx += 1;
        }
        results.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| b.1.cmp(&a.1)));
        results.truncate(limit);
        results
    }

    /// Returns modern corpus character unigram log-prior (`0.0 .. ~14.0`), or `None` if absent.
    #[inline]
    pub fn unigram_prior(&self, ch: char) -> Option<f32> {
        if self.unigram_count == 0 {
            return None;
        }
        let target = ch as u32;
        let b = self.buf.as_slice();
        let mut lo = 0usize;
        let mut hi = self.unigram_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let off = self.unigram_off + mid * UNIGRAM_STRIDE;
            let cp = read_u32(b, off);
            if cp < target {
                lo = mid + 1;
            } else if cp > target {
                hi = mid;
            } else {
                let q8 = read_u16(b, off + 4);
                return Some((q8 as f32) / 256.0);
            }
        }
        None
    }

    /// Returns corpus transition log-boost (in nats, `0.0 .. ~5.5`) from `prev_char` to `next_char`.
    #[inline]
    pub fn bigram_boost(&self, prev_char: char, next_char: char) -> f32 {
        let key = ((prev_char as u64) << 32) | (next_char as u64);
        let b = self.buf.as_slice();
        let mut lo = 0usize;
        let mut hi = self.bigram_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let off = self.bg_off + mid * BIGRAM_STRIDE;
            let k = read_u64(b, off);
            if k < key {
                lo = mid + 1;
            } else if k > key {
                hi = mid;
            } else {
                let q8 = read_u16(b, off + 8);
                return (q8 as f32) / 256.0;
            }
        }
        0.0
    }

    /// Returns residual corpus trigram log-boost (in nats, over the bigram) for `(prev2, prev1) -> next_char`.
    #[inline]
    pub fn trigram_boost(&self, prev2: char, prev1: char, next_char: char) -> f32 {
        if self.trigram_count == 0 {
            return 0.0;
        }
        let u0 = prev2 as u32;
        let u1 = prev1 as u32;
        let u2 = next_char as u32;
        if u0 > 0xFFFF || u1 > 0xFFFF || u2 > 0xFFFF {
            return 0.0;
        }
        let target_key48 = ((u0 as u64) << 32) | ((u1 as u64) << 16) | (u2 as u64);
        let b = self.buf.as_slice();
        let mut lo = 0usize;
        let mut hi = self.trigram_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let off = self.trigram_off + mid * TRIGRAM_STRIDE;
            let packed = read_u64(b, off);
            let k = packed >> 16;
            if k < target_key48 {
                lo = mid + 1;
            } else if k > target_key48 {
                hi = mid;
            } else {
                let q8 = (packed & 0xFFFF) as u16;
                return (q8 as f32) / 256.0;
            }
        }
        0.0
    }

    /// Returns all bigrams `(next_char, pmi_boost)` starting with `prev_char` via O(log N) binary search.
    pub fn bigrams_after(&self, prev_char: char) -> Vec<(char, f32)> {
        if self.bigram_count == 0 {
            return Vec::new();
        }
        let target_hi = prev_char as u64;
        let lo_key = target_hi << 32;
        let b = self.buf.as_slice();
        let mut lo = 0usize;
        let mut hi = self.bigram_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let k = read_u64(b, self.bg_off + mid * BIGRAM_STRIDE);
            if k < lo_key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut out = Vec::new();
        let mut idx = lo;
        while idx < self.bigram_count {
            let off = self.bg_off + idx * BIGRAM_STRIDE;
            let k = read_u64(b, off);
            if (k >> 32) != target_hi {
                break;
            }
            if let Some(next_ch) = char::from_u32((k & 0xFFFF_FFFF) as u32) {
                let q8 = read_u16(b, off + 8);
                out.push((next_ch, (q8 as f32) / 256.0));
            }
            idx += 1;
        }
        out
    }

    /// Returns all trigrams `(next_char, residual_boost)` following `(prev2, prev1)` via O(log N) binary search.
    pub fn trigrams_after(&self, prev2: char, prev1: char) -> Vec<(char, f32)> {
        if self.trigram_count == 0 {
            return Vec::new();
        }
        let u0 = prev2 as u32;
        let u1 = prev1 as u32;
        if u0 > 0xFFFF || u1 > 0xFFFF {
            return Vec::new();
        }
        let prefix32 = ((u0 as u64) << 16) | (u1 as u64);
        let lo_key48 = prefix32 << 16;
        let b = self.buf.as_slice();
        let mut lo = 0usize;
        let mut hi = self.trigram_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let packed = read_u64(b, self.trigram_off + mid * TRIGRAM_STRIDE);
            if (packed >> 16) < lo_key48 {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut out = Vec::new();
        let mut idx = lo;
        while idx < self.trigram_count {
            let off = self.trigram_off + idx * TRIGRAM_STRIDE;
            let packed = read_u64(b, off);
            let k48 = packed >> 16;
            if (k48 >> 16) != prefix32 {
                break;
            }
            if let Some(next_ch) = char::from_u32((k48 & 0xFFFF) as u32) {
                let q8 = (packed & 0xFFFF) as u16;
                out.push((next_ch, (q8 as f32) / 256.0));
            }
            idx += 1;
        }
        out
    }
}
