pub mod code;
pub mod config;
pub mod emoji;
pub mod engine;
pub mod english;
pub mod eval_log;
pub mod eval_support;
pub mod ffi;
mod fixed_decode;
pub mod format;
pub mod json;
mod missed_space;
pub mod mmap_buf;
pub mod neural;
mod personalization;
pub mod pinyin;
pub mod pinyin_dict;
mod predict;
mod spaceless;
pub mod typo;
pub mod user_data;

#[cfg(test)]
mod tests {
    use crate::code::{compose_phrase_code, expand_wildcards, PackedCode, PackedPinyin, Stem2};
    use crate::config::{FourthEmptyBehavior, JuxuConfig, InputSchema};
    use crate::engine::{CandidateBadge, JuxuEngine, KEY_SPACE};
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
            (
                PackedPinyin::from_bytes(b"jing").unwrap(),
                "jìng".to_string(),
                '镜' as u32,
                750_000,
            ),
            (
                PackedPinyin::from_bytes(b"leng").unwrap(),
                "léng".to_string(),
                '棱' as u32,
                700_000,
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
        let mut engine = JuxuEngine::new(JuxuConfig::default());
        engine.load_dict(InputSchema::Wubi86, build_sample_dict_86());

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
        let mut cfg = JuxuConfig::default();
        cfg.auto_phrase = true;
        cfg.auto_phrase_threshold = 2;
        let mut engine = JuxuEngine::new(cfg);
        engine.load_dict(InputSchema::Wubi86, build_sample_dict_86());
        engine.load_dict(InputSchema::Wubi98, build_sample_dict_98());

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
        engine.set_schema(InputSchema::Wubi98);
        for ch in ['s', 'w', 'q'] {
            let _ = engine.handle_key_event(0, Some(ch), false, 4000);
        }
        let out98 = engine.handle_key_event(0, Some('p'), false, 4050);
        assert_eq!(out98.commit_text, "棱镜");
    }

    #[test]
    fn z_pinyin_reverse_lookup_and_wildcard() {
        let mut engine = JuxuEngine::new(JuxuConfig::default());
        engine.load_dict(InputSchema::Wubi86, build_sample_dict_86());
        engine.set_aux(build_sample_aux());

        for ch in ['z', 'g', 'u', 'o'] {
            let (cx, cy) = engine.key_layout.center_of(ch as u8);
            let _ = engine.push_touch_letter(ch as u8, cx, cy, true, 100);
        }
        let cands = engine.page_candidates();
        assert!(!cands.is_empty());
        assert_eq!(cands[0].text, "国");
        assert_eq!(cands[0].badge, CandidateBadge::PinyinLookup);
        assert!(cands[0].comment.contains("lgyi"));
        // Selecting a 4-key `z`+Pinyin reverse lookup candidate (`zguo` -> `lgyi`) must NOT feed
        // Pinyin touch coordinates into Wubi key `'l'`'s touch calibration!
        let _ = engine.select_page_candidate(0, 120).unwrap();
        assert_eq!(engine.user.touch_profile.keys[(b'l' - b'a') as usize].samples, 0);

        // 4-letter Pinyin (`zjing` / `zleng`, 5 keys total > Wubi max_code_len=4) must stay in `z` lookup mode!
        for ch in "zjing".chars() {
            let out = engine.handle_key_event(0, Some(ch), false, 150);
            assert!(out.commit_text.is_empty());
        }
        assert_eq!(engine.page_candidates()[0].text, "镜");
        assert_eq!(engine.page_candidates()[0].badge, CandidateBadge::PinyinLookup);
        assert!(engine.page_candidates()[0].comment.contains("qujq"));
        // Pressing uppercase `'A'` after `"zjing"` commits `"镜"` and starts `'A'` instead of `"zjingA"`
        let out_upper = engine.handle_key_event(0, Some('A'), false, 180);
        assert_eq!(out_upper.commit_text, "镜");
        assert_eq!(out_upper.preedit, "A");
        engine.clear_composition();

        // Wildcard `trzt` finds "我" (`trnt`) without polluting key `'n'` touch offsets
        for ch in ['t', 'r', 'z', 't'] {
            let (cx, cy) = engine.key_layout.center_of(ch as u8);
            let _ = engine.push_touch_letter(ch as u8, cx, cy, true, 200);
        }
        assert_eq!(engine.page_candidates()[0].text, "我");
        assert_eq!(engine.page_candidates()[0].comment, "trnt");
        let _ = engine.select_page_candidate(0, 220).unwrap();
        assert_eq!(engine.user.touch_profile.keys[(b'n' - b'a') as usize].samples, 0);
    }

    #[test]
    fn touch_boundary_typo_rescue_bigram_and_adaptive_hitbox() {
        let mut engine = JuxuEngine::new(JuxuConfig::default());
        engine.load_dict(InputSchema::Wubi86, build_sample_dict_86());
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

        let mut engine = JuxuEngine::new(JuxuConfig::default());
        engine.load_dict(InputSchema::Wubi86, build_sample_dict_86());
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
    fn wbaux_v3_no_neural_and_export_lm_roundtrip() {
        let pair_key = (('入' as u64) << 32) | ('法' as u64);
        let tri_entry = pack_trigram('输', '入', '法', 768).unwrap();
        let blob = serialize_wbaux(AuxBuildInput {
            pinyin_items: vec![],
            unigrams: vec![('法' as u32, 2048)],
            bigrams: vec![(pair_key, 768)],
            trigrams: vec![tri_entry],
            neural_blob: vec![],
        });
        assert_eq!(&blob[0..8], WBAUX_MAGIC);
        let aux = AuxView::from_bytes(blob).unwrap();
        assert!(aux.has_unigrams());
        assert!(!aux.has_neural());
        assert_eq!(aux.stats(), (0, 1, 1, 1));
        assert!((aux.bigram_boost('入', '法') - 3.0).abs() < 0.01);

        let (uni, bi, tri, neural) = aux.export_lm_sections();
        assert_eq!(uni, vec![('法' as u32, 2048)]);
        assert_eq!(bi, vec![(pair_key, 768)]);
        assert_eq!(tri, vec![tri_entry]);
        assert!(neural.is_empty());
    }

    #[test]
    fn config_auto_space_toml_roundtrip() {
        let mut cfg = JuxuConfig::default();
        assert!(cfg.auto_space_zh_en);
        assert!(cfg.auto_space_digits);
        assert!(cfg.typo.missed_space_correction);
        cfg.auto_space_zh_en = true;
        cfg.auto_space_digits = false;
        cfg.typo.missed_space_correction = false;
        let toml = cfg.to_toml_string();
        let parsed = JuxuConfig::from_toml_str(&toml);
        assert!(parsed.auto_space_zh_en);
        assert!(!parsed.auto_space_digits);
        assert!(!parsed.typo.missed_space_correction);
    }

    #[test]
    fn missed_space_empty_split_and_toggle() {
        let mut engine = JuxuEngine::new(JuxuConfig::default());
        engine.load_dict(InputSchema::Wubi86, build_sample_dict_86());
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
    fn v3_neural_cross_clause_punctuation_context() {
        let aux_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../Resources/Dicts/wubi_aux.wbaux");
        let Ok(aux) = AuxView::open_mmap(&aux_path) else {
            return;
        };
        let nv = aux.neural_view().expect("wubi_aux.wbaux should embed WBLM0003");
        let s = nv.offs.summary;
        assert_eq!((s.vocab_size, s.rank, s.d_model, s.n_heads, s.n_layers, s.ffn_dim), (12288, 192, 320, 10, 7, 1024));
        assert!(nv.offs.has_punct_tokens, "V7 model must expose comma/period tokens");
        assert_eq!(nv.offs.ascii_trie.ext_count, 4096, "V7 WBLM0003 model must embed 4,096-entry bare ASCII/English subword Trie");
        assert_eq!(nv.punct_to_token('，'), Some(crate::neural::TOK_COMMA));
        assert_eq!(nv.punct_to_token('。'), Some(crate::neural::TOK_PERIOD));

        // Bit-exact INT8 parity check against V7-Bilingual-Distill WBLM0003 parity.json ("今天天气真" -> 好:-0.180265, 热:-4.410366)
        let mut tracker = crate::neural::NeuralContextTracker::default();
        tracker.set_context_str(&nv, "今天天气真");
        let sc_hao = nv.score_next_char(&tracker.active, '好');
        let sc_re = nv.score_next_char(&tracker.active, '热');
        assert!((sc_hao - (-0.180265)).abs() < 1e-3, "unexpected score_hao={sc_hao}");
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

        // In WBLM0002, newline maps to TOK_PERIOD identically to '。', preserving sentence context
        let mut t_nl = crate::neural::NeuralContextTracker::default();
        t_nl.set_context_str(&nv, "今天下雨了\n我");
        let mut t_period = crate::neural::NeuralContextTracker::default();
        t_period.set_context_str(&nv, "今天下雨了。我");
        let sc_dai_nl = nv.score_next_char(&t_nl.active, '带');
        let sc_dai_period = nv.score_next_char(&t_period.active, '带');
        assert!((sc_dai_nl - sc_dai_period).abs() < 1e-5);
    }

    #[test]
    fn eval_log_captures_commits_manual_select_and_backspace() {
        let mut eng = JuxuEngine::new(JuxuConfig {
            schema: InputSchema::Wubi86,
            eval_logging: true,
            fourth_code_auto_commit: false,
            ..Default::default()
        });
        eng.load_dict(InputSchema::Wubi86, build_sample_dict_86());

        // Top-1 commit ("q" + Space -> "我")
        eng.handle_key_event(0, Some('q'), false, 1000);
        let out1 = eng.handle_key_event(KEY_SPACE, Some(' '), false, 1050);
        assert_eq!(out1.commit_text, "我");

        // Backspace immediately after commit -> backspace_after_commit
        eng.handle_key_event(crate::engine::KEY_BACKSPACE, None, false, 1500);

        // Escape on >=2-key preedit -> abandon
        eng.handle_key_event(0, Some('t'), false, 2000);
        eng.handle_key_event(0, Some('r'), false, 2050);
        eng.handle_key_event(crate::engine::KEY_ESCAPE, None, false, 2100);

        let jsonl = eng.drain_eval_log_jsonl();
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 3, "expected commit + backspace_after_commit + abandon, got:\n{}", jsonl);
        assert!(lines[0].contains("\"event\":\"commit\""));
        assert!(lines[0].contains("\"chosen\":\"我\""));
        assert!(lines[1].contains("\"event\":\"backspace_after_commit\""));
        assert!(lines[2].contains("\"event\":\"abandon\""));
        assert!(lines[2].contains("\"keys\":\"tr\""));
        assert!(eng.drain_eval_log_jsonl().is_empty());

        // Sensitive numeric filter & context scrubbing
        assert!(crate::eval_log::is_sensitive_numeric_text("123456"));
        assert!(crate::eval_log::is_sensitive_numeric_text("42"));
        assert!(crate::eval_log::is_sensitive_numeric_text("验证码839201"));
        assert!(!crate::eval_log::is_sensitive_numeric_text("我们"));
        assert!(!crate::eval_log::is_sensitive_numeric_text("第3次"));
        assert_eq!(crate::eval_log::scrub_numeric_context("验证码839201好了"), "验证码[NUM]好了");
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

        let mut cfg = JuxuConfig::default();
        cfg.schema = InputSchema::Xiaohe;
        let mut engine = JuxuEngine::new(cfg);
        engine.load_dict(InputSchema::Xiaohe, dict);

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
        });
        let pdict = PinyinDictView::from_bytes(py_bytes).unwrap();
        let mut cfg = JuxuConfig::default();
        cfg.schema = InputSchema::Pinyin;
        let mut engine = JuxuEngine::new(cfg);
        engine.load_pinyin_dict(pdict);
        if let Ok(aux) = AuxView::open_mmap(concat!(env!("CARGO_MANIFEST_DIR"), "/../Resources/Dicts/wubi_aux.wbaux")) {
            engine.set_aux(aux);
        }

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
    fn full_pinyin_orthographic_aliases_umlaut_and_uncontracted_finals() {
        use crate::pinyin_dict::{serialize_pydict, PinyinDictBuildInput, PinyinDictView};
        let py_bytes = serialize_pydict(PinyinDictBuildInput {
            chars: vec![
                ("ce".into(), "策".into(), 9000, 0),
                ("lu".into(), "路".into(), 12000, 0),
                ("lv".into(), "旅".into(), 9800, 0),
                ("lv".into(), "绿".into(), 9500, 0),
                ("lve".into(), "略".into(), 9000, 0),
                ("nu".into(), "努".into(), 9000, 0),
                ("nv".into(), "女".into(), 10000, 0),
                ("nve".into(), "虐".into(), 7000, 0),
                ("you".into(), "游".into(), 10000, 0),
                ("hai".into(), "孩".into(), 9500, 0),
                ("dai".into(), "待".into(), 9000, 0),
                ("ju".into(), "句".into(), 10000, 0),
                ("xu".into(), "序".into(), 9500, 0),
                ("jue".into(), "决".into(), 10000, 0),
                ("juan".into(), "卷".into(), 9000, 0),
                ("jun".into(), "军".into(), 10000, 0),
                ("shui".into(), "水".into(), 12000, 0),
                ("liu".into(), "留".into(), 11000, 0),
                ("lun".into(), "论".into(), 10000, 0),
                ("e".into(), "鹅".into(), 6000, 0),
            ],
        });
        let pdict = PinyinDictView::from_bytes(py_bytes).unwrap();
        let mut cfg = JuxuConfig::default();
        cfg.schema = InputSchema::Pinyin;
        let mut engine = JuxuEngine::new(cfg);
        engine.load_pinyin_dict(pdict);
        if let Ok(aux) = AuxView::open_mmap(concat!(env!("CARGO_MANIFEST_DIR"), "/../Resources/Dicts/wubi_aux.wbaux")) {
            engine.set_aux(aux);
        }

        // 1. lue -> lve ("celue" -> "策略", consumes all 5 keys cleanly with zero leftover "ue")
        for ch in "celue".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 100);
        }
        assert_eq!(engine.page_candidates()[0].text, "策略");
        assert!(engine.page_candidates()[0].is_exact);
        assert_eq!(engine.segmented_code(), "ce'lue");
        let committed = engine.handle_key_event(KEY_SPACE, Some(' '), false, 150);
        assert_eq!(committed.commit_text, "策略");
        assert!(!engine.is_composing(), "must consume all 5 keys without leaving 'ue'");

        // 2. nue -> nve ("nuedai" -> "虐待")
        for ch in "nuedai".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 200);
        }
        assert_eq!(engine.page_candidates()[0].text, "虐待");
        assert!(engine.page_candidates()[0].is_exact);
        engine.clear_composition();

        // 3. lu -> lv / nu -> nv ("luyou" -> "旅游", "nuhai" -> "女孩", while bare "lu" keeps "路" #1)
        for ch in "luyou".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 300);
        }
        assert_eq!(engine.page_candidates()[0].text, "旅游");
        assert!(engine.page_candidates()[0].is_exact);
        engine.clear_composition();

        for ch in "nuhai".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 350);
        }
        assert_eq!(engine.page_candidates()[0].text, "女孩");
        engine.clear_composition();

        for ch in "lu".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 380);
        }
        assert_eq!(engine.page_candidates()[0].text, "路");
        assert!(engine.all_candidates().iter().any(|c| c.text == "绿"));
        let mask_after_lu = engine.valid_next_mask();
        assert_ne!(mask_after_lu & (1u32 << (b'e' - b'a')), 0, "'e' must be valid after 'lu' for 'lue'");
        engine.clear_composition();

        // 4. j/q/x/y + v -> u as exact spelling ("jvxv" -> "句序", "jve" -> "决", "jvan" -> "卷", "jvn" -> "军")
        for ch in "jvxv".chars() {
            let _ = engine.handle_key_event(0, Some(ch), false, 400);
        }
        assert_eq!(engine.page_candidates()[0].text, "句序");
        assert!(engine.page_candidates()[0].is_exact);
        assert_eq!(engine.page_candidates()[0].badge, CandidateBadge::None);
        engine.clear_composition();

        // 5. Uncontracted Hanyu Pinyin finals: shuei -> shui, liou -> liu, luen -> lun
        for (keys, expected) in [("shuei", "水"), ("liou", "留"), ("luen", "论")] {
            for ch in keys.chars() {
                let _ = engine.handle_key_event(0, Some(ch), false, 500);
            }
            assert_eq!(engine.page_candidates()[0].text, expected, "{} failed", keys);
            assert!(engine.page_candidates()[0].is_exact);
            engine.clear_composition();
        }
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
        });
        let pdict = PinyinDictView::from_bytes(py_bytes).unwrap();
        let mut cfg = JuxuConfig::default();
        cfg.schema = InputSchema::XiaoheShuangpin;
        let mut engine = JuxuEngine::new(cfg);
        engine.load_pinyin_dict(pdict);
        if let Ok(aux) = AuxView::open_mmap(concat!(env!("CARGO_MANIFEST_DIR"), "/../Resources/Dicts/wubi_aux.wbaux")) {
            engine.set_aux(aux);
        }

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

    #[test]
    fn spaceless_wubi_shows_and_commits_prefix_completions() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let dict_path = manifest_dir.join("../Resources/Dicts/wubi86.wbdict");
        let aux_path = manifest_dir.join("../Resources/Dicts/wubi_aux.wbaux");
        if !dict_path.exists() || !aux_path.exists() {
            return;
        }
        let mut cfg = JuxuConfig::default();
        cfg.schema = InputSchema::Wubi86;
        cfg.spaceless = true;
        cfg.page_size = 9;
        let mut engine = JuxuEngine::new(cfg);
        engine.load_dict(InputSchema::Wubi86, DictView::open_mmap(&dict_path).unwrap());
        engine.set_aux(AuxView::open_mmap(&aux_path).unwrap());

        // 1. Single-key shortcode `q` should keep "我" first and append longer `q*` prefix completions.
        let _ = engine.handle_key_event(0, Some('q'), false, 100);
        let cands: Vec<_> = engine.page_candidates().to_vec();
        assert_eq!(cands[0].text, "我");
        assert!(cands.len() > 1, "expected prefix completions after '我', got {:?}", cands);
        let prefix_idx = cands
            .iter()
            .position(|c| c.matched_code.len() > 1 && c.matched_code.as_string().starts_with('q'))
            .expect("expected at least one longer q* prefix candidate");
        let chosen_text = cands[prefix_idx].text.clone();
        assert!(cands[prefix_idx].comment.starts_with('…'));

        // Committing the prefix completion should commit that candidate cleanly without underflow.
        let out = engine.select_abs_candidate(prefix_idx, 120).unwrap();
        assert_eq!(out.commit_text, chosen_text);
        assert!(!engine.is_composing());

        // 2. Multi-segment spaceless input ending in a short tail (`trwulq` -> "我们" + "罗" (`trwu` + `lq`)):
        //    prefix completions for trailing `lq` should commit `<prefix_segments><chosen_tail>`.
        for (i, ch) in "trwulq".chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 200 + i as u64 * 20);
        }
        let mut all_multi = Vec::new();
        loop {
            all_multi.extend_from_slice(engine.page_candidates());
            if !engine.page_next() {
                break;
            }
        }
        assert_eq!(all_multi[0].text, "我们罗");
        let tail_prefix_idx = all_multi
            .iter()
            .position(|c| c.matched_code.len() > 2 && c.matched_code.as_string().starts_with("lq"))
            .expect("expected trailing lq* prefix completion in multi-segment spaceless mode");
        let tail_text = all_multi[tail_prefix_idx].text.clone();
        let multi_out = engine.select_abs_candidate(tail_prefix_idx, 320).unwrap();
        assert_eq!(multi_out.commit_text, format!("我们{}", tail_text));
        assert!(!engine.is_composing());
    }

    #[test]
    fn english_dict_morphology_and_prefix() {
        let dict = crate::english::EnDictView::builtin_default();
        assert_eq!(dict.lookup_exact("ios").unwrap().display, "iOS");
        assert_eq!(dict.lookup_exact("rust").unwrap().display, "Rust");
        assert_eq!(dict.lookup_exact("github").unwrap().display, "GitHub");

        // Inflectional morphology fallback (`coding` -> `code` + `ing`, `bugs` -> `bug` + `s`)
        assert_eq!(dict.lookup_with_morphology("coding").unwrap().0, "coding");
        assert_eq!(dict.lookup_with_morphology("bugs").unwrap().0, "bugs");
        assert_eq!(dict.lookup_with_morphology("worked").unwrap().0, "worked");

        // Prefix info & completions (`pyth` -> `Python`)
        let (max_w, has_longer) = dict.prefix_info("pyth").unwrap();
        assert!(max_w >= 40_000 && has_longer);
        let comps = dict.lookup_prefix("pyth", 3);
        assert!(comps.iter().any(|e| e.display == "Python"));
    }

    #[test]
    fn mixed_pinyin_sentence_dict_oov_upper_and_user_learning() {
        use crate::pinyin_dict::{serialize_pydict, PinyinDictBuildInput, PinyinDictView};

        let pblob = serialize_pydict(PinyinDictBuildInput {
            chars: vec![
                ("wo".into(), "我".into(), 9_000_000, 0),
                ("men".into(), "们".into(), 8_000_000, 0),
                ("yong".into(), "用".into(), 8_500_000, 0),
                ("xie".into(), "写".into(), 7_500_000, 0),
                ("le".into(), "了".into(), 9_000_000, 0),
                ("shu".into(), "输".into(), 6_000_000, 0),
                ("ru".into(), "入".into(), 6_000_000, 0),
                ("fa".into(), "法".into(), 6_000_000, 0),
            ],
        });
        let pdict = PinyinDictView::from_bytes(pblob).unwrap();

        let mut cfg = JuxuConfig::default();
        cfg.schema = InputSchema::Pinyin;
        let mut engine = JuxuEngine::new(cfg);
        engine.load_pinyin_dict(pdict);
        if let Ok(aux) = AuxView::open_mmap(concat!(env!("CARGO_MANIFEST_DIR"), "/../Resources/Dicts/wubi_aux.wbaux")) {
            engine.set_aux(aux);
        }

        // 1. Dictionary English word inside a Chinese sentence: `woyongrustxiele` -> `我用Rust写了`
        for (i, ch) in "woyongrustxiele".chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 100 + i as u64 * 20);
        }
        assert_eq!(engine.all_candidates()[0].text, "我用Rust写了");
        engine.clear_composition();

        // 2. OOV impossible-pinyin English word inside a Chinese sentence: `woyongworktreexie` -> `我用worktree写`
        for (i, ch) in "woyongworktreexie".chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 500 + i as u64 * 20);
        }
        assert_eq!(engine.all_candidates()[0].text, "我用worktree写");
        engine.clear_composition();

        // 3. Explicit uppercase OOV anchor inside a Chinese sentence (`'` boundary or CamelCase):
        //    `woyongJuxu'xie` -> `我用Juxu写`
        for (i, ch) in "woyongJuxu'xie".chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 900 + i as u64 * 20);
        }
        assert_eq!(engine.all_candidates()[0].text, "我用Juxu写");
        engine.clear_composition();

        // 4. Enter-to-commit OOV learning (`Juxu`), then lowercase mixed sentence without `'` (`woyongjuxuxie` -> `我用Juxu写`)
        for (i, ch) in "Juxu".chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 1200 + i as u64 * 20);
        }
        let ret = engine.handle_key_event(crate::engine::KEY_RETURN, Some('\r'), false, 1300);
        assert_eq!(ret.commit_text, "Juxu");
        assert!(engine.user.lookup_english_exact("juxu").is_some());

        for (i, ch) in "woyongjuxuxie".chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 1400 + i as u64 * 20);
        }
        assert_eq!(engine.all_candidates()[0].text, "我用Juxu写");
        engine.clear_composition();

        // 5. Pure Chinese & initial abbreviation sanity check (zero regression!)
        for (i, ch) in "women".chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 1800 + i as u64 * 20);
        }
        assert_eq!(engine.all_candidates()[0].text, "我们");
        engine.clear_composition();

        for (i, ch) in "srf".chars().enumerate() {
            let _ = engine.handle_key_event(0, Some(ch), false, 2000 + i as u64 * 20);
        }
        assert_eq!(engine.all_candidates()[0].text, "输入法");
    }

    #[test]
    fn wubi_fixed_code_english_anti_premature_commit_and_upper_anchor() {
        let mut entries = vec![
            (PackedCode::from_ascii("lwgl").unwrap(), "输入".to_string(), 650_000_000, 0),
            // Unique 4-code Wubi char that collides with the English word `rust`
            (PackedCode::from_ascii("rust").unwrap(), "搎".to_string(), 50_000, ENTRY_FLAG_SINGLE_CHAR),
        ];
        entries.sort_by_key(|e| e.0);
        let blob = serialize_wbdict(DictBuildInput {
            schema_id: 86,
            entries,
            char_metas: Vec::new(),
        });
        let dict = DictView::from_bytes(blob).unwrap();

        let mut cfg = JuxuConfig::default();
        cfg.schema = InputSchema::Wubi86;
        cfg.fourth_code_auto_commit = true;
        cfg.fifth_code_push_commit = true;
        cfg.fourth_empty_behavior = FourthEmptyBehavior::Block;
        let mut engine = JuxuEngine::new(cfg);
        engine.load_dict(InputSchema::Wubi86, dict);

        // 1. Normal 4-code Wubi word `lwgl` still auto-commits immediately at 4th key
        let mut last_out = Default::default();
        for (i, ch) in "lwgl".chars().enumerate() {
            last_out = engine.handle_key_event(0, Some(ch), false, 100 + i as u64 * 20);
        }
        assert_eq!(last_out.commit_text, "输入");
        assert!(!engine.is_composing());

        // 2. Typing `rust` (collides with unique 4-code Wubi char) does NOT prematurely auto-commit!
        for (i, ch) in "rust".chars().enumerate() {
            last_out = engine.handle_key_event(0, Some(ch), false, 300 + i as u64 * 20);
        }
        assert!(last_out.commit_text.is_empty(), "rust must not prematurely auto-commit as Wubi");
        assert!(engine.is_composing());
        assert!(engine.all_candidates().iter().any(|c| c.text == "Rust"));
        assert!(engine.all_candidates().iter().any(|c| c.text == "搎"));
        engine.clear_composition();

        // 3. Typing 6-letter `python` (`pyth` is empty in Wubi) does NOT block at 4th key or push-commit at 5th key!
        for (i, ch) in "python".chars().enumerate() {
            last_out = engine.handle_key_event(0, Some(ch), false, 500 + i as u64 * 20);
            assert!(!last_out.bell, "python must not ring empty-code bell at char '{}'", ch);
            assert!(last_out.commit_text.is_empty(), "python must not push-commit at char '{}'", ch);
        }
        assert_eq!(engine.all_candidates()[0].text, "Python");
        let sp = engine.handle_key_event(KEY_SPACE, Some(' '), false, 700);
        assert_eq!(sp.commit_text, "Python");

        // 4. Uppercase OOV word `Juxu` in Wubi skips all auto-commit/block/push-commit and learns into UserStore
        for (i, ch) in "Juxu".chars().enumerate() {
            last_out = engine.handle_key_event(0, Some(ch), false, 900 + i as u64 * 20);
            assert!(last_out.commit_text.is_empty());
        }
        assert_eq!(engine.all_candidates()[0].text, "Juxu");
        let sp2 = engine.handle_key_event(KEY_SPACE, Some(' '), false, 1000);
        assert_eq!(sp2.commit_text, "Juxu");
        assert!(engine.user.lookup_english_exact("juxu").is_some());
    }

    #[test]
    fn t9_pinyin_sentence_lock_gaussian_typo_and_adaptive_hitbox() {
        use crate::pinyin_dict::{serialize_pydict, PinyinDictBuildInput, PinyinDictView};

        let pblob = serialize_pydict(PinyinDictBuildInput {
            chars: vec![
                ("ni".to_string(), "你".to_string(), 900_000, 0),
                ("mi".to_string(), "米".to_string(), 500_000, 0),
                ("hao".to_string(), "好".to_string(), 900_000, 0),
                ("gao".to_string(), "高".to_string(), 400_000, 0),
                ("gao".to_string(), "糕".to_string(), 300_000, 0),
                ("zhong".to_string(), "中".to_string(), 900_000, 0),
                ("guo".to_string(), "国".to_string(), 900_000, 0),
                ("xi".to_string(), "西".to_string(), 700_000, 0),
                ("an".to_string(), "安".to_string(), 700_000, 0),
                ("xian".to_string(), "先".to_string(), 800_000, 0),
            ],
        });
        let pdict = PinyinDictView::from_bytes(pblob).unwrap();

        let mut cfg = JuxuConfig::default();
        cfg.schema = InputSchema::Pinyin;
        cfg.pinyin_t9_layout = true;
        cfg.typo.enabled = true;
        cfg.typo.touch_hitbox_adapt = true;
        let toml_rt = JuxuConfig::from_toml_str(&cfg.to_toml_string());
        assert!(toml_rt.pinyin_t9_layout);

        let mut engine = JuxuEngine::new(cfg);
        engine.load_pinyin_dict(pdict);
        if let Ok(aux) = AuxView::open_mmap(concat!(env!("CARGO_MANIFEST_DIR"), "/../Resources/Dicts/wubi_aux.wbaux")) {
            engine.set_aux(aux);
        }
        assert!(engine.is_t9_pinyin_mode());

        // 1. Standard T9 sentence decoding: 64426 -> ni'hao -> "你好"
        for (i, ch) in "64426".bytes().enumerate() {
            let (cx, cy) = engine.key_layout.t9_center_of(ch);
            let _ = engine.push_touch_t9(ch, cx, cy, true, 100 + i as u64 * 40);
        }
        assert_eq!(engine.all_candidates()[0].text, "你好");
        let syls = engine.t9_syllable_options();
        assert!(syls.contains(&"ni".to_string()));
        assert!(syls.contains(&"mi".to_string()));

        // 2. Sidebar syllable lock: locking "mi" switches candidates to "米*" with "米糕" in top-3
        let _ = engine.lock_t9_syllable("mi");
        assert_eq!(engine.t9_locked_prefix(), "mi");
        assert!(engine.all_candidates()[0].text.starts_with('米'));
        assert!(engine.all_candidates().iter().take(3).any(|c| c.text == "米糕"));
        let next_syls = engine.t9_syllable_options();
        assert!(next_syls.contains(&"gao".to_string()));
        engine.clear_composition();

        // 3. '1' separator splits xi'an (94'26) instead of xian (9426 -> 先)
        for (i, ch) in "94126".bytes().enumerate() {
            let (cx, cy) = if ch == b'1' {
                (0.0, 0.0)
            } else {
                engine.key_layout.t9_center_of(ch)
            };
            let _ = engine.push_touch_t9(ch, cx, cy, true, 400 + i as u64 * 40);
        }
        assert_eq!(engine.all_candidates()[0].text, "西安");
        engine.clear_composition();

        // 4. 2D Gaussian key-boundary typo rescue on T9 big keys:
        //    "zhongguo" = 94664486; slip key #3 ('6' at (2,1)) to border of '5' at (1.42, 1.0), reported as '5'.
        let target_seq = b"94664486";
        for (i, &ch) in target_seq.iter().enumerate() {
            let (cx, cy) = engine.key_layout.t9_center_of(ch);
            if i == 2 {
                // Reported '5' near the right border toward '6'
                let _ = engine.push_touch_t9(b'5', 1.44, 1.0, true, 800 + i as u64 * 40);
            } else {
                // Slight consistent rightward bias (+0.08) to verify T9 adaptive TouchProfile learning
                let _ = engine.push_touch_t9(ch, cx + 0.08, cy, true, 800 + i as u64 * 40);
            }
        }
        assert!(
            engine.all_candidates().iter().take(3).any(|c| c.text == "中国"),
            "2D Gaussian T9 boundary typo should rescue '中国', got {:?}",
            engine.all_candidates().iter().take(5).map(|c| &c.text).collect::<Vec<_>>()
        );
        let idx = engine
            .all_candidates()
            .iter()
            .position(|c| c.text == "中国")
            .unwrap();
        let out = engine.select_abs_candidate(idx, 1200).unwrap();
        assert_eq!(out.commit_text, "中国");

        // 5. Independent T9 TouchProfile learned offsets without polluting 26-key profile!
        assert!(engine.user.touch_profile.t9_keys[(b'9' - b'2') as usize].samples > 0);
        assert!(engine.user.touch_profile.offset_for_t9(b'9').0 > 0.0);
        assert!(engine.user.touch_profile.keys.iter().all(|k| k.samples == 0));

        // 6. T9 next-key mask & priors aggregation returns valid 8-key weights
        let (t9_mask, t9_priors) = engine.t9_next_key_mask_and_priors();
        assert_ne!(t9_mask, 0);
        assert!(t9_priors.iter().any(|&p| p > 0));
    }

    #[test]
    fn strategy_p0_p2_auto_phrase_boundary_edge_touch_and_eval_log() {
        // 1. P0-2: Auto-phrase must align strictly with commit boundaries and reject particles/AA stutters.
        let mut store = crate::user_data::UserStore::default();
        let dict = build_sample_dict_86();

        // Mid-word slice: committing 2-char "我们" + "好" must NEVER slice "们好" (non-initial '们')!
        for round in 0..3u64 {
            store.break_single_burst();
            let t = 1000 + round * 1000;
            let _ = store.record_commit("", "我们", t, true, 2, Some(&dict), None, false, None);
            let _ = store.record_commit("我们", "好", t + 100, true, 2, Some(&dict), None, false, None);
        }
        assert!(
            !store.phrases.contains_key("们好"),
            "mid-word slice '们好' must be rejected, got {:?}",
            store.phrases.keys().collect::<Vec<_>>()
        );

        // Zero character blocklist: only 2-char accidental AA stutters ("是是") and mid-word slices ("怎么"+"样" -> "么样")
        // are rejected, whereas modal/function phrases ("傻逼吗", "最好是", "标的", "过拟合", "落地", "我觉得")
        // ARE all valid and learnable!
        store.break_single_burst();
        let _ = store.record_commit("", "是", 5300, true, 2, None, None, false, None);
        let _ = store.record_commit("是", "是", 5400, true, 2, None, None, false, None);
        assert!(!store.phrases.contains_key("是是"), "AA stutter '是是' must not auto-compose");
        // Mid-word slice check for "怎么" + "样": promotes "怎么样", NEVER slices "么样"!
        for r in 0..2u64 {
            store.break_single_burst();
            let t = 5500 + r * 200;
            let _ = store.record_commit("", "怎么", t, true, 2, None, None, false, None);
            let _ = store.record_commit("怎么", "样", t + 80, true, 2, None, None, false, None);
        }
        assert!(store.phrases.contains_key("怎么样"), "'怎么'+'样' must compose full '怎么样'");
        assert!(!store.phrases.contains_key("么样"), "mid-word '么样' must NEVER be sliced out of '怎么'+'样'");
        assert!(crate::user_data::is_valid_auto_phrase_text("傻逼吗"));
        assert!(crate::user_data::is_valid_auto_phrase_text("真的吗"));
        assert!(crate::user_data::is_valid_auto_phrase_text("技术专家"));
        assert!(crate::user_data::is_valid_auto_phrase_text("标的"));
        assert!(crate::user_data::is_valid_auto_phrase_text("过拟合"));
        assert!(crate::user_data::is_valid_auto_phrase_text("落地"));
        assert!(crate::user_data::is_valid_auto_phrase_text("最好是"));
        assert!(crate::user_data::is_valid_auto_phrase_text("我觉得"));

        // Typing "标" + "的" ONCE must NOT 1-shot promote (guards casual '的' suffix),
        // while typing "最" + "好" + "是" or "标" + "的" TWICE DOES promote cleanly!
        store.break_single_burst();
        let _ = store.record_commit("", "标", 6000, true, 2, None, None, false, None);
        let _ = store.record_commit("标", "的", 6100, true, 2, None, None, false, None);
        assert!(!store.phrases.contains_key("标的"), "1st '标'+'的' should wait for repetition");
        store.break_single_burst();
        let _ = store.record_commit("", "标", 7000, true, 2, None, None, false, None);
        let _ = store.record_commit("标", "的", 7100, true, 2, None, None, false, None);
        assert!(store.phrases.contains_key("标的"), "2nd '标'+'的' must auto-compose '标的'");

        for r in 0..2u64 {
            store.break_single_burst();
            let t = 8000 + r * 1000;
            let _ = store.record_commit("", "最", t, true, 2, None, None, false, None);
            let _ = store.record_commit("最", "好", t + 100, true, 2, None, None, false, None);
            let _ = store.record_commit("最好", "是", t + 200, true, 2, None, None, false, None);
        }
        assert!(store.phrases.contains_key("最好是"), "expected '最好是' to auto-compose across single-char commits");

        // 2. P2-5: Outer-edge keys ('a'/'q' left, 'l'/'p' right) allow up to 0.32 offset while interior clamps at 0.20.
        let mut tp = crate::user_data::TouchProfile::default();
        for _ in 0..60 {
            tp.observe(b'a', -0.30, 0.02);
            tp.observe(b'l', 0.30, -0.02);
            tp.observe(b'g', -0.30, 0.30);
        }
        let (ax, _) = tp.offset_for(b'a');
        let (lx, _) = tp.offset_for(b'l');
        let (gx, gy) = tp.offset_for(b'g');
        assert!(ax < -0.25 && ax >= -0.32, "expected outer left 'a' offset < -0.25, got {}", ax);
        assert!(lx > 0.25 && lx <= 0.32, "expected outer right 'l' offset > 0.25, got {}", lx);
        assert!((gx - (-0.20)).abs() < 0.02 && (gy - 0.20).abs() < 0.02, "interior 'g' must clamp at ±0.20, got ({}, {})", gx, gy);

        // 3. P2-6: abandon_composition + notify_post_commit_backspace record telemetry cleanly.
        let mut eng = JuxuEngine::new(JuxuConfig {
            schema: InputSchema::Wubi86,
            eval_logging: true,
            fourth_code_auto_commit: false,
            ..Default::default()
        });
        eng.load_dict(InputSchema::Wubi86, dict);
        eng.handle_key_event(0, Some('q'), false, 10_000);
        let out = eng.handle_key_event(KEY_SPACE, Some(' '), false, 10_050);
        assert_eq!(out.commit_text, "我");
        eng.notify_post_commit_backspace(10_500);
        eng.handle_key_event(0, Some('t'), false, 11_000);
        eng.handle_key_event(0, Some('r'), false, 11_050);
        eng.abandon_composition(11_200);
        assert!(!eng.is_composing());
        let log = eng.drain_eval_log_jsonl();
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 3, "expected commit + backspace_after_commit + abandon, got:\n{}", log);
        assert!(lines[1].contains("\"event\":\"backspace_after_commit\""));
        assert!(lines[2].contains("\"event\":\"abandon\""));
    }

    #[test]
    fn chinese_smart_punctuation_maps_braces_to_corner_brackets() {
        let mut eng = JuxuEngine::new(JuxuConfig::default());
        let open = eng.handle_key_event(0, Some('{'), false, 1_000);
        assert!(open.consumed);
        assert_eq!(open.commit_text, "「");

        let close = eng.handle_key_event(0, Some('}'), false, 1_050);
        assert!(close.consumed);
        assert_eq!(close.commit_text, "」");

        let sq_open = eng.handle_key_event(0, Some('['), false, 1_100);
        assert!(sq_open.consumed);
        assert_eq!(sq_open.commit_text, "【");

        let sq_close = eng.handle_key_event(0, Some(']'), false, 1_150);
        assert!(sq_close.consumed);
        assert_eq!(sq_close.commit_text, "】");
    }
}
