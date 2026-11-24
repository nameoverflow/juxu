use crate::mmap_buf::ByteSource;
use crate::typo::letter_to_t9_digit;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::OnceLock;

pub const ENDICT_MAGIC: &[u8; 8] = b"ENDICT01";
const HEADER_LEN: usize = 32;
const ENTRY_LEN: usize = 16;

pub const EN_FLAG_TECH: u16 = 1 << 0;
pub const EN_FLAG_HIGH_FREQ: u16 = 1 << 1;
pub const EN_FLAG_ABBR: u16 = 1 << 2;

/// Pack a 2..=12 T9 digit sequence (`'2'..='9'`) into a compact `u64` key (`0` allocations).
#[inline]
pub fn pack_t9_digits_slice(digits: &[u8]) -> Option<u64> {
    if !(2..=12).contains(&digits.len()) {
        return None;
    }
    let mut acc: u64 = 1;
    for &b in digits {
        if !(b'2'..=b'9').contains(&b) {
            return None;
        }
        acc = (acc << 4) | u64::from(b - b'0');
    }
    Some(acc)
}

/// Pack an ASCII alphabetic word (`2..=12` letters) into its corresponding T9 `u64` key.
#[inline]
pub fn pack_word_to_t9_key(word: &str) -> Option<u64> {
    let bytes = word.as_bytes();
    if !(2..=12).contains(&bytes.len()) {
        return None;
    }
    let mut acc: u64 = 1;
    for &b in bytes {
        let d = letter_to_t9_digit(b.to_ascii_lowercase())?;
        acc = (acc << 4) | u64::from(d - b'0');
    }
    Some(acc)
}

#[derive(Clone, Copy, Debug)]
pub struct EnEntry<'a> {
    pub key: &'a str,
    pub display: &'a str,
    pub weight: u16,
    pub flags: u16,
}

pub struct EnDictView {
    buf: ByteSource,
    entry_count: usize,
    entries_off: usize,
    str_off: usize,
    t9_index: OnceLock<HashMap<u64, Vec<u32>>>,
}

#[inline]
fn read_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

#[inline]
fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

impl EnDictView {
    pub fn open_mmap<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let buf = ByteSource::mmap_file(path)?;
        Self::parse(buf)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> io::Result<Self> {
        Self::parse(ByteSource::from_vec(bytes))
    }

    /// Built-in compact fallback dictionary so mixed-language decoding and unit tests work
    /// even if `english.endict` is not yet loaded on disk.
    pub fn builtin_default() -> Self {
        let mut raw: Vec<(String, String, u16, u16)> = BUILTIN_WORDS
            .iter()
            .map(|&(disp, weight, flags)| (disp.to_ascii_lowercase(), disp.to_string(), weight, flags))
            .collect();
        raw.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.2.cmp(&a.2)));
        raw.dedup_by(|a, b| a.0 == b.0);
        let bytes = serialize_endict(&raw);
        Self::from_bytes(bytes).expect("builtin ENDICT01 must be valid")
    }

    fn parse(buf: ByteSource) -> io::Result<Self> {
        let b = buf.as_slice();
        if b.len() < HEADER_LEN || &b[0..8] != ENDICT_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid ENDICT01 magic",
            ));
        }
        let version = read_u32(b, 8);
        if version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported ENDICT01 version {version}"),
            ));
        }
        let entry_count = read_u32(b, 12) as usize;
        let entries_off = read_u32(b, 16) as usize;
        let str_off = read_u32(b, 20) as usize;
        let entries_end = entries_off
            .checked_add(entry_count.saturating_mul(ENTRY_LEN))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ENDICT01 overflow"))?;
        if entries_end > str_off || str_off > b.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ENDICT01 truncated",
            ));
        }
        Ok(Self {
            buf,
            entry_count,
            entries_off,
            str_off,
            t9_index: OnceLock::new(),
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entry_count
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entry_count == 0
    }

    #[inline]
    fn entry_at(&self, idx: usize) -> EnEntry<'_> {
        let b = self.buf.as_slice();
        let off = self.entries_off + idx * ENTRY_LEN;
        let key_off = self.str_off + read_u32(b, off) as usize;
        let disp_off = self.str_off + read_u32(b, off + 4) as usize;
        let key_len = read_u16(b, off + 8) as usize;
        let disp_len = read_u16(b, off + 10) as usize;
        let weight = read_u16(b, off + 12);
        let flags = read_u16(b, off + 14);
        let key = std::str::from_utf8(&b[key_off..key_off + key_len]).unwrap_or("");
        let display = std::str::from_utf8(&b[disp_off..disp_off + disp_len]).unwrap_or(key);
        EnEntry {
            key,
            display,
            weight,
            flags,
        }
    }

    fn lower_bound(&self, lower: &str) -> usize {
        let mut lo = 0usize;
        let mut hi = self.entry_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.entry_at(mid).key < lower {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Exact lowercase lookup (`O(log N)`).
    pub fn lookup_exact(&self, lower: &str) -> Option<EnEntry<'_>> {
        if lower.is_empty() || self.entry_count == 0 {
            return None;
        }
        let idx = self.lower_bound(lower);
        if idx < self.entry_count {
            let e = self.entry_at(idx);
            if e.key == lower {
                return Some(e);
            }
        }
        None
    }

    /// Exact lookup with lightweight English inflectional fallback (`-s`, `-es`, `-ed`, `-d`,
    /// `-ing`, `-er`, `-ly`, `-ies`->`-y`, `-ied`->`-y`).
    pub fn lookup_with_morphology(&self, lower: &str) -> Option<(String, u16, u16)> {
        if let Some(e) = self.lookup_exact(lower) {
            return Some((e.display.to_owned(), e.weight, e.flags));
        }
        if lower.len() < 4 || !lower.bytes().all(|b| b.is_ascii_lowercase()) {
            return None;
        }
        let try_stem = |stem: &str, suffix: &str| -> Option<(String, u16, u16)> {
            if stem.len() < 3 {
                return None;
            }
            let e = self.lookup_exact(stem)?;
            let mut disp = e.display.to_owned();
            // Avoid weird casing like "iOSs" or "APIed"
            if (e.flags & EN_FLAG_ABBR) != 0 && suffix != "s" {
                return None;
            }
            if suffix == "ies" || suffix == "ied" {
                if disp.ends_with('y') || disp.ends_with('Y') {
                    disp.pop();
                    disp.push_str(suffix);
                } else {
                    return None;
                }
            } else {
                disp.push_str(suffix);
            }
            Some((disp, (e.weight as u32 * 3 / 4) as u16, e.flags))
        };

        if let Some(stem) = lower.strip_suffix("ies") {
            let mut y_stem = stem.to_owned();
            y_stem.push('y');
            if let Some(hit) = try_stem(&y_stem, "ies") {
                return Some(hit);
            }
        }
        if let Some(stem) = lower.strip_suffix("ied") {
            let mut y_stem = stem.to_owned();
            y_stem.push('y');
            if let Some(hit) = try_stem(&y_stem, "ied") {
                return Some(hit);
            }
        }
        for suf in ["ing", "ed", "es", "er", "ly", "s", "d"] {
            if let Some(stem) = lower.strip_suffix(suf) {
                if let Some(hit) = try_stem(stem, suf) {
                    return Some(hit);
                }
                // Handle silent-e drop for -ing/-ed/-er (e.g. "coding" -> "code")
                if matches!(suf, "ing" | "ed" | "er") && stem.len() >= 3 {
                    let mut e_stem = stem.to_owned();
                    e_stem.push('e');
                    if let Some(e) = self.lookup_exact(&e_stem) {
                        let mut disp = e.display.to_owned();
                        if disp.ends_with('e') {
                            disp.pop();
                            disp.push_str(suf);
                            return Some((disp, (e.weight as u32 * 3 / 4) as u16, e.flags));
                        }
                    }
                }
            }
        }
        None
    }

    /// Check if `lower` is a prefix of any dictionary entry.
    /// Returns `Some((max_weight, has_strictly_longer_extension))`.
    pub fn prefix_info(&self, lower: &str) -> Option<(u16, bool)> {
        if lower.is_empty() || self.entry_count == 0 {
            return None;
        }
        let mut idx = self.lower_bound(lower);
        let mut max_w = 0u16;
        let mut found = false;
        let mut has_longer = false;
        let scan_limit = 96usize;
        let mut scanned = 0usize;
        while idx < self.entry_count && scanned < scan_limit {
            let e = self.entry_at(idx);
            if !e.key.starts_with(lower) {
                break;
            }
            found = true;
            if e.weight > max_w {
                max_w = e.weight;
            }
            if e.key.len() > lower.len() {
                has_longer = true;
            }
            idx += 1;
            scanned += 1;
        }
        found.then_some((max_w, has_longer))
    }

    /// Return top-weighted completions starting with `lower`.
    pub fn lookup_prefix(&self, lower: &str, limit: usize) -> Vec<EnEntry<'_>> {
        if lower.is_empty() || limit == 0 || self.entry_count == 0 {
            return Vec::new();
        }
        let mut idx = self.lower_bound(lower);
        let mut pool = Vec::new();
        let scan_limit = 160usize;
        let mut scanned = 0usize;
        while idx < self.entry_count && scanned < scan_limit {
            let e = self.entry_at(idx);
            if !e.key.starts_with(lower) {
                break;
            }
            pool.push(e);
            idx += 1;
            scanned += 1;
        }
        pool.sort_by(|a, b| {
            let a_exact = a.key.len() == lower.len();
            let b_exact = b.key.len() == lower.len();
            b_exact
                .cmp(&a_exact)
                .then_with(|| b.weight.cmp(&a.weight))
                .then_with(|| a.key.len().cmp(&b.key.len()))
        });
        pool.truncate(limit);
        pool
    }

    /// Look up top-weighted English entries matching a T9 digit sequence (`'2'..='9'`, `len 2..=12`).
    /// Index is built lazily on first T9 query using purely corpus frequency (`0` hardcoded word list).
    pub fn lookup_t9(&self, digits: &[u8], limit: usize) -> Vec<EnEntry<'_>> {
        if limit == 0 || self.entry_count == 0 {
            return Vec::new();
        }
        let Some(t9_key) = pack_t9_digits_slice(digits) else {
            return Vec::new();
        };
        let index = self.t9_index.get_or_init(|| {
            let mut map: HashMap<u64, Vec<u32>> = HashMap::with_capacity(4_096);
            for i in 0..self.entry_count {
                let e = self.entry_at(i);
                let min_w = if e.key.len() == 2 {
                    50_000
                } else if e.key.len() == 3 {
                    42_000
                } else {
                    32_000
                };
                if e.weight >= min_w {
                    if let Some(k) = pack_word_to_t9_key(e.key) {
                        map.entry(k).or_default().push(i as u32);
                    }
                }
            }
            for bucket in map.values_mut() {
                bucket.sort_unstable_by(|&a, &b| {
                    self.entry_at(b as usize)
                        .weight
                        .cmp(&self.entry_at(a as usize).weight)
                });
                bucket.truncate(4);
            }
            map
        });
        let Some(indices) = index.get(&t9_key) else {
            return Vec::new();
        };
        indices
            .iter()
            .take(limit)
            .map(|&i| self.entry_at(i as usize))
            .collect()
    }
}

/// Serialize `(lower_key, canonical_display, weight, flags)` into `ENDICT01` binary bytes.
/// Entries are sorted and deduplicated by `lower_key` (keeping the highest weight).
pub fn serialize_endict(entries: &[(String, String, u16, u16)]) -> Vec<u8> {
    let mut sorted: Vec<(&str, &str, u16, u16)> = entries
        .iter()
        .filter(|(k, d, _, _)| !k.is_empty() && !d.is_empty() && k.len() <= 64 && d.len() <= 64)
        .map(|(k, d, w, f)| (k.as_str(), d.as_str(), *w, *f))
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0).then_with(|| b.2.cmp(&a.2)));
    sorted.dedup_by(|a, b| a.0 == b.0);

    let entry_count = sorted.len();
    let entries_off = HEADER_LEN;
    let str_off = entries_off + entry_count * ENTRY_LEN;

    let mut str_blob = Vec::<u8>::with_capacity(entry_count * 12);
    let mut entry_bytes = Vec::<u8>::with_capacity(entry_count * ENTRY_LEN);

    for (key, disp, weight, flags) in sorted {
        let key_off = str_blob.len() as u32;
        str_blob.extend_from_slice(key.as_bytes());
        let disp_off = if disp == key {
            key_off
        } else {
            let off = str_blob.len() as u32;
            str_blob.extend_from_slice(disp.as_bytes());
            off
        };
        entry_bytes.extend_from_slice(&key_off.to_le_bytes());
        entry_bytes.extend_from_slice(&disp_off.to_le_bytes());
        entry_bytes.extend_from_slice(&(key.len() as u16).to_le_bytes());
        entry_bytes.extend_from_slice(&(disp.len() as u16).to_le_bytes());
        entry_bytes.extend_from_slice(&weight.to_le_bytes());
        entry_bytes.extend_from_slice(&flags.to_le_bytes());
    }

    let mut out = Vec::with_capacity(str_off + str_blob.len());
    out.extend_from_slice(ENDICT_MAGIC);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(entry_count as u32).to_le_bytes());
    out.extend_from_slice(&(entries_off as u32).to_le_bytes());
    out.extend_from_slice(&(str_off as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&entry_bytes);
    out.extend_from_slice(&str_blob);
    out
}

/// Apply user's explicit casing if they typed any uppercase letter; otherwise return canonical display.
pub fn format_english_display(raw_slice: &str, canonical: &str) -> String {
    if raw_slice.bytes().any(|b| b.is_ascii_uppercase()) {
        raw_slice.to_owned()
    } else {
        canonical.to_owned()
    }
}

/// Resolve English display casing without any hardcoded word list (pure information-asymmetry rule):
/// 1. Any explicit uppercase in `raw_slice` comes directly from the user's current keystrokes -> keep `raw_slice`.
/// 2. If personal `user_disp` contains uppercase (`Arbitero`, `Flarepath`, `Juxu`), use `user_disp`.
/// 3. If dictionary `dict_disp` contains uppercase (`iPhone`, `macOS`, `iOS`, `GitHub`, `RWKV`), use `dict_disp`
///    even when an older unmarked all-lowercase `user_disp` (`iphone`, `macos`) exists.
/// 4. Otherwise fall back to `user_disp`, `dict_disp`, or `raw_slice`.
pub fn resolve_english_display(
    raw_slice: &str,
    user_disp: Option<&str>,
    dict_disp: Option<&str>,
) -> String {
    if raw_slice.bytes().any(|b| b.is_ascii_uppercase()) {
        return raw_slice.to_owned();
    }
    if let Some(ud) = user_disp {
        if ud.bytes().any(|b| b.is_ascii_uppercase()) {
            return ud.to_owned();
        }
    }
    if let Some(dd) = dict_disp {
        if dd.bytes().any(|b| b.is_ascii_uppercase()) {
            return dd.to_owned();
        }
    }
    user_disp
        .or(dict_disp)
        .unwrap_or(raw_slice)
        .to_owned()
}

/// Curated fallback list of ultra-common English & developer words (used when `english.endict` is not loaded).
const BUILTIN_WORDS: &[(&str, u16, u16)] = &[
    // Tech / dev / brands
    ("Rust", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Python", 54000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Swift", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("TypeScript", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("JavaScript", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("GitHub", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("iOS", 54000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("macOS", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("iPhone", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("iPad", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Mac", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Linux", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Windows", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Android", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Apple", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Google", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Claude", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Gemini", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("GPT", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("AI", 56000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("LLM", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("API", 54000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("PR", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("UI", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("UX", 46000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("CPU", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("GPU", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("SQL", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("SSH", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("HTTP", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("HTTPS", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("JSON", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("URL", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("PDF", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("PNG", 46000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
    ("App", 54000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Xcode", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Docker", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("React", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Node", 46000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Cargo", 46000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Git", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("Slack", 46000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("WiFi", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("RWKV", 48000, EN_FLAG_TECH | EN_FLAG_ABBR),
    ("MCP", 46000, EN_FLAG_TECH | EN_FLAG_ABBR),
    ("CLI", 46000, EN_FLAG_TECH | EN_FLAG_ABBR),
    ("SDK", 46000, EN_FLAG_TECH | EN_FLAG_ABBR),
    ("IDE", 46000, EN_FLAG_TECH | EN_FLAG_ABBR),
    ("CI", 46000, EN_FLAG_TECH | EN_FLAG_ABBR),
    // High-frequency everyday & dev words
    ("bug", 54000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("demo", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("code", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("test", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("work", 54000, EN_FLAG_HIGH_FREQ),
    ("good", 56000, EN_FLAG_HIGH_FREQ),
    ("time", 56000, EN_FLAG_HIGH_FREQ),
    ("main", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("merge", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("commit", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("branch", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("push", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("pull", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("clone", 46000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("build", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("debug", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("deploy", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("server", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("client", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("model", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("token", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("cache", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("async", 46000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("await", 44000, EN_FLAG_TECH),
    ("error", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("check", 50000, EN_FLAG_HIGH_FREQ),
    ("review", 48000, EN_FLAG_HIGH_FREQ),
    ("update", 50000, EN_FLAG_HIGH_FREQ),
    ("config", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("script", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("prompt", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("agent", 50000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("chat", 50000, EN_FLAG_HIGH_FREQ),
    ("file", 52000, EN_FLAG_HIGH_FREQ),
    ("data", 52000, EN_FLAG_HIGH_FREQ),
    ("user", 52000, EN_FLAG_HIGH_FREQ),
    ("name", 52000, EN_FLAG_HIGH_FREQ),
    ("list", 50000, EN_FLAG_HIGH_FREQ),
    ("type", 50000, EN_FLAG_HIGH_FREQ),
    ("true", 50000, EN_FLAG_HIGH_FREQ),
    ("false", 48000, EN_FLAG_HIGH_FREQ),
    ("from", 54000, EN_FLAG_HIGH_FREQ),
    ("with", 56000, EN_FLAG_HIGH_FREQ),
    ("this", 56000, EN_FLAG_HIGH_FREQ),
    ("that", 56000, EN_FLAG_HIGH_FREQ),
    ("what", 54000, EN_FLAG_HIGH_FREQ),
    ("when", 54000, EN_FLAG_HIGH_FREQ),
    ("where", 52000, EN_FLAG_HIGH_FREQ),
    ("which", 52000, EN_FLAG_HIGH_FREQ),
    ("about", 54000, EN_FLAG_HIGH_FREQ),
    ("hello", 54000, EN_FLAG_HIGH_FREQ),
    ("world", 54000, EN_FLAG_HIGH_FREQ),
    ("thanks", 52000, EN_FLAG_HIGH_FREQ),
    ("sorry", 52000, EN_FLAG_HIGH_FREQ),
    ("please", 52000, EN_FLAG_HIGH_FREQ),
    ("ok", 56000, EN_FLAG_HIGH_FREQ),
    ("yes", 54000, EN_FLAG_HIGH_FREQ),
    ("no", 54000, EN_FLAG_HIGH_FREQ),
    ("hi", 52000, EN_FLAG_HIGH_FREQ),
    ("go", 52000, EN_FLAG_HIGH_FREQ),
    ("pro", 48000, EN_FLAG_HIGH_FREQ),
    ("max", 48000, EN_FLAG_HIGH_FREQ),
    ("plus", 48000, EN_FLAG_HIGH_FREQ),
    ("open", 52000, EN_FLAG_HIGH_FREQ),
    ("make", 54000, EN_FLAG_HIGH_FREQ),
    ("take", 52000, EN_FLAG_HIGH_FREQ),
    ("have", 56000, EN_FLAG_HIGH_FREQ),
    ("will", 56000, EN_FLAG_HIGH_FREQ),
    ("just", 54000, EN_FLAG_HIGH_FREQ),
    ("like", 54000, EN_FLAG_HIGH_FREQ),
    ("more", 54000, EN_FLAG_HIGH_FREQ),
    ("some", 54000, EN_FLAG_HIGH_FREQ),
    ("only", 52000, EN_FLAG_HIGH_FREQ),
    ("also", 52000, EN_FLAG_HIGH_FREQ),
    ("very", 52000, EN_FLAG_HIGH_FREQ),
    ("well", 52000, EN_FLAG_HIGH_FREQ),
    ("back", 52000, EN_FLAG_HIGH_FREQ),
    ("over", 52000, EN_FLAG_HIGH_FREQ),
    ("next", 52000, EN_FLAG_HIGH_FREQ),
    ("last", 52000, EN_FLAG_HIGH_FREQ),
    ("first", 54000, EN_FLAG_HIGH_FREQ),
    ("right", 52000, EN_FLAG_HIGH_FREQ),
    ("think", 52000, EN_FLAG_HIGH_FREQ),
    ("still", 52000, EN_FLAG_HIGH_FREQ),
    ("start", 52000, EN_FLAG_HIGH_FREQ),
    ("state", 50000, EN_FLAG_HIGH_FREQ),
    ("index", 48000, EN_FLAG_HIGH_FREQ),
    ("input", 50000, EN_FLAG_HIGH_FREQ),
    ("output", 48000, EN_FLAG_HIGH_FREQ),
    ("local", 50000, EN_FLAG_HIGH_FREQ),
    ("cloud", 50000, EN_FLAG_HIGH_FREQ),
    ("table", 50000, EN_FLAG_HIGH_FREQ),
    ("query", 48000, EN_FLAG_HIGH_FREQ),
    ("value", 50000, EN_FLAG_HIGH_FREQ),
    ("score", 48000, EN_FLAG_HIGH_FREQ),
    ("train", 50000, EN_FLAG_HIGH_FREQ),
    ("batch", 46000, EN_FLAG_HIGH_FREQ),
    ("layer", 48000, EN_FLAG_HIGH_FREQ),
    ("graph", 48000, EN_FLAG_HIGH_FREQ),
    ("image", 50000, EN_FLAG_HIGH_FREQ),
    ("video", 52000, EN_FLAG_HIGH_FREQ),
    ("audio", 48000, EN_FLAG_HIGH_FREQ),
    ("email", 52000, EN_FLAG_HIGH_FREQ),
    ("phone", 52000, EN_FLAG_HIGH_FREQ),
    ("store", 50000, EN_FLAG_HIGH_FREQ),
    ("order", 50000, EN_FLAG_HIGH_FREQ),
    ("price", 50000, EN_FLAG_HIGH_FREQ),
    ("group", 50000, EN_FLAG_HIGH_FREQ),
    ("share", 50000, EN_FLAG_HIGH_FREQ),
    ("login", 50000, EN_FLAG_HIGH_FREQ),
    ("issue", 50000, EN_FLAG_HIGH_FREQ),
    ("fix", 50000, EN_FLAG_HIGH_FREQ),
    ("log", 50000, EN_FLAG_HIGH_FREQ),
    ("run", 52000, EN_FLAG_HIGH_FREQ),
    ("set", 52000, EN_FLAG_HIGH_FREQ),
    ("get", 54000, EN_FLAG_HIGH_FREQ),
    ("add", 52000, EN_FLAG_HIGH_FREQ),
    ("new", 56000, EN_FLAG_HIGH_FREQ),
    ("dev", 48000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("ops", 44000, EN_FLAG_TECH),
    ("web", 52000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ("net", 48000, EN_FLAG_TECH),
    ("doc", 48000, EN_FLAG_HIGH_FREQ),
    ("docs", 48000, EN_FLAG_HIGH_FREQ),
];
