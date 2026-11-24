//! Offline streaming replay & 1-shot probability-layer adaptation benchmark
//! for the pure char-level neural decoder + `PersonalContinuations`.

use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use juxu_core::config::{InputSchema, JuxuConfig};
use juxu_core::english::EnDictView;
use juxu_core::eval_support::{extract_json_str as extract_str, make_obs};
use juxu_core::format::AuxView;
use juxu_core::neural::NeuralContextTracker;
use juxu_core::pinyin::{decode_pinyin, PinyinDecoder};
use juxu_core::pinyin_dict::PinyinDictView;
use juxu_core::typo::KeyLayout;
use juxu_core::user_data::UserStore;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let log_path = args.first().map(PathBuf::from);
    let dicts_dir = PathBuf::from("Resources/Dicts");
    let dict = PinyinDictView::from_file(dicts_dir.join("pinyin.pydict")).expect("load pinyin.pydict");
    let english = EnDictView::open_mmap(dicts_dir.join("english.endict")).expect("load english.endict");
    let aux = AuxView::open_mmap(dicts_dir.join("wubi_aux.wbaux")).expect("load wubi_aux.wbaux");
    let nv = aux.neural_view().expect("neural view");
    let layout = KeyLayout::default();
    let mut config = JuxuConfig::default();
    config.schema = InputSchema::Pinyin;
    config.dynamic_word_freq = true;
    config.auto_phrase = false;

    // -------------------------------------------------------------------------
    // Part 1: 1-Shot Probability-Layer Personalization Stress Test (zero phrase dict!)
    // -------------------------------------------------------------------------
    println!("=========================================================================");
    println!("Part 1: 1-Shot Probability-Layer Learning (Zero Phrase Dict / Zero Rules)");
    println!("=========================================================================");
    let cases: &[(&str, &str, &str, Option<&str>)] = &[
        ("", "zhuowei", "卓威", Some("zw")),
        ("", "zhuoweiboli", "卓威玻璃", None),
        ("", "shabima", "傻逼吗", Some("sbm")),
        ("", "zuihaoshi", "最好是", Some("zhs")),
        ("", "hajimi", "哈基米", Some("hjm")),
        ("", "juxu", "句序", Some("jx")),
        ("", "guonihe", "过拟合", Some("gnh")),
        ("这个", "biaodi", "标的", None),
        ("", "heyiwei", "何意味", Some("hyw")),
    ];

    let mut user = UserStore::default();
    let mut pass_1shot = 0usize;
    let mut pass_abbr = 0usize;
    let mut total_abbr = 0usize;

    for &(ctx_str, keys, target, abbr_opt) in cases {
        let mut tracker = NeuralContextTracker::default();
        if ctx_str.is_empty() {
            tracker.init_with_model(&nv);
        } else {
            tracker.set_context_str(&nv, ctx_str);
        }
        let ctx_chars: Vec<char> = ctx_str.chars().collect();
        let obs = make_obs(keys, &layout);

        // Cold decode (before learning)
        let dec0 = PinyinDecoder {
            dict: &dict,
            english: &english,
            model: Some(aux.neural_view().unwrap()),
            config: &config,
            user: &user,
            context: &ctx_chars,
            layout: &layout,
        };
        let (c0, _) = decode_pinyin(&obs, keys, &dec0, &tracker.active);
        let before_top1 = c0.first().map(|c| c.candidate.text.as_str()).unwrap_or("").to_string();
        let before_rank = c0.iter().position(|c| c.candidate.text == target);

        // Simulate 1 explicit user commit (probability-layer ONLY: PersonalContinuations::observe)
        user.observe_continuation(
            &ctx_chars,
            target,
            true,
            Some(&before_top1),
            Some((aux.neural_view().unwrap(), tracker.active.clone())),
        );

        // Re-decode immediately after 1-shot learning
        let dec1 = PinyinDecoder {
            dict: &dict,
            english: &english,
            model: Some(aux.neural_view().unwrap()),
            config: &config,
            user: &user,
            context: &ctx_chars,
            layout: &layout,
        };
        let (c1, _) = decode_pinyin(&obs, keys, &dec1, &tracker.active);
        let after_top1 = c1.first().map(|c| c.candidate.text.as_str()).unwrap_or("");
        let after_rank = c1.iter().position(|c| c.candidate.text == target);
        if after_rank == Some(0) {
            pass_1shot += 1;
        }

        let mut abbr_str = String::new();
        if let Some(abbr) = abbr_opt {
            total_abbr += 1;
            let a_obs = make_obs(abbr, &layout);
            let (ca, _) = decode_pinyin(&a_obs, abbr, &dec1, &tracker.active);
            let ar = ca.iter().position(|c| c.candidate.text == target);
            let atop1 = ca.first().map(|c| c.candidate.text.as_str()).unwrap_or("");
            if ar.is_some_and(|r| r < 3) {
                pass_abbr += 1;
            }
            abbr_str = format!(" | abbr '{}': rank={:?} (#1='{}')", abbr, ar.map(|r| r + 1), atop1);
        }

        println!(
            "  {:<12} -> {:<4} | cold: rank={:<6?} (#1='{}') -> 1-shot: rank={:<6?} (#1='{}'){}",
            keys,
            target,
            before_rank.map(|r| r + 1),
            before_top1,
            after_rank.map(|r| r + 1),
            after_top1,
            abbr_str
        );
    }
    println!(
        "  Summary: Full-Pinyin 1-shot Top-1 = {}/{} ({:.1}%) | Abbr Top-3 = {}/{} ({:.1}%)",
        pass_1shot,
        cases.len(),
        100.0 * pass_1shot as f64 / cases.len() as f64,
        pass_abbr,
        total_abbr,
        100.0 * pass_abbr as f64 / total_abbr.max(1) as f64
    );

    // -------------------------------------------------------------------------
    // Part 2: Real-World iPhone eval_log.jsonl Streaming Replay
    // -------------------------------------------------------------------------
    if let Some(path) = log_path {
        println!("\n=========================================================================");
        println!("Part 2: Real-World Streaming Replay ({})", path.display());
        println!("=========================================================================");
        let file = File::open(&path).expect("open log");
        let mut live_user = UserStore::default();
        let cold_user = UserStore::default();

        let mut total_py = 0usize;
        let mut cold_top1 = 0usize;
        let mut cold_top3 = 0usize;
        let mut cold_top6 = 0usize;
        let mut live_top1 = 0usize;
        let mut live_top3 = 0usize;
        let mut live_top6 = 0usize;
        let mut misses: Vec<String> = Vec::new();

        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let schema = extract_str(&line, "schema");
            let event = extract_str(&line, "event");
            if schema != "pinyin" || (event != "commit" && event != "partial_commit") {
                continue;
            }
            let ctx = extract_str(&line, "ctx");
            let keys = extract_str(&line, "keys");
            let chosen = extract_str(&line, "chosen");
            if keys.is_empty() || chosen.is_empty() {
                continue;
            }
            // Skip pure ASCII passthroughs
            if !chosen.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c)) {
                continue;
            }

            let mut tracker = NeuralContextTracker::default();
            if ctx.is_empty() {
                tracker.init_with_model(&nv);
            } else {
                tracker.set_context_str(&nv, &ctx);
            }
            let ctx_chars: Vec<char> = ctx.chars().collect();
            let obs = make_obs(&keys, &layout);

            // Cold decode
            let dec_c = PinyinDecoder {
                dict: &dict,
                english: &english,
                model: Some(aux.neural_view().unwrap()),
                config: &config,
                user: &cold_user,
                context: &ctx_chars,
                layout: &layout,
            };
            let (cc, _) = decode_pinyin(&obs, &keys, &dec_c, &tracker.active);
            let cr = cc.iter().position(|c| c.candidate.text == chosen);
            if cr == Some(0) { cold_top1 += 1; }
            if cr.is_some_and(|r| r < 3) { cold_top3 += 1; }
            if cr.is_some_and(|r| r < 6) { cold_top6 += 1; }

            // Live online-adapted decode
            let dec_l = PinyinDecoder {
                dict: &dict,
                english: &english,
                model: Some(aux.neural_view().unwrap()),
                config: &config,
                user: &live_user,
                context: &ctx_chars,
                layout: &layout,
            };
            let (cl, _) = decode_pinyin(&obs, &keys, &dec_l, &tracker.active);
            let lr = cl.iter().position(|c| c.candidate.text == chosen);
            let ltop1 = cl.first().map(|c| c.candidate.text.as_str()).unwrap_or("").to_string();
            if lr == Some(0) { live_top1 += 1; }
            if lr.is_some_and(|r| r < 3) { live_top3 += 1; }
            if lr.is_some_and(|r| r < 6) { live_top6 += 1; }
            if lr != Some(0) {
                misses.push(format!(
                    "  ctx='{}' keys='{}' gold='{}' | live_rank={:?} (#1='{}') | cold_rank={:?}",
                    ctx,
                    keys,
                    chosen,
                    lr.map(|r| r + 1),
                    ltop1,
                    cr.map(|r| r + 1)
                ));
            }

            // Online observe after commit
            live_user.observe_continuation(
                &ctx_chars,
                &chosen,
                true,
                Some(&ltop1),
                Some((aux.neural_view().unwrap(), tracker.active.clone())),
            );
            total_py += 1;
        }

        println!(
            "  Total Pinyin CJK events: {}",
            total_py
        );
        println!(
            "  Pure-Neural Cold       : Top-1={:>2}/{} ({:5.1}%) | Top-3={:>2}/{} ({:5.1}%) | Top-6={:>2}/{} ({:5.1}%)",
            cold_top1, total_py, 100.0 * cold_top1 as f64 / total_py.max(1) as f64,
            cold_top3, total_py, 100.0 * cold_top3 as f64 / total_py.max(1) as f64,
            cold_top6, total_py, 100.0 * cold_top6 as f64 / total_py.max(1) as f64,
        );
        println!(
            "  Pure-Neural Live-Cache : Top-1={:>2}/{} ({:5.1}%) | Top-3={:>2}/{} ({:5.1}%) | Top-6={:>2}/{} ({:5.1}%)",
            live_top1, total_py, 100.0 * live_top1 as f64 / total_py.max(1) as f64,
            live_top3, total_py, 100.0 * live_top3 as f64 / total_py.max(1) as f64,
            live_top6, total_py, 100.0 * live_top6 as f64 / total_py.max(1) as f64,
        );
        if !misses.is_empty() {
            println!("\n  [Live-Cache Non-Top-1 Events ({})]", misses.len());
            for m in misses.iter().take(25) {
                println!("{}", m);
            }
        }
    }
}
