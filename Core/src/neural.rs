//! Zero-copy `mmap` inference engine for `Micro-RWKV-7` (`WBLM0001`).
//!
//! Designed specifically for mobile IME constraints (iOS Keyboard Extension `< 50 MB` Jetsam limit,
//! 120Hz `< 0.2 ms` keystroke budget, zero external crates):
//! - All weights live in read-only `mmap` pages (`Dirty RSS = 0` for weights).
//! - Symmetric per-row `INT8` quantization accelerated with ARM64 `sdot` NEON intrinsics.
//! - Constant-size recurrent matrix state (`L * H * 32 * 32` floats = `72 KB` for `L=3, H=6`),
//!   with an 8-step ring buffer (`~600 KB` total dirty RAM) for instant `O(1)` Backspace rollback.
//! - Self-normalized character output head + conditional `MTP-2` phrase head + 25-key first-stroke head.

use std::io;

pub const WBLM_MAGIC: &[u8; 8] = b"WBLM0001";
pub const WBLM_MAGIC_V2: &[u8; 8] = b"WBLM0002";
pub const WBLM_MAGIC_V3: &[u8; 8] = b"WBLM0003";
pub const TOK_BOS: usize = 0;
pub const TOK_UNK: usize = 1;
pub const TOK_COMMA: usize = 2;
pub const TOK_PERIOD: usize = 3;
pub const TOK_ENG_NUM: usize = 4;
pub const CJK_LUT_BASE: u32 = 0x3400;
pub const CJK_LUT_LEN: usize = 0xA000 - 0x3400; // 27,648 BMP CJK codepoints (0x3400..=0x9FFF)
const HEADER_LEN: usize = 64;
const HEAD_DIM: usize = 32;

#[derive(Clone, Default)]
struct TrieNode {
    tok: u16, // 0 means no terminal token at this node (valid ext tokens are >= 4)
    edges: Vec<(u8, u32)>,
}

/// Compact greedy longest-match Trie for ASCII/English subwords (`WBLM0002`, RWKV-World style).
#[derive(Clone)]
pub struct AsciiTrie {
    nodes: Vec<TrieNode>,
    single_ascii: [u16; 95], // 0x20..=0x7E fallback so visible ASCII never produces <UNK>
    pub ext_base_id: usize,
    pub ext_count: usize,
}

impl AsciiTrie {
    pub fn build(ext_base_id: usize, entries: &[&[u8]]) -> Self {
        let mut trie = Self {
            nodes: vec![TrieNode::default()],
            single_ascii: [0u16; 95],
            ext_base_id,
            ext_count: entries.len(),
        };
        for (idx, bytes) in entries.iter().enumerate() {
            if bytes.is_empty() {
                continue;
            }
            let tok = (ext_base_id + idx) as u16;
            if bytes.len() == 1 && (0x20..=0x7E).contains(&bytes[0]) {
                trie.single_ascii[(bytes[0] - 0x20) as usize] = tok;
            }
            let mut cur = 0usize;
            for &b in *bytes {
                if let Some(&(_, next)) = trie.nodes[cur].edges.iter().find(|&&(eb, _)| eb == b) {
                    cur = next as usize;
                } else {
                    let next = trie.nodes.len() as u32;
                    trie.nodes.push(TrieNode::default());
                    trie.nodes[cur].edges.push((b, next));
                    cur = next as usize;
                }
            }
            trie.nodes[cur].tok = tok;
        }
        for node in &mut trie.nodes {
            node.edges.sort_unstable_by_key(|&(b, _)| b);
        }
        trie
    }

    /// Encode a contiguous visible ASCII slice using greedy longest-prefix match.
    pub fn encode_ascii(&self, bytes: &[u8], out: &mut Vec<usize>) {
        let mut i = 0usize;
        let n = bytes.len();
        while i < n {
            let mut cur = 0usize;
            let mut best_tok = 0u16;
            let mut best_len = 0usize;
            let mut j = i;
            while j < n {
                let b = bytes[j];
                let edges = &self.nodes[cur].edges;
                if let Ok(pos) = edges.binary_search_by_key(&b, |&(eb, _)| eb) {
                    cur = edges[pos].1 as usize;
                    j += 1;
                    let t = self.nodes[cur].tok;
                    if t != 0 {
                        best_tok = t;
                        best_len = j - i;
                    }
                } else {
                    break;
                }
            }
            if best_len > 0 {
                out.push(best_tok as usize);
                i += best_len;
            } else {
                let b = bytes[i];
                if (0x20..=0x7E).contains(&b) {
                    let fb = self.single_ascii[(b - 0x20) as usize];
                    out.push(if fb != 0 { fb as usize } else { TOK_UNK });
                } else {
                    out.push(TOK_UNK);
                }
                i += 1;
            }
        }
    }
}

#[inline]
fn read_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

#[inline]
fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[inline]
fn read_f32(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).clamp(-20.0, 20.0).exp())
}

#[inline]
fn silu(x: f32) -> f32 {
    x * sigmoid(x)
}

#[inline]
fn rms_norm(x: &[f32], w_bytes: &[u8], out: &mut [f32]) {
    let d = x.len();
    let mut sum_sq = 0.0f32;
    for &v in x {
        sum_sq += v * v;
    }
    let inv_rms = 1.0 / (sum_sq / (d as f32) + 1e-5).sqrt();
    for i in 0..d {
        let gamma = read_f32(w_bytes, i * 4);
        out[i] = x[i] * inv_rms * gamma;
    }
}

/// Quantize an activation vector `x` symmetrically into `i8` (`-127..=127`), returning `scale = max_abs / 127.0`.
#[inline]
pub fn quantize_act_i8(x: &[f32], out_q: &mut [i8]) -> f32 {
    let mut max_abs = 1e-7f32;
    for &v in x {
        let a = v.abs();
        if a > max_abs {
            max_abs = a;
        }
    }
    let inv = 127.0 / max_abs;
    for i in 0..x.len() {
        out_q[i] = (x[i] * inv).round().clamp(-127.0, 127.0) as i8;
    }
    max_abs / 127.0
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn sdot_i8x16(
    mut acc: core::arch::aarch64::int32x4_t,
    a: core::arch::aarch64::int8x16_t,
    b: core::arch::aarch64::int8x16_t,
) -> core::arch::aarch64::int32x4_t {
    core::arch::asm!(
        "sdot {acc:v}.4s, {a:v}.16b, {b:v}.16b",
        acc = inout(vreg) acc,
        a = in(vreg) a,
        b = in(vreg) b,
        options(pure, nomem, nostack)
    );
    acc
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon,dotprod")]
unsafe fn int8_matvec_sdot(
    w_u8: &[u8],
    scales_bytes: &[u8],
    bias_bytes: Option<&[u8]>,
    x_q: &[i8],
    x_scale: f32,
    rows: usize,
    cols: usize,
    out: &mut [f32],
) {
    use core::arch::aarch64::*;
    let w_ptr = w_u8.as_ptr() as *const i8;
    let x_ptr = x_q.as_ptr();
    let mut r = 0usize;
    while r + 4 <= rows {
        let rp0 = w_ptr.add(r * cols);
        let rp1 = w_ptr.add((r + 1) * cols);
        let rp2 = w_ptr.add((r + 2) * cols);
        let rp3 = w_ptr.add((r + 3) * cols);
        let mut a0 = vdupq_n_s32(0);
        let mut a1 = vdupq_n_s32(0);
        let mut a2 = vdupq_n_s32(0);
        let mut a3 = vdupq_n_s32(0);
        let mut c = 0usize;
        while c + 32 <= cols {
            let xb0 = vld1q_s8(x_ptr.add(c));
            let xb1 = vld1q_s8(x_ptr.add(c + 16));
            a0 = sdot_i8x16(a0, vld1q_s8(rp0.add(c)), xb0);
            a1 = sdot_i8x16(a1, vld1q_s8(rp1.add(c)), xb0);
            a2 = sdot_i8x16(a2, vld1q_s8(rp2.add(c)), xb0);
            a3 = sdot_i8x16(a3, vld1q_s8(rp3.add(c)), xb0);
            a0 = sdot_i8x16(a0, vld1q_s8(rp0.add(c + 16)), xb1);
            a1 = sdot_i8x16(a1, vld1q_s8(rp1.add(c + 16)), xb1);
            a2 = sdot_i8x16(a2, vld1q_s8(rp2.add(c + 16)), xb1);
            a3 = sdot_i8x16(a3, vld1q_s8(rp3.add(c + 16)), xb1);
            c += 32;
        }
        while c + 16 <= cols {
            let xb = vld1q_s8(x_ptr.add(c));
            a0 = sdot_i8x16(a0, vld1q_s8(rp0.add(c)), xb);
            a1 = sdot_i8x16(a1, vld1q_s8(rp1.add(c)), xb);
            a2 = sdot_i8x16(a2, vld1q_s8(rp2.add(c)), xb);
            a3 = sdot_i8x16(a3, vld1q_s8(rp3.add(c)), xb);
            c += 16;
        }
        let mut s0 = vaddvq_s32(a0);
        let mut s1 = vaddvq_s32(a1);
        let mut s2 = vaddvq_s32(a2);
        let mut s3 = vaddvq_s32(a3);
        while c < cols {
            let xv = *x_ptr.add(c) as i32;
            s0 += (*rp0.add(c) as i32) * xv;
            s1 += (*rp1.add(c) as i32) * xv;
            s2 += (*rp2.add(c) as i32) * xv;
            s3 += (*rp3.add(c) as i32) * xv;
            c += 1;
        }
        let sums = [s0, s1, s2, s3];
        for k in 0..4 {
            let rr = r + k;
            let w_sc = read_f32(scales_bytes, rr * 4);
            let b = bias_bytes.map(|bb| read_f32(bb, rr * 4)).unwrap_or(0.0);
            *out.get_unchecked_mut(rr) = (sums[k] as f32) * w_sc * x_scale + b;
        }
        r += 4;
    }
    while r < rows {
        let row_ptr = w_ptr.add(r * cols);
        let mut a0 = vdupq_n_s32(0);
        let mut a1 = vdupq_n_s32(0);
        let mut c = 0usize;
        while c + 32 <= cols {
            a0 = sdot_i8x16(a0, vld1q_s8(row_ptr.add(c)), vld1q_s8(x_ptr.add(c)));
            a1 = sdot_i8x16(a1, vld1q_s8(row_ptr.add(c + 16)), vld1q_s8(x_ptr.add(c + 16)));
            c += 32;
        }
        while c + 16 <= cols {
            a0 = sdot_i8x16(a0, vld1q_s8(row_ptr.add(c)), vld1q_s8(x_ptr.add(c)));
            c += 16;
        }
        let mut acc = vaddvq_s32(vaddq_s32(a0, a1));
        while c < cols {
            acc += (*row_ptr.add(c) as i32) * (*x_ptr.add(c) as i32);
            c += 1;
        }
        let w_sc = read_f32(scales_bytes, r * 4);
        let b = bias_bytes.map(|bb| read_f32(bb, r * 4)).unwrap_or(0.0);
        *out.get_unchecked_mut(r) = (acc as f32) * w_sc * x_scale + b;
        r += 1;
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon,dotprod")]
unsafe fn int8_dot_row_sdot(row_u8: &[u8], row_scale: f32, x_q: &[i8], x_scale: f32) -> f32 {
    use core::arch::aarch64::*;
    let rp = row_u8.as_ptr() as *const i8;
    let xp = x_q.as_ptr();
    let n = x_q.len();
    let mut a0 = vdupq_n_s32(0);
    let mut a1 = vdupq_n_s32(0);
    let mut c = 0usize;
    while c + 32 <= n {
        a0 = sdot_i8x16(a0, vld1q_s8(rp.add(c)), vld1q_s8(xp.add(c)));
        a1 = sdot_i8x16(a1, vld1q_s8(rp.add(c + 16)), vld1q_s8(xp.add(c + 16)));
        c += 32;
    }
    while c + 16 <= n {
        a0 = sdot_i8x16(a0, vld1q_s8(rp.add(c)), vld1q_s8(xp.add(c)));
        c += 16;
    }
    let mut acc = vaddvq_s32(vaddq_s32(a0, a1));
    while c < n {
        acc += (*rp.add(c) as i32) * (*xp.add(c) as i32);
        c += 1;
    }
    (acc as f32) * row_scale * x_scale
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn int8_matvec_neon(
    w_u8: &[u8],
    scales_bytes: &[u8],
    bias_bytes: Option<&[u8]>,
    x_q: &[i8],
    x_scale: f32,
    rows: usize,
    cols: usize,
    out: &mut [f32],
) {
    use core::arch::aarch64::*;
    let w_ptr = w_u8.as_ptr() as *const i8;
    let x_ptr = x_q.as_ptr();
    for r in 0..rows {
        let row_ptr = w_ptr.add(r * cols);
        let mut acc_v = vdupq_n_s32(0);
        let mut c = 0usize;
        while c + 16 <= cols {
            let va = vld1q_s8(row_ptr.add(c));
            let vb = vld1q_s8(x_ptr.add(c));
            let lo = vmull_s8(vget_low_s8(va), vget_low_s8(vb));
            let hi = vmull_s8(vget_high_s8(va), vget_high_s8(vb));
            acc_v = vpadalq_s16(acc_v, lo);
            acc_v = vpadalq_s16(acc_v, hi);
            c += 16;
        }
        let mut acc = vaddvq_s32(acc_v);
        while c < cols {
            acc += (*row_ptr.add(c) as i32) * (*x_ptr.add(c) as i32);
            c += 1;
        }
        let w_sc = read_f32(scales_bytes, r * 4);
        let b = bias_bytes.map(|bb| read_f32(bb, r * 4)).unwrap_or(0.0);
        *out.get_unchecked_mut(r) = (acc as f32) * w_sc * x_scale + b;
    }
}

#[inline]
fn int8_matvec(
    w_u8: &[u8],
    scales_bytes: &[u8],
    bias_bytes: Option<&[u8]>,
    x_q: &[i8],
    x_scale: f32,
    rows: usize,
    cols: usize,
    out: &mut [f32],
) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        if std::arch::is_aarch64_feature_detected!("dotprod") {
            return int8_matvec_sdot(w_u8, scales_bytes, bias_bytes, x_q, x_scale, rows, cols, out);
        }
        return int8_matvec_neon(w_u8, scales_bytes, bias_bytes, x_q, x_scale, rows, cols, out);
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        for r in 0..rows {
            let row = &w_u8[r * cols..(r + 1) * cols];
            let mut acc: i32 = 0;
            for c in 0..cols {
                acc += (row[c] as i8 as i32) * (x_q[c] as i32);
            }
            let w_sc = read_f32(scales_bytes, r * 4);
            let b = bias_bytes.map(|bb| read_f32(bb, r * 4)).unwrap_or(0.0);
            out[r] = (acc as f32) * w_sc * x_scale + b;
        }
    }
}

const BATCH_CHUNK: usize = 8;

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon,dotprod")]
unsafe fn int8_matmul_batch_sdot<const B: usize>(
    w_u8: &[u8],
    scales_bytes: &[u8],
    bias_bytes: Option<&[u8]>,
    x_qs: &[&[i8]; B],
    x_scales: &[f32; B],
    rows: usize,
    cols: usize,
    outs: &mut [&mut [f32]; B],
) {
    use core::arch::aarch64::*;
    let w_ptr = w_u8.as_ptr() as *const i8;
    let x_ptrs: [*const i8; B] = std::array::from_fn(|b| x_qs[b].as_ptr());
    let mut r = 0usize;
    while r + 2 <= rows {
        let rp0 = w_ptr.add(r * cols);
        let rp1 = w_ptr.add((r + 1) * cols);
        let mut a0: [int32x4_t; B] = [vdupq_n_s32(0); B];
        let mut a1: [int32x4_t; B] = [vdupq_n_s32(0); B];
        let mut c = 0usize;
        while c + 16 <= cols {
            let w0 = vld1q_s8(rp0.add(c));
            let w1 = vld1q_s8(rp1.add(c));
            for b in 0..B {
                let xb = vld1q_s8(x_ptrs[b].add(c));
                a0[b] = sdot_i8x16(a0[b], w0, xb);
                a1[b] = sdot_i8x16(a1[b], w1, xb);
            }
            c += 16;
        }
        let mut s0: [i32; B] = std::array::from_fn(|b| vaddvq_s32(a0[b]));
        let mut s1: [i32; B] = std::array::from_fn(|b| vaddvq_s32(a1[b]));
        while c < cols {
            let w0v = *rp0.add(c) as i32;
            let w1v = *rp1.add(c) as i32;
            for b in 0..B {
                let xv = *x_ptrs[b].add(c) as i32;
                s0[b] += w0v * xv;
                s1[b] += w1v * xv;
            }
            c += 1;
        }
        let w_sc0 = read_f32(scales_bytes, r * 4);
        let w_sc1 = read_f32(scales_bytes, (r + 1) * 4);
        let b0 = bias_bytes.map(|bb| read_f32(bb, r * 4)).unwrap_or(0.0);
        let b1 = bias_bytes.map(|bb| read_f32(bb, (r + 1) * 4)).unwrap_or(0.0);
        for b in 0..B {
            *outs[b].get_unchecked_mut(r) = (s0[b] as f32) * w_sc0 * x_scales[b] + b0;
            *outs[b].get_unchecked_mut(r + 1) = (s1[b] as f32) * w_sc1 * x_scales[b] + b1;
        }
        r += 2;
    }
    while r < rows {
        let rp = w_ptr.add(r * cols);
        let mut a0: [int32x4_t; B] = [vdupq_n_s32(0); B];
        let mut c = 0usize;
        while c + 16 <= cols {
            let w0 = vld1q_s8(rp.add(c));
            for b in 0..B {
                let xb = vld1q_s8(x_ptrs[b].add(c));
                a0[b] = sdot_i8x16(a0[b], w0, xb);
            }
            c += 16;
        }
        let mut s0: [i32; B] = std::array::from_fn(|b| vaddvq_s32(a0[b]));
        while c < cols {
            let w0v = *rp.add(c) as i32;
            for b in 0..B {
                s0[b] += w0v * (*x_ptrs[b].add(c) as i32);
            }
            c += 1;
        }
        let w_sc = read_f32(scales_bytes, r * 4);
        let bias = bias_bytes.map(|bb| read_f32(bb, r * 4)).unwrap_or(0.0);
        for b in 0..B {
            *outs[b].get_unchecked_mut(r) = (s0[b] as f32) * w_sc * x_scales[b] + bias;
        }
        r += 1;
    }
}

/// Batched INT8 matrix-vector multiply sharing a single pass over `w_u8` across `x_qs.len()` vectors.
/// Cuts memory traffic and L2/DRAM cache energy by `B`x while remaining 100% bit-exact with `int8_matvec`.
fn int8_matmul_slice(
    w_u8: &[u8],
    scales_bytes: &[u8],
    bias_bytes: Option<&[u8]>,
    x_qs: &[&[i8]],
    x_scales: &[f32],
    rows: usize,
    cols: usize,
    outs: &mut [&mut [f32]],
) {
    let b_len = x_qs.len();
    if b_len == 0 {
        return;
    }
    if b_len == 1 {
        int8_matvec(w_u8, scales_bytes, bias_bytes, x_qs[0], x_scales[0], rows, cols, outs[0]);
        return;
    }
    #[cfg(target_arch = "aarch64")]
    if std::arch::is_aarch64_feature_detected!("dotprod") {
        macro_rules! dispatch_b {
            ($N:expr) => {{
                let x_arr: [&[i8]; $N] = std::array::from_fn(|i| x_qs[i]);
                let s_arr: [f32; $N] = std::array::from_fn(|i| x_scales[i]);
                let mut rest = &mut outs[..$N];
                let mut o_vec: Vec<&mut [f32]> = Vec::with_capacity($N);
                while !rest.is_empty() {
                    let (first, tail) = rest.split_first_mut().unwrap();
                    o_vec.push(*first);
                    rest = tail;
                }
                let mut o_arr: [&mut [f32]; $N] = o_vec.try_into().ok().unwrap();
                unsafe {
                    int8_matmul_batch_sdot::<$N>(
                        w_u8,
                        scales_bytes,
                        bias_bytes,
                        &x_arr,
                        &s_arr,
                        rows,
                        cols,
                        &mut o_arr,
                    );
                }
                return;
            }};
        }
        match b_len {
            2 => dispatch_b!(2),
            3 => dispatch_b!(3),
            4 => dispatch_b!(4),
            5 => dispatch_b!(5),
            6 => dispatch_b!(6),
            7 => dispatch_b!(7),
            8 => dispatch_b!(8),
            _ => {}
        }
    }
    for b in 0..b_len {
        int8_matvec(w_u8, scales_bytes, bias_bytes, x_qs[b], x_scales[b], rows, cols, outs[b]);
    }
}

#[inline]
fn int8_dot_row(row_u8: &[u8], row_scale: f32, x_q: &[i8], x_scale: f32) -> f32 {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        if std::arch::is_aarch64_feature_detected!("dotprod") {
            return int8_dot_row_sdot(row_u8, row_scale, x_q, x_scale);
        }
        use core::arch::aarch64::*;
        let rp = row_u8.as_ptr() as *const i8;
        let xp = x_q.as_ptr();
        let n = x_q.len();
        let mut acc_v = vdupq_n_s32(0);
        let mut c = 0usize;
        while c + 16 <= n {
            let va = vld1q_s8(rp.add(c));
            let vb = vld1q_s8(xp.add(c));
            let lo = vmull_s8(vget_low_s8(va), vget_low_s8(vb));
            let hi = vmull_s8(vget_high_s8(va), vget_high_s8(vb));
            acc_v = vpadalq_s16(acc_v, lo);
            acc_v = vpadalq_s16(acc_v, hi);
            c += 16;
        }
        let mut acc = vaddvq_s32(acc_v);
        while c < n {
            acc += (*rp.add(c) as i32) * (*xp.add(c) as i32);
            c += 1;
        }
        (acc as f32) * row_scale * x_scale
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut acc: i32 = 0;
        for c in 0..x_q.len() {
            acc += (row_u8[c] as i8 as i32) * (x_q[c] as i32);
        }
        (acc as f32) * row_scale * x_scale
    }
}

#[derive(Clone, Copy)]
struct QuantMatOff {
    w_off: usize,
    w_len: usize,
    sc_off: usize,
    sc_len: usize,
    bias_off: Option<(usize, usize)>,
}

#[derive(Clone, Copy)]
struct LayerOffsets {
    ln1_off: usize,
    mu_tm_off: usize,
    tm_proj: QuantMatOff, // [6*D, D] + bias [6*D]
    rho_off: usize,       // [D] direct bonus
    tm_out: QuantMatOff,  // [D, D] (no bias)
    ln2_off: usize,
    mu_cm_off: usize,
    cm_up: QuantMatOff,   // [ffn_dim, D] + bias [ffn_dim]
    cm_dn: QuantMatOff,   // [D, ffn_dim] (no bias)
}

/// Zero-copy view into a `WBLM0001` blob slice (inside `AuxView`'s `mmap` buffer).
#[derive(Clone, Copy)]
pub struct NeuralLmSummary {
    pub vocab_size: usize,
    pub rank: usize,
    pub d_model: usize,
    pub n_heads: usize,
    pub n_layers: usize,
    pub ffn_dim: usize,
}

#[derive(Clone)]
pub struct NeuralOffsets {
    pub summary: NeuralLmSummary,
    pub tok_to_char: Vec<char>,
    pub has_punct_tokens: bool,
    pub has_eng_num_token: bool,
    pub ascii_trie: Option<AsciiTrie>,
    pub(crate) profile_id: u64,
    lut_off: usize,
    emb_w_off: usize,
    emb_sc_off: usize,
    uni_bias_off: usize,
    w_up: QuantMatOff,
    layers: Vec<LayerOffsets>,
    ln_out_off: usize,
    w_dn: QuantMatOff,     // [rank, D] + bias [rank]
    w_mtp_h: QuantMatOff,  // [rank, D] + bias [rank]
    w_mtp_c1: QuantMatOff, // [rank, rank] (no bias)
}

impl NeuralOffsets {
    pub fn parse(blob: &[u8]) -> io::Result<Self> {
        let (has_ext_vocab, has_legacy_key1) = if blob.len() >= HEADER_LEN && &blob[0..8] == WBLM_MAGIC_V3 {
            (true, false)
        } else if blob.len() >= HEADER_LEN && &blob[0..8] == WBLM_MAGIC_V2 {
            (true, true)
        } else if blob.len() >= HEADER_LEN && &blob[0..8] == WBLM_MAGIC {
            (false, true)
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid WBLM0001/WBLM0002/WBLM0003 magic",
            ));
        };
        let vocab_size = read_u32(blob, 8) as usize;
        let rank = read_u32(blob, 12) as usize;
        let d_model = read_u32(blob, 16) as usize;
        let n_heads = read_u32(blob, 20) as usize;
        let n_layers = read_u32(blob, 24) as usize;
        let ffn_dim = read_u32(blob, 28) as usize;

        if vocab_size < 16
            || rank == 0
            || rank > 512
            || d_model == 0
            || d_model > 1024
            || n_heads == 0
            || d_model != n_heads * HEAD_DIM
            || n_layers == 0
            || n_layers > 12
            || ffn_dim == 0
            || ffn_dim > 2048
        {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid WBLM0001 dims"));
        }

        fn take(cur: &mut usize, total: usize, n: usize) -> io::Result<usize> {
            let aligned = (*cur + 3) & !3;
            if aligned + n > total {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated WBLM0001 section"));
            }
            *cur = aligned + n;
            Ok(aligned)
        }

        fn take_qmat(cur: &mut usize, total: usize, rows: usize, cols: usize, has_bias: bool) -> io::Result<QuantMatOff> {
            let w_len = rows * cols;
            let w_off = take(cur, total, w_len)?;
            let sc_len = rows * 4;
            let sc_off = take(cur, total, sc_len)?;
            let bias_off = if has_bias {
                let bl = rows * 4;
                let bo = take(cur, total, bl)?;
                Some((bo, bl))
            } else {
                None
            };
            Ok(QuantMatOff {
                w_off,
                w_len,
                sc_off,
                sc_len,
                bias_off,
            })
        }

        let total = blob.len();
        let mut cur = HEADER_LEN;
        let lut_off = take(&mut cur, total, CJK_LUT_LEN * 2)?;
        let emb_w_off = take(&mut cur, total, vocab_size * rank)?;
        let emb_sc_off = take(&mut cur, total, vocab_size * 4)?;
        let uni_bias_off = take(&mut cur, total, vocab_size * 4)?;
        let w_up = take_qmat(&mut cur, total, d_model, rank, false)?;

        let mut layers = Vec::with_capacity(n_layers);
        for _ in 0..n_layers {
            let ln1_off = take(&mut cur, total, d_model * 4)?;
            let mu_tm_off = take(&mut cur, total, d_model * 4)?;
            let tm_proj = take_qmat(&mut cur, total, 6 * d_model, d_model, true)?;
            let rho_off = take(&mut cur, total, d_model * 4)?;
            let tm_out = take_qmat(&mut cur, total, d_model, d_model, false)?;
            let ln2_off = take(&mut cur, total, d_model * 4)?;
            let mu_cm_off = take(&mut cur, total, d_model * 4)?;
            let cm_up = take_qmat(&mut cur, total, ffn_dim, d_model, true)?;
            let cm_dn = take_qmat(&mut cur, total, d_model, ffn_dim, false)?;
            layers.push(LayerOffsets {
                ln1_off,
                mu_tm_off,
                tm_proj,
                rho_off,
                tm_out,
                ln2_off,
                mu_cm_off,
                cm_up,
                cm_dn,
            });
        }

        let ln_out_off = take(&mut cur, total, d_model * 4)?;
        let w_dn = take_qmat(&mut cur, total, rank, d_model, true)?;
        let w_mtp_h = take_qmat(&mut cur, total, rank, d_model, true)?;
        let w_mtp_c1 = take_qmat(&mut cur, total, rank, rank, false)?;
        if has_legacy_key1 {
            // Legacy WBLM0001/WBLM0002 carried an 86-Wubi key1 head; skip over its bytes cleanly.
            // WBLM0003+ is 100% scheme-free and omits it altogether.
            let _ = take_qmat(&mut cur, total, 25, d_model, true)?;
        }

        let mut tok_to_char = vec!['\0'; vocab_size];
        for idx in 0..CJK_LUT_LEN {
            let tok = read_u16(blob, lut_off + idx * 2) as usize;
            if tok >= 2 && tok < vocab_size && tok_to_char[tok] == '\0' {
                if let Some(ch) = char::from_u32(CJK_LUT_BASE + idx as u32) {
                    tok_to_char[tok] = ch;
                }
            }
        }

        let has_punct_tokens = vocab_size > TOK_PERIOD
            && tok_to_char[TOK_COMMA] == '\0'
            && tok_to_char[TOK_PERIOD] == '\0';

        // Trailing EXT_VOCAB table in WBLM0002 / WBLM0003:
        // [u32 ext_base_id][u32 ext_count] followed by ext_count records of [u8 len][len bytes].
        let ascii_trie = if has_ext_vocab {
            let hdr_off = take(&mut cur, total, 8)?;
            let ext_base_id = read_u32(blob, hdr_off) as usize;
            let ext_count = read_u32(blob, hdr_off + 4) as usize;
            if ext_base_id + ext_count > vocab_size {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "WBLM ext_vocab exceeds vocab_size",
                ));
            }
            let mut entries: Vec<&[u8]> = Vec::with_capacity(ext_count);
            for _ in 0..ext_count {
                if cur >= total {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated WBLM ext_vocab"));
                }
                let blen = blob[cur] as usize;
                cur += 1;
                if cur + blen > total {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated WBLM ext_vocab entry"));
                }
                entries.push(&blob[cur..cur + blen]);
                cur += blen;
            }
            Some(AsciiTrie::build(ext_base_id, &entries))
        } else {
            None
        };

        let has_eng_num_token = !has_ext_vocab
            && has_punct_tokens
            && vocab_size > TOK_ENG_NUM
            && tok_to_char[TOK_ENG_NUM] == '\0';

        Ok(Self {
            summary: NeuralLmSummary {
                vocab_size,
                rank,
                d_model,
                n_heads,
                n_layers,
                ffn_dim,
            },
            tok_to_char,
            has_punct_tokens,
            has_eng_num_token,
            ascii_trie,
            profile_id: blob.iter().step_by((blob.len() / 64).max(1)).fold(
                blob.len() as u64,
                |hash, &byte| (hash ^ byte as u64).wrapping_mul(0x100_0000_01b3),
            ),
            lut_off,
            emb_w_off,
            emb_sc_off,
            uni_bias_off,
            w_up,
            layers,
            ln_out_off,
            w_dn,
            w_mtp_h,
            w_mtp_c1,
        })
    }
}

/// Borrowed zero-copy view of `Micro-RWKV-7` weights backed by `AuxView`.
pub struct NeuralView<'a> {
    pub blob: &'a [u8],
    pub offs: &'a NeuralOffsets,
}

/// Compact recurrent state + cached output projections at one position in the sentence.
#[derive(Clone, Default)]
pub struct RwkvState {
    pub mat_state: Vec<f32>,  // L * H * 32 * 32
    pub prev_tm: Vec<f32>,    // L * D
    pub prev_cm: Vec<f32>,    // L * D
    pub p1_q: Vec<i8>,        // [rank] quantized +1 char query vector
    pub p1_scale: f32,
    pub mtp_h: Vec<f32>,      // [rank] context component of MTP-2
}

impl RwkvState {
    pub fn new_zero(s: &NeuralLmSummary) -> Self {
        Self {
            mat_state: vec![0.0f32; s.n_layers * s.n_heads * HEAD_DIM * HEAD_DIM],
            prev_tm: vec![0.0f32; s.n_layers * s.d_model],
            prev_cm: vec![0.0f32; s.n_layers * s.d_model],
            p1_q: vec![0i8; s.rank],
            p1_scale: 0.0,
            mtp_h: vec![0.0f32; s.rank],
        }
    }
}

const HISTORY_RING_CAP: usize = 8;

/// Rolling sentence tracker with 8-step commit history for instant Backspace rollback
/// and token-sequence deduplication so `set_context_str` (`textDidChange`) skips redundant RWKV unrolls in `O(1)`.
pub struct NeuralContextTracker {
    pub active: RwkvState,
    active_tokens: Vec<u16>,
    bos_cached: Option<RwkvState>,
    ring: Vec<(Vec<u16>, RwkvState)>,
    dims: Option<NeuralLmSummary>,
}

impl Default for NeuralContextTracker {
    fn default() -> Self {
        Self {
            active: RwkvState {
                mat_state: Vec::new(),
                prev_tm: Vec::new(),
                prev_cm: Vec::new(),
                p1_q: Vec::new(),
                p1_scale: 0.0,
                mtp_h: Vec::new(),
            },
            active_tokens: Vec::new(),
            bos_cached: None,
            ring: Vec::with_capacity(HISTORY_RING_CAP),
            dims: None,
        }
    }
}

impl NeuralContextTracker {
    pub fn init_with_model(&mut self, nv: &NeuralView<'_>) {
        let s = nv.offs.summary;
        let mut st = RwkvState::new_zero(&s);
        // Step token 0 (<BOS>) once and cache forever so sentence resets are O(1) memcpy!
        nv.step_token(0, &mut st);
        self.active = st.clone();
        self.active_tokens.clear();
        self.bos_cached = Some(st);
        self.ring.clear();
        self.dims = Some(s);
    }

    #[inline]
    pub fn is_ready(&self) -> bool {
        self.bos_cached.is_some()
    }

    #[inline]
    pub fn bos_state(&self) -> Option<&RwkvState> {
        self.bos_cached.as_ref()
    }

    fn copy_state_from(dst: &mut RwkvState, src: &RwkvState) {
        if dst.mat_state.len() == src.mat_state.len()
            && dst.prev_tm.len() == src.prev_tm.len()
            && dst.p1_q.len() == src.p1_q.len()
        {
            dst.mat_state.copy_from_slice(&src.mat_state);
            dst.prev_tm.copy_from_slice(&src.prev_tm);
            dst.prev_cm.copy_from_slice(&src.prev_cm);
            dst.p1_q.copy_from_slice(&src.p1_q);
            dst.p1_scale = src.p1_scale;
            dst.mtp_h.copy_from_slice(&src.mtp_h);
        } else {
            *dst = src.clone();
        }
    }

    pub fn reset_to_bos(&mut self, nv: Option<&NeuralView<'_>>) {
        if let Some(ref bos) = self.bos_cached {
            Self::copy_state_from(&mut self.active, bos);
            self.active_tokens.clear();
            self.ring.clear();
        } else if let Some(v) = nv {
            self.init_with_model(v);
        }
    }

    /// Encode `text` into the exact post-`<BOS>` token sequence `(tokens, saw_bos_reset)`.
    pub fn encode_context_tokens(nv: &NeuralView<'_>, text: &str) -> (Vec<u16>, bool) {
        let mut out: Vec<u16> = Vec::with_capacity(text.len());
        let mut saw_reset = false;
        if nv.offs.ascii_trie.is_some() {
            let mut raw_toks = Vec::with_capacity(text.len());
            nv.encode_mixed_str(text, &mut raw_toks);
            for tok in raw_toks {
                if tok == TOK_BOS {
                    out.clear();
                    saw_reset = true;
                } else {
                    out.push(tok as u16);
                }
            }
            return (out, saw_reset);
        }
        let mut prev_special: Option<usize> = None;
        for ch in text.chars() {
            let u = ch as u32;
            if (CJK_LUT_BASE..0xA000).contains(&u) {
                out.push(nv.char_to_token(ch) as u16);
                prev_special = None;
            } else if let Some(ptok) = nv.punct_to_token(ch) {
                if prev_special != Some(ptok) {
                    out.push(ptok as u16);
                    prev_special = Some(ptok);
                }
            } else if nv.offs.has_eng_num_token && ch.is_ascii_alphanumeric() {
                if prev_special != Some(TOK_ENG_NUM) {
                    out.push(TOK_ENG_NUM as u16);
                    prev_special = Some(TOK_ENG_NUM);
                }
            } else if nv.offs.has_eng_num_token
                && prev_special == Some(TOK_ENG_NUM)
                && matches!(ch, ' ' | '-' | '_' | '.' | '+' | '/')
            {
                // Keep bridging ASCII tokens inside a single <ENG_NUM> span
            } else {
                out.clear();
                saw_reset = true;
                prev_special = None;
            }
        }
        (out, saw_reset)
    }

    fn step_token_slice_into(nv: &NeuralView<'_>, toks: &[u16], st: &mut RwkvState) {
        let n = toks.len();
        for (i, &t) in toks.iter().enumerate() {
            let is_last = i + 1 == n;
            nv.step_token_heads(t as usize, st, is_last, is_last);
        }
    }

    fn step_context_str(&mut self, nv: &NeuralView<'_>, text: &str) {
        let (toks, saw_reset) = Self::encode_context_tokens(nv, text);
        if saw_reset {
            if let Some(ref bos) = self.bos_cached {
                Self::copy_state_from(&mut self.active, bos);
            }
            self.active_tokens.clear();
        } else if let (Some(&last_prev), Some(&first_new)) = (self.active_tokens.last(), toks.first()) {
            // Coalesce duplicate special tokens across incremental commit boundaries
            if last_prev == first_new
                && matches!(
                    first_new as usize,
                    TOK_COMMA | TOK_PERIOD | TOK_ENG_NUM
                )
            {
                Self::step_token_slice_into(nv, &toks[1..], &mut self.active);
                self.active_tokens.extend_from_slice(&toks[1..]);
                return;
            }
        }
        Self::step_token_slice_into(nv, &toks, &mut self.active);
        self.active_tokens.extend_from_slice(&toks);
    }

    /// Push a committed word/character/punctuation sequence, saving pre-commit state in the 8-step rollback ring.
    pub fn commit_cjk_str(&mut self, nv: &NeuralView<'_>, text: &str) {
        if !self.is_ready() {
            self.init_with_model(nv);
        }
        if self.ring.len() >= HISTORY_RING_CAP {
            self.ring.remove(0);
        }
        self.ring
            .push((self.active_tokens.clone(), self.active.clone()));
        self.step_context_str(nv, text);
    }

    /// Adopt a pre-stepped `new_state` for a pure CJK commit without repeating `step_token` calls.
    pub fn adopt_cjk_commit_state(&mut self, nv: &NeuralView<'_>, cjk_tokens: &[u16], new_state: RwkvState) {
        if !self.is_ready() {
            self.init_with_model(nv);
        }
        if self.ring.len() >= HISTORY_RING_CAP {
            self.ring.remove(0);
        }
        self.ring
            .push((self.active_tokens.clone(), self.active.clone()));
        self.active = new_state;
        self.active_tokens.extend_from_slice(cjk_tokens);
    }

    /// Roll back one committed unit on empty-preedit Backspace (or reset to BOS if history exhausted).
    pub fn pop_commit_or_reset(&mut self, nv: Option<&NeuralView<'_>>) {
        if let Some((prev_toks, prev_st)) = self.ring.pop() {
            self.active = prev_st;
            self.active_tokens = prev_toks;
        } else {
            self.reset_to_bos(nv);
        }
    }

    /// Synchronize state with surrounding host text, reusing `active` or `ring` in `O(1)` / `O(delta)`
    /// whenever the host context matches what was already incrementally stepped.
    pub fn set_context_str(&mut self, nv: &NeuralView<'_>, text: &str) {
        if !self.is_ready() {
            self.init_with_model(nv);
        }
        let (target_toks, saw_reset) = Self::encode_context_tokens(nv, text);

        // 1. Exact token match (or sliding suffix window match on long unbroken clauses) -> 0 RWKV steps!
        if target_toks == self.active_tokens {
            return;
        }
        if !saw_reset
            && target_toks.len() >= 12
            && self.active_tokens.len() > target_toks.len()
            && self.active_tokens.ends_with(&target_toks)
        {
            return;
        }

        // 2. Append-only continuation of `active` -> step only newly appended tokens!
        if !self.active_tokens.is_empty() && target_toks.starts_with(&self.active_tokens) {
            let delta = &target_toks[self.active_tokens.len()..];
            Self::step_token_slice_into(nv, delta, &mut self.active);
            self.active_tokens.extend_from_slice(delta);
            return;
        }

        // 3. Match against rollback ring (e.g. partial 1-char backspace into a multi-char commit)
        if let Some((idx, _)) = self
            .ring
            .iter()
            .enumerate()
            .rev()
            .find(|(_, (rtoks, _))| target_toks.starts_with(rtoks))
        {
            let base_len = self.ring[idx].0.len();
            Self::copy_state_from(&mut self.active, &self.ring[idx].1);
            self.active_tokens.clear();
            self.active_tokens.extend_from_slice(&target_toks);
            self.ring.truncate(idx + 1);
            if target_toks.len() > base_len {
                Self::step_token_slice_into(nv, &target_toks[base_len..], &mut self.active);
            }
            return;
        }

        // 4. Genuine cursor jump / external edit -> reset cleanly to cached <BOS> and step target tokens
        self.reset_to_bos(Some(nv));
        Self::step_token_slice_into(nv, &target_toks, &mut self.active);
        self.active_tokens = target_toks;
    }
}

impl<'a> NeuralView<'a> {
    /// Mixed Chinese (char-level LUT) + ASCII/English (greedy longest-match Trie) tokenizer (`WBLM0002`).
    /// Must stay bit-exact with `Scripts/corpus_v4.py::encode_mixed_line`.
    pub fn encode_mixed_str(&self, text: &str, out: &mut Vec<usize>) {
        let Some(ref trie) = self.offs.ascii_trie else {
            return;
        };
        let mut ascii_buf: Vec<u8> = Vec::new();
        let mut prev_punct: Option<usize> = None;

        let flush_ascii = |buf: &mut Vec<u8>, out: &mut Vec<usize>, prev_punct: &mut Option<usize>| {
            if buf.is_empty() {
                return;
            }
            // Collapse runs of spaces and trim outer spaces
            let mut norm: Vec<u8> = Vec::with_capacity(buf.len());
            let mut last_sp = true;
            for &b in buf.iter() {
                if b == b' ' {
                    if !last_sp {
                        norm.push(b' ');
                        last_sp = true;
                    }
                } else {
                    norm.push(b);
                    last_sp = false;
                }
            }
            while norm.last() == Some(&b' ') {
                norm.pop();
            }
            buf.clear();
            if !norm.is_empty() {
                trie.encode_ascii(&norm, out);
                *prev_punct = None;
            }
        };

        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        for i in 0..n {
            let ch = chars[i];
            let u = ch as u32;
            if (CJK_LUT_BASE..0xA000).contains(&u) {
                flush_ascii(&mut ascii_buf, out, &mut prev_punct);
                out.push(self.char_to_token(ch));
                prev_punct = None;
            } else if (0xF900..=0xFAFF).contains(&u) || (0x20000..=0x3FFFF).contains(&u) {
                // Rare CJK Compatibility / Extension B..I ideographs: step <UNK> instead of dropping context
                flush_ascii(&mut ascii_buf, out, &mut prev_punct);
                out.push(TOK_UNK);
                prev_punct = None;
            } else if matches!(ch, '，' | '、' | '；' | '：' | '—') {
                flush_ascii(&mut ascii_buf, out, &mut prev_punct);
                if prev_punct != Some(TOK_COMMA) {
                    out.push(TOK_COMMA);
                    prev_punct = Some(TOK_COMMA);
                }
            } else if matches!(ch, '。' | '？' | '！' | '\n') {
                flush_ascii(&mut ascii_buf, out, &mut prev_punct);
                if prev_punct != Some(TOK_PERIOD) {
                    out.push(TOK_PERIOD);
                    prev_punct = Some(TOK_PERIOD);
                }
            } else if ch.is_ascii() && (0x20..=0x7E).contains(&u) {
                // Keep '.' / ',' inside numbers or identifiers like "v2.1", "3.14", "node.js";
                // map sentence-level ASCII punctuation (not followed by ASCII alnum) to TOK_COMMA/TOK_PERIOD.
                let next_is_alnum = i + 1 < n && chars[i + 1].is_ascii_alphanumeric();
                if matches!(ch, ',' | ';' | ':') && !next_is_alnum {
                    flush_ascii(&mut ascii_buf, out, &mut prev_punct);
                    if prev_punct != Some(TOK_COMMA) {
                        out.push(TOK_COMMA);
                        prev_punct = Some(TOK_COMMA);
                    }
                } else if matches!(ch, '.' | '?' | '!') && !next_is_alnum {
                    flush_ascii(&mut ascii_buf, out, &mut prev_punct);
                    if prev_punct != Some(TOK_PERIOD) {
                        out.push(TOK_PERIOD);
                        prev_punct = Some(TOK_PERIOD);
                    }
                } else {
                    ascii_buf.push(ch as u8);
                }
            } else if ch.is_whitespace() {
                if !ascii_buf.is_empty() {
                    ascii_buf.push(b' ');
                }
            }
        }
        flush_ascii(&mut ascii_buf, out, &mut prev_punct);
    }

    #[inline]
    pub fn punct_to_token(&self, ch: char) -> Option<usize> {
        if !self.offs.has_punct_tokens {
            return None;
        }
        match ch {
            '，' | '、' | '；' | '：' | '—' | ',' | ';' | ':' => Some(TOK_COMMA),
            '。' | '？' | '！' | '.' | '?' | '!' => Some(TOK_PERIOD),
            _ => None,
        }
    }

    #[inline]
    pub fn char_to_token(&self, ch: char) -> usize {
        let u = ch as u32;
        if (CJK_LUT_BASE..0xA000).contains(&u) {
            let idx = (u - CJK_LUT_BASE) as usize;
            let tok = read_u16(self.blob, self.offs.lut_off + idx * 2) as usize;
            if tok < self.offs.summary.vocab_size {
                return tok;
            }
        }
        1 // <UNK>
    }

    #[inline]
    fn eval_qmat(&self, qm: &QuantMatOff, x_q: &[i8], x_scale: f32, rows: usize, cols: usize, out: &mut [f32]) {
        let w = &self.blob[qm.w_off..qm.w_off + qm.w_len];
        let sc = &self.blob[qm.sc_off..qm.sc_off + qm.sc_len];
        let bias = qm.bias_off.map(|(bo, bl)| &self.blob[bo..bo + bl]);
        int8_matvec(w, sc, bias, x_q, x_scale, rows, cols, out);
    }

    #[inline]
    fn eval_qmat_batch(
        &self,
        qm: &QuantMatOff,
        x_qs: &[&[i8]],
        x_scales: &[f32],
        rows: usize,
        cols: usize,
        outs: &mut [&mut [f32]],
    ) {
        let w = &self.blob[qm.w_off..qm.w_off + qm.w_len];
        let sc = &self.blob[qm.sc_off..qm.sc_off + qm.sc_len];
        let bias = qm.bias_off.map(|(bo, bl)| &self.blob[bo..bo + bl]);
        int8_matmul_slice(w, sc, bias, x_qs, x_scales, rows, cols, outs);
    }

    #[inline]
    fn emb_row(&self, tok: usize) -> (&'a [u8], f32, f32) {
        let r = self.offs.summary.rank;
        let w_start = self.offs.emb_w_off + tok * r;
        let row = &self.blob[w_start..w_start + r];
        let sc = read_f32(self.blob, self.offs.emb_sc_off + tok * 4);
        let ub = read_f32(self.blob, self.offs.uni_bias_off + tok * 4);
        (row, sc, ub)
    }

    #[inline(always)]
    fn step_layer_heads(
        &self,
        loff: &LayerOffsets,
        d: usize,
        h: usize,
        proj_6d: &[f32],
        st_l: &mut [f32],
        y_tm: &mut [f32],
    ) {
        for head in 0..h {
            let base = head * HEAD_DIM;
            let mut k_norm_sq = 1e-8f32;
            let mut k_hat = [0.0f32; HEAD_DIM];
            let mut w_dec = [0.0f32; HEAD_DIM];
            let mut r_vec = [0.0f32; HEAD_DIM];
            for j in 0..HEAD_DIM {
                let kj = proj_6d[d + base + j];
                k_norm_sq += kj * kj;
                r_vec[j] = proj_6d[base + j];
                w_dec[j] = sigmoid(proj_6d[3 * d + base + j]);
            }
            let inv_k = 1.0 / k_norm_sq.sqrt();
            for j in 0..HEAD_DIM {
                k_hat[j] = proj_6d[d + base + j] * inv_k;
            }

            let s_mat = &mut st_l[head * HEAD_DIM * HEAD_DIM..(head + 1) * HEAD_DIM * HEAD_DIM];
            #[cfg(target_arch = "aarch64")]
            unsafe {
                use core::arch::aarch64::*;
                let wp = w_dec.as_ptr();
                let kp = k_hat.as_ptr();
                let rp = r_vec.as_ptr();
                let sm_ptr = s_mat.as_mut_ptr();
                for i in 0..HEAD_DIM {
                    let vi = proj_6d[2 * d + base + i];
                    let beta_i = sigmoid(proj_6d[4 * d + base + i]);
                    let gi = proj_6d[5 * d + base + i];
                    let rho_i = read_f32(self.blob, loff.rho_off + (base + i) * 4);

                    let sp = sm_ptr.add(i * HEAD_DIM);
                    let mut vp0 = vdupq_n_f32(0.0);
                    let mut vp1 = vdupq_n_f32(0.0);
                    let mut j = 0usize;
                    while j < HEAD_DIM {
                        let d0 = vmulq_f32(vld1q_f32(sp.add(j)), vld1q_f32(wp.add(j)));
                        let d1 = vmulq_f32(vld1q_f32(sp.add(j + 4)), vld1q_f32(wp.add(j + 4)));
                        vst1q_f32(sp.add(j), d0);
                        vst1q_f32(sp.add(j + 4), d1);
                        vp0 = vfmaq_f32(vp0, d0, vld1q_f32(kp.add(j)));
                        vp1 = vfmaq_f32(vp1, d1, vld1q_f32(kp.add(j + 4)));
                        j += 8;
                    }
                    let v_pred = vaddvq_f32(vaddq_f32(vp0, vp1));
                    let delta_v = vdupq_n_f32(beta_i * (vi - v_pred));
                    let mut ro0 = vdupq_n_f32(0.0);
                    let mut ro1 = vdupq_n_f32(0.0);
                    j = 0;
                    while j < HEAD_DIM {
                        let n0 = vfmaq_f32(vld1q_f32(sp.add(j)), delta_v, vld1q_f32(kp.add(j)));
                        let n1 = vfmaq_f32(vld1q_f32(sp.add(j + 4)), delta_v, vld1q_f32(kp.add(j + 4)));
                        vst1q_f32(sp.add(j), n0);
                        vst1q_f32(sp.add(j + 4), n1);
                        ro0 = vfmaq_f32(ro0, n0, vld1q_f32(rp.add(j)));
                        ro1 = vfmaq_f32(ro1, n1, vld1q_f32(rp.add(j + 4)));
                        j += 8;
                    }
                    let read_out = rho_i * r_vec[i] * vi + vaddvq_f32(vaddq_f32(ro0, ro1));
                    y_tm[base + i] = read_out * silu(gi);
                }
            }
            #[cfg(not(target_arch = "aarch64"))]
            for i in 0..HEAD_DIM {
                let vi = proj_6d[2 * d + base + i];
                let beta_i = sigmoid(proj_6d[4 * d + base + i]);
                let gi = proj_6d[5 * d + base + i];
                let rho_i = read_f32(self.blob, loff.rho_off + (base + i) * 4);

                let row_s = &mut s_mat[i * HEAD_DIM..(i + 1) * HEAD_DIM];
                let mut v_pred = 0.0f32;
                for j in 0..HEAD_DIM {
                    let decayed = row_s[j] * w_dec[j];
                    row_s[j] = decayed;
                    v_pred += decayed * k_hat[j];
                }
                let delta_i = beta_i * (vi - v_pred);
                let mut read_out = rho_i * r_vec[i] * vi;
                for j in 0..HEAD_DIM {
                    let ns = row_s[j] + delta_i * k_hat[j];
                    row_s[j] = ns;
                    read_out += ns * r_vec[j];
                }
                y_tm[base + i] = read_out * silu(gi);
            }
        }
    }

    /// Advance recurrent state by 1 token and update all cached output heads (`p1`, `mtp_h`).
    pub fn step_token(&self, tok: usize, state: &mut RwkvState) {
        self.step_token_heads(tok, state, true, false);
    }

    /// Advance recurrent state by 1 token, computing `p1` and optionally `mtp_h`.
    /// (`_compute_key1` is retained only for API compatibility; first-key priors are projected scheme-freely in the engine.)
    pub fn step_token_heads(&self, tok: usize, state: &mut RwkvState, compute_mtp: bool, _compute_key1: bool) {
        let s = &self.offs.summary;
        let d = s.d_model;
        let h = s.n_heads;
        let r = s.rank;
        let ffn = s.ffn_dim;

        let mut x = [0.0f32; 512];
        let mut u = [0.0f32; 512];
        let mut s_shift = [0.0f32; 512];
        let mut q_buf = [0i8; 1024];
        let mut proj_6d = [0.0f32; 3072]; // up to 6 * 512
        let mut y_tm = [0.0f32; 512];
        let mut tmp_d = [0.0f32; 512];
        let mut ffn_buf = [0.0f32; 1024];

        // 1. Factorized input embedding -> x[..d]
        let (e_row, e_sc, _) = self.emb_row(tok);
        for k in 0..r {
            q_buf[k] = e_row[k] as i8;
        }
        self.eval_qmat(&self.offs.w_up, &q_buf[..r], e_sc, d, r, &mut x[..d]);

        // 2. Recurrent RWKV-7 Gated DeltaNet + ReLU^2 ChannelMix layers
        let layer_state_stride = h * HEAD_DIM * HEAD_DIM;
        for (l, loff) in self.offs.layers.iter().enumerate() {
            // --- TimeMix ---
            rms_norm(&x[..d], &self.blob[loff.ln1_off..loff.ln1_off + d * 4], &mut u[..d]);
            let prev_tm = &mut state.prev_tm[l * d..(l + 1) * d];
            for i in 0..d {
                let mu = read_f32(self.blob, loff.mu_tm_off + i * 4);
                s_shift[i] = u[i] + mu * (prev_tm[i] - u[i]);
                prev_tm[i] = u[i];
            }

            let tm_sc = quantize_act_i8(&s_shift[..d], &mut q_buf[..d]);
            self.eval_qmat(&loff.tm_proj, &q_buf[..d], tm_sc, 6 * d, d, &mut proj_6d[..6 * d]);

            let st_l = &mut state.mat_state[l * layer_state_stride..(l + 1) * layer_state_stride];
            self.step_layer_heads(loff, d, h, &proj_6d[..6 * d], st_l, &mut y_tm[..d]);

            let y_sc = quantize_act_i8(&y_tm[..d], &mut q_buf[..d]);
            self.eval_qmat(&loff.tm_out, &q_buf[..d], y_sc, d, d, &mut tmp_d[..d]);
            for i in 0..d {
                x[i] += tmp_d[i];
            }

            // --- ChannelMix (ReLU^2 FFN) ---
            rms_norm(&x[..d], &self.blob[loff.ln2_off..loff.ln2_off + d * 4], &mut u[..d]);
            let prev_cm = &mut state.prev_cm[l * d..(l + 1) * d];
            for i in 0..d {
                let mu = read_f32(self.blob, loff.mu_cm_off + i * 4);
                s_shift[i] = u[i] + mu * (prev_cm[i] - u[i]);
                prev_cm[i] = u[i];
            }
            let cm_sc = quantize_act_i8(&s_shift[..d], &mut q_buf[..d]);
            self.eval_qmat(&loff.cm_up, &q_buf[..d], cm_sc, ffn, d, &mut ffn_buf[..ffn]);
            for i in 0..ffn {
                let rv = ffn_buf[i].max(0.0);
                ffn_buf[i] = rv * rv;
            }
            let ffn_sc = quantize_act_i8(&ffn_buf[..ffn], &mut q_buf[..ffn]);
            self.eval_qmat(&loff.cm_dn, &q_buf[..ffn], ffn_sc, d, ffn, &mut tmp_d[..d]);
            for i in 0..d {
                x[i] += tmp_d[i];
            }
        }

        // 3. Final RMSNorm & cached output heads (`p1`, optional `mtp_h`, optional `key1_logp`)
        rms_norm(&x[..d], &self.blob[self.offs.ln_out_off..self.offs.ln_out_off + d * 4], &mut u[..d]);
        let h_sc = quantize_act_i8(&u[..d], &mut q_buf[..d]);

        let mut p1_fp = [0.0f32; 512];
        self.eval_qmat(&self.offs.w_dn, &q_buf[..d], h_sc, r, d, &mut p1_fp[..r]);
        state.p1_scale = quantize_act_i8(&p1_fp[..r], &mut state.p1_q[..r]);

        if compute_mtp {
            self.eval_qmat(&self.offs.w_mtp_h, &q_buf[..d], h_sc, r, d, &mut state.mtp_h[..r]);
        }
    }

    /// Batched RWKV-7 single-token transition across multiple independent `(tok, state, compute_mtp, compute_key1)`
    /// beam hypotheses. Streams the 9.8 MB INT8 weight matrices ONCE per chunk of 8 paths instead of 8 times,
    /// cutting memory traffic & power while remaining 100% bit-exact with `step_token_heads`.
    pub fn step_tokens_batch(&self, items: &mut [(usize, &mut RwkvState, bool, bool)]) {
        if items.is_empty() {
            return;
        }
        let s = &self.offs.summary;
        let d = s.d_model;
        let h = s.n_heads;
        let r = s.rank;
        let ffn = s.ffn_dim;
        let layer_state_stride = h * HEAD_DIM * HEAD_DIM;

        for chunk in items.chunks_mut(BATCH_CHUNK) {
            let cb = chunk.len();
            if cb == 1 {
                let (tok, ref mut st, mtp, key1) = chunk[0];
                self.step_token_heads(tok, st, mtp, key1);
                continue;
            }

            let mut x = [[0.0f32; 512]; BATCH_CHUNK];
            let mut u = [0.0f32; 512];
            let mut s_shift = [0.0f32; 512];
            let mut q_batch = [[0i8; 1024]; BATCH_CHUNK];
            let mut q_scales = [0.0f32; BATCH_CHUNK];
            let mut work = [[0.0f32; 3072]; BATCH_CHUNK];
            let mut y_tm = [0.0f32; 512];

            // 1. Batched input embedding -> x[b][..d]
            for b in 0..cb {
                let (e_row, e_sc, _) = self.emb_row(chunk[b].0);
                for k in 0..r {
                    q_batch[b][k] = e_row[k] as i8;
                }
                q_scales[b] = e_sc;
            }
            {
                let q_refs: [&[i8]; BATCH_CHUNK] = std::array::from_fn(|b| &q_batch[b][..r]);
                let mut outs: Vec<&mut [f32]> =
                    work[..cb].iter_mut().map(|buf| &mut buf[..d]).collect();
                self.eval_qmat_batch(&self.offs.w_up, &q_refs[..cb], &q_scales[..cb], d, r, &mut outs);
            }
            for b in 0..cb {
                x[b][..d].copy_from_slice(&work[b][..d]);
            }

            // 2. Batched layers
            for (l, loff) in self.offs.layers.iter().enumerate() {
                let ln1_bytes = &self.blob[loff.ln1_off..loff.ln1_off + d * 4];
                for b in 0..cb {
                    rms_norm(&x[b][..d], ln1_bytes, &mut u[..d]);
                    let prev_tm = &mut chunk[b].1.prev_tm[l * d..(l + 1) * d];
                    for i in 0..d {
                        let mu = read_f32(self.blob, loff.mu_tm_off + i * 4);
                        s_shift[i] = u[i] + mu * (prev_tm[i] - u[i]);
                        prev_tm[i] = u[i];
                    }
                    q_scales[b] = quantize_act_i8(&s_shift[..d], &mut q_batch[b][..d]);
                }

                {
                    let q_refs: [&[i8]; BATCH_CHUNK] = std::array::from_fn(|b| &q_batch[b][..d]);
                    let mut outs: Vec<&mut [f32]> =
                        work[..cb].iter_mut().map(|buf| &mut buf[..6 * d]).collect();
                    self.eval_qmat_batch(
                        &loff.tm_proj,
                        &q_refs[..cb],
                        &q_scales[..cb],
                        6 * d,
                        d,
                        &mut outs,
                    );
                }

                for b in 0..cb {
                    let st_l = &mut chunk[b].1.mat_state
                        [l * layer_state_stride..(l + 1) * layer_state_stride];
                    self.step_layer_heads(loff, d, h, &work[b][..6 * d], st_l, &mut y_tm[..d]);
                    q_scales[b] = quantize_act_i8(&y_tm[..d], &mut q_batch[b][..d]);
                }

                {
                    let q_refs: [&[i8]; BATCH_CHUNK] = std::array::from_fn(|b| &q_batch[b][..d]);
                    let mut outs: Vec<&mut [f32]> =
                        work[..cb].iter_mut().map(|buf| &mut buf[..d]).collect();
                    self.eval_qmat_batch(&loff.tm_out, &q_refs[..cb], &q_scales[..cb], d, d, &mut outs);
                }
                for b in 0..cb {
                    for i in 0..d {
                        x[b][i] += work[b][i];
                    }
                }

                // ChannelMix
                let ln2_bytes = &self.blob[loff.ln2_off..loff.ln2_off + d * 4];
                for b in 0..cb {
                    rms_norm(&x[b][..d], ln2_bytes, &mut u[..d]);
                    let prev_cm = &mut chunk[b].1.prev_cm[l * d..(l + 1) * d];
                    for i in 0..d {
                        let mu = read_f32(self.blob, loff.mu_cm_off + i * 4);
                        s_shift[i] = u[i] + mu * (prev_cm[i] - u[i]);
                        prev_cm[i] = u[i];
                    }
                    q_scales[b] = quantize_act_i8(&s_shift[..d], &mut q_batch[b][..d]);
                }

                {
                    let q_refs: [&[i8]; BATCH_CHUNK] = std::array::from_fn(|b| &q_batch[b][..d]);
                    let mut outs: Vec<&mut [f32]> =
                        work[..cb].iter_mut().map(|buf| &mut buf[..ffn]).collect();
                    self.eval_qmat_batch(&loff.cm_up, &q_refs[..cb], &q_scales[..cb], ffn, d, &mut outs);
                }

                for b in 0..cb {
                    for i in 0..ffn {
                        let rv = work[b][i].max(0.0);
                        work[b][i] = rv * rv;
                    }
                    q_scales[b] = quantize_act_i8(&work[b][..ffn], &mut q_batch[b][..ffn]);
                }

                {
                    let q_refs: [&[i8]; BATCH_CHUNK] = std::array::from_fn(|b| &q_batch[b][..ffn]);
                    let mut outs: Vec<&mut [f32]> =
                        work[..cb].iter_mut().map(|buf| &mut buf[..d]).collect();
                    self.eval_qmat_batch(&loff.cm_dn, &q_refs[..cb], &q_scales[..cb], d, ffn, &mut outs);
                }
                for b in 0..cb {
                    for i in 0..d {
                        x[b][i] += work[b][i];
                    }
                }
            }

            // 3. Output heads
            let ln_out_bytes = &self.blob[self.offs.ln_out_off..self.offs.ln_out_off + d * 4];
            for b in 0..cb {
                rms_norm(&x[b][..d], ln_out_bytes, &mut u[..d]);
                q_scales[b] = quantize_act_i8(&u[..d], &mut q_batch[b][..d]);
            }
            {
                let q_refs: [&[i8]; BATCH_CHUNK] = std::array::from_fn(|b| &q_batch[b][..d]);
                let mut outs: Vec<&mut [f32]> =
                    work[..cb].iter_mut().map(|buf| &mut buf[..r]).collect();
                self.eval_qmat_batch(&self.offs.w_dn, &q_refs[..cb], &q_scales[..cb], r, d, &mut outs);
            }
            for b in 0..cb {
                chunk[b].1.p1_scale = quantize_act_i8(&work[b][..r], &mut chunk[b].1.p1_q[..r]);
                if chunk[b].2 {
                    self.eval_qmat(
                        &self.offs.w_mtp_h,
                        &q_batch[b][..d],
                        q_scales[b],
                        r,
                        d,
                        &mut chunk[b].1.mtp_h[..r],
                    );
                }
            }
        }
    }

    /// Fast self-normalized log-probability `log P(tok | state)` (`~0.02 µs`).
    #[inline]
    pub fn score_next_token(&self, state: &RwkvState, tok: usize) -> f32 {
        let tok = if tok < self.offs.summary.vocab_size {
            tok
        } else {
            TOK_UNK
        };
        let (e_row, e_sc, ub) = self.emb_row(tok);
        int8_dot_row(e_row, e_sc, &state.p1_q, state.p1_scale) + ub
    }

    /// Fast self-normalized log-probability `log P(ch | state)` (`~0.02 µs` — single `r=128` dot product!).
    #[inline]
    pub fn score_next_char(&self, state: &RwkvState, ch: char) -> f32 {
        let tok = self.char_to_token(ch);
        self.score_next_token(state, tok)
    }

    /// Fast zero-unroll estimate of `log P(latin_word | state)` for beam-search ranking.
    /// Works seamlessly on both `WBLM0002` (native 4,096 ASCII subword Trie) and `WBLM0001` (`<ENG_NUM>`).
    pub fn estimate_latin_logp(&self, state: &RwkvState, word: &str) -> f32 {
        if word.is_empty() {
            return 0.0;
        }
        if let Some(ref trie) = self.offs.ascii_trie {
            let mut toks = Vec::with_capacity(4);
            trie.encode_ascii(word.as_bytes(), &mut toks);
            if let Some(&first_tok) = toks.first() {
                let lp1 = self.score_next_token(state, first_tok);
                if toks.len() == 1 {
                    return lp1;
                }
                let extra: f32 = toks[1..]
                    .iter()
                    .map(|&t| self.emb_row(t).2.clamp(-10.0, 0.0))
                    .sum();
                return (lp1 + 0.55 * extra) / (toks.len() as f32).powf(0.45);
            }
        }
        if self.offs.has_eng_num_token {
            return self.score_next_token(state, TOK_ENG_NUM);
        }
        -3.2
    }

    /// Score and step a Latin/English word through the RWKV recurrent state.
    pub fn score_and_step_latin(
        &self,
        base_state: &RwkvState,
        word: &str,
        compute_mtp: bool,
        compute_key1: bool,
    ) -> (f32, RwkvState) {
        let mut st = base_state.clone();
        if word.is_empty() {
            return (0.0, st);
        }
        if let Some(ref trie) = self.offs.ascii_trie {
            let mut toks = Vec::with_capacity(6);
            trie.encode_ascii(word.as_bytes(), &mut toks);
            if !toks.is_empty() {
                let mut sum_lp = 0.0f32;
                let last_idx = toks.len() - 1;
                for (i, &tok) in toks.iter().enumerate() {
                    sum_lp += self.score_next_token(&st, tok);
                    let is_last = i == last_idx;
                    self.step_token_heads(tok, &mut st, compute_mtp && is_last, compute_key1 && is_last);
                }
                let norm_lp = sum_lp / (toks.len() as f32).powf(0.45);
                return (norm_lp, st);
            }
        }
        if self.offs.has_eng_num_token {
            let lp = self.score_next_token(&st, TOK_ENG_NUM);
            self.step_token_heads(TOK_ENG_NUM, &mut st, compute_mtp, compute_key1);
            return (lp, st);
        }
        (-3.2, st)
    }

    /// Compute `(self_norm_score, log_z)` over the full vocabulary for calibration & exact-NLL evaluation.
    /// Exact log-probability is `self_norm_score - log_z`. Not used on the hot typing path.
    pub fn eval_next_char_calibration(&self, state: &RwkvState, ch: char) -> (f32, f32) {
        let s = &self.offs.summary;
        let v = s.vocab_size;
        let r = s.rank;
        let emb_w = &self.blob[self.offs.emb_w_off..self.offs.emb_w_off + v * r];
        let emb_sc = &self.blob[self.offs.emb_sc_off..self.offs.emb_sc_off + v * 4];
        let uni_b = &self.blob[self.offs.uni_bias_off..self.offs.uni_bias_off + v * 4];
        let mut scores = vec![f32::NEG_INFINITY; v];
        int8_matvec(emb_w, emb_sc, Some(uni_b), &state.p1_q[..r], state.p1_scale, v, r, &mut scores);
        let max_s = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut sum_exp = 0.0f64;
        for &sc in &scores {
            sum_exp += ((sc - max_s) as f64).exp();
        }
        let log_z = max_s + (sum_exp.ln() as f32);
        let tok = self.char_to_token(ch);
        (scores[tok], log_z)
    }

    /// Retrieve top-K highest-probability next CJK characters `(ch, log_prob)` from cached `p1_q` (`~25 µs`).
    pub fn top_next_chars(&self, state: &RwkvState, top_k: usize) -> Vec<(char, f32)> {
        let s = &self.offs.summary;
        let v_cjk = self
            .offs
            .ascii_trie
            .as_ref()
            .map_or(s.vocab_size, |t| t.ext_base_id.min(s.vocab_size));
        let r = s.rank;
        if top_k == 0 || v_cjk <= 2 {
            return Vec::new();
        }
        let emb_w = &self.blob[self.offs.emb_w_off..self.offs.emb_w_off + v_cjk * r];
        let emb_sc = &self.blob[self.offs.emb_sc_off..self.offs.emb_sc_off + v_cjk * 4];
        let uni_b = &self.blob[self.offs.uni_bias_off..self.offs.uni_bias_off + v_cjk * 4];
        let mut scores = vec![f32::NEG_INFINITY; v_cjk];
        int8_matvec(emb_w, emb_sc, Some(uni_b), &state.p1_q[..r], state.p1_scale, v_cjk, r, &mut scores);

        let k = top_k.min(v_cjk.saturating_sub(2));
        let mut best: Vec<(char, f32)> = Vec::with_capacity(k + 1);
        let mut floor = f32::NEG_INFINITY;
        for tok in 2..v_cjk {
            let lp = scores[tok];
            if best.len() == k && lp <= floor {
                continue;
            }
            let ch = self.offs.tok_to_char[tok];
            if ch == '\0' {
                continue;
            }
            let pos = best.partition_point(|x| x.1 > lp);
            if pos < k {
                best.insert(pos, (ch, lp));
                if best.len() > k {
                    best.pop();
                }
                if best.len() == k {
                    floor = best.last().unwrap().1;
                }
            }
        }
        best
    }

    /// Scheme-free runtime projection from universal `P(char | state)` onto any active dictionary's
    /// precomputed `tok -> 26-bit first-key mask` table (`O(1)` lookup per CJK token, zero `.wbaux` bytes).
    pub fn project_first_key_priors(
        &self,
        state: &RwkvState,
        tok_key_masks: &[u32],
    ) -> Option<[u8; 26]> {
        let s = &self.offs.summary;
        let v_cjk = self
            .offs
            .ascii_trie
            .as_ref()
            .map_or(s.vocab_size, |t| t.ext_base_id.min(s.vocab_size))
            .min(tok_key_masks.len());
        let r = s.rank;
        if v_cjk <= 4 {
            return None;
        }
        let emb_w = &self.blob[self.offs.emb_w_off..self.offs.emb_w_off + v_cjk * r];
        let emb_sc = &self.blob[self.offs.emb_sc_off..self.offs.emb_sc_off + v_cjk * 4];
        let uni_b = &self.blob[self.offs.uni_bias_off..self.offs.uni_bias_off + v_cjk * 4];
        let mut scores = vec![f32::NEG_INFINITY; v_cjk];
        int8_matvec(emb_w, emb_sc, Some(uni_b), &state.p1_q[..r], state.p1_scale, v_cjk, r, &mut scores);

        let k = 48usize.min(v_cjk.saturating_sub(2));
        let mut best: [(usize, f32); 48] = [(0, f32::NEG_INFINITY); 48];
        let mut len = 0usize;
        let mut floor = f32::NEG_INFINITY;
        for tok in 2..v_cjk {
            let mask = tok_key_masks[tok];
            if mask == 0 {
                continue;
            }
            let lp = scores[tok];
            if len == k && lp <= floor {
                continue;
            }
            let pos = best[..len].partition_point(|x| x.1 > lp);
            if pos < k {
                if len < k {
                    len += 1;
                }
                best.copy_within(pos..len - 1, pos + 1);
                best[pos] = (tok, lp);
                if len == k {
                    floor = best[len - 1].1;
                }
            }
        }
        if len == 0 {
            return None;
        }
        let mut mass = [0.0f32; 26];
        let mut total = 0.0f32;
        for &(tok, lp) in &best[..len] {
            let mask = tok_key_masks[tok];
            let p = lp.clamp(-13.5, 0.0).exp();
            let share = p / (mask.count_ones() as f32);
            for idx in 0..26usize {
                if (mask & (1u32 << idx)) != 0 {
                    mass[idx] += share;
                }
            }
            total += p;
        }
        if total <= 1e-6 {
            return None;
        }
        let mut out = [100u8; 26];
        let uniform = 1.0 / 26.0;
        for idx in 0..26usize {
            let p = (mass[idx] / total).max(1e-4);
            let delta = (p / uniform).ln().clamp(-2.5, 3.0);
            out[idx] = (100.0 + 22.0 * delta).round().clamp(68.0, 172.0) as u8;
        }
        Some(out)
    }

    /// Compute isolated `<BOS>` lexical cohesion `(mean_cond_logp, mean_pmi)` for a `2..=4` CJK candidate
    /// independent of the preceding sentence context, cleanly separating true words/colloquial phrases
    /// (including function-word phrases like `来个`, `买的`, `最快的`) from accidental cross-word seams (`洁一`, `点我`).
    pub fn isolated_phrase_cohesion(&self, bos_state: &RwkvState, phrase: &str) -> Option<(f32, f32)> {
        let chars: Vec<char> = phrase.chars().collect();
        if chars.len() < 2 || chars.len() > 4 {
            return None;
        }
        let mut st = bos_state.clone();
        self.step_token_heads(self.char_to_token(chars[0]), &mut st, false, false);
        let mut sum_cond = 0.0f32;
        let mut sum_uncond = 0.0f32;
        for (idx, &ch) in chars[1..].iter().enumerate() {
            let lp_cond = self.score_next_char(&st, ch).clamp(-13.5, -0.01);
            let lp_uncond = self.score_next_char(bos_state, ch).clamp(-13.5, -0.01);
            sum_cond += lp_cond;
            sum_uncond += lp_uncond;
            if idx + 2 < chars.len() {
                self.step_token_heads(self.char_to_token(ch), &mut st, false, false);
            }
        }
        let n = (chars.len() - 1) as f32;
        Some((sum_cond / n, (sum_cond - sum_uncond) / n))
    }

    /// Precompute quantized `MTP-2` conditional projection `p2_q` for `c1` given `state.mtp_h` (`~0.4 µs`).
    /// Reuse `p2_q` across many `c2` candidates via `score_mtp2_char` (`~0.02 µs` each).
    #[inline]
    pub fn prepare_mtp2_vec(&self, state: &RwkvState, c1: char, p2_q: &mut [i8; 512]) -> f32 {
        let r = self.offs.summary.rank;
        let tok1 = self.char_to_token(c1);
        let (e1_row, e1_sc, _) = self.emb_row(tok1);
        let mut e1_q = [0i8; 512];
        for k in 0..r {
            e1_q[k] = e1_row[k] as i8;
        }
        let mut c1_contrib = [0.0f32; 512];
        self.eval_qmat(&self.offs.w_mtp_c1, &e1_q[..r], e1_sc, r, r, &mut c1_contrib[..r]);
        for k in 0..r {
            c1_contrib[k] += state.mtp_h[k];
        }
        quantize_act_i8(&c1_contrib[..r], &mut p2_q[..r])
    }

    /// Score `log P(c2 | state, c1)` using a precomputed `MTP-2` vector from `prepare_mtp2_vec` (`~0.02 µs`).
    #[inline]
    pub fn score_mtp2_char(&self, p2_q: &[i8; 512], p2_scale: f32, c2: char) -> f32 {
        let r = self.offs.summary.rank;
        let tok2 = self.char_to_token(c2);
        let (e2_row, e2_sc, ub2) = self.emb_row(tok2);
        int8_dot_row(e2_row, e2_sc, &p2_q[..r], p2_scale) + ub2
    }

    /// Fast zero-unroll phrase log-probability `log P(c1, c2.. | state)` using cached `p1` + `MTP-2` (`~0.4 µs`).
    #[inline]
    pub fn score_phrase_fast(&self, state: &RwkvState, text: &str) -> f32 {
        let mut chars = text.chars();
        let Some(c1) = chars.next() else {
            return 0.0;
        };
        let lp1 = self.score_next_char(state, c1);
        let Some(c2) = chars.next() else {
            return lp1;
        };
        let r = self.offs.summary.rank;
        let tok1 = self.char_to_token(c1);
        let (e1_row, e1_sc, _) = self.emb_row(tok1);
        let mut e1_q = [0i8; 512];
        for k in 0..r {
            e1_q[k] = e1_row[k] as i8;
        }
        let mut c1_contrib = [0.0f32; 512];
        self.eval_qmat(&self.offs.w_mtp_c1, &e1_q[..r], e1_sc, r, r, &mut c1_contrib[..r]);
        for k in 0..r {
            c1_contrib[k] += state.mtp_h[k];
        }
        let mut p2_q = [0i8; 512];
        let p2_sc = quantize_act_i8(&c1_contrib[..r], &mut p2_q[..r]);

        let tok2 = self.char_to_token(c2);
        let (e2_row, e2_sc, ub2) = self.emb_row(tok2);
        let lp2 = int8_dot_row(e2_row, e2_sc, &p2_q[..r], p2_sc) + ub2;

        // Average per-char log-prob * sublinear length factor so 2..4 char phrases calibrate cleanly against 1-char
        let mut sum_lp = lp1 + lp2;
        let mut count = 2usize;
        for c_rest in chars.take(2) {
            let tok_r = self.char_to_token(c_rest);
            let (_, _, ub_r) = self.emb_row(tok_r);
            sum_lp += ub_r;
            count += 1;
        }
        sum_lp / (count as f32).powf(0.65)
    }

    /// Exact autoregressive recurrent unroll over a multi-character candidate (`~38 µs` per extra step,
    /// used selectively on top-4 competing fuzzy multi-character candidates).
    pub fn score_phrase_exact(&self, base_state: &RwkvState, text: &str) -> f32 {
        let mut chars = text.chars();
        let Some(c1) = chars.next() else {
            return 0.0;
        };
        let lp1 = self.score_next_char(base_state, c1);
        let Some(c2) = chars.next() else {
            return lp1;
        };

        let mut tmp_state = base_state.clone();
        self.step_token(self.char_to_token(c1), &mut tmp_state);
        let mut sum_lp = lp1 + self.score_next_char(&tmp_state, c2);
        let mut count = 2usize;
        let mut prev_c = c2;

        for cn in chars.take(2) {
            self.step_token(self.char_to_token(prev_c), &mut tmp_state);
            sum_lp += self.score_next_char(&tmp_state, cn);
            prev_c = cn;
            count += 1;
        }
        sum_lp / (count as f32).powf(0.65)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_tiny_wblm0002(ext_vocab: &[&[u8]]) -> Vec<u8> {
        let vocab_size: usize = 32 + ext_vocab.len();
        let rank: usize = 16;
        let d_model: usize = 32;
        let n_heads: usize = 1;
        let n_layers: usize = 1;
        let ffn_dim: usize = 32;

        let mut blob = Vec::new();
        blob.extend_from_slice(WBLM_MAGIC_V2);
        for v in [vocab_size, rank, d_model, n_heads, n_layers, ffn_dim] {
            blob.extend_from_slice(&(v as u32).to_le_bytes());
        }
        blob.resize(HEADER_LEN, 0);

        let align4 = |b: &mut Vec<u8>| {
            while b.len() % 4 != 0 {
                b.push(0);
            }
        };
        let push_zeros = |b: &mut Vec<u8>, n: usize| {
            align4(b);
            b.resize(b.len() + n, 0);
        };
        let push_qmat = |b: &mut Vec<u8>, rows: usize, cols: usize, has_bias: bool| {
            push_zeros(b, rows * cols);
            push_zeros(b, rows * 4);
            if has_bias {
                push_zeros(b, rows * 4);
            }
        };

        // CJK LUT (map '用'=4, '写'=5, '好'=6)
        align4(&mut blob);
        let lut_start = blob.len();
        blob.resize(lut_start + CJK_LUT_LEN * 2, 0);
        for i in 0..CJK_LUT_LEN {
            let off = lut_start + i * 2;
            blob[off..off + 2].copy_from_slice(&(TOK_UNK as u16).to_le_bytes());
        }
        for (ch, tok) in [('用', 4u16), ('写', 5u16), ('好', 6u16)] {
            let idx = (ch as u32 - CJK_LUT_BASE) as usize;
            let off = lut_start + idx * 2;
            blob[off..off + 2].copy_from_slice(&tok.to_le_bytes());
        }

        push_zeros(&mut blob, vocab_size * rank);
        push_zeros(&mut blob, vocab_size * 4);
        push_zeros(&mut blob, vocab_size * 4);
        push_qmat(&mut blob, d_model, rank, false);
        for _ in 0..n_layers {
            push_zeros(&mut blob, d_model * 4);
            push_zeros(&mut blob, d_model * 4);
            push_qmat(&mut blob, 6 * d_model, d_model, true);
            push_zeros(&mut blob, d_model * 4);
            push_qmat(&mut blob, d_model, d_model, false);
            push_zeros(&mut blob, d_model * 4);
            push_zeros(&mut blob, d_model * 4);
            push_qmat(&mut blob, ffn_dim, d_model, true);
            push_qmat(&mut blob, d_model, ffn_dim, false);
        }
        push_zeros(&mut blob, d_model * 4);
        push_qmat(&mut blob, rank, d_model, true);
        push_qmat(&mut blob, rank, d_model, true);
        push_qmat(&mut blob, rank, rank, false);
        push_qmat(&mut blob, 25, d_model, true);

        // Trailing WBLM0002 EXT_VOCAB section
        align4(&mut blob);
        blob.extend_from_slice(&(32u32).to_le_bytes());
        blob.extend_from_slice(&(ext_vocab.len() as u32).to_le_bytes());
        for entry in ext_vocab {
            blob.push(entry.len() as u8);
            blob.extend_from_slice(entry);
        }
        blob
    }

    #[test]
    fn wblm0002_mixed_trie_and_rare_cjk() {
        let mut ext: Vec<Vec<u8>> = (0x20u8..=0x7Eu8).map(|b| vec![b]).collect();
        let rust_id = 32 + ext.len();
        ext.push(b"Rust".to_vec());
        let v21_id = 32 + ext.len();
        ext.push(b"v2.1".to_vec());
        let ext_refs: Vec<&[u8]> = ext.iter().map(|v| v.as_slice()).collect();

        let blob = build_tiny_wblm0002(&ext_refs);
        let offs = NeuralOffsets::parse(&blob).expect("parse WBLM0002");
        assert!(offs.ascii_trie.is_some());
        assert!(!offs.has_eng_num_token);
        let nv = NeuralView { blob: &blob, offs: &offs };

        let mut toks = Vec::new();
        nv.encode_mixed_str("用 Rust 写 v2.1，𠮷好。", &mut toks);
        assert_eq!(toks, vec![4, rust_id, 5, v21_id, TOK_COMMA, TOK_UNK, 6, TOK_PERIOD]);

        let mut tracker = NeuralContextTracker::default();
        tracker.set_context_str(&nv, "用 Rust 写 v2.1，𠮷好。");
        assert!(tracker.is_ready());
    }

    #[test]
    fn wblm0003_scheme_free_without_key1_head() {
        // Build a WBLM0003 blob omitting the legacy 25xD w_key1 matrix completely
        let mut v3 = build_tiny_wblm0002(&[]);
        // Strip trailing ext_vocab (8 bytes) + w_key1 (25*32 + 25*4 + 25*4 = 1000 bytes), rewrite WBLM0003 + ext_vocab
        let old_ext_off = v3.len() - 8;
        let w_key1_bytes = 25 * 32 + 25 * 4 + 25 * 4;
        v3.truncate(old_ext_off - w_key1_bytes);
        v3[0..8].copy_from_slice(WBLM_MAGIC_V3);
        while v3.len() % 4 != 0 {
            v3.push(0);
        }
        v3.extend_from_slice(&(32u32).to_le_bytes());
        v3.extend_from_slice(&(2u32).to_le_bytes());
        for entry in [&b"review"[..], &b"commit"[..]] {
            v3.push(entry.len() as u8);
            v3.extend_from_slice(entry);
        }
        // Adjust vocab_size in header to 34
        v3[8..12].copy_from_slice(&(34u32).to_le_bytes());
        // Resize emb_w / emb_sc / uni_bias? Easier: build directly from helper logic:
        let vocab_size: usize = 34;
        let rank: usize = 16;
        let d_model: usize = 32;
        let n_heads: usize = 1;
        let n_layers: usize = 1;
        let ffn_dim: usize = 32;
        let mut blob = Vec::new();
        blob.extend_from_slice(WBLM_MAGIC_V3);
        for v in [vocab_size, rank, d_model, n_heads, n_layers, ffn_dim] {
            blob.extend_from_slice(&(v as u32).to_le_bytes());
        }
        blob.resize(HEADER_LEN, 0);
        let align4 = |b: &mut Vec<u8>| {
            while b.len() % 4 != 0 {
                b.push(0);
            }
        };
        let push_zeros = |b: &mut Vec<u8>, n: usize| {
            align4(b);
            b.resize(b.len() + n, 0);
        };
        let push_qmat = |b: &mut Vec<u8>, rows: usize, cols: usize, has_bias: bool| {
            push_zeros(b, rows * cols);
            push_zeros(b, rows * 4);
            if has_bias {
                push_zeros(b, rows * 4);
            }
        };
        push_zeros(&mut blob, CJK_LUT_LEN * 2);
        push_zeros(&mut blob, vocab_size * rank);
        push_zeros(&mut blob, vocab_size * 4);
        push_zeros(&mut blob, vocab_size * 4);
        push_qmat(&mut blob, d_model, rank, false);
        push_zeros(&mut blob, d_model * 4);
        push_zeros(&mut blob, d_model * 4);
        push_qmat(&mut blob, 6 * d_model, d_model, true);
        push_zeros(&mut blob, d_model * 4);
        push_qmat(&mut blob, d_model, d_model, false);
        push_zeros(&mut blob, d_model * 4);
        push_zeros(&mut blob, d_model * 4);
        push_qmat(&mut blob, ffn_dim, d_model, true);
        push_qmat(&mut blob, d_model, ffn_dim, false);
        push_zeros(&mut blob, d_model * 4);
        push_qmat(&mut blob, rank, d_model, true);
        push_qmat(&mut blob, rank, d_model, true);
        push_qmat(&mut blob, rank, rank, false);
        // NO w_key1 here!
        align4(&mut blob);
        blob.extend_from_slice(&(32u32).to_le_bytes());
        blob.extend_from_slice(&(2u32).to_le_bytes());
        for entry in [&b"review"[..], &b"commit"[..]] {
            blob.push(entry.len() as u8);
            blob.extend_from_slice(entry);
        }
        let offs = NeuralOffsets::parse(&blob).expect("parse WBLM0003 without w_key1");
        assert_eq!(offs.ascii_trie.as_ref().unwrap().ext_count, 2);
    }

    #[test]
    fn batch_step_matches_single_step_and_tracker_deduplicates() {
        let mut blob = build_tiny_wblm0002(&[b"a", b"b", b"Rust"]);
        // Populate deterministic pseudo-random non-zero weights/scales after LUT so matmuls exercise non-trivial values
        let lut_end = HEADER_LEN + CJK_LUT_LEN * 2;
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        // Re-build with realistic small scales by filling weight bytes and 1.0/16.0 scales cleanly via helper:
        blob.clear();
        let vocab_size: usize = 35;
        let rank: usize = 32;
        let d_model: usize = 64;
        let n_heads: usize = 2;
        let n_layers: usize = 2;
        let ffn_dim: usize = 96;
        blob.extend_from_slice(WBLM_MAGIC_V2);
        for v in [vocab_size, rank, d_model, n_heads, n_layers, ffn_dim] {
            blob.extend_from_slice(&(v as u32).to_le_bytes());
        }
        blob.resize(HEADER_LEN, 0);
        let mut next_u8 = || -> u8 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed & 0xFF) as u8
        };
        let align4 = |b: &mut Vec<u8>| {
            while b.len() % 4 != 0 {
                b.push(0);
            }
        };
        let push_i8 = |b: &mut Vec<u8>, n: usize, rng: &mut dyn FnMut() -> u8| {
            align4(b);
            for _ in 0..n {
                let v = ((rng() % 31) as i8) - 15;
                b.push(v as u8);
            }
        };
        let push_f32 = |b: &mut Vec<u8>, n: usize, scale: f32, rng: &mut dyn FnMut() -> u8| {
            align4(b);
            for _ in 0..n {
                let f = ((rng() as f32 / 255.0) - 0.5) * scale;
                b.extend_from_slice(&f.to_le_bytes());
            }
        };
        let push_pos_f32 = |b: &mut Vec<u8>, n: usize, scale: f32, rng: &mut dyn FnMut() -> u8| {
            align4(b);
            for _ in 0..n {
                let f = 0.002 + (rng() as f32 / 255.0) * scale;
                b.extend_from_slice(&f.to_le_bytes());
            }
        };
        let push_qmat = |b: &mut Vec<u8>, rows: usize, cols: usize, has_bias: bool, rng: &mut dyn FnMut() -> u8| {
            push_i8(b, rows * cols, rng);
            push_pos_f32(b, rows, 0.01, rng);
            if has_bias {
                push_f32(b, rows, 0.05, rng);
            }
        };
        let _ = lut_end;
        align4(&mut blob);
        let lut_start = blob.len();
        blob.resize(lut_start + CJK_LUT_LEN * 2, 0);
        for (i, ch_u32) in (CJK_LUT_BASE..(CJK_LUT_BASE + 20)).enumerate() {
            let _ = ch_u32;
            let off = lut_start + i * 2;
            blob[off..off + 2].copy_from_slice(&((4 + i) as u16).to_le_bytes());
        }
        push_i8(&mut blob, vocab_size * rank, &mut next_u8);
        push_pos_f32(&mut blob, vocab_size, 0.02, &mut next_u8);
        push_f32(&mut blob, vocab_size, 0.2, &mut next_u8);
        push_qmat(&mut blob, d_model, rank, false, &mut next_u8);
        for _ in 0..n_layers {
            push_pos_f32(&mut blob, d_model, 0.8, &mut next_u8);
            push_f32(&mut blob, d_model, 0.3, &mut next_u8);
            push_qmat(&mut blob, 6 * d_model, d_model, true, &mut next_u8);
            push_f32(&mut blob, d_model, 0.3, &mut next_u8);
            push_qmat(&mut blob, d_model, d_model, false, &mut next_u8);
            push_pos_f32(&mut blob, d_model, 0.8, &mut next_u8);
            push_f32(&mut blob, d_model, 0.3, &mut next_u8);
            push_qmat(&mut blob, ffn_dim, d_model, true, &mut next_u8);
            push_qmat(&mut blob, d_model, ffn_dim, false, &mut next_u8);
        }
        push_pos_f32(&mut blob, d_model, 0.8, &mut next_u8);
        push_qmat(&mut blob, rank, d_model, true, &mut next_u8);
        push_qmat(&mut blob, rank, d_model, true, &mut next_u8);
        push_qmat(&mut blob, rank, rank, false, &mut next_u8);
        push_qmat(&mut blob, 25, d_model, true, &mut next_u8);
        align4(&mut blob);
        blob.extend_from_slice(&(32u32).to_le_bytes());
        blob.extend_from_slice(&(3u32).to_le_bytes());
        for entry in [&b"a"[..], &b"b"[..], &b"Rust"[..]] {
            blob.push(entry.len() as u8);
            blob.extend_from_slice(entry);
        }

        let offs = NeuralOffsets::parse(&blob).expect("parse synthetic WBLM0002");
        let nv = NeuralView { blob: &blob, offs: &offs };

        for bsz in [1usize, 2, 5, 8, 11] {
            let mut base = RwkvState::new_zero(&offs.summary);
            nv.step_token(0, &mut base);
            let mut single_states: Vec<RwkvState> = (0..bsz).map(|_| base.clone()).collect();
            let mut batch_states: Vec<RwkvState> = (0..bsz).map(|_| base.clone()).collect();
            let toks: Vec<usize> = (0..bsz).map(|i| 4 + (i % 16)).collect();
            let mtp_flags: Vec<bool> = (0..bsz).map(|i| i < 2 || i % 3 == 0).collect();

            let key1_flags: Vec<bool> = (0..bsz).map(|i| i % 4 == 0).collect();

            for i in 0..bsz {
                nv.step_token_heads(toks[i], &mut single_states[i], mtp_flags[i], key1_flags[i]);
            }
            let mut refs: Vec<(usize, &mut RwkvState, bool, bool)> = toks
                .iter()
                .copied()
                .zip(batch_states.iter_mut())
                .zip(mtp_flags.iter().copied())
                .zip(key1_flags.iter().copied())
                .map(|(((t, s), m), k)| (t, s, m, k))
                .collect();
            nv.step_tokens_batch(&mut refs);

            for i in 0..bsz {
                assert_eq!(single_states[i].p1_q, batch_states[i].p1_q, "p1_q mismatch at bsz={bsz} i={i}");
                assert!((single_states[i].p1_scale - batch_states[i].p1_scale).abs() < 1e-6);
                for (a, b) in single_states[i].mat_state.iter().zip(batch_states[i].mat_state.iter()) {
                    assert!((a - b).abs() < 1e-5, "mat_state diff {a} vs {b}");
                }
                if mtp_flags[i] {
                    for (a, b) in single_states[i].mtp_h.iter().zip(batch_states[i].mtp_h.iter()) {
                        assert!((a - b).abs() < 1e-5, "mtp_h diff {a} vs {b}");
                    }
                }
            }
        }

        // Verify `NeuralContextTracker` exact-match, append-delta, and ring-prefix recovery
        let c0 = char::from_u32(CJK_LUT_BASE).unwrap();
        let c1 = char::from_u32(CJK_LUT_BASE + 1).unwrap();
        let c2 = char::from_u32(CJK_LUT_BASE + 2).unwrap();
        let word1: String = [c0, c1].iter().collect();
        let full: String = [c0, c1, c2].iter().collect();
        let after_bs1: String = [c0].iter().collect();

        let mut inc = NeuralContextTracker::default();
        inc.commit_cjk_str(&nv, &word1);
        let ring_before = inc.ring.len();
        inc.set_context_str(&nv, &word1); // exact O(1) hit!
        assert_eq!(inc.ring.len(), ring_before);

        inc.set_context_str(&nv, &full); // append delta [c2]
        let mut cold = NeuralContextTracker::default();
        cold.set_context_str(&nv, &full);
        assert_eq!(inc.active.p1_q, cold.active.p1_q);

        // Partial 1-char backspace into `word1` recovers from `ring[0]` (<BOS>) + 1 step (`c0`)
        inc.set_context_str(&nv, &after_bs1);
        let mut cold_bs = NeuralContextTracker::default();
        cold_bs.set_context_str(&nv, &after_bs1);
        assert_eq!(inc.active.p1_q, cold_bs.active.p1_q);
    }
}
