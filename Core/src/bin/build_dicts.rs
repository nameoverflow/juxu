use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use juxu_core::code::{PackedCode, PackedPinyin, Stem2};
use juxu_core::english::{serialize_endict, EN_FLAG_ABBR, EN_FLAG_HIGH_FREQ, EN_FLAG_TECH};
use juxu_core::format::{
    pack_trigram, serialize_wbaux, serialize_wbdict, AuxBuildInput, CharMeta, DictBuildInput,
    ENTRY_FLAG_EXT_RARE, ENTRY_FLAG_SINGLE_CHAR, ENTRY_FLAG_SYMBOL,
};
use juxu_core::pinyin_dict::{serialize_pydict, PinyinDictBuildInput, PY_FLAG_EXT_RARE};

fn strip_pinyin_tone(toned: &str) -> (String, Option<String>) {
    let mut primary = String::with_capacity(toned.len());
    let mut has_umlaut = false;
    for ch in toned.chars() {
        let mapped = match ch {
            'ā' | 'á' | 'ǎ' | 'à' | 'a' => 'a',
            'ē' | 'é' | 'ě' | 'è' | 'ê' | 'e' => 'e',
            'ī' | 'í' | 'ǐ' | 'ì' | 'i' => 'i',
            'ō' | 'ó' | 'ǒ' | 'ò' | 'o' => 'o',
            'ū' | 'ú' | 'ǔ' | 'ù' | 'u' => 'u',
            'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' | 'ü' => {
                has_umlaut = true;
                'v'
            }
            'ń' | 'ň' | 'ǹ' | 'n' => 'n',
            'ḿ' | 'm' => 'm',
            c if c.is_ascii_lowercase() => c,
            _ => continue,
        };
        primary.push(mapped);
    }
    let alt = if has_umlaut {
        Some(primary.replace('v', "u"))
    } else {
        None
    };
    (primary, alt)
}

struct ParsedSchema {
    entries: Vec<(PackedCode, String, u32, u16)>,
    char_metas: Vec<CharMeta>,
    char_weights: HashMap<u32, u64>,
    phrase_weights: Vec<(String, u64)>,
}

fn parse_rime_dict(
    path: &PathBuf,
    schema_id: u32,
    tghz_set: &HashSet<u32>,
) -> ParsedSchema {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    let mut raw_rows: Vec<(String, String, u64, Option<String>)> = Vec::new();
    let mut max_wt = 0u64;
    let mut in_body = false;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if !in_body {
            if line == "..." {
                in_body = true;
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let text = parts[0].trim();
        let code_str = parts[1].trim();
        if text.is_empty() || code_str.is_empty() || code_str.len() > 4 {
            continue;
        }
        if !code_str.bytes().all(|b| b.is_ascii_lowercase()) {
            continue;
        }
        let wt: u64 = parts
            .get(2)
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if wt > max_wt {
            max_wt = wt;
        }
        let stem = parts
            .get(3)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        raw_rows.push((text.to_string(), code_str.to_string(), wt, stem));
    }

    // Normalize weight scale across schemas (e.g. wubi06 uses 0..20,000 while 86/98 use 0..4e9)
    let scale: u64 = if max_wt > 0 && max_wt < 10_000_000 {
        100_000
    } else {
        1
    };

    // Preserve file order tie-breaking within same bucket by adding tiny descending rank delta if needed
    let mut entries = Vec::with_capacity(raw_rows.len());
    let mut char_shortest: HashMap<u32, (PackedCode, u32)> = HashMap::new();
    let mut char_full: HashMap<u32, (PackedCode, u32)> = HashMap::new();
    let mut char_explicit_stem: HashMap<u32, Stem2> = HashMap::new();
    let mut char_weights: HashMap<u32, u64> = HashMap::new();
    let mut phrase_weights: Vec<(String, u64)> = Vec::new();

    // Track per-code count to preserve exact file order when weights are equal
    let mut per_code_seen: HashMap<PackedCode, u32> = HashMap::new();

    for (text, code_str, raw_wt, stem_opt) in raw_rows {
        let Some(pcode) = PackedCode::from_ascii(&code_str) else {
            continue;
        };
        let seen_idx = per_code_seen.entry(pcode).or_insert(0);
        let scaled = raw_wt.saturating_mul(scale);
        let wt_u32 = scaled
            .min(u32::MAX as u64 - 1000)
            .saturating_add(500u64.saturating_sub(*seen_idx as u64)) as u32;
        *seen_idx += 1;

        let mut chars_it = text.chars();
        let first_ch = chars_it.next().unwrap();
        let is_single = chars_it.next().is_none();

        let mut flags = 0u16;
        if code_str.starts_with('z') {
            flags |= ENTRY_FLAG_SYMBOL;
        }
        if is_single {
            flags |= ENTRY_FLAG_SINGLE_CHAR;
            let cp = first_ch as u32;
            // Mark non-BMP characters outside 通用规范汉字表 as rare extension chars
            if cp > 0xFFFF && !tghz_set.contains(&cp) {
                flags |= ENTRY_FLAG_EXT_RARE;
            }
            if !code_str.contains('z') {
                let cw = char_weights.entry(cp).or_insert(0);
                if scaled > *cw {
                    *cw = scaled;
                }
                if let Some(ref st) = stem_opt {
                    if let Some(s2) = Stem2::from_bytes(st.as_bytes()) {
                        char_explicit_stem.insert(cp, s2);
                    }
                }
                char_shortest
                    .entry(cp)
                    .and_modify(|prev| {
                        if pcode.len() < prev.0.len()
                            || (pcode.len() == prev.0.len() && wt_u32 > prev.1)
                        {
                            *prev = (pcode, wt_u32);
                        }
                    })
                    .or_insert((pcode, wt_u32));

                char_full
                    .entry(cp)
                    .and_modify(|prev| {
                        if pcode.len() > prev.0.len()
                            || (pcode.len() == prev.0.len() && wt_u32 > prev.1)
                        {
                            *prev = (pcode, wt_u32);
                        }
                    })
                    .or_insert((pcode, wt_u32));
            }
        } else if !code_str.contains('z') {
            phrase_weights.push((text.clone(), scaled.max(1_000)));
        }

        entries.push((pcode, text, if raw_wt == 0 && (flags & ENTRY_FLAG_EXT_RARE) != 0 { 0 } else { wt_u32 }, flags));
    }

    let mut all_cps: HashSet<u32> = char_full.keys().copied().collect();
    all_cps.extend(char_shortest.keys().copied());

    let mut char_metas = Vec::with_capacity(all_cps.len());
    for cp in all_cps {
        let shortest = char_shortest.get(&cp).map(|x| x.0).unwrap_or_default();
        let full = char_full.get(&cp).map(|x| x.0).unwrap_or(shortest);
        let stem2 = char_explicit_stem.get(&cp).copied().unwrap_or_else(|| {
            let (buf, n) = full.to_bytes_buf();
            Stem2::from_bytes(&buf[..n]).unwrap_or_default()
        });
        let flags = if cp > 0xFFFF && !tghz_set.contains(&cp) {
            ENTRY_FLAG_EXT_RARE
        } else {
            0
        };
        char_metas.push(CharMeta {
            codepoint: cp,
            stem2,
            flags,
            shortest_code: shortest,
            primary_short_code: PackedCode::EMPTY,
            full_code: full,
        });
    }

    let _ = schema_id;
    ParsedSchema {
        entries,
        char_metas,
        char_weights,
        phrase_weights,
    }
}

fn load_tghz_set(pinyin_dir: &PathBuf) -> HashSet<u32> {
    let mut set = HashSet::new();
    let p = pinyin_dir.join("kTGHZ2013.txt");
    if let Ok(content) = fs::read_to_string(p) {
        for line in content.lines() {
            let line = line.trim();
            if !line.starts_with("U+") {
                continue;
            }
            if let Some(colon) = line.find(':') {
                if let Ok(cp) = u32::from_str_radix(&line[2..colon], 16) {
                    set.insert(cp);
                }
            }
        }
    }
    set
}

fn load_corpus_lm(dir: &Path) -> (Vec<(u32, u16)>, Vec<(u64, u16)>, Vec<u64>, Vec<u8>) {
    let mut unigrams = Vec::new();
    let uni_path = dir.join("unigrams.tsv");
    if let Ok(content) = fs::read_to_string(&uni_path) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.split('\t');
            let Some(ch_str) = it.next() else { continue };
            let Some(sc_str) = it.next() else { continue };
            let mut chars = ch_str.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                continue;
            };
            if let Ok(score) = sc_str.parse::<f64>() {
                let q8 = (score.clamp(0.0, 15.0) * 256.0).round() as u16;
                unigrams.push((ch as u32, q8));
            }
        }
    }

    let mut bigrams = Vec::new();
    let bg_path = dir.join("bigrams.tsv");
    if let Ok(content) = fs::read_to_string(&bg_path) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.split('\t');
            let Some(pair_str) = it.next() else { continue };
            let Some(sc_str) = it.next() else { continue };
            let mut chars = pair_str.chars();
            let (Some(c1), Some(c2), None) = (chars.next(), chars.next(), chars.next()) else {
                continue;
            };
            if let Ok(score) = sc_str.parse::<f64>() {
                let q8 = (score.clamp(0.0, 6.0) * 256.0).round() as u16;
                if q8 > 0 {
                    let key = ((c1 as u64) << 32) | (c2 as u64);
                    bigrams.push((key, q8));
                }
            }
        }
    }

    let mut trigrams = Vec::new();
    let tri_path = dir.join("trigrams.tsv");
    if let Ok(content) = fs::read_to_string(&tri_path) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.split('\t');
            let Some(tri_str) = it.next() else { continue };
            let Some(sc_str) = it.next() else { continue };
            let mut chars = tri_str.chars();
            let (Some(c0), Some(c1), Some(c2), None) =
                (chars.next(), chars.next(), chars.next(), chars.next())
            else {
                continue;
            };
            if let Ok(score) = sc_str.parse::<f64>() {
                let q8 = (score.clamp(0.0, 6.0) * 256.0).round() as u16;
                if q8 > 0 {
                    if let Some(packed) = pack_trigram(c0, c1, c2, q8) {
                        trigrams.push(packed);
                    }
                }
            }
        }
    }

    let neural_path = dir.join("neural_rwkv7.wblm");
    let neural_blob = fs::read(&neural_path).unwrap_or_default();

    (unigrams, bigrams, trigrams, neural_blob)
}

fn build_aux_data(
    pinyin_dir: &PathBuf,
    global_char_weights: &HashMap<u32, u64>,
    all_phrases: &[(String, u64)],
    corpus_lm_dir: Option<&Path>,
) -> AuxBuildInput {
    // 1. Parse kTGHZ2013.txt + pinyin.txt (BMP + TGHZ characters)
    let mut py_items = Vec::<(PackedPinyin, String, u32, u32)>::new();
    for filename in ["kTGHZ2013.txt", "kMandarin_8105.txt", "pinyin.txt"] {
        let path = pinyin_dir.join(filename);
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let is_core_std = filename != "pinyin.txt";
        for raw_line in content.lines() {
            let line = raw_line.trim();
            if !line.starts_with("U+") {
                continue;
            }
            let Some(colon) = line.find(':') else {
                continue;
            };
            let Ok(cp) = u32::from_str_radix(&line[2..colon], 16) else {
                continue;
            };
            if !is_core_std && cp > 0x9FFF {
                continue;
            }
            let Some(&c_wt) = global_char_weights.get(&cp) else {
                continue;
            };
            let rest = &line[colon + 1..];
            let readings_part = match rest.find('#') {
                Some(h) => &rest[..h],
                None => rest,
            }
            .trim();

            for (r_idx, reading) in readings_part.split(',').enumerate() {
                let r = reading.trim();
                if r.is_empty() {
                    continue;
                }
                let (ascii_py, alt_u) = strip_pinyin_tone(r);
                // Primary reading gets higher weight than rare secondary polyphone readings
                let wt = ((c_wt / 1000).min(u32::MAX as u64 / 2) as u32)
                    .saturating_add(if is_core_std { 50_000 } else { 0 })
                    / ((r_idx as u32) + 1);
                if let Some(pp) = PackedPinyin::from_bytes(ascii_py.as_bytes()) {
                    py_items.push((pp, r.to_string(), cp, wt.max(1)));
                }
                if let Some(alt) = alt_u {
                    if let Some(pp) = PackedPinyin::from_bytes(alt.as_bytes()) {
                        py_items.push((pp, r.to_string(), cp, wt.max(1)));
                    }
                }
            }
        }
    }

    if let Some(lm_dir) = corpus_lm_dir {
        let (unigrams, bigrams, trigrams, neural_blob) = load_corpus_lm(lm_dir);
        println!(
            "Loaded corpus LM from {}: {} unigrams, {} bigrams, {} trigrams, neural={:.2} MB",
            lm_dir.display(),
            unigrams.len(),
            bigrams.len(),
            trigrams.len(),
            neural_blob.len() as f64 / (1024.0 * 1024.0)
        );
        return AuxBuildInput {
            pinyin_items: py_items,
            bigrams,
            unigrams,
            trigrams,
            neural_blob,
        };
    }

    // 2. Fallback: Build character-transition Bigram log-PMI table from weighted phrase corpus + conversational transitions
    let mut pair_mass: HashMap<u64, f64> = HashMap::new();
    let mut left_mass: HashMap<u32, f64> = HashMap::new();
    let mut right_mass: HashMap<u32, f64> = HashMap::new();
    let mut total_mass = 0.0f64;

    for (phrase, wt) in all_phrases {
        let chars: Vec<char> = phrase.chars().collect();
        if chars.len() < 2 {
            continue;
        }
        // Dampen huge outliers with powf(0.65) so top 20 words don't drown all other transitions
        let w = (*wt as f64).max(1000.0).powf(0.65);
        for win in chars.windows(2) {
            let c1 = win[0] as u32;
            let c2 = win[1] as u32;
            let key = ((c1 as u64) << 32) | (c2 as u64);
            *pair_mass.entry(key).or_insert(0.0) += w;
            *left_mass.entry(c1).or_insert(0.0) += w;
            *right_mass.entry(c2).or_insert(0.0) += w;
            total_mass += w;
        }
    }

    // Inject curated high-frequency cross-word & conversational transitions
    let conversational = [
        ("输入法", 5e8),
        ("五笔字型", 4e8),
        ("误触纠正", 3e8),
        ("苹果原生", 2e8),
        ("没问题", 5e8),
        ("不知道", 6e8),
        ("为什么", 7e8),
        ("怎么样", 5e8),
        ("可以的", 4e8),
        ("如果是", 3e8),
        ("已经在", 3e8),
        ("我是", 6e8),
        ("我在", 5e8),
        ("我想", 5e8),
        ("我要", 4e8),
        ("我会", 4e8),
        ("我的", 8e8),
        ("你的", 6e8),
        ("他的", 5e8),
        ("不是", 8e8),
        ("不会", 6e8),
        ("不能", 6e8),
        ("不要", 5e8),
        ("这个", 7e8),
        ("这是", 7e8),
        ("这样", 5e8),
        ("这里", 5e8),
    ];
    for (p, raw_w) in conversational {
        let w = (raw_w as f64).powf(0.65);
        let chars: Vec<char> = p.chars().collect();
        for win in chars.windows(2) {
            let c1 = win[0] as u32;
            let c2 = win[1] as u32;
            let key = ((c1 as u64) << 32) | (c2 as u64);
            *pair_mass.entry(key).or_insert(0.0) += w;
            *left_mass.entry(c1).or_insert(0.0) += w;
            *right_mass.entry(c2).or_insert(0.0) += w;
            total_mass += w;
        }
    }

    let mut scored_bigrams = Vec::<(u64, f64, u16)>::new();
    for (key, pm) in pair_mass {
        let c1 = (key >> 32) as u32;
        let c2 = (key & 0xFFFF_FFFF) as u32;
        let lm = left_mass.get(&c1).copied().unwrap_or(1.0);
        let rm = right_mass.get(&c2).copied().unwrap_or(1.0);
        // Smoothed PMI + conditional probability blend
        let p_cond = pm / (lm + 500.0);
        let p_marg = (rm + 100.0) / (total_mass + 10_000.0);
        let pmi = (p_cond / p_marg.max(1e-9)).ln();
        if pmi > 0.8 && pm > 150.0 {
            let boost = (pmi * 0.48).clamp(0.35, 4.20);
            let q8 = (boost * 256.0).round() as u16;
            scored_bigrams.push((key, pm * pmi, q8));
        }
    }
    scored_bigrams.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
    scored_bigrams.truncate(55_000);

    let bigrams = scored_bigrams.into_iter().map(|(k, _, q)| (k, q)).collect();
    AuxBuildInput {
        pinyin_items: py_items,
        bigrams,
        unigrams: Vec::new(),
        trigrams: Vec::new(),
        neural_blob: Vec::new(),
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "Usage: build-dicts <raw-tables-dir> <output-dicts-dir> [--corpus-lm <dir>] [--aux-only] [--wbdict-only]"
        );
        std::process::exit(1);
    }
    let raw_dir = PathBuf::from(&args[1]);
    let out_dir = PathBuf::from(&args[2]);
    let mut corpus_lm_dir: Option<PathBuf> = env::var("FORMORDO_CORPUS_LM")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let mut aux_only = false;
    let mut wbdict_only = false;

    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--corpus-lm" if i + 1 < args.len() => {
                corpus_lm_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--aux-only" => {
                aux_only = true;
                i += 1;
            }
            "--wbdict-only" => {
                wbdict_only = true;
                i += 1;
            }
            "--english-only" => {
                fs::create_dir_all(&out_dir).expect("failed to create output dir");
                let ice_dir = raw_dir.join("pinyin_ice");
                let (en_blob, en_cnt) = build_english_dict(&ice_dir);
                let out_path = out_dir.join("english.endict");
                fs::write(&out_path, &en_blob).expect("failed to write english.endict");
                println!(
                    "Built english.endict ({:.2} KB, {} entries)",
                    en_blob.len() as f64 / 1024.0,
                    en_cnt
                );
                return;
            }
            "--pinyin-only" => {
                fs::create_dir_all(&out_dir).expect("failed to create output dir");
                let pinyin_dir = raw_dir.join("pinyin");
                let tghz_set = load_tghz_set(&pinyin_dir);
                let ice_dir = raw_dir.join("pinyin_ice");
                let py_blob = build_pinyin_dict(&ice_dir, &tghz_set);
                let out_path = out_dir.join("pinyin.pydict");
                fs::write(&out_path, &py_blob).expect("failed to write pinyin.pydict");
                println!(
                    "Built char-only pinyin.pydict ({:.2} KB)",
                    py_blob.len() as f64 / 1024.0
                );
                return;
            }
            other => {
                eprintln!("Unknown argument: {}", other);
                std::process::exit(1);
            }
        }
    }
    fs::create_dir_all(&out_dir).expect("failed to create output dir");

    let pinyin_dir = raw_dir.join("pinyin");
    let tghz_set = load_tghz_set(&pinyin_dir);
    println!("Loaded {} standard TGHZ-2013 characters", tghz_set.len());

    let specs = [
        (86u32, "wubi86/wubi86.dict.yaml", "wubi86.wbdict"),
        (98u32, "wubi98/wubi98.dict.yaml", "wubi98.wbdict"),
        (6u32, "wubi06/wubi06.dict.yaml", "wubi06.wbdict"),
    ];

    let mut global_char_weights = HashMap::<u32, u64>::new();
    let mut all_phrases = Vec::<(String, u64)>::new();

    for (schema_id, rel_in, out_name) in specs {
        let in_path = raw_dir.join(rel_in);
        let parsed = parse_rime_dict(&in_path, schema_id, &tghz_set);
        for (cp, wt) in &parsed.char_weights {
            let e = global_char_weights.entry(*cp).or_insert(0);
            if *wt > *e {
                *e = *wt;
            }
        }
        all_phrases.extend(parsed.phrase_weights);

        if !aux_only {
            let entry_cnt = parsed.entries.len();
            let char_cnt = parsed.char_metas.len();
            let blob = serialize_wbdict(DictBuildInput {
                schema_id,
                entries: parsed.entries,
                char_metas: parsed.char_metas,
            });
            let out_path = out_dir.join(out_name);
            fs::write(&out_path, &blob).expect("failed to write wbdict");
            println!(
                "Built {} ({:.2} MB): {} entries, {} chars",
                out_name,
                blob.len() as f64 / (1024.0 * 1024.0),
                entry_cnt,
                char_cnt
            );
        }
    }

    if !aux_only {
        let flypy_dir = raw_dir.join("flypy");
        if flypy_dir.exists() {
            let xh_blob = build_xiaohe_dict(&flypy_dir, &tghz_set, &global_char_weights);
            let out_path = out_dir.join("xiaohe.wbdict");
            fs::write(&out_path, &xh_blob).expect("failed to write xiaohe.wbdict");
            println!(
                "Built xiaohe.wbdict ({:.2} MB)",
                xh_blob.len() as f64 / (1024.0 * 1024.0)
            );
        }

        let cj_specs: &[(u32, &str, &[&str], bool, &str)] = &[
            (
                33,
                "cangjie3",
                &["cangjie.3-base.dict.yaml", "cangjie.3-base-dups.dict.yaml"],
                true, // columns: code \t text
                "cangjie3.wbdict",
            ),
            (
                35,
                "cangjie5",
                &["cangjie5.base.dict.yaml", "cangjie5.extended.dict.yaml"],
                false, // columns: text \t code
                "cangjie5.wbdict",
            ),
            (
                36,
                "cangjie6",
                &["cangjie6.dict.yaml"],
                false, // columns: text \t code \t weight \t stem
                "cangjie6.wbdict",
            ),
        ];
        for &(schema_id, sub_dir, files, code_first, out_name) in cj_specs {
            let cj_dir = raw_dir.join(sub_dir);
            if cj_dir.exists() {
                let (cj_blob, entry_cnt, char_cnt) =
                    build_cangjie_dict(&cj_dir, schema_id, files, code_first, &tghz_set, &global_char_weights);
                let out_path = out_dir.join(out_name);
                fs::write(&out_path, &cj_blob).expect("failed to write cangjie wbdict");
                println!(
                    "Built {} ({:.2} MB): {} entries, {} chars",
                    out_name,
                    cj_blob.len() as f64 / (1024.0 * 1024.0),
                    entry_cnt,
                    char_cnt
                );
            }
        }

        if !wbdict_only {
            let ice_dir = raw_dir.join("pinyin_ice");
            if ice_dir.exists() {
                let py_blob = build_pinyin_dict(&ice_dir, &tghz_set);
                let out_path = out_dir.join("pinyin.pydict");
                fs::write(&out_path, &py_blob).expect("failed to write pinyin.pydict");
                println!(
                    "Built pinyin.pydict ({:.2} MB)",
                    py_blob.len() as f64 / (1024.0 * 1024.0)
                );
                let (en_blob, en_cnt) = build_english_dict(&ice_dir);
                if en_cnt > 0 {
                    let en_out = out_dir.join("english.endict");
                    fs::write(&en_out, &en_blob).expect("failed to write english.endict");
                    println!(
                        "Built english.endict ({:.2} KB, {} entries)",
                        en_blob.len() as f64 / 1024.0,
                        en_cnt
                    );
                }
            }
        }
    }

    if wbdict_only {
        return;
    }

    let aux_input = build_aux_data(
        &pinyin_dir,
        &global_char_weights,
        &all_phrases,
        corpus_lm_dir.as_deref(),
    );
    let py_cnt = aux_input.pinyin_items.len();
    let uni_cnt = aux_input.unigrams.len();
    let bg_cnt = aux_input.bigrams.len();
    let tri_cnt = aux_input.trigrams.len();
    let aux_blob = serialize_wbaux(aux_input);
    let aux_path = out_dir.join("wubi_aux.wbaux");
    fs::write(&aux_path, &aux_blob).expect("failed to write wbaux");
    println!(
        "Built wubi_aux.wbaux ({:.2} MB): {} pinyin, {} unigrams, {} bigrams, {} trigrams",
        aux_blob.len() as f64 / (1024.0 * 1024.0),
        py_cnt,
        uni_cnt,
        bg_cnt,
        tri_cnt
    );
}

fn build_xiaohe_dict(
    flypy_dir: &Path,
    tghz_set: &HashSet<u32>,
    global_char_weights: &HashMap<u32, u64>,
) -> Vec<u8> {
    let files: &[(&str, u32)] = &[
        ("flypy.primary.dict.yaml", 3_000_000_000),
        ("flypy.secondary.short.code.dict.yaml", 2_000_000_000),
        ("flypy.secondary.dict.yaml", 1_500_000_000),
        ("flypy.full.char.dict.yaml", 500_000_000),
        ("flypy.full.dict.yaml", 100_000_000),
    ];

    let mut entries: Vec<(PackedCode, String, u32, u16)> = Vec::new();
    let mut char_shortest: HashMap<u32, (PackedCode, u32)> = HashMap::new();
    let mut char_full: HashMap<u32, (PackedCode, u32)> = HashMap::new();

    for &(fname, base_wt) in files {
        let path = flypy_dir.join(fname);
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let mut in_body = false;
        let mut per_code_seen: HashMap<PackedCode, u32> = HashMap::new();
        for raw_line in content.lines() {
            let line = raw_line.trim();
            if !in_body {
                if line == "..." {
                    in_body = true;
                }
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split('\t');
            let Some(text) = parts.next().map(str::trim) else {
                continue;
            };
            let Some(code_str) = parts.next().map(str::trim) else {
                continue;
            };
            if text.is_empty() || code_str.is_empty() || code_str.len() > 4 {
                continue;
            }
            if !code_str.bytes().all(|b| b.is_ascii_lowercase()) {
                continue;
            }
            let Some(pcode) = PackedCode::from_ascii(code_str) else {
                continue;
            };
            let seen_idx = per_code_seen.entry(pcode).or_insert(0);
            let order_wt = base_wt.saturating_sub((*seen_idx).saturating_mul(100));
            *seen_idx += 1;

            let is_single = text.chars().count() == 1;
            let mut flags = 0u16;
            let mut final_wt = order_wt;
            if is_single {
                flags |= ENTRY_FLAG_SINGLE_CHAR;
                let cp = text.chars().next().unwrap() as u32;
                if !tghz_set.contains(&cp) {
                    flags |= ENTRY_FLAG_EXT_RARE;
                }
                if let Some(&gw) = global_char_weights.get(&cp) {
                    if fname == "flypy.full.char.dict.yaml" {
                        final_wt = base_wt.saturating_add((gw / 1000).min(400_000_000) as u32);
                    }
                }
                char_shortest
                    .entry(cp)
                    .and_modify(|prev| {
                        if code_str.len() < prev.0.len()
                            || (code_str.len() == prev.0.len() && final_wt > prev.1)
                        {
                            *prev = (pcode, final_wt);
                        }
                    })
                    .or_insert((pcode, final_wt));
                char_full
                    .entry(cp)
                    .and_modify(|prev| {
                        if code_str.len() > prev.0.len()
                            || (code_str.len() == prev.0.len() && final_wt > prev.1)
                        {
                            *prev = (pcode, final_wt);
                        }
                    })
                    .or_insert((pcode, final_wt));
            }
            entries.push((pcode, text.to_string(), final_wt, flags));
        }
    }

    let mut char_metas = Vec::with_capacity(char_full.len());
    for (cp, (full_code, _)) in &char_full {
        let shortest_code = char_shortest.get(cp).map(|x| x.0).unwrap_or(*full_code);
        let (buf, n) = full_code.to_bytes_buf();
        let stem2 = Stem2::from_bytes(&buf[..n.min(2)]).unwrap_or_default();
        char_metas.push(CharMeta {
            codepoint: *cp,
            stem2,
            flags: if tghz_set.contains(cp) {
                0
            } else {
                ENTRY_FLAG_EXT_RARE
            },
            shortest_code,
            primary_short_code: PackedCode::EMPTY,
            full_code: *full_code,
        });
    }

    serialize_wbdict(DictBuildInput {
        schema_id: 10,
        entries,
        char_metas,
    })
}

fn build_cangjie_dict(
    cj_dir: &Path,
    schema_id: u32,
    files: &[&str],
    code_first: bool,
    tghz_set: &HashSet<u32>,
    global_char_weights: &HashMap<u32, u64>,
) -> (Vec<u8>, usize, usize) {
    let mut entries: Vec<(PackedCode, String, u32, u16)> = Vec::new();
    let mut char_shortest: HashMap<u32, (PackedCode, u32)> = HashMap::new();
    let mut char_full: HashMap<u32, (PackedCode, u32)> = HashMap::new();
    let mut per_code_seen: HashMap<PackedCode, u32> = HashMap::new();

    for (file_idx, &fname) in files.iter().enumerate() {
        let path = cj_dir.join(fname);
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let mut in_body = false;
        let file_base_wt: u32 = if file_idx == 0 {
            1_000_000_000
        } else {
            200_000_000
        };

        for raw_line in content.lines() {
            let line = raw_line.trim();
            if !in_body {
                if line == "..." {
                    in_body = true;
                }
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 2 {
                continue;
            }
            let (text, code_str) = if code_first {
                (parts[1].trim(), parts[0].trim())
            } else {
                (parts[0].trim(), parts[1].trim())
            };
            if text.is_empty() || code_str.is_empty() || code_str.len() > 5 {
                continue;
            }
            if !code_str.bytes().all(|b| b.is_ascii_lowercase()) {
                continue;
            }
            let Some(pcode) = PackedCode::from_ascii(code_str) else {
                continue;
            };

            let explicit_wt: Option<u64> = if !code_first {
                parts
                    .get(2)
                    .and_then(|s| s.trim().trim_end_matches('%').parse::<u64>().ok())
            } else {
                None
            };

            let seen_idx = per_code_seen.entry(pcode).or_insert(0);
            let order_tie = 500_000u32.saturating_sub((*seen_idx).saturating_mul(10));
            *seen_idx += 1;

            let mut chars_it = text.chars();
            let first_ch = chars_it.next().unwrap();
            let is_single = chars_it.next().is_none();

            let mut flags = 0u16;
            if is_single {
                flags |= ENTRY_FLAG_SINGLE_CHAR;
                let cp = first_ch as u32;
                // Keep all BMP CJK characters (both Traditional and Simplified) in standard charset;
                // mark non-BMP extension characters (Ext B-I) outside TGHZ as rare.
                if cp > 0xFFFF && !tghz_set.contains(&cp) {
                    flags |= ENTRY_FLAG_EXT_RARE;
                }
            }

            let final_wt: u32 = if let Some(ew) = explicit_wt {
                // Cangjie 6 provides explicit relative weights (0, 1, 2, ...):
                // prioritize explicit weight first, then preserve file order.
                file_base_wt
                    .saturating_add((ew.min(10_000) as u32).saturating_mul(1_000_000))
                    .saturating_add(order_tie)
            } else if is_single && (flags & ENTRY_FLAG_EXT_RARE) == 0 {
                // Preserve exact canonical file order for Cangjie 3/5, with gentle frequency nudge only for ties
                let freq_nudge = global_char_weights
                    .get(&(first_ch as u32))
                    .map(|&gw| (gw / 10_000_000).min(9) as u32)
                    .unwrap_or(0);
                file_base_wt.saturating_add(order_tie).saturating_add(freq_nudge)
            } else {
                order_tie
            };

            if is_single {
                let cp = first_ch as u32;
                char_shortest
                    .entry(cp)
                    .and_modify(|prev| {
                        if code_str.len() < prev.0.len()
                            || (code_str.len() == prev.0.len() && final_wt > prev.1)
                        {
                            *prev = (pcode, final_wt);
                        }
                    })
                    .or_insert((pcode, final_wt));
                char_full
                    .entry(cp)
                    .and_modify(|prev| {
                        if code_str.len() > prev.0.len()
                            || (code_str.len() == prev.0.len() && final_wt > prev.1)
                        {
                            *prev = (pcode, final_wt);
                        }
                    })
                    .or_insert((pcode, final_wt));
            }
            entries.push((pcode, text.to_string(), final_wt, flags));
        }
    }

    let mut char_metas = Vec::with_capacity(char_full.len());
    for (cp, (full_code, _)) in &char_full {
        let shortest_code = char_shortest.get(cp).map(|x| x.0).unwrap_or(*full_code);
        let (buf, n) = full_code.to_bytes_buf();
        let stem2 = Stem2::from_bytes(&buf[..n.min(2)]).unwrap_or_default();
        let flags = if *cp > 0xFFFF && !tghz_set.contains(cp) {
            ENTRY_FLAG_EXT_RARE
        } else {
            0
        };
        char_metas.push(CharMeta {
            codepoint: *cp,
            stem2,
            flags,
            shortest_code,
            primary_short_code: PackedCode::EMPTY,
            full_code: *full_code,
        });
    }

    let entry_cnt = entries.len();
    let char_cnt = char_metas.len();
    let blob = serialize_wbdict(DictBuildInput {
        schema_id,
        entries,
        char_metas,
    });
    (blob, entry_cnt, char_cnt)
}

fn build_pinyin_dict(ice_dir: &Path, tghz_set: &HashSet<u32>) -> Vec<u8> {
    // Pure char-level neural Pinyin decoding only needs the single-character syllable reading table
    // (`8105.dict.yaml` + `41448.dict.yaml`) for phonetic candidates and Bayesian `P(syl | char)`.
    // Multi-character phrase tables (~66 MB) are no longer used or bundled.
    let mut chars: Vec<(String, String, u32, u16)> = Vec::new();

    for (fname, is_rare_file) in [("8105.dict.yaml", false), ("41448.dict.yaml", true)] {
        let path = ice_dir.join(fname);
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let mut in_body = false;
        for raw_line in content.lines() {
            let line = raw_line.trim();
            if !in_body {
                if line == "..." {
                    in_body = true;
                }
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 2 {
                continue;
            }
            let text = parts[0].trim();
            let syl = parts[1].trim();
            if text.chars().count() != 1 || syl.is_empty() || syl.len() > 7 {
                continue;
            }
            if !syl.bytes().all(|b| b.is_ascii_lowercase()) {
                continue;
            }
            let wt = parts
                .get(2)
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(1)
                .min(u32::MAX as u64) as u32;
            let ch = text.chars().next().unwrap();
            let flags = if is_rare_file && !tghz_set.contains(&(ch as u32)) {
                PY_FLAG_EXT_RARE
            } else {
                0
            };
            chars.push((syl.to_string(), text.to_string(), wt, flags));
        }
    }

    serialize_pydict(PinyinDictBuildInput {
        chars,
        phrases: Vec::new(),
    })
}

fn build_english_dict(ice_dir: &Path) -> (Vec<u8>, usize) {
    // 1. Load frequency rank map from `google_20k.txt` (rank 0 = most frequent)
    let mut freq_rank: HashMap<String, usize> = HashMap::new();
    if let Ok(content) = fs::read_to_string(ice_dir.join("google_20k.txt")) {
        for (rank, line) in content.lines().enumerate() {
            let w = line.trim().to_ascii_lowercase();
            if w.len() >= 2 && w.bytes().all(|b| b.is_ascii_lowercase()) {
                freq_rank.entry(w).or_insert(rank);
            }
        }
    }

    // 2. Map lower -> (canonical_display, weight, flags)
    let mut map: HashMap<String, (String, u16, u16)> = HashMap::new();

    let mut insert_word = |disp: &str, weight: u16, flags: u16| {
        let lower = disp.to_ascii_lowercase();
        if !(2..=40).contains(&lower.len()) || !lower.bytes().all(|b| b.is_ascii_lowercase()) {
            return;
        }
        let has_upper = disp.bytes().any(|b| b.is_ascii_uppercase());
        map.entry(lower)
            .and_modify(|e| {
                if weight > e.1 {
                    e.1 = weight;
                }
                e.2 |= flags;
                if has_upper && e.0 == e.0.to_ascii_lowercase() {
                    e.0 = disp.to_owned();
                }
            })
            .or_insert_with(|| (disp.to_owned(), weight, flags));
    };

    // 2a. Parse `en.dict.yaml` (core 20k+ English words with canonical casing)
    if let Ok(content) = fs::read_to_string(ice_dir.join("en.dict.yaml")) {
        let mut in_body = false;
        for raw_line in content.lines() {
            let line = raw_line.trim();
            if !in_body {
                if line == "..." {
                    in_body = true;
                }
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let disp = line.split('\t').next().unwrap_or("").trim();
            if disp.is_empty() || !disp.bytes().all(|b| b.is_ascii_alphabetic()) {
                continue;
            }
            let lower = disp.to_ascii_lowercase();
            let (weight, flags) = if let Some(&rank) = freq_rank.get(&lower) {
                let w = if rank < 1000 {
                    62_000 - (rank as u16) * 10
                } else if rank < 5000 {
                    52_000 - ((rank - 1000) as u16) * 4
                } else {
                    (36_000u32.saturating_sub(((rank - 5000) as u32) * 2)).max(8_000) as u16
                };
                let f = if rank < 5000 { EN_FLAG_HIGH_FREQ } else { 0 };
                (w, f)
            } else {
                (14_000, 0)
            };
            insert_word(disp, weight, flags);
        }
    }

    // 2b. Parse `en_ext.dict.yaml` (tech, internet, brand & abbreviation words)
    if let Ok(content) = fs::read_to_string(ice_dir.join("en_ext.dict.yaml")) {
        let mut in_body = false;
        for raw_line in content.lines() {
            let line = raw_line.trim();
            if !in_body {
                if line == "..." {
                    in_body = true;
                }
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let disp = line.split('\t').next().unwrap_or("").trim();
            if disp.is_empty() || !disp.bytes().all(|b| b.is_ascii_alphabetic()) {
                continue;
            }
            let is_abbr = disp.len() <= 4 && disp.bytes().all(|b| b.is_ascii_uppercase());
            let mut flags = EN_FLAG_TECH | EN_FLAG_HIGH_FREQ;
            if is_abbr {
                flags |= EN_FLAG_ABBR;
            }
            insert_word(disp, 44_000, flags);
        }
    }

    // 2c. Ensure all top-20k frequency words are present even if omitted from `en.dict.yaml`
    for (w, rank) in &freq_rank {
        let weight = if *rank < 1000 {
            60_000 - (*rank as u16) * 10
        } else if *rank < 5000 {
            50_000 - ((*rank - 1000) as u16) * 4
        } else {
            (34_000u32.saturating_sub(((*rank - 5000) as u32) * 2)).max(8_000) as u16
        };
        let flags = if *rank < 5000 { EN_FLAG_HIGH_FREQ } else { 0 };
        insert_word(w, weight, flags);
    }
    drop(insert_word);

    // 2d. Curated modern tech/AI canonical-case overrides (2024–2026)
    let tech_overrides: &[(&str, u16, u16)] = &[
        ("Rust", 54_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("Python", 56_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("Swift", 50_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("TypeScript", 50_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("JavaScript", 50_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("GitHub", 54_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("iOS", 56_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
        ("macOS", 54_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("iPhone", 54_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("iPad", 52_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("Mac", 52_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("Claude", 52_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("Gemini", 52_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("DeepSeek", 50_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("Qwen", 48_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("Cursor", 50_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("GPT", 54_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
        ("AI", 58_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
        ("LLM", 52_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
        ("API", 56_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
        ("PR", 52_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
        ("MCP", 48_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ | EN_FLAG_ABBR),
        ("RWKV", 48_000, EN_FLAG_TECH | EN_FLAG_ABBR),
        ("Xcode", 50_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("Docker", 50_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("Kubernetes", 48_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("PyTorch", 50_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("CUDA", 48_000, EN_FLAG_TECH | EN_FLAG_ABBR),
        ("MLX", 46_000, EN_FLAG_TECH | EN_FLAG_ABBR),
        ("bug", 56_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("demo", 54_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
        ("Git", 54_000, EN_FLAG_TECH | EN_FLAG_HIGH_FREQ),
    ];
    for &(disp, w, f) in tech_overrides {
        let lower = disp.to_ascii_lowercase();
        map.insert(lower, (disp.to_owned(), w, f));
    }

    let entries: Vec<(String, String, u16, u16)> = map
        .into_iter()
        .map(|(k, (d, w, f))| (k, d, w, f))
        .collect();
    let cnt = entries.len();
    (serialize_endict(&entries), cnt)
}
