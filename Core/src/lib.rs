pub mod code;
pub mod config;
pub mod engine;
pub mod ffi;
pub mod format;
pub mod mmap_buf;
pub mod neural;
mod personalization;
pub mod pinyin;
pub mod pinyin_dict;
mod spaceless;
pub mod typo;
pub mod user_data;

#[cfg(test)]
mod tests {
    use crate::code::{compose_phrase_code, expand_wildcards, PackedCode, PackedPinyin, Stem2};
    use crate::config::{FourthEmptyBehavior, WubiConfig, WubiSchema};
    use crate::engine::{CandidateBadge, WubiEngine, KEY_SPACE};
    use crate::format::{
        pack_trigram, serialize_wbaux, serialize_wbdict, AuxBuildInput, AuxView, CharMeta,
        DictBuildInput, DictView, ENTRY_FLAG_SINGLE_CHAR, WBAUX_MAGIC,
    };

    fn build_sample_dict_86() -> DictView {
        let entries = vec![
            (
                PackedCode::from_ascii("q").unwrap(),
                "我".to_string(),
                2_200_000_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("trnt").unwrap(),
                "我".to_string(),
                500_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("w").unwrap(),
                "人".to_string(),
                2_000_000_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("wu").unwrap(),
                "们".to_string(),
                900_000_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("wun").unwrap(),
                "们".to_string(),
                500_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("trwu").unwrap(),
                "我们".to_string(),
                1_800_000_000,
                0,
            ),
            (
                PackedCode::from_ascii("l").unwrap(),
                "国".to_string(),
                800_000_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("lgyi").unwrap(),
                "国".to_string(),
                500_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("lgyi").unwrap(),
                "囗".to_string(),
                10_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("lwgl").unwrap(),
                "输入".to_string(),
                650_000_000,
                0,
            ),
            (
                PackedCode::from_ascii("ifcy").unwrap(),
                "法".to_string(),
                750_000_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("ufcy").unwrap(),
                "䇂".to_string(),
                8_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("swfh").unwrap(),
                "棱".to_string(),
                30_000_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("qujq").unwrap(),
                "镜".to_string(),
                60_000_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
        ];
        let char_metas = vec![
            CharMeta {
                codepoint: '我' as u32,
                stem2: Stem2::from_bytes(b"tr").unwrap(),
                flags: 0,
                shortest_code: PackedCode::from_ascii("q").unwrap(),
                primary_short_code: PackedCode::EMPTY,
                full_code: PackedCode::from_ascii("trnt").unwrap(),
            },
            CharMeta {
                codepoint: '们' as u32,
                stem2: Stem2::from_bytes(b"wu").unwrap(),
                flags: 0,
                shortest_code: PackedCode::from_ascii("wu").unwrap(),
                primary_short_code: PackedCode::EMPTY,
                full_code: PackedCode::from_ascii("wun").unwrap(),
            },
            CharMeta {
                codepoint: '国' as u32,
                stem2: Stem2::from_bytes(b"lg").unwrap(),
                flags: 0,
                shortest_code: PackedCode::from_ascii("l").unwrap(),
                primary_short_code: PackedCode::EMPTY,
                full_code: PackedCode::from_ascii("lgyi").unwrap(),
            },
            CharMeta {
                codepoint: '法' as u32,
                stem2: Stem2::from_bytes(b"if").unwrap(),
                flags: 0,
                shortest_code: PackedCode::from_ascii("ifcy").unwrap(),
                primary_short_code: PackedCode::EMPTY,
                full_code: PackedCode::from_ascii("ifcy").unwrap(),
            },
            CharMeta {
                codepoint: '棱' as u32,
                stem2: Stem2::from_bytes(b"sw").unwrap(),
                flags: 0,
                shortest_code: PackedCode::from_ascii("swfh").unwrap(),
                primary_short_code: PackedCode::EMPTY,
                full_code: PackedCode::from_ascii("swfh").unwrap(),
            },
            CharMeta {
                codepoint: '镜' as u32,
                stem2: Stem2::from_bytes(b"qu").unwrap(),
                flags: 0,
                shortest_code: PackedCode::from_ascii("qujq").unwrap(),
                primary_short_code: PackedCode::EMPTY,
                full_code: PackedCode::from_ascii("qujq").unwrap(),
            },
        ];
        let bytes = serialize_wbdict(DictBuildInput {
            schema_id: 86,
            entries,
            char_metas,
        });
        DictView::from_bytes(bytes).unwrap()
    }

    fn build_sample_dict_98() -> DictView {
        // In 98 Wubi, 镜 has different radical code `q...` -> say `qujq` vs `棱` `swfh` -> let's give 镜 `qpjq` in 98 to test cross-schema re-encoding!
        let entries = vec![
            (
                PackedCode::from_ascii("swfh").unwrap(),
                "棱".to_string(),
                30_000_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("qpjq").unwrap(),
                "镜".to_string(),
                60_000_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
        ];
        let char_metas = vec![
            CharMeta {
                codepoint: '棱' as u32,
                stem2: Stem2::from_bytes(b"sw").unwrap(),
                flags: 0,
                shortest_code: PackedCode::from_ascii("swfh").unwrap(),
                primary_short_code: PackedCode::EMPTY,
                full_code: PackedCode::from_ascii("swfh").unwrap(),
            },
            CharMeta {
                codepoint: '镜' as u32,
                stem2: Stem2::from_bytes(b"qp").unwrap(),
                flags: 0,
                shortest_code: PackedCode::from_ascii("qpjq").unwrap(),
                primary_short_code: PackedCode::EMPTY,
                full_code: PackedCode::from_ascii("qpjq").unwrap(),
            },
        ];
        DictView::from_bytes(serialize_wbdict(DictBuildInput {
            schema_id: 98,
            entries,
            char_metas,
        }))
        .unwrap()
    }

    fn build_sample_aux() -> AuxView {
        let pinyin_items = vec![
            (
                PackedPinyin::from_bytes(b"guo").unwrap(),
                "guó".to_string(),
                '国' as u32,
                800_000,
            ),
            (
                PackedPinyin::from_bytes(b"wo").unwrap(),
                "wǒ".to_string(),
                '我' as u32,
                900_000,
            ),
        ];
        // Strong corpus bigram: '入' -> '法' (`boost = 3.5` nats => `3.5 * 256 = 896`)
        let pair_key = (('入' as u64) << 32) | ('法' as u64);
        let bigrams = vec![(pair_key, 896u16)];
        AuxView::from_bytes(serialize_wbaux(AuxBuildInput {
            pinyin_items,
            bigrams,
            ..Default::default()
        }))
        .unwrap()
    }

    #[test]
    fn packed_code_order_and_prefix() {
        let a = PackedCode::from_ascii("a").unwrap();
        let aa = PackedCode::from_ascii("aa").unwrap();
        let aazz = PackedCode::from_ascii("aazz").unwrap();
        let ab = PackedCode::from_ascii("ab").unwrap();
        assert!(a < aa && aa < aazz && aazz < ab);
        let (lo, hi) = aa.prefix_range();
        assert!(aazz >= lo && aazz <= hi);
        assert!(ab > hi);

        let phrase = compose_phrase_code(&[
            Stem2::from_bytes(b"tr").unwrap(),
            Stem2::from_bytes(b"wu").unwrap(),
        ])
        .unwrap();
        assert_eq!(phrase.as_string(), "trwu");
        assert_eq!(expand_wildcards(b"trzt").len(), 25);
    }

    #[test]
    fn four_code_auto_commit_and_fifth_push() {
        let mut engine = WubiEngine::new(WubiConfig::default());
        engine.load_dict(WubiSchema::Wubi86, build_sample_dict_86());

        // `trwu` ("我们") is unique at 4 codes -> auto-commits immediately on 'u'!
        for ch in ['t', 'r', 'w'] {
            let out = engine.handle_key_event(0, Some(ch), false, 100);
            assert!(out.commit_text.is_empty());
        }
        let out4 = engine.handle_key_event(0, Some('u'), false, 110);
        assert_eq!(out4.commit_text, "我们");
        assert!(!engine.is_composing());

        // `lgyi` has 2 candidates ("国", "囗") -> stays open on 4th code, then 5th key `'q'` pushes "国"!
        for ch in ['l', 'g', 'y', 'i'] {
            let out = engine.handle_key_event(0, Some(ch), false, 200);
            assert!(out.commit_text.is_empty());
        }
        assert_eq!(engine.page_candidates()[0].text, "国");
        assert_eq!(engine.page_candidates()[0].comment, "⚡l");

        // Strict shortcode ("出简不出全"): when enabled, "国" (which is #1 at `l`) demotes behind "囗" at `lgyi`!
        let mut cfg = engine.config.clone();
        cfg.strict_shortcode = true;
        engine.set_config(cfg);
        assert_eq!(engine.page_candidates()[0].text, "囗");
        cfg = engine.config.clone();
        cfg.strict_shortcode = false;
        engine.set_config(cfg);

        // 5th key `'q'` pushes "国" and begins composing `"q"` ("我")
        let out5 = engine.handle_key_event(0, Some('q'), false, 250);
        assert_eq!(out5.commit_text, "国");
        assert_eq!(out5.preedit, "q");
        assert_eq!(engine.page_candidates()[0].text, "我");
    }

    #[test]
    fn auto_phrase_and_cross_schema_switching() {
        let mut cfg = WubiConfig::default();
        cfg.auto_phrase = true;
        cfg.auto_phrase_threshold = 2;
        let mut engine = WubiEngine::new(cfg);
        engine.load_dict(WubiSchema::Wubi86, build_sample_dict_86());
        engine.load_dict(WubiSchema::Wubi98, build_sample_dict_98());

        // Type "棱" (`swfh`) + "镜" (`qujq`) twice consecutively
        for round in 0..2u64 {
            let base_t = 1000 + round * 500;
            for (i, ch) in ['s', 'w', 'f', 'h'].iter().enumerate() {
                let _ = engine.handle_key_event(0, Some(*ch), false, base_t + i as u64 * 20);
            }
            let mut last_out = Default::default();
            for (i, ch) in ['q', 'u', 'j', 'q'].iter().enumerate() {
                last_out =
                    engine.handle_key_event(0, Some(*ch), false, base_t + 100 + i as u64 * 20);
            }
            if round == 1 {
                assert!(
                    last_out.notice.contains("棱镜"),
                    "expected auto-phrase notice, got: {}",
                    last_out.notice
                );
            }
        }

        // Now in 86 Wubi, `swqu` immediately auto-commits "棱镜"!
        for ch in ['s', 'w', 'q'] {
            let _ = engine.handle_key_event(0, Some(ch), false, 3000);
        }
        let out86 = engine.handle_key_event(0, Some('u'), false, 3050);
        assert_eq!(out86.commit_text, "棱镜");

        // Switch to 98 Wubi! "棱镜" automatically re-indexes to 98 code `swqp`!
        engine.set_schema(WubiSchema::Wubi98);
        for ch in ['s', 'w', 'q'] {
            let _ = engine.handle_key_event(0, Some(ch), false, 4000);
        }
        let out98 = engine.handle_key_event(0, Some('p'), false, 4050);
        assert_eq!(out98.commit_text, "棱镜");
    }

    #[test]
    fn z_pinyin_reverse_lookup_and_wildcard() {
        let mut engine = WubiEngine::new(WubiConfig::default());
        engine.load_dict(WubiSchema::Wubi86, build_sample_dict_86());
        engine.set_aux(build_sample_aux());

        for ch in ['z', 'g', 'u', 'o'] {
            let _ = engine.handle_key_event(0, Some(ch), false, 100);
        }
        let cands = engine.page_candidates();
        assert!(!cands.is_empty());
        assert_eq!(cands[0].text, "国");
        assert_eq!(cands[0].badge, CandidateBadge::PinyinLookup);
        assert!(cands[0].comment.contains("lgyi"));
        engine.clear_composition();

        // Wildcard `trzt` finds "我" (`trnt`)
        for ch in ['t', 'r', 'z', 't'] {
            let _ = engine.handle_key_event(0, Some(ch), false, 200);
        }
        assert_eq!(engine.page_candidates()[0].text, "我");
        assert_eq!(engine.page_candidates()[0].comment, "trnt");
    }

    #[test]
    fn touch_boundary_typo_rescue_bigram_and_adaptive_hitbox() {
        let mut engine = WubiEngine::new(WubiConfig::default());
        engine.load_dict(WubiSchema::Wubi86, build_sample_dict_86());
        engine.set_aux(build_sample_aux());

        // 1. Empty-code rescue on touchscreen: user aims for `trwu` ("我们"),
        //    first tap slips right of `t` (4.0, 0.0) onto `y` border (4.55, 0.02) -> `yrwu` (empty!)
        let _ = engine.push_touch_letter(b'y', 4.55, 0.02, true, 1000);
        let _ = engine.push_touch_letter(b'r', 3.00, 0.00, true, 1080);
        let _ = engine.push_touch_letter(b'w', 1.00, 0.00, true, 1160);
        let out = engine.push_touch_letter(b'u', 6.00, 0.00, true, 1240);
        // Because `suppress_auto_commit_on_correction = true` by default, it rescues to #1 without blind auto-commit!
        assert!(out.commit_text.is_empty());
        assert_eq!(engine.page_candidates()[0].text, "我们");
        assert_eq!(engine.page_candidates()[0].badge, CandidateBadge::TypoCorrected);
        assert!(engine.page_candidates()[0].confidence > 0.80);

        // User confirms candidate #1 with Space -> triggers online touch-region calibration on `'t'`!
        let commit_out = engine.handle_key_event(KEY_SPACE, Some(' '), false, 1300);
        assert_eq!(commit_out.commit_text, "我们");
        let (dx_t, _) = engine.user.touch_profile.offset_for(b't');
        assert!(dx_t > 0.03, "expected positive rightward shift on 't', got {}", dx_t);

        // 2. Contextual Bigram Disambiguation on boundary touch:
        //    User first commits "输入" (`lwgl`), then wants "法" (`ifcy`).
        //    `u` is at (6.0, 0.0), `i` is at (7.0, 0.0). User taps border at (6.46, 0.02) (slightly closer to `u`!),
        //    where `ufcy` IS a real rare char `"䇂"`, while `ifcy` is `"法"` boosted by Bigram("入" -> "法")!
        for ch in ['l', 'w', 'g', 'l'] {
            let _ = engine.handle_key_event(0, Some(ch), false, 2000);
        }
        let _ = engine.push_touch_letter(b'u', 6.46, 0.02, true, 2200);
        let _ = engine.push_touch_letter(b'f', 3.50, 1.00, true, 2260);
        let _ = engine.push_touch_letter(b'c', 3.50, 2.00, true, 2320);
        let _ = engine.push_touch_letter(b'y', 5.00, 0.00, true, 2380);
        assert_eq!(
            engine.page_candidates()[0].text,
            "法",
            "expected contextual bigram + boundary snap to promote 法 over rare 䇂"
        );

        // 3. Repeated systematic rightward thumb drift on `'t'` shifts its Voronoi boundary!
        engine.clear_composition();
        for i in 0..8 {
            engine.user.touch_profile.observe(b't', 0.25, 0.0);
            let _ = i;
        }
        // Now tapping at `x = 4.56` (past geometric midpoint 4.50 between T=4.0 and Y=5.0!) snaps directly to `'t'`!
        let snapped = engine
            .key_layout
            .nearest_key(4.56, 0.0, &engine.user.touch_profile, true);
        assert_eq!(snapped, b't');

        // 4. Fourth-code truly empty block test (`zzzz`-free empty code `pppp`)
        engine.clear_composition();
        assert_eq!(engine.config.fourth_empty_behavior, FourthEmptyBehavior::Block);
        for ch in ['p', 'p', 'p'] {
            let _ = engine.handle_key_event(0, Some(ch), false, 3000);
        }
        let empty4 = engine.handle_key_event(0, Some('p'), false, 3050);
        assert!(empty4.bell);
        assert_eq!(empty4.preedit, "ppp");
    }

    #[test]
    fn wbaux_v2_unigram_trigram_and_context_reset() {
        let tri_entry = pack_trigram('输', '入', '法', (3.4 * 256.0) as u16).unwrap();
        let bg_key = (('入' as u64) << 32) | ('法' as u64);
        let aux = AuxView::from_bytes(serialize_wbaux(AuxBuildInput {
            pinyin_items: vec![],
            unigrams: vec![
                ('输' as u32, (6.5 * 256.0) as u16),
                ('入' as u32, (7.0 * 256.0) as u16),
                ('法' as u32, (8.2 * 256.0) as u16),
                ('䇂' as u32, (1.0 * 256.0) as u16),
            ],
            bigrams: vec![(bg_key, (2.2 * 256.0) as u16)],
            trigrams: vec![tri_entry],
            ..Default::default()
        }))
        .unwrap();

        assert!(aux.has_unigrams());
        assert_eq!(aux.stats(), (0, 4, 1, 1));
        assert!((aux.unigram_prior('法').unwrap() - 8.2).abs() < 0.02);
        assert!((aux.bigram_boost('入', '法') - 2.2).abs() < 0.02);
        assert!((aux.trigram_boost('输', '入', '法') - 3.4).abs() < 0.02);
        assert_eq!(aux.trigram_boost('我', '入', '法'), 0.0);

        let mut engine = WubiEngine::new(WubiConfig::default());
        engine.load_dict(WubiSchema::Wubi86, build_sample_dict_86());
        engine.set_aux(aux);

        // Commit "输入" (`lwgl`), then slip `u`->`i` border (6.46, 0.02) on `ifcy` ("法") vs `ufcy` ("䇂")
        for ch in ['l', 'w', 'g', 'l'] {
            let _ = engine.handle_key_event(0, Some(ch), false, 100);
        }
        let _ = engine.push_touch_letter(b'u', 6.46, 0.02, true, 200);
        let _ = engine.push_touch_letter(b'f', 3.50, 1.00, true, 260);
        let _ = engine.push_touch_letter(b'c', 3.50, 2.00, true, 320);
        let _ = engine.push_touch_letter(b'y', 5.00, 0.00, true, 380);
        assert_eq!(engine.page_candidates()[0].text, "法");
        let conf_with_ctx = engine.page_candidates()[0].confidence;

        // Committing punctuation (',') resets rolling context so prior sentence does not leak
        engine.clear_composition();
        let punc_out = engine.handle_key_event(0, Some(','), false, 450);
        assert_eq!(punc_out.commit_text, "，");
        let _ = engine.push_touch_letter(b'u', 6.46, 0.02, true, 500);
        let _ = engine.push_touch_letter(b'f', 3.50, 1.00, true, 560);
        let _ = engine.push_touch_letter(b'c', 3.50, 2.00, true, 620);
        let _ = engine.push_touch_letter(b'y', 5.00, 0.00, true, 680);
        let conf_after_punc = engine
            .page_candidates()
            .iter()
            .find(|c| c.text == "法")
            .map(|c| c.confidence)
            .unwrap_or(0.0);
        assert!(
            conf_with_ctx > conf_after_punc,
            "expected higher confidence with '输入' trigram context ({}) than after punctuation reset ({})",
            conf_with_ctx,
            conf_after_punc
        );
    }

    #[test]
    fn wbaux_v1_backward_compat() {
        // Synthesize a minimal valid legacy WBAUX001 blob (48-byte header + 1 bigram entry)
        let mut blob = Vec::new();
        blob.extend_from_slice(WBAUX_MAGIC);
        blob.extend_from_slice(&0u32.to_le_bytes()); // py_bucket_cnt (8)
        blob.extend_from_slice(&0u32.to_le_bytes()); // py_item_cnt (12)
        blob.extend_from_slice(&1u32.to_le_bytes()); // bigram_cnt (16)
        blob.extend_from_slice(&48u32.to_le_bytes()); // py_buckets_off (20)
        blob.extend_from_slice(&48u32.to_le_bytes()); // py_items_off (24)
        blob.extend_from_slice(&48u32.to_le_bytes()); // bg_off (28)
        blob.extend_from_slice(&58u32.to_le_bytes()); // pool_off (32)
        blob.extend_from_slice(&0u32.to_le_bytes()); // pool_len (36)
        blob.extend_from_slice(&[0u8; 8]); // v1 header padding (40..48)
        assert_eq!(blob.len(), 48);
        let pair_key = (('入' as u64) << 32) | ('法' as u64);
        blob.extend_from_slice(&pair_key.to_le_bytes());
        blob.extend_from_slice(&(768u16).to_le_bytes()); // 3.0 nats

        let aux = AuxView::from_bytes(blob).unwrap();
        assert!(!aux.has_unigrams());
        assert_eq!(aux.stats(), (0, 0, 1, 0));
        assert!((aux.bigram_boost('入', '法') - 3.0).abs() < 0.01);
        assert_eq!(aux.trigram_boost('输', '入', '法'), 0.0);
    }

    #[test]
    fn config_auto_space_toml_roundtrip() {
        let mut cfg = WubiConfig::default();
        assert!(cfg.auto_space_zh_en);
        assert!(cfg.auto_space_digits);
        assert!(cfg.typo.missed_space_correction);
        cfg.auto_space_zh_en = true;
        cfg.auto_space_digits = false;
        cfg.typo.missed_space_correction = false;
        let toml = cfg.to_toml_string();
        let parsed = WubiConfig::from_toml_str(&toml);
        assert!(parsed.auto_space_zh_en);
        assert!(!parsed.auto_space_digits);
        assert!(!parsed.typo.missed_space_correction);
    }

    #[test]
    fn missed_space_empty_split_and_toggle() {
        let mut engine = WubiEngine::new(WubiConfig::default());
        engine.load_dict(WubiSchema::Wubi86, build_sample_dict_86());
        engine.set_aux(build_sample_aux());

        // Type 3-code `wun` ("们") without Space, followed by `q` ("我") -> `wunq` is empty!
        // Missed-space decoder smart-pushes "们" and keeps "q" composing.
        for ch in ['w', 'u', 'n'] {
            let out = engine.handle_key_event(0, Some(ch), false, 100);
            assert!(out.commit_text.is_empty());
        }
        let out4 = engine.handle_key_event(0, Some('q'), false, 160);
        assert!(!out4.bell, "missed-space split should avoid empty-code bell");
        assert_eq!(out4.commit_text, "们");
        assert_eq!(out4.preedit, "q");
        let out_sp = engine.handle_key_event(KEY_SPACE, Some(' '), false, 220);
        assert_eq!(out_sp.commit_text, "我");

        // Disabling `missed_space_correction` restores blocking behavior on `wunq`
        let mut cfg = engine.config.clone();
        cfg.typo.missed_space_correction = false;
        engine.set_config(cfg);
        engine.clear_composition();
        for ch in ['w', 'u', 'n'] {
            let _ = engine.handle_key_event(0, Some(ch), false, 300);
        }
        let blocked = engine.handle_key_event(0, Some('q'), false, 360);
        assert!(blocked.bell);
        assert_eq!(blocked.preedit, "wun");
    }

    #[test]
    fn neural_wblm_parity_if_present() {
        let Ok(blob) = std::fs::read("/tmp/test_micro_rwkv7.wblm") else {
            return;
        };
        let offs = crate::neural::NeuralOffsets::parse(&blob).unwrap();
        let nv = crate::neural::NeuralView { blob: &blob, offs: &offs };
        let mut tracker = crate::neural::NeuralContextTracker::default();
        tracker.set_context_str(&nv, "我输");
        let sc = nv.score_next_char(&tracker.active, '入');
        println!("RUST_SCORE_RU={:.6}", sc);
    }

    #[test]
    fn v2_neural_cross_clause_punctuation_context() {
        let aux_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../Resources/Dicts/wubi_aux.wbaux");
        let Ok(aux) = AuxView::open_mmap(&aux_path) else {
            return;
        };
        let nv = aux.neural_view().expect("wubi_aux.wbaux should embed WBLM0001");
        let s = nv.offs.summary;
        assert_eq!((s.vocab_size, s.rank, s.d_model, s.n_heads, s.n_layers, s.ffn_dim), (8192, 160, 256, 8, 6, 768));
        assert!(nv.offs.has_punct_tokens, "V2 model must expose comma/period tokens");
        assert_eq!(nv.punct_to_token('，'), Some(crate::neural::TOK_COMMA));
        assert_eq!(nv.punct_to_token('。'), Some(crate::neural::TOK_PERIOD));

        // Parity check against V2 export parity.json ("今天天气真" -> 好/热)
        let mut tracker = crate::neural::NeuralContextTracker::default();
        tracker.set_context_str(&nv, "今天天气真");
        let sc_hao = nv.score_next_char(&tracker.active, '好');
        let sc_re = nv.score_next_char(&tracker.active, '热');
        assert!((sc_hao - (-0.255945)).abs() < 1e-3, "unexpected score_hao={sc_hao}");
        assert!(sc_hao > sc_re);

        // Cross-clause state must survive comma & period, and differ from a reset clause
        let mut t_cross = crate::neural::NeuralContextTracker::default();
        t_cross.set_context_str(&nv, "今天下雨了，我");
        let mut t_short = crate::neural::NeuralContextTracker::default();
        t_short.set_context_str(&nv, "我");
        let sc_dai_cross = nv.score_next_char(&t_cross.active, '带');
        let sc_dai_short = nv.score_next_char(&t_short.active, '带');
        assert!(
            sc_dai_cross > sc_dai_short + 0.5,
            "cross-clause comma context should boost '带' after '今天下雨了，我': cross={sc_dai_cross}, short={sc_dai_short}"
        );

        // Incremental commit of punctuation preserves state identically to set_context_str
        let mut t_inc = crate::neural::NeuralContextTracker::default();
        t_inc.commit_cjk_str(&nv, "今天下雨了");
        t_inc.commit_cjk_str(&nv, "，");
        t_inc.commit_cjk_str(&nv, "我");
        let sc_dai_inc = nv.score_next_char(&t_inc.active, '带');
        assert!((sc_dai_inc - sc_dai_cross).abs() < 1e-5);

        // Non-mapped boundary (newline) resets to BOS
        let mut t_nl = crate::neural::NeuralContextTracker::default();
        t_nl.set_context_str(&nv, "今天下雨了\n我");
        let sc_dai_nl = nv.score_next_char(&t_nl.active, '带');
        assert!((sc_dai_nl - sc_dai_short).abs() < 1e-5);
    }

    #[test]
    fn xiaohe_z_code_and_touch_typo_rescue() {
        let entries = vec![
            (
                PackedCode::from_ascii("z").unwrap(),
                "在".to_string(),
                3_000_000_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("zn").unwrap(),
                "真".to_string(),
                2_000_000_000,
                ENTRY_FLAG_SINGLE_CHAR,
            ),
            (
                PackedCode::from_ascii("uuip").unwrap(),
                "输入".to_string(),
                500_000_000,
                0,
            ),
        ];
        let dict = DictView::from_bytes(serialize_wbdict(DictBuildInput {
            schema_id: 10,
            entries,
            char_metas: vec![],
        }))
        .unwrap();

        let mut cfg = WubiConfig::default();
        cfg.schema = WubiSchema::Xiaohe;
        let mut engine = WubiEngine::new(cfg);
        engine.load_dict(WubiSchema::Xiaohe, dict);

        // In Xiaohe, 'z' is a normal letter key (not Wubi omnipotent mode)!
        let out_z = engine.handle_key_event(0, Some('z'), false, 100);
        assert_eq!(out_z.preedit, "z");
        assert_eq!(engine.page_candidates()[0].text, "在");
        let out_n = engine.handle_key_event(0, Some('n'), false, 150);
        assert_eq!(out_n.preedit, "zn");
        assert_eq!(engine.page_candidates()[0].text, "真");

        // Touch slip on 'z'->'x' for "zn" -> rescues "真" because allow_z is enabled for Xiaohe!
        engine.clear_composition();
        let _ = engine.push_touch_letter(b'x', 1.95, 2.0, true, 200);
        let _ = engine.push_touch_letter(b'n', 6.5, 2.0, true, 250);
        assert_eq!(engine.page_candidates()[0].text, "真");
    }

    #[test]
    fn full_pinyin_sentence_abbr_apostrophe_and_typo() {
        use crate::pinyin_dict::{serialize_pydict, PinyinDictBuildInput, PinyinDictView};
        let py_bytes = serialize_pydict(PinyinDictBuildInput {
            chars: vec![
                ("wo".into(), "我".into(), 10000, 0),
                ("men".into(), "们".into(), 9000, 0),
                ("shu".into(), "输".into(), 8000, 0),
                ("ru".into(), "入".into(), 8500, 0),
                ("fa".into(), "法".into(), 9500, 0),
                ("xi".into(), "西".into(), 8000, 0),
                ("an".into(), "安".into(), 8200, 0),
                ("xian".into(), "先".into(), 9000, 0),
            ],
            phrases: vec![
                (vec!["wo".into(), "men".into()], "我们".into(), 50000, 0),
                (vec!["shu".into(), "ru".into(), "fa".into()], "输入法".into(), 45000, 0),
                (vec!["xi".into(), "an".into()], "西安".into(), 30000, 0),
            ],
        });
        let pdict = PinyinDictView::from_bytes(py_bytes).unwrap();
        let mut cfg = WubiConfig::default();
        cfg.schema = WubiSchema::Pinyin;
        let mut engine = WubiEngine::new(cfg);
        engine.load_pinyin_dict(pdict);

        // 1. Full sentence: "womenshurufa" -> "我们输入法"
        for ch in "womenshurufa".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 100);
        }
        assert_eq!(engine.page_candidates()[0].text, "我们输入法");
        engine.clear_composition();

        // 2. Initial abbreviation: "srf" -> "输入法"
        for ch in "srf".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 200);
        }
        assert_eq!(engine.page_candidates()[0].text, "输入法");
        engine.clear_composition();

        // 3. Apostrophe separator: "xi'an" -> "西安" (not "先")
        for ch in "xi'an".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 300);
        }
        assert_eq!(engine.page_candidates()[0].text, "西安");
        engine.clear_composition();

        // 4. Touch typo rescue: "wpmen" (o->p neighbor) -> "我们"
        let _ = engine.push_touch_letter(b'w', 1.0, 0.0, true, 400);
        let _ = engine.push_touch_letter(b'p', 8.55, 0.0, true, 450); // near o/p border
        for ch in "men".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 500);
        }
        assert_eq!(engine.page_candidates()[0].text, "我们");
    }

    #[test]
    fn xiaohe_shuangpin_sentence_zero_initial_and_typo() {
        use crate::pinyin_dict::{serialize_pydict, PinyinDictBuildInput, PinyinDictView};
        let py_bytes = serialize_pydict(PinyinDictBuildInput {
            chars: vec![
                ("wo".into(), "我".into(), 10000, 0),
                ("men".into(), "们".into(), 9000, 0),
                ("shu".into(), "输".into(), 8000, 0),
                ("ru".into(), "入".into(), 8500, 0),
                ("fa".into(), "法".into(), 9500, 0),
                ("xi".into(), "西".into(), 8000, 0),
                ("an".into(), "安".into(), 8200, 0),
                ("xian".into(), "先".into(), 9000, 0),
                ("zhong".into(), "中".into(), 9800, 0),
                ("guo".into(), "国".into(), 9700, 0),
            ],
            phrases: vec![
                (vec!["wo".into(), "men".into()], "我们".into(), 50000, 0),
                (vec!["shu".into(), "ru".into(), "fa".into()], "输入法".into(), 45000, 0),
                (vec!["xi".into(), "an".into()], "西安".into(), 30000, 0),
                (vec!["zhong".into(), "guo".into()], "中国".into(), 48000, 0),
            ],
        });
        let pdict = PinyinDictView::from_bytes(py_bytes).unwrap();
        let mut cfg = WubiConfig::default();
        cfg.schema = WubiSchema::XiaoheShuangpin;
        let mut engine = WubiEngine::new(cfg);
        engine.load_pinyin_dict(pdict);

        // 1. Full Shuangpin sentence: wo + mf(men) + uu(shu) + ru + fa -> "我们输入法"
        for ch in "womfuurufa".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 100);
        }
        assert_eq!(engine.page_candidates()[0].text, "我们输入法");
        assert_eq!(engine.segmented_code(), "wo'men'shu'ru'fa");
        engine.clear_composition();

        // 2. Zero-initial + consonant-initial: xi + an -> "西安", xm -> "先"
        for ch in "xian".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 200);
        }
        assert_eq!(engine.page_candidates()[0].text, "西安");
        engine.clear_composition();

        for ch in "xm".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 250);
        }
        assert_eq!(engine.page_candidates()[0].text, "先");
        engine.clear_composition();

        // 3. Digraph initial v=zh + s=ong, g + o=uo: "vsgo" -> "中国"
        for ch in "vsgo".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 300);
        }
        assert_eq!(engine.page_candidates()[0].text, "中国");
        engine.clear_composition();

        // 4. Touch typo rescue: "wpmf" (o->p near border) -> "我们"
        let _ = engine.push_touch_letter(b'w', 1.0, 0.0, true, 400);
        let _ = engine.push_touch_letter(b'p', 8.55, 0.0, true, 450);
        for ch in "mf".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 500);
        }
        assert_eq!(engine.page_candidates()[0].text, "我们");
        engine.clear_composition();

        // 5. Shuangpin valid_next_mask after 'v' (zh): 's' (ong) is valid
        let _ = engine.handle_key_event(0, Some('v'), false, 600);
        let mask = engine.valid_next_mask();
        assert_ne!(mask & (1u32 << (b's' - b'a')), 0);
    }
}
