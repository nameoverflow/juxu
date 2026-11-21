use std::path::PathBuf;
use std::time::Instant;
use wubi_core::config::{WubiConfig, WubiSchema};
use wubi_core::engine::{CandidateBadge, WubiEngine, KEY_BACKSPACE, KEY_RETURN, KEY_SPACE};
use wubi_core::format::{AuxView, DictView};
use wubi_core::user_data::UserStore;

fn dicts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("Resources/Dicts")
}

#[test]
fn spaceless_personal_context_generalizes_beyond_the_shared_suffix() {
    let dir = dicts_dir();
    if !dir.join("wubi86.wbdict").exists() {
        return;
    }
    let config = WubiConfig {
        spaceless: true,
        auto_phrase: false,
        ..WubiConfig::default()
    };
    let mut engine = WubiEngine::new(config.clone());
    engine.load_dict(
        WubiSchema::Wubi86,
        DictView::open_mmap(dir.join("wubi86.wbdict")).unwrap(),
    );
    engine.set_aux(AuxView::open_mmap(dir.join("wubi_aux.wbaux")).unwrap());
    let enter = |engine: &mut WubiEngine, context: &str, code: &str| {
        engine.clear_composition();
        engine.set_recent_context(context);
        for (i, ch) in code.chars().enumerate() {
            engine.handle_key_event(0, Some(ch), false, 100 + i as u64 * 90);
        }
    };

    let legal = "法律团队为了明天的研讨会正在收集";
    let safety = "安全部门为了明天的检查会正在收集";
    for context in [legal, safety] {
        enter(&mut engine, context, "pvwguqou");
        assert_eq!(engine.preedit(), "安全资料");
    }
    for _ in 0..12 {
        for (context, preferred) in [
            ("法务团队为了下周的研讨会正在收集", "案例材料"),
            ("安全团队为了下周的检查会正在收集", "安全材料"),
        ] {
            enter(&mut engine, context, "pvwgsfou");
            let index = engine
                .all_candidates()
                .iter()
                .position(|candidate| candidate.text == preferred)
                .unwrap();
            let selected = engine.select_abs_candidate(index, 1_200).unwrap();
            assert_eq!(selected.commit_text, preferred);
        }
    }

    let path = std::env::temp_dir().join(format!("wubi-personal-{}.tsv", std::process::id()));
    engine.user.save_file(&path).unwrap();
    engine.user = UserStore::load_file(&path).unwrap();

    // Neither entire prefix nor the "资料" ending was seen during training.
    // The prefixes share the final four characters, but the RWKV memories differ.
    for code in ["pvwguqou", "ovwguqou"] {
        enter(&mut engine, legal, code);
        assert_eq!(engine.preedit(), "案例资料");
        assert_eq!(engine.all_candidates()[0].is_exact, code.starts_with('p'));
        enter(&mut engine, safety, code);
        assert_eq!(engine.preedit(), "安全资料");
    }

    // An incompatible neural checkpoint must fall back to the same short statistics.
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.lines().any(|line| line.starts_with("CONT_MODEL\t")));
    let short_only: String = saved.lines()
        .filter(|line| !line.starts_with("CONT_MODEL\t"))
        .flat_map(|line| line.chars().chain(['\n']))
        .collect();
    std::fs::write(&path, short_only).unwrap();
    engine.user = UserStore::load_file(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    for context in [legal, safety] {
        enter(&mut engine, context, "pvwguqou");
        assert_eq!(engine.preedit(), "安全资料");
    }
}

#[test]
fn spaceless_touch_correction_learns_confirmed_code() {
    let dir = dicts_dir();
    if !dir.join("wubi86.wbdict").exists() {
        return;
    }
    let mut config = WubiConfig { spaceless: true, ..WubiConfig::default() };
    let mut engine = WubiEngine::new(config.clone());
    engine.load_dict(WubiSchema::Wubi86, DictView::open_mmap(dir.join("wubi86.wbdict")).unwrap());
    engine.set_aux(AuxView::open_mmap(dir.join("wubi_aux.wbaux")).unwrap());
    let taps = [
        (b'y', 4.54, 0.03),
        (b'r', 3.02, 0.02),
        (b'w', 1.01, -0.02),
        (b'u', 5.98, 0.01),
        (b'j', 6.5, 1.0),
    ];
    let type_taps = |engine: &mut WubiEngine| {
        for (i, &(letter, x, y)) in taps.iter().enumerate() {
            engine.push_touch_letter(letter, x, y, false, 100 + i as u64 * 70);
        }
    };
    type_taps(&mut engine);
    assert_eq!(engine.raw_code(), "yrwuj");
    assert_eq!(engine.preedit(), "我们是");
    let first = &engine.all_candidates()[0];
    assert_eq!(first.badge, CandidateBadge::TypoCorrected);
    assert_eq!(first.comment, "trwu");
    assert!(!first.is_exact);
    assert_eq!(engine.user.touch_profile.offset_for(b't'), (0.0, 0.0));

    engine.handle_key_event(KEY_BACKSPACE, None, false, 500);
    assert_eq!(engine.preedit(), "我们");
    engine.push_touch_letter(b'j', 6.5, 1.0, false, 570);
    let result = engine.handle_key_event(KEY_SPACE, Some(' '), false, 640);
    assert_eq!(result.commit_text, "我们是");
    assert!(engine.user.touch_profile.offset_for(b't').0 > 0.07);
    assert_eq!(engine.user.touch_profile.keys[(b'j' - b'a') as usize].samples, 1);
    assert_eq!(engine.user.touch_profile.keys[(b'y' - b'a') as usize].samples, 0);

    engine.clear_context();
    config.typo.enabled = false;
    engine.set_config(config);
    type_taps(&mut engine);
    assert_eq!(engine.raw_code(), "yrwuj");
    assert_ne!(engine.preedit(), "我们是");
    assert!(engine.all_candidates().iter().all(|candidate| candidate.is_exact));
    let raw = engine.handle_key_event(KEY_RETURN, None, false, 900);
    assert_eq!(raw.commit_text, "yrwuj");
}

#[test]
fn real_dicts_latency_schemas_and_typo_correction() {
    let dir = dicts_dir();
    if !dir.join("wubi86.wbdict").exists() {
        return;
    }

    let t0 = Instant::now();
    let d86 = DictView::open_mmap(dir.join("wubi86.wbdict")).unwrap();
    let d98 = DictView::open_mmap(dir.join("wubi98.wbdict")).unwrap();
    let d06 = DictView::open_mmap(dir.join("wubi06.wbdict")).unwrap();
    let aux = AuxView::open_mmap(dir.join("wubi_aux.wbaux")).unwrap();
    let mmap_us = t0.elapsed().as_micros();
    let (py_cnt, uni_cnt, bi_cnt, tri_cnt) = aux.stats();
    println!(
        "Cold-start mmap of 86 + 98 + 06 + aux (py={}, uni={}, bi={}, tri={}, neural={}) took: {} us",
        py_cnt,
        uni_cnt,
        bi_cnt,
        tri_cnt,
        aux.has_neural(),
        mmap_us
    );
    assert!(mmap_us < 10_000, "mmap should be sub-millisecond to few ms");
    assert!(aux.has_unigrams(), "expected WBAUX002/WBAUX003 with corpus unigrams");
    assert!(aux.has_neural(), "expected WBAUX003 with Micro-RWKV-7 neural blob");
    assert!(uni_cnt >= 7_500 && bi_cnt >= 50_000 && tri_cnt >= 25_000);
    assert!(aux.trigram_boost('输', '入', '法') > 0.5);
    assert!(aux.trigram_boost('小', '红', '书') > 0.5);
    assert!(aux.bigram_boost('模', '型') > 1.0);

    // Verify exact INT8 numerical parity against Python QAT reference ("今天天气真" -> 好/热 vs 油/突)
    {
        let nv = aux.neural_view().expect("neural_view");
        let mut tracker = wubi_core::neural::NeuralContextTracker::default();
        tracker.init_with_model(&nv);
        tracker.commit_cjk_str(&nv, "今天天气真");
        let s_hao = nv.score_next_char(&tracker.active, '好');
        let s_re = nv.score_next_char(&tracker.active, '热');
        let s_you = nv.score_next_char(&tracker.active, '油');
        let s_tu = nv.score_next_char(&tracker.active, '突');
        println!(
            "RWKV-7 parity check '今天天气真' -> 好={:.6}, 热={:.6}, 油={:.6}, 突={:.6}",
            s_hao, s_re, s_you, s_tu
        );
        assert!((s_hao - (-0.255945)).abs() < 2e-4, "hao parity mismatch: {}", s_hao);
        assert!((s_re - (-5.737987)).abs() < 2e-4, "re parity mismatch: {}", s_re);
        assert!((s_you - (-10.955288)).abs() < 2e-4, "you parity mismatch: {}", s_you);
        assert!((s_tu - (-11.684685)).abs() < 2e-4, "tu parity mismatch: {}", s_tu);

        // Verify modern tech entity awareness in Micro-RWKV-7 ("大模" -> '型')
        tracker.init_with_model(&nv);
        tracker.commit_cjk_str(&nv, "大模");
        let s_xing = nv.score_next_char(&tracker.active, '型');
        println!("RWKV-7 modern tech check '大模' -> 型={:.4}", s_xing);
        assert!(s_xing > -2.0, "expected RWKV-7 to strongly predict '型' after '大模', got {}", s_xing);
    }

    // Verify authentic differences across 86, 98, and 06 (新世纪)!
    // E.g. '码': 86 = dcg, 98 = dcg, '软': 86 = lqw, 98 = lqw; '气': 86 = rnb, 98 = rty, 06 = rty/rn...
    let code_86_jsj = d86.encode_phrase("计算机").unwrap().as_string();
    let code_98_jsj = d98.encode_phrase("计算机").unwrap().as_string();
    let code_06_jsj = d06.encode_phrase("计算机").unwrap().as_string();
    println!(
        "Phrase '计算机' codes -> 86: {}, 98: {}, 06: {}",
        code_86_jsj, code_98_jsj, code_06_jsj
    );

    let mut engine = WubiEngine::new(WubiConfig::default());
    engine.load_dict(WubiSchema::Wubi86, d86);
    engine.load_dict(WubiSchema::Wubi98, d98);
    engine.load_dict(WubiSchema::Wubi06, d06);
    engine.set_aux(aux);

    // Benchmark 1000 full 4-keystroke sequences WITH statistical typo-correction enabled!
    let bench_start = Instant::now();
    let iterations = 250;
    for _ in 0..iterations {
        for ch in ['t', 'r', 'w', 'u'] {
            let _ = engine.handle_key_event(0, Some(ch), false, 1000);
        }
        engine.clear_composition();
    }
    let avg_keystroke_us = bench_start.elapsed().as_secs_f64() * 1e6 / ((iterations * 4) as f64);
    println!(
        "Average full keystroke + 24-beam Bayesian typo decode latency: {:.2} us",
        avg_keystroke_us
    );
    assert!(avg_keystroke_us < 500.0, "avg_keystroke_us={avg_keystroke_us:.2}");

    // Real touchscreen slip test on 136k-entry Wubi86:
    // User wants "我们" (`trwu`), thumb hits right edge of `t` (4.0,0) -> (4.54, 0.03) (`y`)
    let _ = engine.push_touch_letter(b'y', 4.54, 0.03, true, 100);
    let _ = engine.push_touch_letter(b'r', 3.02, 0.02, true, 170);
    let _ = engine.push_touch_letter(b'w', 1.01, -0.02, true, 240);
    let _ = engine.push_touch_letter(b'u', 5.98, 0.01, true, 310);
    let top_cands: Vec<String> = engine
        .page_candidates()
        .iter()
        .map(|c| format!("{}[{} {:.0}%]", c.text, c.comment, c.confidence * 100.0))
        .collect();
    println!("Touch slip 'yrwu' (near t/y border) -> {:?}", top_cands);
    assert!(
        engine.page_candidates().iter().take(2).any(|c| c.text == "我们"),
        "expected '我们' in top-2 candidates for touch slip yrwu, got {:?}",
        top_cands
    );
    engine.clear_composition();

    // Switch to 98 and 06 and verify instant lookup
    engine.set_schema(WubiSchema::Wubi98);
    let _ = engine.handle_key_event(0, Some('q'), false, 500);
    assert_eq!(engine.page_candidates()[0].text, "我");
    engine.clear_composition();

    engine.set_schema(WubiSchema::Wubi06);
    let _ = engine.handle_key_event(0, Some('q'), false, 600);
    assert_eq!(engine.page_candidates()[0].text, "我");
    engine.clear_composition();

    // Verify high-frequency exact homocode protection in Wubi98:
    // "还能" + wdwx must prefer "优化" (weight 61M) over "拿破仑" (weight 2.3M)
    engine.set_schema(WubiSchema::Wubi98);
    engine.set_recent_context("还能");
    for (i, ch) in "wdwx".chars().enumerate() {
        let _ = engine.handle_key_event(0, Some(ch), false, 3000 + i as u64 * 60);
    }
    let wdwx_cands: Vec<String> = engine.page_candidates().iter().map(|c| c.text.clone()).collect();
    println!("Wubi98 '还能' + wdwx -> {:?}", wdwx_cands);
    assert_eq!(wdwx_cands[0], "优化", "expected 优化 over 拿破仑 after 还能");
    engine.clear_composition();

    // Test lightweight N-gram idle next-word prediction & latency
    engine.set_schema(WubiSchema::Wubi86);
    let samples = [
        "",
        "我",
        "我们",
        "今天",
        "这个",
        "不知",
        "小红",
        "大模",
        "没问",
        "真的",
        "为什么",
        "但是",
        "如果",
        "现在",
        "输入",
        "情绪",
        "性价",
        "绷不",
        "最近",
        "可以",
    ];
    let pt0 = Instant::now();
    for s in samples {
        engine.set_recent_context(s);
        let preds = engine.predict_next_words(8);
        println!("Predict after {:<6} -> {:?}", s, preds);
        assert_eq!(preds.len(), 8);
    }
    let avg_pred_us = pt0.elapsed().as_secs_f64() * 1e6 / (samples.len() as f64);
    println!("Average idle next-word prediction latency: {:.2} us", avg_pred_us);
    assert!(avg_pred_us < 1500.0, "prediction latency too high: {:.2} us", avg_pred_us);

    // Verify specific high-confidence completions
    engine.set_recent_context("不知");
    assert!(engine.predict_next_words(8).iter().take(3).any(|w| w == "道"));
    engine.set_recent_context("小红");
    assert!(engine.predict_next_words(8).iter().take(3).any(|w| w == "书"));
    engine.set_recent_context("大模");
    assert!(engine.predict_next_words(8).iter().take(3).any(|w| w == "型"));
    engine.set_recent_context("输入");
    assert!(engine.predict_next_words(8).iter().take(4).any(|w| w == "法"));

    // Test LM-guided missed-space correction on real Wubi86 + Micro-RWKV-7:
    // 1) 3-code + 4-code (empty at key #4): "周末我想" + 去(fcu) + [no space] + 爬山(rhmm) -> "去爬山"
    engine.clear_composition();
    engine.set_recent_context("这个周末我想");
    let mut got = String::new();
    for (i, ch) in "fcurhmm".chars().enumerate() {
        let out = engine.handle_key_event(0, Some(ch), false, 2000 + i as u64 * 60);
        assert!(!out.bell, "unexpected bell on fcurhmm at {}", ch);
        got.push_str(&out.commit_text);
    }
    if engine.is_composing() {
        let out = engine.handle_key_event(wubi_core::engine::KEY_SPACE, Some(' '), false, 2500);
        got.push_str(&out.commit_text);
    }
    assert_eq!(got, "去爬山", "expected empty-collision missed-space rescue '去爬山', got '{}'", got);

    // 2) 3-code + 4-code (colliding 4-code exact match at key #4!):
    //    "我们一起" + 去(fcu) + [no space] + 吃饭(ktqn) -> `fcuk` collides in Wubi86, resolved at Key #5!
    engine.clear_composition();
    engine.set_recent_context("今天天气真好我们一起");
    let mut got2 = String::new();
    for (i, ch) in "fcuktqn".chars().enumerate() {
        let out = engine.handle_key_event(0, Some(ch), false, 3000 + i as u64 * 60);
        got2.push_str(&out.commit_text);
    }
    if engine.is_composing() {
        let out = engine.handle_key_event(wubi_core::engine::KEY_SPACE, Some(' '), false, 3600);
        got2.push_str(&out.commit_text);
    }
    assert_eq!(got2, "去吃饭", "expected colliding missed-space rescue '去吃饭', got '{}'", got2);

    // 3) 3-code + 1-code: "你觉得这家" + 店(yhk) + [no space] + 的(r) + Space -> "店的"
    engine.clear_composition();
    engine.set_recent_context("你觉得这家");
    let mut got3 = String::new();
    for (i, ch) in "yhkr".chars().enumerate() {
        let out = engine.handle_key_event(0, Some(ch), false, 4000 + i as u64 * 60);
        got3.push_str(&out.commit_text);
    }
    let out3_sp = engine.handle_key_event(wubi_core::engine::KEY_SPACE, Some(' '), false, 4300);
    got3.push_str(&out3_sp.commit_text);
    assert_eq!(got3, "店的", "expected 3+1 missed-space rescue '店的', got '{}'", got3);
}

#[test]
fn real_dicts_full_pinyin_and_xiaohe_sentence_and_typo() {
    use wubi_core::pinyin_dict::PinyinDictView;

    let base = dicts_dir();
    let pdict = PinyinDictView::from_file(base.join("pinyin.pydict")).expect("open pinyin.pydict");
    let xh_dict = DictView::open_mmap(base.join("xiaohe.wbdict")).expect("open xiaohe.wbdict");
    let aux = AuxView::open_mmap(base.join("wubi_aux.wbaux")).expect("open wubi_aux.wbaux");

    let (syl_cnt, char_cnt, phrase_cnt) = pdict.stats();
    println!("Loaded pinyin.pydict: {} syllables, {} chars, {} phrases", syl_cnt, char_cnt, phrase_cnt);
    assert!(syl_cnt > 390 && char_cnt > 8000 && phrase_cnt > 100_000);

    // 1. Full Pinyin real-dict sentence + abbr + apostrophe + touch typo + latency
    let mut cfg = WubiConfig::default();
    cfg.schema = WubiSchema::Pinyin;
    let mut engine = WubiEngine::new(cfg);
    engine.load_pinyin_dict(pdict);
    engine.set_aux(aux);

    let sentences = [
        ("womenshurufa", "我们输入法"),
        ("jintiantianqizhenhao", "今天天气真好"),
        ("srf", "输入法"),
        ("xi'an", "西安"),
    ];
    let t0 = Instant::now();
    let mut total_keys = 0usize;
    for (keys, expected) in sentences {
        engine.clear_composition();
        for (i, ch) in keys.chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 1000 + i as u64 * 40);
            total_keys += 1;
        }
        let top = engine.page_candidates().first().map(|c| c.text.as_str()).unwrap_or("");
        println!("Pinyin {:<22} -> top='{}' (seg='{}')", keys, top, engine.segmented_code());
        assert_eq!(top, expected, "pinyin {} failed: got '{}'", keys, top);
    }
    let avg_us = t0.elapsed().as_secs_f64() * 1e6 / (total_keys as f64);
    println!("Real pinyin.pydict average per-key latency: {:.2} us", avg_us);
    assert!(avg_us < 8000.0, "pinyin per-key latency too high: {:.2} us", avg_us);

    // Touch typo rescue on real pinyin.pydict: "jontiantianqi" (i->o border) -> "今天天气"
    engine.clear_composition();
    let _ = engine.push_touch_letter(b'j', 6.5, 1.0, true, 2000);
    let _ = engine.push_touch_letter(b'o', 7.45, 0.0, true, 2040); // near i(7.0)/o(8.0) border
    for (i, ch) in "ntiantianqi".chars().enumerate() {
        let _ = engine.handle_key_event(0, Some(ch), false, 2080 + i as u64 * 40);
    }
    let typo_top = engine.page_candidates().first().map(|c| c.text.as_str()).unwrap_or("");
    println!("Pinyin touch typo j(o)ntiantianqi -> '{}'", typo_top);
    assert_eq!(typo_top, "今天天气");

    // Verify `ju` has rich homophone candidates including `橘`, and committing `橘子` boosts `橘`
    engine.clear_composition();
    for ch in "ju".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 2300);
    }
    let ju_cands = engine.all_candidates();
    println!("Pinyin 'ju' total candidates: {}, includes 橘: {}", ju_cands.len(), ju_cands.iter().any(|c| c.text == "橘"));
    assert!(ju_cands.len() >= 25, "expected >=25 candidates for ju, got {}", ju_cands.len());
    assert!(ju_cands.iter().any(|c| c.text == "橘"), "expected '橘' in ju candidates");

    // Commit `橘子`, then type `ju` again and verify `橘` rises in rank!
    engine.clear_composition();
    for ch in "juzi".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 2400);
    }
    if let Some(idx) = engine.all_candidates().iter().position(|c| c.text == "橘子") {
        let _ = engine.select_abs_candidate(idx, 2450);
    }
    for ch in "ju".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 2500);
    }
    let ju_pos_after = engine.all_candidates().iter().position(|c| c.text == "橘");
    println!("Pinyin 'ju' position of '橘' after committing '橘子': {:?}", ju_pos_after);
    assert!(ju_pos_after.is_some_and(|p| p < 12), "expected '橘' to rank in top 12 after 橘子, got {:?}", ju_pos_after);

    // Verify jv->ju compatibility + corrected spelling comment + missing/extra letter correction
    for (keys, expected_sub) in [
        ("jv", "举"),
        ("fanan", "方案"),
        ("zhogwen", "中文"),
        ("zhonngwen", "中文"),
    ] {
        engine.clear_composition();
        for ch in keys.chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 2600);
        }
        let top = &engine.page_candidates()[0];
        println!("Pinyin spelling/typo '{}' -> '{}' (comment='{}')", keys, top.text, top.comment);
        assert!(
            engine.all_candidates().iter().any(|c| c.text.contains(expected_sub)),
            "expected '{}' candidates to contain '{}'",
            keys,
            expected_sub
        );
    }

    // Verify shenjingbing, sjb, shenjb, jiandan, and jianpin->简
    for (keys, expected_top) in [
        ("shenjingbing", "神经病"),
        ("jiandan", "简单"),
    ] {
        engine.clear_composition();
        for ch in keys.chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 2700);
        }
        let top = &engine.page_candidates()[0];
        println!("Pinyin exact '{}' -> '{}' (seg='{}')", keys, top.text, engine.segmented_code());
        assert_eq!(top.text, expected_top);
    }
    for keys in ["sjb", "shenjb"] {
        engine.clear_composition();
        for ch in keys.chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 2800);
        }
        assert!(
            engine.all_candidates().iter().any(|c| c.text == "神经病"),
            "expected '{}' candidates to include '神经病', got {:?}",
            keys,
            engine.page_candidates().iter().map(|c| &c.text).collect::<Vec<_>>()
        );
    }
    engine.clear_composition();
    for ch in "jianpin".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 2900);
    }
    assert!(
        engine.all_candidates().iter().any(|c| c.text == "简"),
        "expected 'jianpin' candidates to include '简'"
    );

    // 2. Xiaohe YinXing real-dict lookup + 'z' typo rescue + spaceless sentence
    let mut cfg_xh = WubiConfig::default();
    cfg_xh.schema = WubiSchema::Xiaohe;
    cfg_xh.spaceless = false;
    engine.set_config(cfg_xh);
    engine.load_dict(WubiSchema::Xiaohe, xh_dict);

    for ch in "uuip".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 3000);
    }
    // `uuip` in flypy is "输入" (or auto-committed if unique!)
    // Let's check `zz` / `zn` ("真" or "这" / "在")
    engine.clear_composition();
    let _ = engine.handle_key_event(0, Some('z'), false, 3100);
    let top_z = engine.page_candidates().first().map(|c| c.text.as_str()).unwrap_or("");
    println!("Xiaohe 'z' -> '{}'", top_z);
    assert!(!top_z.is_empty());
}

#[test]
fn integrated_cross_mode_auto_phrase_and_rwkv_next_key_priors() {
    use wubi_core::pinyin_dict::PinyinDictView;

    let base = dicts_dir();
    if !base.join("pinyin.pydict").exists() || !base.join("wubi86.wbdict").exists() {
        return;
    }
    let pdict = PinyinDictView::from_file(base.join("pinyin.pydict")).expect("open pinyin.pydict");
    let d86 = DictView::open_mmap(base.join("wubi86.wbdict")).expect("open wubi86.wbdict");
    let xh = DictView::open_mmap(base.join("xiaohe.wbdict")).expect("open xiaohe.wbdict");
    let aux = AuxView::open_mmap(base.join("wubi_aux.wbaux")).expect("open wubi_aux.wbaux");

    let mut cfg = WubiConfig::default();
    cfg.schema = WubiSchema::Pinyin;
    cfg.auto_phrase = true;
    cfg.auto_phrase_threshold = 2;
    let mut engine = WubiEngine::new(cfg.clone());
    engine.load_dict(WubiSchema::Wubi86, d86);
    engine.load_dict(WubiSchema::Xiaohe, xh);
    engine.load_pinyin_dict(pdict);
    engine.set_aux(aux);

    // 1. RWKV-driven next_key_priors in Full Pinyin: after "今天天气真", 'h' (好 hao) must beat 'q'
    engine.clear_composition();
    engine.set_recent_context("今天天气真");
    let py_priors = engine.next_key_priors();
    let p_h = py_priors[(b'h' - b'a') as usize];
    let p_q = py_priors[(b'q' - b'a') as usize];
    println!("Pinyin RWKV next_key_priors after '今天天气真': h={}, q={}", p_h, p_q);
    assert!(p_h > p_q, "expected RWKV Pinyin prior for 'h' ({}) > 'q' ({})", p_h, p_q);

    // 2. RWKV-driven next_key_priors in Spaceless Wubi86: after "大模", 'g' (型 = gaej) must be boosted (> 100)
    let mut cfg_sp = cfg.clone();
    cfg_sp.schema = WubiSchema::Wubi86;
    cfg_sp.spaceless = true;
    engine.set_config(cfg_sp);
    engine.set_recent_context("大模");
    let sp_priors = engine.next_key_priors();
    let p_g = sp_priors[(b'g' - b'a') as usize];
    println!("Spaceless Wubi86 RWKV next_key_priors after '大模': g={}", p_g);
    assert!(p_g > 110, "expected RWKV Spaceless prior for 'g' (型) > 110, got {}", p_g);

    // 3. RWKV-driven next_key_priors in Xiaohe: after "大模", 'x' (型 = xk..) must be boosted (> 110)
    let mut cfg_xh = cfg.clone();
    cfg_xh.schema = WubiSchema::Xiaohe;
    cfg_xh.spaceless = false;
    engine.set_config(cfg_xh);
    engine.set_recent_context("大模");
    let xh_priors = engine.next_key_priors();
    let p_x = xh_priors[(b'x' - b'a') as usize];
    println!("Xiaohe RWKV next_key_priors after '大模': x={}", p_x);
    assert!(p_x > 110, "expected RWKV Xiaohe prior for 'x' (型) > 110, got {}", p_x);

    // 4. Cross-mode dynamic phrase: a phrase added via Wubi86/UserStore is immediately retrievable in Full Pinyin!
    engine.set_schema(WubiSchema::Wubi86);
    {
        let d = engine.dicts[WubiSchema::Wubi86.slot_index()].as_ref();
        let pd = engine.pinyin_dict.as_ref();
        let _ = engine.user.add_phrase_with_pinyin("句序", false, d, pd);
    }
    engine.set_schema(WubiSchema::Pinyin);
    engine.clear_context();
    for (i, ch) in "juxu".chars().enumerate() {
        let _ = engine.handle_key_event(0, Some(ch), false, 4000 + i as u64 * 40);
    }
    let top_juxu = engine.page_candidates().first().map(|c| c.text.as_str()).unwrap_or("");
    println!("Cross-mode Pinyin 'juxu' after adding '句序' -> top='{}'", top_juxu);
    assert_eq!(top_juxu, "句序", "expected cross-mode user phrase '句序' at #1 in Full Pinyin");

    // 5. Surprisal-driven auto-phrase: low-surprisal pair "我的" must NOT pollute user phrases,
    //    while high-surprisal OOV pair "星锚" promotes quickly on repeat!
    engine.clear_composition();
    engine.set_schema(WubiSchema::Wubi86);
    for round in 0..3u64 {
        engine.clear_context();
        let base_t = 5000 + round * 1000;
        let _ = engine.user.record_commit_with_surprisal(
            "",
            "我",
            base_t,
            true,
            2,
            engine.dicts[0].as_ref(),
            engine.pinyin_dict.as_ref(),
            Some(2.0),
        );
        let _ = engine.user.record_commit_with_surprisal(
            "我",
            "的",
            base_t + 200,
            true,
            2,
            engine.dicts[0].as_ref(),
            engine.pinyin_dict.as_ref(),
            Some(0.8), // very low surprisal (RWKV strongly predicts '的' after '我')
        );
    }
    assert!(
        !engine.user.phrases.contains_key("我的"),
        "low-surprisal glue pair '我的' must not be promoted into user phrases"
    );

    // Rare coined term "星锚" (high surprisal = 8.5 nats) requires repeat (2 bursts) so accidental
    // cross-word pairs don't pollute phrases, then immediately indexes into Full Pinyin!
    let mut promoted = None;
    for round in 0..2u64 {
        let base_t = 9000 + round * 1000;
        let _ = engine.user.record_commit_with_surprisal(
            "",
            "星",
            base_t,
            true,
            2,
            engine.dicts[0].as_ref(),
            engine.pinyin_dict.as_ref(),
            Some(5.0),
        );
        promoted = engine.user.record_commit_with_surprisal(
            "星",
            "锚",
            base_t + 200,
            true,
            2,
            engine.dicts[0].as_ref(),
            engine.pinyin_dict.as_ref(),
            Some(8.5),
        );
    }
    assert_eq!(promoted.as_deref(), Some("星锚"));
    // Verify it is immediately indexed and retrievable in Full Pinyin!
    engine.set_schema(WubiSchema::Pinyin);
    engine.clear_context();
    for (i, ch) in "xingmao".chars().enumerate() {
        let _ = engine.handle_key_event(0, Some(ch), false, 9500 + i as u64 * 40);
    }
    let top_xm = engine.page_candidates().first().map(|c| c.text.as_str()).unwrap_or("");
    println!("Surprisal-promoted '星锚' in Full Pinyin 'xingmao' -> top='{}'", top_xm);
    assert_eq!(top_xm, "星锚");

    // 6. Pinyin touch-offset learning must learn the *corrected* syllable letter (`i`), NOT the mis-tapped (`o`)!
    engine.clear_composition();
    engine.clear_context();
    engine.user.touch_profile.reset();
    let _ = engine.push_touch_letter(b'j', 6.5, 1.0, true, 10_000);
    let _ = engine.push_touch_letter(b'o', 7.42, 0.0, true, 10_040); // between i(7.0) and o(8.0)
    for (i, ch) in "ntiantianqi".chars().enumerate() {
        let _ = engine.handle_key_event(0, Some(ch), false, 10_080 + i as u64 * 40);
    }
    assert_eq!(engine.page_candidates()[0].text, "今天天气");
    let _ = engine.select_abs_candidate(0, 10_600);
    assert!(
        engine.user.touch_profile.offset_for(b'i').0 > 0.02,
        "expected corrected Pinyin touch commit to adapt 'i' offset (+x), got {:?}",
        engine.user.touch_profile.offset_for(b'i')
    );
    assert_eq!(
        engine.user.touch_profile.keys[(b'o' - b'a') as usize].samples,
        0,
        "mis-tapped key 'o' must not absorb corrected 'i' touch sample"
    );

    // 7. Softened validNextMask after a boundary-slip Key #1 in Wubi86:
    //    Touching near t(4.0)/y(5.0) border -> reported 'y', mask must still allow 'r' (from 'tr..' = 我们)
    let mut cfg_wb = cfg.clone();
    cfg_wb.schema = WubiSchema::Wubi86;
    cfg_wb.spaceless = false;
    engine.set_config(cfg_wb);
    engine.clear_composition();
    let _ = engine.push_touch_letter(b'y', 4.54, 0.02, true, 11_000);
    let mask = engine.valid_next_mask();
    let r_bit = 1u32 << (b'r' - b'a');
    assert_ne!(mask & r_bit, 0, "softened validNextMask should keep 'r' valid after t/y border slip");

    // 8. Full-Pinyin tiered candidate list: cap long full sentences at <= 3, then surface high-confidence
    //    multi-character prefix words (e.g. `试试`/`事实`/`实时`) BEFORE single characters!
    engine.set_schema(WubiSchema::Pinyin);
    engine.clear_composition();
    engine.clear_context();
    for (i, ch) in "shishijiuss".chars().enumerate() {
        let _ = engine.handle_key_event(0, Some(ch), false, 12_000 + i as u64 * 40);
    }
    let all = engine.all_candidates();
    let preview: Vec<&str> = all.iter().take(12).map(|c| c.text.as_str()).collect();
    println!("Pinyin 'shishijiuss' tiered top-12: {:?}", preview);
    let full_sent_cnt = all.iter().take(6).filter(|c| c.text.chars().count() >= 5).count();
    assert!(full_sent_cnt <= 3, "expected <= 3 full sentences at top, got {} in {:?}", full_sent_cnt, preview);
    let prefix_word_cnt = all
        .iter()
        .skip(full_sent_cnt)
        .take(6)
        .filter(|c| (2..=4).contains(&c.text.chars().count()))
        .count();
    assert!(
        prefix_word_cnt >= 4,
        "expected >= 4 high-confidence prefix words right after full sentences, got {} in {:?}",
        prefix_word_cnt,
        preview
    );

    // 9. Partial prefix selection in Full Pinyin must drain committed keys AND reset PinyinStream
    //    so remaining preedit/candidates decode ONLY the uncommitted tail!
    engine.clear_composition();
    engine.clear_context();
    for (i, ch) in "zhegejianpan".chars().enumerate() {
        let _ = engine.handle_key_event(0, Some(ch), false, 13_000 + i as u64 * 40);
    }
    let idx_zhe = engine
        .all_candidates()
        .iter()
        .position(|c| c.text == "这")
        .expect("expected single-char '这' in candidates for zhegejianpan");
    let out1 = engine.select_abs_candidate(idx_zhe, 13_600).unwrap();
    assert_eq!(out1.commit_text, "这");
    assert_eq!(engine.raw_code(), "gejianpan");
    assert!(
        !engine.preedit().starts_with('这'),
        "remaining preedit must not keep committed prefix '这', got '{}'",
        engine.preedit()
    );
    assert_eq!(engine.preedit(), "个键盘");

    // 10. High-cohesion modern word missing from Wubi86 ("微信" = tmg + wy -> tmwy):
    //     Must be auto-composed into user phrase `tmwy` instead of rejected by low RWKV surprisal!
    let mut cfg_wb_auto = cfg.clone();
    cfg_wb_auto.schema = WubiSchema::Wubi86;
    cfg_wb_auto.spaceless = true;
    cfg_wb_auto.auto_phrase = true;
    cfg_wb_auto.auto_phrase_threshold = 2;
    engine.set_config(cfg_wb_auto);
    engine.clear_composition();
    engine.clear_context();
    for (i, ch) in "tmgwy".chars().enumerate() {
        let _ = engine.handle_key_event(0, Some(ch), false, 14_000 + i as u64 * 50);
    }
    let out_wx = engine.handle_key_event(KEY_SPACE, Some(' '), false, 14_350);
    assert_eq!(out_wx.commit_text, "微信");
    assert!(
        engine.user.phrases.contains_key("微信"),
        "expected missing high-cohesion word '微信' to be auto-composed into user phrases"
    );
    // Switch to standard Wubi86 and verify `tmwy` now directly yields "微信"!
    let mut cfg_wb_std = cfg.clone();
    cfg_wb_std.schema = WubiSchema::Wubi86;
    cfg_wb_std.spaceless = false;
    engine.set_config(cfg_wb_std);
    engine.clear_composition();
    for (i, ch) in "tmwy".chars().enumerate() {
        let _ = engine.handle_key_event(0, Some(ch), false, 15_000 + i as u64 * 50);
    }
    assert!(
        engine.all_candidates().iter().any(|c| c.text == "微信"),
        "expected 'tmwy' to match auto-composed '微信', got {:?}",
        engine.page_candidates().iter().map(|c| &c.text).collect::<Vec<_>>()
    );

    // 11. 3-char coined phrase ("哈基米"): must NOT be short-circuited by 2-char tail "基米",
    //     and must compose across both single-char bursts AND mixed 2+1 bursts ("哈基" + "米")!
    engine.clear_composition();
    engine.clear_context();
    for round in 0..2u64 {
        let base_t = 16_000 + round * 1000;
        engine.user.break_single_burst();
        if round == 0 {
            for (i, ch) in ["哈", "基", "米"].iter().enumerate() {
                let _ = engine.user.record_commit_with_surprisal(
                    "",
                    ch,
                    base_t + i as u64 * 150,
                    true,
                    2,
                    engine.dicts[0].as_ref(),
                    engine.pinyin_dict.as_ref(),
                    Some(5.5),
                );
            }
        } else {
            // Mixed 2-char + 1-char burst ("哈基" + "米")
            let _ = engine.user.record_commit_with_surprisal(
                "",
                "哈基",
                base_t,
                true,
                2,
                engine.dicts[0].as_ref(),
                engine.pinyin_dict.as_ref(),
                Some(5.5),
            );
            let _ = engine.user.record_commit_with_surprisal(
                "哈基",
                "米",
                base_t + 150,
                true,
                2,
                engine.dicts[0].as_ref(),
                engine.pinyin_dict.as_ref(),
                Some(5.5),
            );
        }
    }
    assert!(
        engine.user.phrases.contains_key("哈基米"),
        "expected 3-char '哈基米' to auto-compose across bursts, got {:?}",
        engine.user.phrases.keys().collect::<Vec<_>>()
    );
    // Verify 4-code Wubi lookup (`kaoy` or encoded code) AND Full Pinyin (`hajimi`) both hit "哈基米"!
    let hjm_code = engine.dicts[0].as_ref().unwrap().encode_phrase("哈基米").unwrap().as_string();
    engine.set_schema(WubiSchema::Wubi86);
    engine.clear_composition();
    let mut hjm_committed = String::new();
    for (i, ch) in hjm_code.chars().enumerate() {
        let out = engine.handle_key_event(0, Some(ch), false, 19_000 + i as u64 * 40);
        hjm_committed.push_str(&out.commit_text);
    }
    assert!(
        hjm_committed == "哈基米" || engine.all_candidates().iter().any(|c| c.text == "哈基米"),
        "expected Wubi code '{}' to commit or match '哈基米', got commit='{}'",
        hjm_code,
        hjm_committed
    );
    engine.set_schema(WubiSchema::Pinyin);
    engine.clear_composition();
    for (i, ch) in "hajimi".chars().enumerate() {
        let _ = engine.handle_key_event(0, Some(ch), false, 19_500 + i as u64 * 40);
    }
    assert_eq!(
        engine.page_candidates().first().map(|c| c.text.as_str()),
        Some("哈基米"),
        "expected Full Pinyin 'hajimi' to rank '哈基米' at #1"
    );
}

#[test]
fn real_dicts_xiaohe_shuangpin_sentence_and_priors() {
    use wubi_core::pinyin_dict::PinyinDictView;

    let base = dicts_dir();
    let pdict = PinyinDictView::from_file(base.join("pinyin.pydict")).expect("open pinyin.pydict");
    let aux = AuxView::open_mmap(base.join("wubi_aux.wbaux")).expect("open wubi_aux.wbaux");

    let mut cfg = WubiConfig::default();
    cfg.schema = WubiSchema::XiaoheShuangpin;
    let mut engine = WubiEngine::new(cfg);
    engine.load_pinyin_dict(pdict);
    engine.set_aux(aux);

    let cases = [
        ("jbtmtmqivfhc", "今天天气真好"),
        ("womfuurufa", "我们输入法"),
        ("vsgorfmb", "中国人民"),
        ("xnheulpb", "小鹤双拼"),
        ("xian", "西安"),
    ];
    for (keys, expected) in cases {
        engine.clear_composition();
        for (i, ch) in keys.chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 3000 + i as u64 * 40);
        }
        let top = engine.page_candidates().first().map(|c| c.text.as_str()).unwrap_or("");
        println!("XiaoheSP {:<16} -> top='{}' (seg='{}')", keys, top, engine.segmented_code());
        assert_eq!(top, expected, "xiaohe_sp {} failed: got '{}'", keys, top);
    }

    // Zero-initial `an`/`ang`/`er` check
    engine.clear_composition();
    for ch in "ah".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 4000);
    }
    assert!(
        engine.all_candidates().iter().any(|c| c.text == "昂"),
        "expected '昂' for zero-initial 'ah' (ang)"
    );
}

#[test]
fn real_dicts_cangjie_3_5_6_input_and_five_code_rules() {
    let base = dicts_dir();
    let cj3 = DictView::open_mmap(base.join("cangjie3.wbdict")).expect("open cangjie3.wbdict");
    let cj5 = DictView::open_mmap(base.join("cangjie5.wbdict")).expect("open cangjie5.wbdict");
    let cj6 = DictView::open_mmap(base.join("cangjie6.wbdict")).expect("open cangjie6.wbdict");
    let aux = AuxView::open_mmap(base.join("wubi_aux.wbaux")).expect("open wubi_aux.wbaux");

    let mut cfg = WubiConfig::default();
    cfg.schema = WubiSchema::Cangjie5;
    let mut engine = WubiEngine::new(cfg);
    engine.load_dict(WubiSchema::Cangjie3, cj3);
    engine.load_dict(WubiSchema::Cangjie5, cj5);
    engine.load_dict(WubiSchema::Cangjie6, cj6);
    engine.set_aux(aux);

    // 1. Verify Cangjie 5 radical preedit and basic characters
    // "hqi" (竹手戈) -> "我"; "oi" (人戈) -> "代" / "仓" etc.; "a" (日) -> "日"
    let out_a = engine.handle_key_event(0, Some('a'), false, 100);
    assert_eq!(out_a.preedit, "日");
    assert_eq!(engine.page_candidates().first().map(|c| c.text.as_str()), Some("日"));
    engine.clear_composition();

    for ch in "hqi".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 200);
    }
    assert_eq!(engine.preedit(), "竹手戈");
    assert_eq!(engine.page_candidates().first().map(|c| c.text.as_str()), Some("我"));
    engine.clear_composition();

    // 2. Verify 5-code Cangjie character does NOT prematurely commit or block on 4th code!
    // "蒼" in Cangjie 5 is "toir" (4 codes), "頡" is "grmbc" (5 codes: 土口一月金)!
    for ch in "grmb".chars() {
        let out = engine.handle_key_event(0, Some(ch), false, 300);
        assert!(out.commit_text.is_empty(), "4th code in Cangjie must NOT auto-commit");
    }
    assert_eq!(engine.preedit(), "土口一月");
    let out5 = engine.handle_key_event(0, Some('c'), false, 350);
    // "grmbc" is unique for 頡 -> auto-commits on 5th code!
    assert_eq!(out5.commit_text, "頡", "expected 5th-code unique auto-commit '頡', got '{}'", out5.commit_text);

    // 3. Verify Cangjie 3 and Cangjie 6 switching and lookup
    engine.set_schema(WubiSchema::Cangjie3);
    for ch in "hqi".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 400);
    }
    assert_eq!(engine.preedit(), "竹手戈");
    assert!(
        engine.all_candidates().iter().any(|c| c.text == "我"),
        "expected '我' in Cangjie 3 for 'hqi'"
    );
    engine.clear_composition();

    engine.set_schema(WubiSchema::Cangjie6);
    for ch in "a".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 500);
    }
    assert_eq!(engine.preedit(), "日");
    assert_eq!(engine.page_candidates().first().map(|c| c.text.as_str()), Some("日"));
    engine.clear_composition();

    // 4. Verify authentic differences across Cangjie 3, 5, and 6:
    // "面": Cangjie 3 = "mwyl" (一田卜中), Cangjie 5 = "mwsl" (一田尸中), Cangjie 6 = "mws" (一田尸)
    engine.set_schema(WubiSchema::Cangjie3);
    for ch in "mwyl".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 600);
    }
    assert!(
        engine.all_candidates().iter().any(|c| c.text == "面"),
        "expected '面' at 'mwyl' in Cangjie 3"
    );
    engine.clear_composition();

    engine.set_schema(WubiSchema::Cangjie5);
    for ch in "mwsl".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 700);
    }
    assert!(
        engine.all_candidates().iter().any(|c| c.text == "面"),
        "expected '面' at 'mwsl' in Cangjie 5"
    );
    engine.clear_composition();

    engine.set_schema(WubiSchema::Cangjie6);
    for ch in "mws".chars() {
        let _ = engine.handle_key_event(0, Some(ch), false, 800);
    }
    assert!(
        engine.all_candidates().iter().any(|c| c.text == "面"),
        "expected '面' at 'mws' in Cangjie 6"
    );
}
