//! Zero-Hardcode Chinese-English Mixed Input Evaluation Suite (`eval_english_mix`).
//!
//! Evaluates:
//! 1. Full-Pinyin mixed CN-EN sentences (dictionary English, OOV English, uppercase anchors, canonical casing).
//! 2. Pure-Chinese ambiguity guardrails (ensuring valid Pinyin / initial abbreviations never false-trigger English).
//! 3. Fixed & Spaceless Wubi 98 CN-EN mixing and T9 English lookup.

use juxu_core::config::{InputSchema, JuxuConfig};
use juxu_core::engine::{JuxuEngine, KEY_SPACE};
use juxu_core::english::EnDictView;
use juxu_core::eval_support::{pct, percentile_ms};
use juxu_core::format::{AuxView, DictView};
use juxu_core::pinyin_dict::PinyinDictView;
use juxu_core::typo::KeyLayout;
use std::path::PathBuf;
use std::time::Instant;

/// `(context, keys, expected_text, must_be_top1)`
const PINYIN_MIX_CASES: &[(&str, &str, &str, bool)] = &[
    ("", "woyongrustxiele", "我用Rust写了", true),
    ("", "woyongworktreexie", "我用worktree写", true),
    ("", "xianzuomacos", "先做macOS", true),
    ("", "woyongJuxu'xie", "我用Juxu写", true),
    ("把这个分支合到", "main", "main", true),
    ("", "sglang", "sglang", true),
    ("根据我的句序日志", "reviewyixia", "review一下", false),
    ("", "zhegebugxiuhaole", "这个bug修好了", false),
    ("", "yongpythonxiejiaoben", "用Python写脚本", false),
    ("", "bushudaodockerli", "部署到Docker里", false),
];

/// `(context, keys, expected_top1_chinese)` — must NEVER rank ASCII/English at `#1`.
const CHINESE_GUARD_CASES: &[(&str, &str, Option<&str>)] = &[
    ("他大学学", "des", Some("的是")),
    ("", "wobud", Some("我不懂")),
    ("", "tas", Some("他是")),
    ("", "dans", Some("但是")),
    ("", "tadaxuexuedesjsj", None),
    ("他大学学", "desjsj", None),
    ("", "srf", Some("输入法")),
    ("", "womenshurufa", Some("我们输入法")),
];

fn main() {
    let dir = PathBuf::from("Resources/Dicts");
    let pdict = PinyinDictView::from_file(dir.join("pinyin.pydict")).expect("load pinyin.pydict");
    let endict = EnDictView::open_mmap(dir.join("english.endict")).expect("load english.endict");
    let wb98 = DictView::open_mmap(dir.join("wubi98.wbdict")).expect("load wubi98.wbdict");
    let aux = AuxView::open_mmap(dir.join("wubi_aux.wbaux")).expect("load wubi_aux.wbaux");

    let mut cfg = JuxuConfig::default();
    cfg.schema = InputSchema::Pinyin;
    let mut engine = JuxuEngine::new(cfg.clone());
    engine.load_pinyin_dict(pdict);
    engine.load_english_dict(endict);
    engine.load_dict(InputSchema::Wubi98, wb98);
    engine.set_aux(aux);

    println!("=========================================================================");
    println!("Juxu CN-EN Mixed Input Evaluation (Zero-Hardcode / Canonical Casing)");
    println!("=========================================================================");

    // 1. Full-Pinyin CN-EN mixing
    let mut mix_t1 = 0usize;
    let mut mix_t3 = 0usize;
    let mut lat_us = Vec::new();
    for &(ctx, keys, expected, must_t1) in PINYIN_MIX_CASES {
        engine.clear_composition();
        engine.clear_context();
        if !ctx.is_empty() {
            engine.set_recent_context(ctx);
        }
        let t0 = Instant::now();
        for (i, ch) in keys.chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 1000 + i as u64 * 50);
        }
        lat_us.push(t0.elapsed().as_micros());
        let cands = engine.all_candidates();
        let r = cands.iter().position(|c| c.text == expected);
        if r == Some(0) {
            mix_t1 += 1;
        }
        if r.is_some_and(|idx| idx < 3) {
            mix_t3 += 1;
        }
        let got = cands.first().map(|c| c.text.as_str()).unwrap_or("");
        if (must_t1 && r != Some(0)) || r.is_none_or(|idx| idx >= 3) {
            println!("  [MIX MISS] ctx='{ctx}' keys='{keys}' exp='{expected}' got='{got}' rank={r:?}");
        }
    }
    println!(
        "\n[1] Full-Pinyin CN-EN Mix ({} cases) | Top-1={}/{} ({:.1}%) | Top-3={}/{} ({:.1}%)",
        PINYIN_MIX_CASES.len(),
        mix_t1,
        PINYIN_MIX_CASES.len(),
        pct(mix_t1, PINYIN_MIX_CASES.len()),
        mix_t3,
        PINYIN_MIX_CASES.len(),
        pct(mix_t3, PINYIN_MIX_CASES.len()),
    );

    // 2. Pure-Chinese guardrails
    let mut guard_ok = 0usize;
    for &(ctx, keys, exp_opt) in CHINESE_GUARD_CASES {
        engine.clear_composition();
        engine.clear_context();
        if !ctx.is_empty() {
            engine.set_recent_context(ctx);
        }
        for (i, ch) in keys.chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 2000 + i as u64 * 50);
        }
        let top1 = engine
            .all_candidates()
            .first()
            .map(|c| c.text.as_str())
            .unwrap_or("");
        let no_ascii = !top1.bytes().any(|b| b.is_ascii_alphabetic());
        let exact_match = exp_opt.is_none_or(|exp| top1 == exp);
        if no_ascii && exact_match {
            guard_ok += 1;
        } else {
            println!("  [GUARD FAIL] ctx='{ctx}' keys='{keys}' exp={exp_opt:?} got='{top1}'");
        }
    }
    println!(
        "[2] Pure-Chinese Guardrails ({} cases) | Pass={}/{} ({:.1}%)",
        CHINESE_GUARD_CASES.len(),
        guard_ok,
        CHINESE_GUARD_CASES.len(),
        pct(guard_ok, CHINESE_GUARD_CASES.len()),
    );

    // 3. Wubi 98 Spaceless + Fixed + T9 English
    let mut shape_ok = 0usize;
    let shape_total = 4usize;

    let mut wcfg = cfg.clone();
    wcfg.schema = InputSchema::Wubi98;
    wcfg.spaceless = true;
    engine.set_config(wcfg);

    engine.clear_composition();
    engine.set_recent_context("在感觉怎么样装到");
    for (i, ch) in "iphonehrhrh".chars().enumerate() {
        let _ = engine.handle_key_event(0, Some(ch), false, 3000 + i as u64 * 30);
    }
    if engine.all_candidates().first().is_some_and(|c| c.text == "iPhone上看看") {
        shape_ok += 1;
    }

    engine.clear_composition();
    engine.set_recent_context("云端");
    let mut diet_out = String::new();
    for (i, ch) in "ivqdietkhlf".chars().enumerate() {
        let out = engine.handle_key_event(0, Some(ch), false, 4000 + i as u64 * 30);
        diet_out.push_str(&out.commit_text);
    }
    diet_out.push_str(&engine.handle_key_event(KEY_SPACE, Some(' '), false, 4500).commit_text);
    if diet_out == "当然不用跳转" {
        shape_ok += 1;
    }

    let mut fcfg = cfg.clone();
    fcfg.schema = InputSchema::Wubi98;
    fcfg.spaceless = false;
    engine.set_config(fcfg);
    engine.clear_composition();
    engine.clear_context();
    let mut had_bell = false;
    for (i, ch) in "python".chars().enumerate() {
        let out = engine.handle_key_event(0, Some(ch), false, 5000 + i as u64 * 30);
        had_bell |= out.bell;
    }
    if !had_bell && engine.all_candidates().first().is_some_and(|c| c.text == "Python") {
        shape_ok += 1;
    }

    let mut t9cfg = cfg;
    t9cfg.schema = InputSchema::Pinyin;
    t9cfg.pinyin_t9_layout = true;
    engine.set_config(t9cfg);
    engine.clear_composition();
    engine.clear_context();
    let layout = KeyLayout::default();
    for (i, ch) in "467".bytes().enumerate() {
        let (tx, ty) = layout.t9_center_of(ch);
        let _ = engine.push_touch_t9(ch, tx, ty, true, 6000 + i as u64 * 30);
    }
    if engine.all_candidates().iter().take(5).any(|c| c.text == "iOS") {
        shape_ok += 1;
    }

    lat_us.sort_unstable();
    println!(
        "[3] Wubi98 & T9 CN-EN Checks          | Pass={}/{} ({:.1}%)",
        shape_ok,
        shape_total,
        pct(shape_ok, shape_total)
    );
    println!(
        "[4] Mixed Sentence Cold Latency       | p50={:.2} ms | p95={:.2} ms",
        percentile_ms(&lat_us, 0.50),
        percentile_ms(&lat_us, 0.95)
    );
}
