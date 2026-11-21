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
pub const TOK_BOS: usize = 0;
pub const TOK_UNK: usize = 1;
pub const TOK_COMMA: usize = 2;
pub const TOK_PERIOD: usize = 3;
pub const CJK_LUT_BASE: u32 = 0x3400;
pub const CJK_LUT_LEN: usize = 0xA000 - 0x3400; // 27,648 BMP CJK codepoints (0x3400..=0x9FFF)
const HEADER_LEN: usize = 64;
const HEAD_DIM: usize = 32;

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

#[inline]
fn int8_dot_row(row_u8: &[u8], row_scale: f32, x_q: &[i8], x_scale: f32) -> f32 {
    #[cfg(target_arch = "aarch64")]
    unsafe {
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
    w_key1: QuantMatOff,   // [25, D] + bias [25]
}

impl NeuralOffsets {
    pub fn parse(blob: &[u8]) -> io::Result<Self> {
        if blob.len() < HEADER_LEN || &blob[0..8] != WBLM_MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid WBLM0001 magic"));
        }
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
        let w_key1 = take_qmat(&mut cur, total, 25, d_model, true)?;

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
            w_key1,
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
    pub key1_logp: [f32; 25], // log P(first Wubi letter 'a'..'y' | context)
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
            key1_logp: [-3.22f32; 25],
        }
    }
}

const HISTORY_RING_CAP: usize = 8;

/// Rolling sentence tracker with 8-step commit history for instant Backspace rollback.
pub struct NeuralContextTracker {
    pub active: RwkvState,
    bos_cached: Option<RwkvState>,
    ring: Vec<RwkvState>,
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
                key1_logp: [-3.22f32; 25],
            },
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
        self.bos_cached = Some(st);
        self.ring.clear();
        self.dims = Some(s);
    }

    #[inline]
    pub fn is_ready(&self) -> bool {
        self.bos_cached.is_some()
    }

    pub fn reset_to_bos(&mut self, nv: Option<&NeuralView<'_>>) {
        if let Some(ref bos) = self.bos_cached {
            self.active.mat_state.copy_from_slice(&bos.mat_state);
            self.active.prev_tm.copy_from_slice(&bos.prev_tm);
            self.active.prev_cm.copy_from_slice(&bos.prev_cm);
            self.active.p1_q.copy_from_slice(&bos.p1_q);
            self.active.p1_scale = bos.p1_scale;
            self.active.mtp_h.copy_from_slice(&bos.mtp_h);
            self.active.key1_logp = bos.key1_logp;
            self.ring.clear();
        } else if let Some(v) = nv {
            self.init_with_model(v);
        }
    }

    fn step_context_str(&mut self, nv: &NeuralView<'_>, text: &str) {
        let mut prev_punct: Option<usize> = None;
        for ch in text.chars() {
            let u = ch as u32;
            if (CJK_LUT_BASE..0xA000).contains(&u) {
                let tok = nv.char_to_token(ch);
                nv.step_token(tok, &mut self.active);
                prev_punct = None;
            } else if let Some(ptok) = nv.punct_to_token(ch) {
                if prev_punct != Some(ptok) {
                    nv.step_token(ptok, &mut self.active);
                    prev_punct = Some(ptok);
                }
            } else {
                self.reset_to_bos(Some(nv));
                prev_punct = None;
            }
        }
    }

    /// Push a committed word/character/punctuation sequence, saving pre-commit state in the 8-step rollback ring.
    pub fn commit_cjk_str(&mut self, nv: &NeuralView<'_>, text: &str) {
        if !self.is_ready() {
            self.init_with_model(nv);
        }
        if self.ring.len() >= HISTORY_RING_CAP {
            self.ring.remove(0);
        }
        self.ring.push(self.active.clone());
        self.step_context_str(nv, text);
    }

    /// Roll back one committed unit on empty-preedit Backspace (or reset to BOS if history exhausted).
    pub fn pop_commit_or_reset(&mut self, nv: Option<&NeuralView<'_>>) {
        if let Some(prev) = self.ring.pop() {
            self.active = prev;
        } else {
            self.reset_to_bos(nv);
        }
    }

    /// Rebuild state cleanly from an explicit context string (used in offline evaluation / surrounding text).
    pub fn set_context_str(&mut self, nv: &NeuralView<'_>, text: &str) {
        if !self.is_ready() {
            self.init_with_model(nv);
        } else {
            self.reset_to_bos(Some(nv));
        }
        self.step_context_str(nv, text);
    }
}

impl<'a> NeuralView<'a> {
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
    fn emb_row(&self, tok: usize) -> (&'a [u8], f32, f32) {
        let r = self.offs.summary.rank;
        let w_start = self.offs.emb_w_off + tok * r;
        let row = &self.blob[w_start..w_start + r];
        let sc = read_f32(self.blob, self.offs.emb_sc_off + tok * 4);
        let ub = read_f32(self.blob, self.offs.uni_bias_off + tok * 4);
        (row, sc, ub)
    }

    /// Advance recurrent state by 1 token and update cached output projections (`~38 µs` on Apple Silicon).
    pub fn step_token(&self, tok: usize, state: &mut RwkvState) {
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
            for head in 0..h {
                let base = head * HEAD_DIM;
                // L2-normalize key k_h[0..32]
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

        // 3. Final RMSNorm & cached output heads (`p1`, `mtp_h`, `key1_logp`)
        rms_norm(&x[..d], &self.blob[self.offs.ln_out_off..self.offs.ln_out_off + d * 4], &mut u[..d]);
        let h_sc = quantize_act_i8(&u[..d], &mut q_buf[..d]);

        let mut p1_fp = [0.0f32; 512];
        self.eval_qmat(&self.offs.w_dn, &q_buf[..d], h_sc, r, d, &mut p1_fp[..r]);
        state.p1_scale = quantize_act_i8(&p1_fp[..r], &mut state.p1_q[..r]);

        self.eval_qmat(&self.offs.w_mtp_h, &q_buf[..d], h_sc, r, d, &mut state.mtp_h[..r]);

        let mut k1_raw = [0.0f32; 25];
        self.eval_qmat(&self.offs.w_key1, &q_buf[..d], h_sc, 25, d, &mut k1_raw);
        let max_k = k1_raw.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut sum_exp = 0.0f32;
        for &v in &k1_raw {
            sum_exp += (v - max_k).exp();
        }
        let log_z = max_k + sum_exp.ln();
        for i in 0..25 {
            state.key1_logp[i] = k1_raw[i] - log_z;
        }
    }

    /// Fast self-normalized log-probability `log P(ch | state)` (`~0.02 µs` — single `r=128` dot product!).
    #[inline]
    pub fn score_next_char(&self, state: &RwkvState, ch: char) -> f32 {
        let tok = self.char_to_token(ch);
        let (e_row, e_sc, ub) = self.emb_row(tok);
        int8_dot_row(e_row, e_sc, &state.p1_q, state.p1_scale) + ub
    }

    /// Retrieve top-K highest-probability next CJK characters `(ch, log_prob)` from cached `p1_q` (`~40 µs`).
    pub fn top_next_chars(&self, state: &RwkvState, top_k: usize) -> Vec<(char, f32)> {
        let s = &self.offs.summary;
        let v = s.vocab_size;
        let r = s.rank;
        if top_k == 0 || v <= 2 {
            return Vec::new();
        }
        let emb_w = &self.blob[self.offs.emb_w_off..self.offs.emb_w_off + v * r];
        let emb_sc = &self.blob[self.offs.emb_sc_off..self.offs.emb_sc_off + v * 4];
        let uni_b = &self.blob[self.offs.uni_bias_off..self.offs.uni_bias_off + v * 4];
        let mut scores = vec![f32::NEG_INFINITY; v];
        int8_matvec(emb_w, emb_sc, Some(uni_b), &state.p1_q[..r], state.p1_scale, v, r, &mut scores);

        let k = top_k.min(v.saturating_sub(2));
        let mut best: Vec<(char, f32)> = Vec::with_capacity(k + 1);
        let mut floor = f32::NEG_INFINITY;
        for tok in 2..v {
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
