/// Base-27 positional encoding for 1..=5 lowercase ASCII letters ('a'..='z').
///
/// Each slot holds 0 (empty) or 1..=26 ('a'..='z'):
/// `packed = d0 * 27^4 + d1 * 27^3 + d2 * 27^2 + d3 * 27 + d4`
///
/// Key property: integer `<` matches exact ASCII lexicographical order, and
/// any prefix `p` spans a contiguous integer range `[min, max]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PackedCode(pub u32);

pub const B0: u32 = 27 * 27 * 27 * 27; // 531,441
pub const B1: u32 = 27 * 27 * 27;      // 19,683
pub const B2: u32 = 27 * 27;           // 729
pub const B3: u32 = 27;
pub const MAX_PACKED_CODE: u32 = 26 * B0 + 26 * B1 + 26 * B2 + 26 * B3 + 26; // 14,348,906

impl PackedCode {
    pub const EMPTY: Self = Self(0);

    #[inline]
    pub fn from_ascii(s: &str) -> Option<Self> {
        Self::from_bytes(s.as_bytes())
    }

    #[inline]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > 5 {
            return None;
        }
        let mut acc = 0u32;
        let weights = [B0, B1, B2, B3, 1];
        for (i, &b) in bytes.iter().enumerate() {
            let d = match b {
                b'a'..=b'z' => (b - b'a' + 1) as u32,
                b'A'..=b'Z' => (b - b'A' + 1) as u32,
                _ => return None,
            };
            acc += d * weights[i];
        }
        Some(Self(acc))
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub fn digits(self) -> [u8; 5] {
        let v = self.0;
        let d0 = (v / B0) as u8;
        let r0 = v % B0;
        let d1 = (r0 / B1) as u8;
        let r1 = r0 % B1;
        let d2 = (r1 / B2) as u8;
        let r2 = r1 % B2;
        let d3 = (r2 / B3) as u8;
        let d4 = (r2 % B3) as u8;
        [d0, d1, d2, d3, d4]
    }

    #[inline]
    pub fn len(self) -> usize {
        let d = self.digits();
        if d[0] == 0 {
            0
        } else if d[1] == 0 {
            1
        } else if d[2] == 0 {
            2
        } else if d[3] == 0 {
            3
        } else if d[4] == 0 {
            4
        } else {
            5
        }
    }

    #[inline]
    pub fn to_bytes_buf(self) -> ([u8; 5], usize) {
        let d = self.digits();
        let mut out = [0u8; 5];
        let mut n = 0;
        for slot in d {
            if slot == 0 {
                break;
            }
            out[n] = b'a' + slot - 1;
            n += 1;
        }
        (out, n)
    }

    pub fn as_string(self) -> String {
        let (buf, n) = self.to_bytes_buf();
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    /// Inclusive range `[lo, hi]` matching all codes that start with `self`.
    #[inline]
    pub fn prefix_range(self) -> (Self, Self) {
        let n = self.len();
        let hi_tail = match n {
            0 => MAX_PACKED_CODE,
            1 => 26 * B1 + 26 * B2 + 26 * B3 + 26,
            2 => 26 * B2 + 26 * B3 + 26,
            3 => 26 * B3 + 26,
            4 => 26,
            _ => 0,
        };
        (self, Self(self.0 + hi_tail))
    }
}

/// True if `ch` is a CJK ideograph (Unified CJK, Ext A, Ext B–I, Compatibility Ideographs, or `〇`).
#[inline]
pub fn is_cjk_ideograph(ch: char) -> bool {
    matches!(
        ch as u32,
        0x4E00..=0x9FFF
            | 0x3400..=0x4DBF
            | 0x20000..=0x323AF
            | 0xF900..=0xFAFF
            | 0x3007
    )
}

/// Standard Cangjie radical glyph for ASCII letters `'a'..='z'`.
#[inline]
pub fn cangjie_radical_char(b: u8) -> Option<char> {
    const RADICALS: [char; 26] = [
        '日', '月', '金', '木', '水', '火', '土', '竹', '戈', '十', '大', '中', '一',
        '弓', '人', '心', '手', '口', '尸', '廿', '山', '女', '田', '難', '卜', '重',
    ];
    let idx = match b {
        b'a'..=b'z' => (b - b'a') as usize,
        b'A'..=b'Z' => (b - b'A') as usize,
        _ => return None,
    };
    Some(RADICALS[idx])
}

/// Convert an ASCII Cangjie code string into Cangjie radical glyphs (e.g. `"oi"` -> `"人戈"`).
pub fn cangjie_radicals_str(code: &str) -> String {
    let mut out = String::with_capacity(code.len() * 3);
    for ch in code.chars() {
        if ch.is_ascii() {
            if let Some(rad) = cangjie_radical_char(ch as u8) {
                out.push(rad);
                continue;
            }
        }
        out.push(ch);
    }
    out
}

/// Compact 2-letter character stem (`AaAb`) used by Wubi phrase encoding rules:
/// - 2-char word: `A1 A2 B1 B2`
/// - 3-char word: `A1 B1 C1 C2`
/// - >=4-char word: `A1 B1 C1 Z1`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Stem2(pub u16);

impl Stem2 {
    #[inline]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        let d0 = Self::digit(bytes[0])?;
        let d1 = if bytes.len() >= 2 {
            Self::digit(bytes[1])?
        } else {
            d0
        };
        Some(Self(((d0 as u16) << 8) | (d1 as u16)))
    }

    #[inline]
    fn digit(b: u8) -> Option<u8> {
        match b {
            b'a'..=b'y' => Some(b - b'a' + 1),
            b'A'..=b'Y' => Some(b - b'A' + 1),
            _ => None,
        }
    }

    #[inline]
    pub fn first(self) -> u8 {
        (self.0 >> 8) as u8
    }

    #[inline]
    pub fn second(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    #[inline]
    pub fn is_valid(self) -> bool {
        (1..=25).contains(&self.first()) && (1..=25).contains(&self.second())
    }
}

#[inline]
pub fn compose_phrase_code(stems: &[Stem2]) -> Option<PackedCode> {
    let digits: [u8; 4] = match stems.len() {
        0 | 1 => return None,
        2 => [
            stems[0].first(),
            stems[0].second(),
            stems[1].first(),
            stems[1].second(),
        ],
        3 => [
            stems[0].first(),
            stems[1].first(),
            stems[2].first(),
            stems[2].second(),
        ],
        n => [
            stems[0].first(),
            stems[1].first(),
            stems[2].first(),
            stems[n - 1].first(),
        ],
    };
    if digits.iter().any(|&d| !(1..=25).contains(&d)) {
        return None;
    }
    let raw = (digits[0] as u32) * B0
        + (digits[1] as u32) * B1
        + (digits[2] as u32) * B2
        + (digits[3] as u32) * B3;
    Some(PackedCode(raw))
}

/// Expand `'z'` wildcards at positions `1..len` (0-indexed) over `'a'..='y'`.
/// Caps expansion to at most 625 codes (up to two `'z'` wildcards in positions 2..=4).
pub fn expand_wildcards(code: &[u8]) -> Vec<PackedCode> {
    if code.is_empty() || code.len() > 4 {
        return Vec::new();
    }
    let z_count = code.iter().skip(1).filter(|&&b| b == b'z').count();
    if z_count == 0 || z_count > 2 {
        return PackedCode::from_bytes(code).into_iter().collect();
    }
    let mut results = Vec::with_capacity(25usize.pow(z_count as u32));
    let mut buf = [0u8; 4];
    let len = code.len();
    buf[..len].copy_from_slice(code);

    fn dfs(pos: usize, len: usize, buf: &mut [u8; 4], out: &mut Vec<PackedCode>) {
        if pos == len {
            if let Some(p) = PackedCode::from_bytes(&buf[..len]) {
                out.push(p);
            }
            return;
        }
        if pos > 0 && buf[pos] == b'z' {
            for c in b'a'..=b'y' {
                buf[pos] = c;
                dfs(pos + 1, len, buf, out);
            }
            buf[pos] = b'z';
        } else {
            dfs(pos + 1, len, buf, out);
        }
    }

    dfs(0, len, &mut buf, &mut results);
    results
}

/// Base-27 positional encoding for 1..=6 lowercase ASCII letters (Mandarin Pinyin syllables).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PackedPinyin(pub u32);

const P0: u32 = 27 * 27 * 27 * 27 * 27; // 14,348,907
const P1: u32 = 27 * 27 * 27 * 27;      // 531,441
const P2: u32 = 27 * 27 * 27;           // 19,683
const P3: u32 = 27 * 27;                // 729
const P4: u32 = 27;

impl PackedPinyin {
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > 6 {
            return None;
        }
        let weights = [P0, P1, P2, P3, P4, 1];
        let mut acc = 0u32;
        for (i, &b) in bytes.iter().enumerate() {
            let d = match b {
                b'a'..=b'z' => (b - b'a' + 1) as u32,
                b'A'..=b'Z' => (b - b'A' + 1) as u32,
                _ => return None,
            };
            acc += d * weights[i];
        }
        Some(Self(acc))
    }

    pub fn prefix_range(bytes: &[u8]) -> Option<(Self, Self)> {
        let base = Self::from_bytes(bytes)?;
        let hi_tail = match bytes.len() {
            1 => 26 * P1 + 26 * P2 + 26 * P3 + 26 * P4 + 26,
            2 => 26 * P2 + 26 * P3 + 26 * P4 + 26,
            3 => 26 * P3 + 26 * P4 + 26,
            4 => 26 * P4 + 26,
            5 => 26,
            6 => 0,
            _ => return None,
        };
        Some((base, Self(base.0 + hi_tail)))
    }
}
