use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use juxu_core::code::PackedCode;
use juxu_core::config::{InputSchema, JuxuConfig};
use juxu_core::engine::{JuxuEngine, LmWeights};
use juxu_core::eval_support::{
    pct, segment_wubi_sentence, wubi_neighbor_keys as neighbor_keys, XorshiftRng as Rng,
};
use juxu_core::format::{AuxView, DictView};
use juxu_core::typo::KeyLayout;

fn segment_sentence(dict: &DictView, sent: &str) -> Vec<(String, String)> {
    segment_wubi_sentence(dict, sent, false)
}

#[derive(Default, Clone, Debug)]
struct Metrics {
    // 1. Touch boundary slip onto a VALID colliding code (hardest disambiguation)
    touch_coll_n: usize,
    touch_coll_top1: usize,
    touch_coll_top3: usize,
    // 2. Touch boundary slip onto an EMPTY code (empty-code rescue)
    touch_empty_n: usize,
    touch_empty_top1: usize,
    touch_empty_top3: usize,
    // 3. Physical neighbor key typo (empty-code rescue)
    phys_empty_n: usize,
    phys_empty_top1: usize,
    phys_empty_top3: usize,
    // 4. Correct touch near border (0.36..0.46 toward neighbor, still on intended key): false positive rate!
    correct_border_n: usize,
    correct_border_fp: usize,
}

impl Metrics {
    fn add(&mut self, o: &Metrics) {
        self.touch_coll_n += o.touch_coll_n;
        self.touch_coll_top1 += o.touch_coll_top1;
        self.touch_coll_top3 += o.touch_coll_top3;
        self.touch_empty_n += o.touch_empty_n;
        self.touch_empty_top1 += o.touch_empty_top1;
        self.touch_empty_top3 += o.touch_empty_top3;
        self.phys_empty_n += o.phys_empty_n;
        self.phys_empty_top1 += o.phys_empty_top1;
        self.phys_empty_top3 += o.phys_empty_top3;
        self.correct_border_n += o.correct_border_n;
        self.correct_border_fp += o.correct_border_fp;
    }

    fn pct(a: usize, b: usize) -> f64 {
        if b == 0 {
            0.0
        } else {
            100.0 * (a as f64) / (b as f64)
        }
    }

    fn print_row(&self, label: &str) {
        println!(
            "{:<20} | TouchColl({:4}): T1={:5.2}% T3={:5.2}% | TouchEmpty({:4}): T1={:5.2}% T3={:5.2}% | PhysEmpty({:4}): T1={:5.2}% T3={:5.2}% | BorderFP({:4}): {:5.2}%",
            label,
            self.touch_coll_n,
            Self::pct(self.touch_coll_top1, self.touch_coll_n),
            Self::pct(self.touch_coll_top3, self.touch_coll_n),
            self.touch_empty_n,
            Self::pct(self.touch_empty_top1, self.touch_empty_n),
            Self::pct(self.touch_empty_top3, self.touch_empty_n),
            self.phys_empty_n,
            Self::pct(self.phys_empty_top1, self.phys_empty_n),
            Self::pct(self.phys_empty_top3, self.phys_empty_n),
            self.correct_border_n,
            Self::pct(self.correct_border_fp, self.correct_border_n),
        );
    }

    /// Combined validation objective (higher is better; penalizes false positives strongly)
    fn objective(&self) -> f64 {
        let tc1 = Self::pct(self.touch_coll_top1, self.touch_coll_n);
        let tc3 = Self::pct(self.touch_coll_top3, self.touch_coll_n);
        let te1 = Self::pct(self.touch_empty_top1, self.touch_empty_n);
        let pe1 = Self::pct(self.phys_empty_top1, self.phys_empty_n);
        let fp = Self::pct(self.correct_border_fp, self.correct_border_n);
        0.40 * tc1 + 0.15 * tc3 + 0.30 * te1 + 0.15 * pe1 - 2.5 * fp
    }
}

fn run_eval_with_engine(
    engine: &mut JuxuEngine,
    sentences: &[(String, String)],
    seed: u64,
    max_per_domain: usize,
) -> (Metrics, BTreeMap<String, Metrics>) {
    let layout = KeyLayout::default();
    let mut rng = Rng::new(seed);
    let mut per_dom = BTreeMap::<String, Metrics>::new();
    let mut dom_sent_cnt = BTreeMap::<String, usize>::new();

    for (dom, sent) in sentences {
        let sc = dom_sent_cnt.entry(dom.clone()).or_insert(0);
        if *sc >= max_per_domain {
            continue;
        }
        *sc += 1;

        let tokens = {
            let d = engine.active_dict().unwrap();
            segment_sentence(d, sent)
        };
        let m = per_dom.entry(dom.clone()).or_default();
        engine.clear_composition();
        engine.clear_context();

        for (target_text, target_code) in tokens {
            let code_bytes = target_code.as_bytes();
            let code_len = code_bytes.len();

            if code_len >= 2 && code_len <= 4 {
                // Collect neighbor mutations classified into colliding vs empty
                let mut coll_muts = Vec::<(usize, u8)>::new();
                let mut empty_muts = Vec::<(usize, u8)>::new();
                {
                    let d = engine.active_dict().unwrap();
                    for pos in 0..code_len {
                        let orig = code_bytes[pos];
                        for nb in neighbor_keys(&layout, orig) {
                            let mut mut_code = code_bytes.to_vec();
                            mut_code[pos] = nb;
                            if let Some(pc) = PackedCode::from_bytes(&mut_code) {
                                let hits = d.lookup_exact(pc, false);
                                if hits.is_empty() {
                                    empty_muts.push((pos, nb));
                                } else if hits[0].text != target_text {
                                    coll_muts.push((pos, nb));
                                }
                            }
                        }
                    }
                }

                // 1) Touch Boundary Slip -> Colliding code
                if !coll_muts.is_empty() {
                    let (pos, nb) = coll_muts[rng.range(coll_muts.len())];
                    let alpha = 0.53 + rng.next_f32() * 0.08; // 0.53..0.61 past midpoint onto neighbor
                    let jitter_y = (rng.next_f32() - 0.5) * 0.12;
                    engine.clear_composition();
                    for j in 0..code_len {
                        let orig = code_bytes[j];
                        let (cx, cy) = layout.center_of(orig);
                        if j == pos {
                            let (nx, ny) = layout.center_of(nb);
                            let tx = cx + alpha * (nx - cx);
                            let ty = cy + alpha * (ny - cy) + jitter_y;
                            let _ = engine.push_touch_letter(nb, tx, ty, true, 1000 + j as u64 * 70);
                        } else {
                            let jx = (rng.next_f32() - 0.5) * 0.10;
                            let jy = (rng.next_f32() - 0.5) * 0.10;
                            let _ = engine.push_touch_letter(orig, cx + jx, cy + jy, true, 1000 + j as u64 * 70);
                        }
                    }
                    let cands = engine.all_candidates();
                    m.touch_coll_n += 1;
                    if cands.first().map(|c| c.text == target_text).unwrap_or(false) {
                        m.touch_coll_top1 += 1;
                    }
                    if cands.iter().take(3).any(|c| c.text == target_text) {
                        m.touch_coll_top3 += 1;
                    }
                }

                // 2) Touch Boundary Slip -> Empty code
                if !empty_muts.is_empty() {
                    let (pos, nb) = empty_muts[rng.range(empty_muts.len())];
                    let alpha = 0.54 + rng.next_f32() * 0.16; // 0.54..0.70 onto neighbor
                    let jitter_y = (rng.next_f32() - 0.5) * 0.12;
                    engine.clear_composition();
                    for j in 0..code_len {
                        let orig = code_bytes[j];
                        let (cx, cy) = layout.center_of(orig);
                        if j == pos {
                            let (nx, ny) = layout.center_of(nb);
                            let tx = cx + alpha * (nx - cx);
                            let ty = cy + alpha * (ny - cy) + jitter_y;
                            let _ = engine.push_touch_letter(nb, tx, ty, true, 1000 + j as u64 * 70);
                        } else {
                            let jx = (rng.next_f32() - 0.5) * 0.10;
                            let jy = (rng.next_f32() - 0.5) * 0.10;
                            let _ = engine.push_touch_letter(orig, cx + jx, cy + jy, true, 1000 + j as u64 * 70);
                        }
                    }
                    let cands = engine.all_candidates();
                    m.touch_empty_n += 1;
                    if cands.first().map(|c| c.text == target_text).unwrap_or(false) {
                        m.touch_empty_top1 += 1;
                    }
                    if cands.iter().take(3).any(|c| c.text == target_text) {
                        m.touch_empty_top3 += 1;
                    }

                    // 3a) Physical keyboard empty-code neighbor typo
                    engine.clear_composition();
                    for j in 0..code_len {
                        let ch = if j == pos { nb } else { code_bytes[j] };
                        let _ = engine.handle_key_event(0, Some(ch as char), false, 1000 + j as u64 * 70);
                    }
                    let pcands = engine.all_candidates();
                    m.phys_empty_n += 1;
                    if pcands.first().map(|c| c.text == target_text).unwrap_or(false) {
                        m.phys_empty_top1 += 1;
                    }
                    if pcands.iter().take(3).any(|c| c.text == target_text) {
                        m.phys_empty_top3 += 1;
                    }
                }

                // 4) Correct Touch near key border (0.36..0.46 toward neighbor -> still inside intended key!)
                //    Measures false-positive over-correction rate!
                if !coll_muts.is_empty() {
                    let (pos, nb) = coll_muts[rng.range(coll_muts.len())];
                    let alpha = 0.36 + rng.next_f32() * 0.10; // 0.36..0.46 (near border, still on intended key!)
                    engine.clear_composition();
                    for j in 0..code_len {
                        let orig = code_bytes[j];
                        let (cx, cy) = layout.center_of(orig);
                        if j == pos {
                            let (nx, ny) = layout.center_of(nb);
                            let tx = cx + alpha * (nx - cx);
                            let ty = cy + alpha * (ny - cy);
                            let _ = engine.push_touch_letter(orig, tx, ty, true, 1000 + j as u64 * 70);
                        } else {
                            let _ = engine.push_touch_letter(orig, cx, cy, true, 1000 + j as u64 * 70);
                        }
                    }
                    let cands = engine.all_candidates();
                    m.correct_border_n += 1;
                    if !cands.first().map(|c| c.text == target_text).unwrap_or(false) {
                        m.correct_border_fp += 1;
                    }
                }
            }

            engine.clear_composition();
            engine.append_recent_context(&target_text);
        }
    }

    let mut total = Metrics::default();
    for m in per_dom.values() {
        total.add(m);
    }
    (total, per_dom)
}

fn ios_gutter_score(dist_from_center_pt: f32, p_val: u8, has_mask_weights: bool) -> f32 {
    // Exact 1:1 mirror of `IOSKeyboardLayoutEngine.hitTest` horizontal gutter scoring
    // (`pitchX = 39.0pt`, `keyW = 33.0pt` -> `halfW = 16.5pt`, `gapX = 6.0pt`):
    let half_w = 16.5f32;
    let bias = ((p_val as f32) - 100.0) / 25.0;
    let (exp_x, dead_pen) = if !has_mask_weights {
        (
            (bias * 1.45).clamp(-1.6, 3.8),
            -bias * 0.85,
        )
    } else {
        (
            (4.8 + bias * 1.15).clamp(1.2, 6.2),
            -bias * 1.15,
        )
    };
    let eff_half_w = half_w + exp_x;
    let dx = (dist_from_center_pt - eff_half_w).max(0.0);
    if dx == 0.0 {
        let inner_depth = eff_half_w - dist_from_center_pt;
        -inner_depth + dead_pen * 0.25
    } else {
        dx + dead_pen
    }
}

fn eval_dynamic_hitbox(
    engine: &mut JuxuEngine,
    sentences: &[(String, String)],
    seed: u64,
    max_per_domain: usize,
) {
    // Mirrors `IOSKeyboardLayoutEngine.hitTest` gutter disambiguation on disputed inter-key
    // gutter taps (`alpha = 0.51..0.56`, i.e. `0.4..2.3pt` past midpoint into neighbor's half
    // of the 6pt gutter, strictly outside the 3.0pt Sacred Keycap Core).
    let layout = KeyLayout::default();
    let mut rng = Rng::new(seed);
    let mut dom_sent_cnt = BTreeMap::<String, usize>::new();

    let mut k1_n = 0usize;
    let mut k1_geom_win = 0usize;
    let mut k1_prior_win = 0usize;

    let mut k24_vv_n = 0usize;
    let mut k24_geom_win = 0usize;
    let mut k24_prior_win = 0usize;

    for (dom, sent) in sentences {
        let sc = dom_sent_cnt.entry(dom.clone()).or_insert(0);
        if *sc >= max_per_domain {
            continue;
        }
        *sc += 1;

        let tokens = {
            let d = engine.active_dict().unwrap();
            segment_sentence(d, sent)
        };
        engine.clear_composition();
        engine.clear_context();

        for (target_text, target_code) in tokens {
            let cb = target_code.as_bytes();
            if cb.len() >= 2 && cb.len() <= 4 {
                // Key #1 gutter slip (`pos = 0`): all 25 Wubi keys 'a'..'y' are valid at pos 0!
                let k_orig = cb[0];
                let nbs0 = neighbor_keys(&layout, k_orig);
                if !nbs0.is_empty() {
                    let nb = nbs0[rng.range(nbs0.len())];
                    let alpha = 0.51 + rng.next_f32() * 0.05; // 0.51..0.56 in inter-key gutter
                    engine.clear_composition();
                    let priors = engine.next_key_priors();
                    let p_orig = priors[(k_orig - b'a') as usize];
                    let p_nb = priors[(nb - b'a') as usize];
                    let d_orig = alpha * 39.0;
                    let d_nb = (1.0 - alpha) * 39.0;
                    k1_n += 1;
                    if d_orig < d_nb {
                        k1_geom_win += 1;
                    }
                    if ios_gutter_score(d_orig, p_orig, false) < ios_gutter_score(d_nb, p_nb, false) {
                        k1_prior_win += 1;
                    }
                }

                // Keys #2..#4 valid-vs-valid gutter slip (BOTH `orig` and `nb` are valid next keys!)
                for pos in 1..cb.len() {
                    engine.clear_composition();
                    for j in 0..pos {
                        let c = cb[j];
                        let (cx, cy) = layout.center_of(c);
                        let _ = engine.push_touch_letter(c, cx, cy, true, 1000 + j as u64 * 70);
                    }
                    let mask = engine.valid_next_mask();
                    let priors = engine.next_key_priors();
                    let k_orig = cb[pos];
                    let valid_nbs: Vec<u8> = neighbor_keys(&layout, k_orig)
                        .into_iter()
                        .filter(|&nb| (mask & (1u32 << (nb - b'a'))) != 0)
                        .collect();
                    if !valid_nbs.is_empty() {
                        let nb = valid_nbs[rng.range(valid_nbs.len())];
                        let alpha = 0.51 + rng.next_f32() * 0.05;
                        let p_orig = priors[(k_orig - b'a') as usize];
                        let p_nb = priors[(nb - b'a') as usize];
                        let d_orig = alpha * 39.0;
                        let d_nb = (1.0 - alpha) * 39.0;
                        k24_vv_n += 1;
                        if d_orig < d_nb {
                            k24_geom_win += 1;
                        }
                        if ios_gutter_score(d_orig, p_orig, true) < ios_gutter_score(d_nb, p_nb, true) {
                            k24_prior_win += 1;
                        }
                    }
                }
            }
            engine.clear_composition();
            engine.append_recent_context(&target_text);
        }
    }

    println!(
        "Dynamic Hitbox Gutter Disambiguation -> Key#1({}): static_mask={:.2}% -> nextKeyPriors={:.2}% | Keys#2-4 Valid-vs-Valid({}): binary_validMask={:.2}% -> nextKeyPriors={:.2}%",
        k1_n,
        Metrics::pct(k1_geom_win, k1_n),
        Metrics::pct(k1_prior_win, k1_n),
        k24_vv_n,
        Metrics::pct(k24_geom_win, k24_vv_n),
        Metrics::pct(k24_prior_win, k24_vv_n),
    );
}

fn load_sentences(path: &PathBuf) -> Vec<(String, String)> {
    let content = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.splitn(2, '\t');
        let Some(dom) = it.next() else { continue };
        let Some(sent) = it.next() else { continue };
        if !sent.is_empty() {
            out.push((dom.to_string(), sent.to_string()));
        }
    }
    out
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut dicts_dir = PathBuf::from("Resources/Dicts");
    let mut aux_path: Option<PathBuf> = None;
    let mut eval_tsv: Option<PathBuf> = None;
    let mut schema = InputSchema::Wubi86;
    let mut max_per_domain = 250usize;
    let mut grid_search = false;
    let mut eval_hitbox = false;
    let mut custom_weights = false;
    let mut lm_w = LmWeights::default();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--dicts-dir" => { dicts_dir = PathBuf::from(&args[i + 1]); i += 2; }
            "--aux" => { aux_path = Some(PathBuf::from(&args[i + 1])); i += 2; }
            "--eval" => { eval_tsv = Some(PathBuf::from(&args[i + 1])); i += 2; }
            "--schema" => { schema = InputSchema::from_str_tag(&args[i + 1]); i += 2; }
            "--max-per-domain" => { max_per_domain = args[i + 1].parse().unwrap(); i += 2; }
            "--grid-search" => { grid_search = true; i += 1; }
            "--eval-hitbox" => { eval_hitbox = true; i += 1; }
            "--uni" => { lm_w.corpus_unigram_scale = args[i + 1].parse().unwrap(); custom_weights = true; i += 2; }
            "--dict-prior" => { lm_w.dict_prior_scale = args[i + 1].parse().unwrap(); custom_weights = true; i += 2; }
            "--bg" => { lm_w.bigram_scale = args[i + 1].parse().unwrap(); custom_weights = true; i += 2; }
            "--tg" => { lm_w.trigram_scale = args[i + 1].parse().unwrap(); custom_weights = true; i += 2; }
            "--int-bg" => { lm_w.internal_bigram_scale = args[i + 1].parse().unwrap(); custom_weights = true; i += 2; }
            "--spatial" => { lm_w.spatial_scale = args[i + 1].parse().unwrap(); custom_weights = true; i += 2; }
            "--ns" => { lm_w.neural_scale = args[i + 1].parse().unwrap(); custom_weights = true; i += 2; }
            other => {
                eprintln!("Unknown arg: {}", other);
                std::process::exit(1);
            }
        }
    }

    if schema.is_pinyin() {
        eprintln!(
            "Error: eval_typo only supports fixed-code .wbdict schemas (wubi86/wubi98/wubi06/xiaohe/cangjie*), not '{}'. Use eval_pinyin or eval_touch_and_bmk instead.",
            schema.as_tag()
        );
        std::process::exit(1);
    }

    let Some(eval_tsv) = eval_tsv else {
        eprintln!("Usage: eval_typo --eval <sentences.tsv> [--schema wubi86|wubi98|wubi06|xiaohe|cangjie5]");
        std::process::exit(1);
    };
    let _ = pct;
    let aux_file = aux_path.unwrap_or_else(|| dicts_dir.join("wubi_aux.wbaux"));
    let dict_file = match schema {
        InputSchema::Wubi86 => dicts_dir.join("wubi86.wbdict"),
        InputSchema::Wubi98 => dicts_dir.join("wubi98.wbdict"),
        InputSchema::Wubi06 => dicts_dir.join("wubi06.wbdict"),
        InputSchema::Xiaohe => dicts_dir.join("xiaohe.wbdict"),
        InputSchema::Cangjie3 => dicts_dir.join("cangjie3.wbdict"),
        InputSchema::Cangjie5 => dicts_dir.join("cangjie5.wbdict"),
        InputSchema::Cangjie6 => dicts_dir.join("cangjie6.wbdict"),
        InputSchema::Pinyin | InputSchema::XiaoheShuangpin => unreachable!(),
    };
    if !dict_file.exists() {
        eprintln!("Error: dictionary file not found: {}", dict_file.display());
        std::process::exit(1);
    }

    let dict = DictView::open_mmap(&dict_file).expect("open dict");
    let aux = AuxView::open_mmap(&aux_file).expect("open aux");
    let stats = aux.stats();
    let has_neural = aux.has_neural();
    println!(
        "Loaded schema={} aux={} (py={}, uni={}, bi={}, tri={}, neural={})",
        schema.as_tag(),
        aux_file.display(),
        stats.0,
        stats.1,
        stats.2,
        stats.3,
        has_neural
    );

    let sentences = load_sentences(&eval_tsv);
    let mut cfg = JuxuConfig::default();
    cfg.schema = schema;
    cfg.fourth_code_auto_commit = false;
    cfg.typo.enabled = true;
    cfg.typo.touch_hitbox_adapt = false;

    let mut engine = JuxuEngine::new(cfg);
    engine.load_dict(schema, dict);
    engine.set_aux(aux);

    if eval_hitbox {
        eval_dynamic_hitbox(&mut engine, &sentences, 20261119, max_per_domain);
        return;
    }

    if grid_search {
        println!("Running validation grid search (has_neural={})...", has_neural);
        let mut best_obj = f64::NEG_INFINITY;
        let mut best_w = engine.lm_weights;
        if has_neural {
            for &sp in &[2.85f32, 3.05, 3.25] {
                for &dp in &[0.32f32] {
                    for &bg in &[0.65f32] {
                        for &ns in &[0.65f32, 0.80, 0.95] {
                            let w = LmWeights {
                                spatial_scale: sp,
                                dict_prior_scale: dp,
                                corpus_unigram_scale: 0.24,
                                internal_bigram_scale: 0.28,
                                bigram_scale: bg,
                                trigram_scale: 0.65,
                                cross_trigram_scale: 0.45,
                                phrase_bonus: 0.65,
                                neural_scale: ns,
                            };
                            engine.lm_weights = w;
                            let (tot, _) = run_eval_with_engine(&mut engine, &sentences, 20261119, max_per_domain);
                            let fp = Metrics::pct(tot.correct_border_fp, tot.correct_border_n);
                            let obj = tot.objective();
                            // Enforce strict false-positive guard (<= 0.62% on val!)
                            if fp <= 0.62 && obj > best_obj {
                                best_obj = obj;
                                best_w = w;
                                println!(
                                    "NEW BEST obj={:6.2} (sp={:.2} dp={:.2} bg={:.2} ns={:.2}) -> TC1={:.2}% TC3={:.2}% TE1={:.2}% PE1={:.2}% FP={:.2}%",
                                    obj, sp, dp, bg, ns,
                                    Metrics::pct(tot.touch_coll_top1, tot.touch_coll_n),
                                    Metrics::pct(tot.touch_coll_top3, tot.touch_coll_n),
                                    Metrics::pct(tot.touch_empty_top1, tot.touch_empty_n),
                                    Metrics::pct(tot.phys_empty_top1, tot.phys_empty_n),
                                    fp,
                                );
                            }
                        }
                    }
                }
            }
        }
        println!("Best weights: {:?}", best_w);
    } else {
        if custom_weights {
            engine.lm_weights = lm_w;
        }
        let (total, per_dom) = run_eval_with_engine(&mut engine, &sentences, 20261119, max_per_domain);
        println!("----------------------------------------------------------------------------------------------------------------------------------");
        for (dom, m) in &per_dom {
            m.print_row(dom);
        }
        println!("----------------------------------------------------------------------------------------------------------------------------------");
        total.print_row("OVERALL");
        println!("Objective score: {:.2}", total.objective());
    }
}
