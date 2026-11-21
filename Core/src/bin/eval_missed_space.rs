use std::path::PathBuf;
use std::time::Instant;
use juxu_core::code::PackedCode;
use juxu_core::config::{JuxuConfig, InputSchema};
use juxu_core::engine::{JuxuEngine, KEY_SPACE};
use juxu_core::format::{AuxView, DictView};
use juxu_core::typo::KeyLayout;

/// Realistic modern Chinese evaluation sentences across daily IM, work/tech, life, and formal prose.
const SENTENCES: &[&str] = &[
    // Daily IM & chat
    "今天天气真好我们一起去吃饭吧",
    "你现在在哪里我马上就过去找你",
    "明天早上九点开会别忘记带电脑",
    "我刚才在路上看到一个很可爱的猫",
    "这个周末我想去爬山你要不要一起去",
    "不好意思刚才在忙没来得及回你消息",
    "你觉得这家店的咖啡味道怎么样",
    "我已经到了就在地铁口旁边等你",
    "昨天晚上睡得太晚今天早上起不来",
    "能不能帮我把那份文件发到我邮箱里",
    "我觉得这个方案比之前那个好多了",
    "待会儿下班以后我们去超市买点水果",
    "你最近有没有看什么好看的电影推荐一下",
    "这件事其实没有你想的那么复杂",
    "我本来以为今天会下雨结果大太阳",
    "别着急慢慢来反正还有好几天时间",
    "刚刚收到快递打开一看质量还挺不错的",
    "今天实在太累了回去洗个澡就睡觉",
    "下周我要去北京出差大概要待三天",
    "你先把需求发给我我下午仔细看一下",
    "这次旅游拍了好多照片回来发给你看",
    "我觉得他说得挺有道理的可以试试",
    "家里没有菜了晚上我们点外卖吃吧",
    "手机快没电了我先找个地方充一下电",
    "好久不见了最近工作还顺利吗",
    "麻烦你把门关一下外面风有点大",
    "我记得上次来这里的时候还没有这家店",
    "你明天有空吗我想请你帮个小忙",
    "听起来不错那就这么说定了不见不散",
    "我一直在想这个问题到底该怎么解决",
    // Tech, coding & product
    "这个输入法的纠错模型响应速度非常快",
    "我们在本地部署了一个轻量级语言模型",
    "刚才提交的代码在持续集成里跑过了所有测试",
    "你需要先把这个分支合并到主干再发布版本",
    "内存占用控制在五十兆以内才能稳定运行",
    "键盘扩展在冷启动时需要直接映射只读词库",
    "这里有一个边界条件会导致候选词排序异常",
    "如果用户在打完三码以后忘记按空格就会空码",
    "利用上下文概率可以把粘连的编码重新切分开",
    "我用性能分析工具测了一下单次按键只要几十微秒",
    "这个接口返回的数据结构需要保持向后兼容",
    "测试环境已经部署好了你可以直接上去验证",
    "日志里显示网络连接超时可能是代理配置有问题",
    "把配置文件改完保存之后输入法会实时重新加载",
    "在手机屏幕上单手打字经常会不小心碰到旁边的键",
    "这套算法同时支持八六版九八版和新世纪五笔",
    "数据库查询太慢了最好给这个字段加一个索引",
    "前端页面在暗黑模式下的对比度还需要再调一下",
    "今天把所有的单元测试和集成测试都补齐了",
    "我们要尽量降低误纠率不能影响正常盲打手感",
    // Work, study & life
    "下午三点产品经理要组织大家过一遍交互稿",
    "这份季度报告的数据我已经核对过两遍了",
    "最近大家都很辛苦项目上线以后好好休息几天",
    "图书馆三楼靠窗的位置今天居然还有空座",
    "学五笔最重要的是把字根表背熟然后多练习拆字",
    "很多常用字在五笔里其实只有三级简码或者三码全码",
    "打完四码词组不用按空格所以打三码字时容易顺手连打",
    "早上喝了一杯美式咖啡现在精神好多了",
    "去机场的路上有点堵车幸好我们提前出发了",
    "这本书写得非常通俗易懂很适合入门阅读",
    "秋天到了早晚温差很大出门记得多穿一件外套",
    "这家餐厅的招牌菜是清蒸鲈鱼和红烧肉",
    "我们团队准备下个月开始做一个全新的开源项目",
    "会议纪要已经整理好发在群里了请大家确认",
    "遇到问题不要慌先复现再逐步排查根因",
    "他做事一直非常认真负责大家都特别信任他",
    "公园里的银杏叶全都黄了周末去拍照一定很美",
    "今年冬天感觉比去年冷了不少暖气开得很足",
    "把桌面收拾干净之后工作效率明显提高了很多",
    "锻炼身体贵在坚持每天跑步半小时就很有效果",
    // Formal & news-like prose
    "随着人工智能技术的不断发展终端侧推理越来越普及",
    "科学研究表明良好的睡眠习惯有助于提高记忆力",
    "城市公共交通系统的优化显著缩短了居民的通勤时间",
    "近年来新能源行业保持了较快的增长势头",
    "在制定长期规划时需要充分考虑各种不确定因素",
    "保护生态环境需要全社会每一个人的共同努力",
    "高质量的开源社区能够有效促进技术标准的演进",
    "通过持续优化底层架构系统吞吐量提升了近一倍",
    "数字化转型不仅涉及技术升级更关乎组织协作方式",
    "文化交流让不同背景的人们增进了彼此的理解与信任",
    // Extra 3-code rich colloquial sentences
    "我好想去海边吹吹风看一看日落",
    "今天太热了我想去买一杯冰奶茶喝",
    "他说话总是很直接其实人特别好",
    "你来得正好帮我看看这个字怎么拆",
    "手心里全是汗我走过去把门推开了",
    "一年过得真快转眼又到年底了",
    "里外都找遍了还是没见到钥匙在哪里",
    "这么多东西我一个人拿不动你来帮一下",
    "少吃点辛辣的东西对胃会好一些",
    "可以先把灯关掉再打开投影仪吗",
    "只差最后一步就能全部完成了别放弃",
    "头有点疼我先闭上眼睛歇一会儿",
    "问了半天终于弄明白到底是怎么回事",
    "下楼买点早餐顺便把快递取回来",
    "走慢一点等等我我鞋带松了系一下",
    "开会之前先把资料发给大家预习一下",
    "多喝热水早点休息明天就会好起来",
    "还要等多久才能轮到我们进去吃饭",
    "见了面再详细聊手机上打字说不清楚",
    "去年买的那件羽绒服今年穿着还是很暖和",
];

/// Segment sentence into realistic Wubi tokens `(text, code)` using either:
/// - `prefer_single_short = false`: phrase-first blind typing (2..4 char phrases when #1, else primary short/full char)
/// - `prefer_single_short = true`: single-char-heavy typing (exercises many 3-code full/short chars like 好/去/来/说/天/可/想!)
fn segment_sentence(dict: &DictView, sent: &str, single_char_mode: bool) -> Vec<(String, String)> {
    let chars: Vec<char> = sent.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if !single_char_mode {
            let mut matched: Option<(usize, String, String)> = None;
            for len in (2..=4).rev() {
                if i + len > chars.len() {
                    continue;
                }
                let sub: String = chars[i..i + len].iter().collect();
                if let Some(code) = dict.encode_phrase(&sub) {
                    let hits = dict.lookup_exact(code, false);
                    if hits.first().map(|e| e.text == sub).unwrap_or(false) {
                        matched = Some((len, sub, code.as_string()));
                        break;
                    }
                }
            }
            if let Some((len, text, code)) = matched {
                tokens.push((text, code));
                i += len;
                continue;
            }
        }
        let ch = chars[i];
        let mut pushed = false;
        if let Some(meta) = dict.char_meta(ch) {
            // In single_char_mode, prefer full_code if it's 3-letter (realistic for users who type 3-code full chars like 好 vbg, 去 fcu, 来 goi, 说 yuk!)
            let candidates_codes = if single_char_mode && meta.full_code.len() == 3 {
                [meta.full_code, meta.primary_short_code, meta.shortest_code]
            } else {
                [meta.primary_short_code, meta.shortest_code, meta.full_code]
            };
            for code in candidates_codes {
                if code.is_empty() {
                    continue;
                }
                let cs = code.as_string();
                if !cs.is_empty() && cs.bytes().all(|b| (b'a'..=b'y').contains(&b)) {
                    let hits = dict.lookup_exact(code, false);
                    if hits.first().map(|e| e.text == ch.to_string()).unwrap_or(false) {
                        tokens.push((ch.to_string(), cs));
                        pushed = true;
                        break;
                    }
                }
            }
        }
        if !pushed {
            tokens.push((ch.to_string(), String::new()));
        }
        i += 1;
    }
    tokens
}

fn type_code_and_space(engine: &mut JuxuEngine, code: &str, need_space: bool, ts: &mut u64) -> String {
    let mut committed = String::new();
    for b in code.bytes() {
        *ts += 60;
        let out = engine.handle_key_event(0, Some(b as char), false, *ts);
        committed.push_str(&out.commit_text);
    }
    if need_space && engine.is_composing() {
        *ts += 60;
        let out = engine.handle_key_event(KEY_SPACE, Some(' '), false, *ts);
        committed.push_str(&out.commit_text);
    }
    committed
}

#[derive(Default, Debug)]
struct BucketStat {
    total: usize,
    exact_ok: usize,
    top2_ok: usize,
    blocked_bell: usize,
}

impl BucketStat {
    fn pct(a: usize, b: usize) -> f64 {
        if b == 0 {
            0.0
        } else {
            100.0 * (a as f64) / (b as f64)
        }
    }
    fn fmt_line(&self, name: &str) -> String {
        format!(
            "{:<28} | n={:4} | Exact={:6.2}% | Top2={:6.2}% | BellBlock={:5.2}%",
            name,
            self.total,
            Self::pct(self.exact_ok, self.total),
            Self::pct(self.top2_ok, self.total),
            Self::pct(self.blocked_bell, self.total),
        )
    }
}

fn eval_schema(schema: InputSchema, dict_path: &PathBuf, aux_path: &PathBuf, enabled: bool, verbose: bool) {
    let dict = DictView::open_mmap(dict_path).expect("open dict");
    let aux = AuxView::open_mmap(aux_path).expect("open aux");

    let mut cfg = JuxuConfig::default();
    cfg.schema = schema;
    cfg.fourth_code_auto_commit = true;
    cfg.fifth_code_push_commit = true;
    cfg.typo.enabled = true;
    cfg.typo.missed_space_correction = enabled;
    cfg.typo.touch_hitbox_adapt = false;

    let mut engine = JuxuEngine::new(cfg);
    engine.load_dict(schema, dict);
    engine.set_aux(aux);

    let mut s3_empty = BucketStat::default();
    let mut s3_coll_len1 = BucketStat::default();
    let mut s3_coll_len234 = BucketStat::default();
    let mut s2_empty = BucketStat::default();
    let mut s2_coll = BucketStat::default();
    let mut s1_empty = BucketStat::default();
    let mut s1_coll = BucketStat::default();

    let mut normal_4code_total = 0usize;
    let mut normal_4code_fp_split = 0usize;
    let mut normal_4code_held = 0usize;
    let mut normal_sent_total = 0usize;
    let mut normal_sent_exact = 0usize;

    let mut sample_ok = Vec::<String>::new();
    let mut sample_fail = Vec::<String>::new();
    let mut sample_fp = Vec::<String>::new();

    for &single_mode in &[false, true] {
        for &sent in SENTENCES {
            let tokens = {
                let d = engine.active_dict().unwrap();
                segment_sentence(d, sent, single_mode)
            };
            if tokens.iter().any(|(_, c)| c.is_empty()) {
                continue;
            }

            // 1. Normal typing guardrail (ZERO missed spaces!)
            normal_sent_total += 1;
            engine.clear_composition();
            engine.clear_context();
            let mut ts = 1000u64;
            let mut got_sent = String::new();
            for (idx, (_ttext, tcode)) in tokens.iter().enumerate() {
                let is_4 = tcode.len() == 4;
                if is_4 {
                    normal_4code_total += 1;
                }
                let mut token_commit = String::new();
                for b in tcode.bytes() {
                    ts += 60;
                    let out = engine.handle_key_event(0, Some(b as char), false, ts);
                    token_commit.push_str(&out.commit_text);
                }
                if is_4 {
                    // Check if unique 4-code char was held (didn't auto-commit)
                    let is_unique = {
                        let d = engine.active_dict().unwrap();
                        d.lookup_exact(PackedCode::from_ascii(tcode).unwrap(), false).len() == 1
                    };
                    if is_unique && engine.is_composing() {
                        normal_4code_held += 1;
                        // In real blind typing, if a 4-code word didn't auto-commit, the user immediately
                        // types the next word's first key (5th-code push!) or presses Space at end of sentence.
                        // Here let's check what happens if user types next word cleanly (or Space if last token)!
                        if idx + 1 == tokens.len() {
                            ts += 60;
                            let out = engine.handle_key_event(KEY_SPACE, Some(' '), false, ts);
                            token_commit.push_str(&out.commit_text);
                        }
                    } else if !is_unique && engine.is_composing() {
                        // Multi-candidate 4-code word: user presses Space (or types next word)
                        ts += 60;
                        let out = engine.handle_key_event(KEY_SPACE, Some(' '), false, ts);
                        token_commit.push_str(&out.commit_text);
                    }
                } else if engine.is_composing() {
                    ts += 60;
                    let out = engine.handle_key_event(KEY_SPACE, Some(' '), false, ts);
                    token_commit.push_str(&out.commit_text);
                }
                got_sent.push_str(&token_commit);
            }
            if engine.is_composing() {
                ts += 60;
                let out = engine.handle_key_event(KEY_SPACE, Some(' '), false, ts);
                got_sent.push_str(&out.commit_text);
            }
            if got_sent == sent {
                normal_sent_exact += 1;
            } else {
                normal_4code_fp_split += 1;
                if sample_fp.len() < 8 {
                    sample_fp.push(format!("mode={} exp={} got={}", single_mode, sent, got_sent));
                }
            }

            // 2. Missed-space injection at every pair (T_i, T_{i+1}) where 1 <= len(T_i) <= 3!
            for i in 0..tokens.len().saturating_sub(1) {
                let (ref t1, ref c1) = tokens[i];
                let (ref t2, ref c2) = tokens[i + 1];
                if c1.is_empty() || c1.len() >= 4 || c2.is_empty() {
                    continue;
                }
                let combined_codes = format!("{}{}", c1, c2);
                if combined_codes.len() < 4 {
                    continue;
                }
                let first4 = &combined_codes[..4];
                let collides4 = {
                    let d = engine.active_dict().unwrap();
                    PackedCode::from_ascii(first4)
                        .map(|pc| d.has_exact(pc, false))
                        .unwrap_or(false)
                };

                // Set exact preceding sentence context!
                let prefix_ctx: String = tokens[..i].iter().map(|(t, _)| t.as_str()).collect();
                engine.clear_composition();
                engine.set_recent_context(&prefix_ctx);

                let expected = format!("{}{}", t1, t2);
                let mut out_text = String::new();
                let mut had_bell = false;
                let mut top2_matched = false;
                let mut ts2 = 5000u64;

                // Type c1 WITHOUT Space, immediately followed by c2!
                for b in combined_codes.bytes() {
                    ts2 += 60;
                    let out = engine.handle_key_event(0, Some(b as char), false, ts2);
                    out_text.push_str(&out.commit_text);
                    if out.bell {
                        had_bell = true;
                    }
                }
                // Check if expected is in top-2 candidates right before final Space!
                if engine.is_composing() {
                    let cands = engine.all_candidates();
                    if cands
                        .iter()
                        .take(2)
                        .any(|c| format!("{}{}", out_text, c.text) == expected)
                    {
                        top2_matched = true;
                    }
                    // User finishes T_{i+1} with Space (if T_{i+1} is <4 codes or still composing)
                    ts2 += 60;
                    let out = engine.handle_key_event(KEY_SPACE, Some(' '), false, ts2);
                    out_text.push_str(&out.commit_text);
                }

                let ok = out_text == expected;
                if ok {
                    top2_matched = true;
                }

                let bucket = match (c1.len(), collides4, c2.len()) {
                    (3, false, _) => &mut s3_empty,
                    (3, true, 1) => &mut s3_coll_len1,
                    (3, true, _) => &mut s3_coll_len234,
                    (2, false, _) => &mut s2_empty,
                    (2, true, _) => &mut s2_coll,
                    (1, false, _) => &mut s1_empty,
                    _ => &mut s1_coll,
                };
                bucket.total += 1;
                if ok {
                    bucket.exact_ok += 1;
                    if c1.len() == 3 && sample_ok.len() < 10 {
                        sample_ok.push(format!(
                            "[{}] ctx='{}' {}({})+{}( {}) coll={} -> '{}'",
                            schema.as_tag(),
                            prefix_ctx.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>(),
                            t1, c1, t2, c2, collides4, out_text
                        ));
                    }
                } else if c1.len() == 3 && sample_fail.len() < 10 {
                    sample_fail.push(format!(
                        "[{}] ctx='{}' {}({})+{}({}) coll={} exp='{}' got='{}' bell={}",
                        schema.as_tag(),
                        prefix_ctx.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>(),
                        t1, c1, t2, c2, collides4, expected, out_text, had_bell
                    ));
                }
                if top2_matched {
                    bucket.top2_ok += 1;
                }
                if had_bell {
                    bucket.blocked_bell += 1;
                }
            }
        }
    }

    // Latency check on normal typing
    engine.clear_composition();
    engine.clear_context();
    let t0 = Instant::now();
    let mut keys = 0usize;
    let mut ts = 1000u64;
    for &sent in SENTENCES.iter().take(40) {
        let tokens = {
            let d = engine.active_dict().unwrap();
            segment_sentence(d, sent, false)
        };
        engine.clear_composition();
        engine.clear_context();
        for (_, tcode) in &tokens {
            let _ = type_code_and_space(&mut engine, tcode, tcode.len() < 4, &mut ts);
            keys += tcode.len() + usize::from(tcode.len() < 4);
        }
    }
    let avg_us = t0.elapsed().as_secs_f64() * 1e6 / (keys.max(1) as f64);

    println!("\n=== Schema={} | missed_space_correction={} | normal avg={:.2} us/key ===", schema.as_tag(), enabled, avg_us);
    println!("{}", s3_empty.fmt_line("3-code + next (EMPTY)"));
    println!("{}", s3_coll_len1.fmt_line("3-code + 1-code (COLLIDE)"));
    println!("{}", s3_coll_len234.fmt_line("3-code + 2..4 (COLLIDE)"));
    println!("{}", s2_empty.fmt_line("2-code + next (EMPTY)"));
    println!("{}", s2_coll.fmt_line("2-code + next (COLLIDE)"));
    println!("{}", s1_empty.fmt_line("1-code + next (EMPTY)"));
    println!("{}", s1_coll.fmt_line("1-code + next (COLLIDE)"));
    println!(
        "Normal Typing Guardrail      | SentExact={}/{} ({:.2}%) | 4code_held={}/{} ({:.2}%) | SentFP={}",
        normal_sent_exact,
        normal_sent_total,
        BucketStat::pct(normal_sent_exact, normal_sent_total),
        normal_4code_held,
        normal_4code_total,
        BucketStat::pct(normal_4code_held, normal_4code_total),
        normal_4code_fp_split,
    );

    if verbose && enabled {
        println!("  -- Sample 3-code Rescued --");
        for s in &sample_ok {
            println!("    OK   {}", s);
        }
        println!("  -- Sample 3-code Missed --");
        for s in &sample_fail {
            println!("    FAIL {}", s);
        }
        if !sample_fp.is_empty() {
            println!("  -- Normal Typing FP --");
            for s in &sample_fp {
                println!("    FP   {}", s);
            }
        }
    }
}

fn neighbor_keys(layout: &KeyLayout, ch: u8) -> Vec<u8> {
    let (cx0, cy0) = layout.center_of(ch);
    let mut out = Vec::new();
    for d in 0..25u8 {
        let k = b'a' + d;
        if k == ch {
            continue;
        }
        let (cx, cy) = layout.center_of(k);
        let dx = (cx - cx0).abs();
        let dy = (cy - cy0).abs();
        if (dy < 0.1 && dx <= 1.05) || (dy <= 1.05 && dx <= 0.85) {
            out.push(k);
        }
    }
    out
}

fn eval_typo_regression(schema: InputSchema, dict_path: &PathBuf, aux_path: &PathBuf, enabled: bool) {
    let dict = DictView::open_mmap(dict_path).expect("open dict");
    let aux = AuxView::open_mmap(aux_path).expect("open aux");
    let layout = KeyLayout::default();

    let mut cfg = JuxuConfig::default();
    cfg.schema = schema;
    cfg.fourth_code_auto_commit = false;
    cfg.fifth_code_push_commit = true;
    cfg.typo.enabled = true;
    cfg.typo.missed_space_correction = enabled;
    cfg.typo.touch_hitbox_adapt = false;

    let mut engine = JuxuEngine::new(cfg);
    engine.load_dict(schema, dict);
    engine.set_aux(aux);

    let (mut tc_n, mut tc_1) = (0usize, 0usize);
    let (mut te_n, mut te_1) = (0usize, 0usize);
    let (mut pe_n, mut pe_1) = (0usize, 0usize);
    let (mut fp_n, mut fp_bad) = (0usize, 0usize);
    let mut seed = 20261119u64;
    let mut next_u64 = || {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    for &sent in SENTENCES {
        let tokens = {
            let d = engine.active_dict().unwrap();
            segment_sentence(d, sent, false)
        };
        engine.clear_composition();
        engine.clear_context();
        for (target_text, target_code) in tokens {
            let cb = target_code.as_bytes();
            let cl = cb.len();
            if cl >= 2 && cl <= 4 {
                let mut coll = Vec::new();
                let mut empty = Vec::new();
                {
                    let d = engine.active_dict().unwrap();
                    for pos in 0..cl {
                        for nb in neighbor_keys(&layout, cb[pos]) {
                            let mut mc = cb.to_vec();
                            mc[pos] = nb;
                            if let Some(pc) = PackedCode::from_bytes(&mc) {
                                let hits = d.lookup_exact(pc, false);
                                if hits.is_empty() {
                                    empty.push((pos, nb));
                                } else if hits[0].text != target_text {
                                    coll.push((pos, nb));
                                }
                            }
                        }
                    }
                }
                if !coll.is_empty() {
                    let (pos, nb) = coll[(next_u64() as usize) % coll.len()];
                    let alpha = 0.56f32;
                    engine.clear_composition();
                    for j in 0..cl {
                        let orig = cb[j];
                        let (cx, cy) = layout.center_of(orig);
                        if j == pos {
                            let (nx, ny) = layout.center_of(nb);
                            let _ = engine.push_touch_letter(nb, cx + alpha * (nx - cx), cy + alpha * (ny - cy), true, 1000 + j as u64 * 70);
                        } else {
                            let _ = engine.push_touch_letter(orig, cx, cy, true, 1000 + j as u64 * 70);
                        }
                    }
                    tc_n += 1;
                    if engine.all_candidates().first().map(|c| c.text == target_text).unwrap_or(false) {
                        tc_1 += 1;
                    }
                    // Border FP check
                    let alpha_ok = 0.42f32;
                    engine.clear_composition();
                    for j in 0..cl {
                        let orig = cb[j];
                        let (cx, cy) = layout.center_of(orig);
                        if j == pos {
                            let (nx, ny) = layout.center_of(nb);
                            let _ = engine.push_touch_letter(orig, cx + alpha_ok * (nx - cx), cy + alpha_ok * (ny - cy), true, 1000 + j as u64 * 70);
                        } else {
                            let _ = engine.push_touch_letter(orig, cx, cy, true, 1000 + j as u64 * 70);
                        }
                    }
                    fp_n += 1;
                    if !engine.all_candidates().first().map(|c| c.text == target_text).unwrap_or(false) {
                        fp_bad += 1;
                    }
                }
                if !empty.is_empty() {
                    let (pos, nb) = empty[(next_u64() as usize) % empty.len()];
                    let alpha = 0.58f32;
                    engine.clear_composition();
                    for j in 0..cl {
                        let orig = cb[j];
                        let (cx, cy) = layout.center_of(orig);
                        if j == pos {
                            let (nx, ny) = layout.center_of(nb);
                            let _ = engine.push_touch_letter(nb, cx + alpha * (nx - cx), cy + alpha * (ny - cy), true, 1000 + j as u64 * 70);
                        } else {
                            let _ = engine.push_touch_letter(orig, cx, cy, true, 1000 + j as u64 * 70);
                        }
                    }
                    te_n += 1;
                    if engine.all_candidates().first().map(|c| c.text == target_text).unwrap_or(false) {
                        te_1 += 1;
                    }
                    engine.clear_composition();
                    for j in 0..cl {
                        let ch = if j == pos { nb } else { cb[j] };
                        let _ = engine.handle_key_event(0, Some(ch as char), false, 1000 + j as u64 * 70);
                    }
                    pe_n += 1;
                    if engine.all_candidates().first().map(|c| c.text == target_text).unwrap_or(false) {
                        pe_1 += 1;
                    }
                }
            }
            engine.clear_composition();
            engine.append_recent_context(&target_text);
        }
    }
    println!(
        "  TypoRegression(ms={:<5}) | TouchColl({:3})={:5.2}% | TouchEmpty({:3})={:5.2}% | PhysEmpty({:3})={:5.2}% | BorderFP({:3})={:5.2}%",
        enabled,
        tc_n, BucketStat::pct(tc_1, tc_n),
        te_n, BucketStat::pct(te_1, te_n),
        pe_n, BucketStat::pct(pe_1, pe_n),
        fp_n, BucketStat::pct(fp_bad, fp_n),
    );
}

fn main() {
    let dir = PathBuf::from("Resources/Dicts");
    let aux = dir.join("wubi_aux.wbaux");
    let verbose = std::env::args().any(|a| a == "--verbose");

    for (schema, fname) in [
        (InputSchema::Wubi86, "wubi86.wbdict"),
        (InputSchema::Wubi98, "wubi98.wbdict"),
        (InputSchema::Wubi06, "wubi06.wbdict"),
    ] {
        let dp = dir.join(fname);
        if !dp.exists() {
            continue;
        }
        eval_schema(schema, &dp, &aux, false, false);
        eval_typo_regression(schema, &dp, &aux, false);
        eval_schema(schema, &dp, &aux, true, verbose);
        eval_typo_regression(schema, &dp, &aux, true);
    }
}
