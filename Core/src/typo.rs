use crate::code::{PackedCode, B0, B1, B2, B3};
use crate::config::TypoConfig;
use crate::user_data::TouchProfile;

/// Observation of a single keystroke (either physical key press or touchscreen `(x, y)` tap).
#[derive(Clone, Copy, Debug)]
pub struct KeyObs {
    pub ch: u8,
    pub has_touch: bool,
    /// Touch coordinates in normalized key-pitch units (or view coordinates converted via `KeyLayout`).
    pub norm_x: f32,
    pub norm_y: f32,
    pub timestamp_ms: u64,
}

impl KeyObs {
    pub fn physical(ch: u8, timestamp_ms: u64) -> Self {
        Self {
            ch: ch.to_ascii_lowercase(),
            has_touch: false,
            norm_x: 0.0,
            norm_y: 0.0,
            timestamp_ms,
        }
    }

    pub fn touch(ch: u8, norm_x: f32, norm_y: f32, timestamp_ms: u64) -> Self {
        Self {
            ch: ch.to_ascii_lowercase(),
            has_touch: true,
            norm_x,
            norm_y,
            timestamp_ms,
        }
    }
}

/// Canonical 26-key layout geometry in key-pitch units (`pitch_x = 1.0`, `pitch_y = 1.0`).
/// Defaults to Apple's symmetric 26-key iOS/macOS QWERTY geometry:
/// - Row 0 (`q..p`): `y = 0.0, x = 0.0 .. 9.0`
/// - Row 1 (`a..l`): `y = 1.0, x = 0.5 .. 8.5`
/// - Row 2 (`z..m`): `y = 2.0, x = 1.5 .. 7.5`
#[derive(Clone, Debug)]
pub struct KeyLayout {
    pub centers: [(f32, f32); 26],
    /// Optional view-to-pitch linear transform (`norm_x = (view_x - origin_x) / pitch_x`)
    pub origin_x: f32,
    pub origin_y: f32,
    pub pitch_x: f32,
    pub pitch_y: f32,
}

impl Default for KeyLayout {
    fn default() -> Self {
        let mut centers = [(0.0f32, 0.0f32); 26];
        for (i, &b) in b"qwertyuiop".iter().enumerate() {
            centers[(b - b'a') as usize] = (i as f32, 0.0);
        }
        for (i, &b) in b"asdfghjkl".iter().enumerate() {
            centers[(b - b'a') as usize] = (i as f32 + 0.5, 1.0);
        }
        for (i, &b) in b"zxcvbnm".iter().enumerate() {
            centers[(b - b'a') as usize] = (i as f32 + 1.5, 2.0);
        }
        Self {
            centers,
            origin_x: 0.0,
            origin_y: 0.0,
            pitch_x: 1.0,
            pitch_y: 1.0,
        }
    }
}

impl KeyLayout {
    #[inline]
    pub fn center_of(&self, key_byte: u8) -> (f32, f32) {
        if key_byte.is_ascii_lowercase() {
            self.centers[(key_byte - b'a') as usize]
        } else {
            (0.0, 0.0)
        }
    }

    #[inline]
    pub fn view_to_norm(&self, view_x: f32, view_y: f32) -> (f32, f32) {
        let px = if self.pitch_x.abs() > 1e-4 {
            self.pitch_x
        } else {
            1.0
        };
        let py = if self.pitch_y.abs() > 1e-4 {
            self.pitch_y
        } else {
            1.0
        };
        ((view_x - self.origin_x) / px, (view_y - self.origin_y) / py)
    }

    /// Find the closest letter key ('a'..='z') to normalized coordinates `(nx, ny)`
    /// taking the user's adaptive touch profile into account.
    pub fn nearest_key(&self, nx: f32, ny: f32, profile: &TouchProfile, adapt: bool) -> u8 {
        let mut best_k = b'a';
        let mut best_d2 = f32::MAX;
        for idx in 0..26u8 {
            let k = b'a' + idx;
            let (cx, cy) = self.centers[idx as usize];
            let (dx, dy) = if adapt {
                profile.offset_for(k)
            } else {
                (0.0, 0.0)
            };
            let d2 = (nx - (cx + dx)).powi(2) + (ny - (cy + dy)).powi(2);
            if d2 < best_d2 {
                best_d2 = d2;
                best_k = k;
            }
        }
        best_k
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorrectionKind {
    Exact,
    TouchBoundary,
    NeighborKey,
    Transposition,
}

#[derive(Clone, Copy, Debug)]
pub struct CodeHypothesis {
    pub code: PackedCode,
    /// Relative spatial/keystroke log-likelihood (`0.0` for top spatial path, negative for alternatives).
    pub spatial_logp: f32,
    pub kind: CorrectionKind,
    /// True if at least one substituted position had a touch near the border of two keys.
    pub boundary_slip: bool,
}

const SIGMA_X: f32 = 0.34;
const SIGMA_Y: f32 = 0.36;
const MAX_BEAM: usize = 24;

/// Generate ranked code hypotheses (`'a'..='y'`, or `'a'..='z'` when `allow_z` is true)
/// for `observations` (`len = 1..=4`).
pub fn generate_hypotheses(
    observations: &[KeyObs],
    layout: &KeyLayout,
    profile: &TouchProfile,
    cfg: &TypoConfig,
) -> Vec<CodeHypothesis> {
    generate_hypotheses_with_options(observations, layout, profile, cfg, false)
}

/// Generate per-keystroke candidate choices `(digit_1_to_26, rel_logp, is_pressed_key, is_boundary)`.
pub fn key_choices_for_obs(
    obs: &KeyObs,
    layout: &KeyLayout,
    profile: &TouchProfile,
    cfg: &TypoConfig,
    allow_z: bool,
) -> Vec<(u8, f32, bool, bool)> {
    let max_d = if allow_z { 26u8 } else { 25u8 };
    let max_ch = if allow_z { b'z' } else { b'y' };
    let mut choices = Vec::<(u8, f32, bool, bool)>::new();
    if obs.has_touch {
        let mut raw_scores = [(0u8, f32::NEG_INFINITY); 26];
        let mut max_lp = f32::NEG_INFINITY;
        for d in 1..=max_d {
            let k = b'a' + d - 1;
            let (cx, cy) = layout.center_of(k);
            let (ox, oy) = if cfg.touch_hitbox_adapt {
                profile.offset_for(k)
            } else {
                (0.0, 0.0)
            };
            let dx = obs.norm_x - (cx + ox);
            let dy = obs.norm_y - (cy + oy);
            let lp = -(dx * dx) / (2.0 * SIGMA_X * SIGMA_X)
                - (dy * dy) / (2.0 * SIGMA_Y * SIGMA_Y);
            raw_scores[(d - 1) as usize] = (d, lp);
            if lp > max_lp {
                max_lp = lp;
            }
        }
        let slice = &mut raw_scores[..max_d as usize];
        slice.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        let second_rel = slice[1].1 - max_lp;
        let near_border = second_rel >= -1.25; // Within ~0.40 key-widths of midpoint!

        let pressed_d = if (b'a'..=max_ch).contains(&obs.ch) {
            obs.ch - b'a' + 1
        } else {
            slice[0].0
        };

        for &(d, lp) in slice.iter().take(4) {
            let rel = lp - max_lp;
            if rel >= -4.8 || d == pressed_d {
                let is_pressed = d == pressed_d;
                choices.push((d, rel, is_pressed, !is_pressed && near_border && rel >= -1.45));
            }
        }
        if !choices.iter().any(|x| x.0 == pressed_d) {
            let pressed_lp = slice
                .iter()
                .find(|x| x.0 == pressed_d)
                .map(|x| x.1 - max_lp)
                .unwrap_or(-4.5);
            choices.push((pressed_d, pressed_lp, true, false));
        }
    } else {
        // Physical keyboard adjacency confusion model
        let pressed_d = if (b'a'..=max_ch).contains(&obs.ch) {
            obs.ch - b'a' + 1
        } else {
            return choices;
        };
        let (cx0, cy0) = layout.center_of(obs.ch);
        choices.push((pressed_d, 0.0, true, false));
        let mut neighbors = Vec::<(u8, f32)>::new();
        for d in 1..=max_d {
            if d == pressed_d {
                continue;
            }
            let k = b'a' + d - 1;
            let (cx, cy) = layout.center_of(k);
            let dx = (cx - cx0).abs();
            let dy = (cy - cy0).abs();
            if dy < 0.1 && dx <= 1.05 {
                // Horizontal adjacent key on same row
                neighbors.push((d, -3.15));
            } else if dy <= 1.05 && dx <= 0.85 {
                // Adjacent row neighbor
                neighbors.push((d, -3.80));
            }
        }
        for (nd, lp) in neighbors.into_iter().take(4) {
            choices.push((nd, lp, false, false));
        }
    }
    choices
}

pub fn generate_hypotheses_with_options(
    observations: &[KeyObs],
    layout: &KeyLayout,
    profile: &TouchProfile,
    cfg: &TypoConfig,
    allow_z: bool,
) -> Vec<CodeHypothesis> {
    if observations.is_empty() || observations.len() > 5 {
        return Vec::new();
    }
    let raw_bytes: Vec<u8> = observations.iter().map(|o| o.ch).collect();
    let exact_packed = PackedCode::from_bytes(&raw_bytes);

    // If typo correction is disabled or `'z'` omnipotent query is being used (in Wubi), only return exact.
    if !cfg.enabled || (!allow_z && raw_bytes.contains(&b'z')) {
        return exact_packed
            .map(|code| {
                vec![CodeHypothesis {
                    code,
                    spatial_logp: 0.0,
                    kind: CorrectionKind::Exact,
                    boundary_slip: false,
                }]
            })
            .unwrap_or_default();
    }

    // Step 1: Per-position candidate keys
    let mut step_choices: Vec<Vec<(u8, f32, bool, bool)>> = Vec::with_capacity(observations.len());
    for obs in observations {
        let choices = key_choices_for_obs(obs, layout, profile, cfg, allow_z);
        if choices.is_empty() {
            return Vec::new();
        }
        step_choices.push(choices);
    }

    // Step 2: Beam search across positions 0..len
    // Tuple: `(digits: [u8; 5], len: usize, cum_logp: f32, all_exact: bool, any_boundary: bool, edits: u8)`
    let mut beam: Vec<([u8; 5], usize, f32, bool, bool, u8)> =
        vec![([0u8; 5], 0, 0.0, true, false, 0)];

    for choices in &step_choices {
        let mut next_beam = Vec::with_capacity(beam.len() * choices.len());
        for &(digits, len, cum_lp, all_exact, any_boundary, edits) in &beam {
            for &(d, lp, is_exact_step, is_boundary_step) in choices {
                let new_edits = edits + u8::from(!is_exact_step);
                // Allow up to 2 edits on touch (e.g. fast two-thumb slips), 1 edit on physical keyboard
                let max_edits = if observations[0].has_touch { 2 } else { 1 };
                if new_edits > max_edits {
                    continue;
                }
                let new_lp = cum_lp + lp;
                if new_lp < -5.2 && !all_exact {
                    continue;
                }
                let mut nd = digits;
                nd[len] = d;
                next_beam.push((
                    nd,
                    len + 1,
                    new_lp,
                    all_exact && is_exact_step,
                    any_boundary || is_boundary_step,
                    new_edits,
                ));
            }
        }
        next_beam.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(core::cmp::Ordering::Equal));
        next_beam.truncate(MAX_BEAM);
        beam = next_beam;
    }

    let weights = [B0, B1, B2, B3, 1];
    let mut out = Vec::<CodeHypothesis>::with_capacity(beam.len() + 3);
    for (digits, len, lp, all_exact, any_boundary, _) in beam {
        let mut raw = 0u32;
        for i in 0..len {
            raw += (digits[i] as u32) * weights[i];
        }
        let kind = if all_exact {
            CorrectionKind::Exact
        } else if any_boundary {
            CorrectionKind::TouchBoundary
        } else {
            CorrectionKind::NeighborKey
        };
        out.push(CodeHypothesis {
            code: PackedCode(raw),
            spatial_logp: lp,
            kind,
            boundary_slip: any_boundary,
        });
    }

    // Step 3: Adjacent keystroke transposition (`ab -> ba`) for rapid bursts
    let max_ch = if allow_z { b'z' } else { b'y' };
    if observations.len() >= 2 && raw_bytes.iter().all(|b| (b'a'..=max_ch).contains(b)) {
        for i in 0..(observations.len() - 1) {
            if raw_bytes[i] == raw_bytes[i + 1] {
                continue;
            }
            let dt = observations[i + 1]
                .timestamp_ms
                .saturating_sub(observations[i].timestamp_ms);
            let trans_lp = if observations[i + 1].timestamp_ms > 0 && dt <= 65 {
                -2.45 // Very rapid two-hand/two-thumb rollover!
            } else {
                -3.45
            };
            let mut swapped = raw_bytes.clone();
            swapped.swap(i, i + 1);
            if let Some(code) = PackedCode::from_bytes(&swapped) {
                if !out.iter().any(|h| h.code == code) {
                    out.push(CodeHypothesis {
                        code,
                        spatial_logp: trans_lp,
                        kind: CorrectionKind::Transposition,
                        boundary_slip: false,
                    });
                }
            }
        }
    }

    // Ensure exact code is always present even if touch landed slightly across a key boundary!
    if let Some(ec) = exact_packed {
        if !out.iter().any(|h| h.code == ec) {
            out.push(CodeHypothesis {
                code: ec,
                spatial_logp: -1.5,
                kind: CorrectionKind::Exact,
                boundary_slip: false,
            });
        }
    }

    out
}

/// Convert raw unnormalized log-scores of top candidates into calibrated Softmax probabilities.
pub fn softmax_confidences(scores: &[f32]) -> Vec<f32> {
    if scores.is_empty() {
        return Vec::new();
    }
    let max_s = scores
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let mut exps = Vec::with_capacity(scores.len());
    let mut sum = 0.0f32;
    for &s in scores {
        let e = (s - max_s).clamp(-20.0, 0.0).exp();
        exps.push(e);
        sum += e;
    }
    if sum <= 0.0 {
        return vec![0.0; scores.len()];
    }
    exps.into_iter().map(|e| e / sum).collect()
}
