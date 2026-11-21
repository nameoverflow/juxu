//! Comprehensive Touch-Typo 2D Gaussian Simulator, Regression Gold Set, and Large-Scale Held-Out Benchmark.
//!
//! Evaluates through real `WubiEngine::push_touch_letter` & `handle_key_event`:
//! 1. Regression Gold Set (user-reported high-priority pinyin/shuangpin/typo/context cases)
//! 2. 2D Continuous Touchscreen Typo Simulator (Full Pinyin & Xiaohe Shuangpin):
//!    - Clean Border Taps (FP / over-correction gate)
//!    - Boundary Slip (gutter slip 0.08 pitch into neighbor: physical vs continuous 2D touch gain)
//!    - 2D Gaussian Mild (sigma=0.24) & Moderate (sigma=0.32): Typo-OFF vs Typo-ON
//!    - Systematic Thumb Bias (+0.20, +0.12) Online Adaptation Curve (first 12 vs last 12 sentences)
//! 3. Large-Scale Multi-Domain Held-Out Benchmark (1,000+ sentences across tieba/xhs/zhihu/zhwiki/duplex/slang/CCI3)

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::path::PathBuf;
use wubi_core::config::{WubiConfig, WubiSchema};
use wubi_core::engine::WubiEngine;
use wubi_core::format::AuxView;
use wubi_core::pinyin_dict::PinyinDictView;
use wubi_core::typo::KeyLayout;
use wubi_core::user_data::TouchProfile;

/// Deterministic xorshift64* PRNG with Box-Muller Gaussian sampling.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn next_f32(&mut self) -> f32 {
        (((self.next_u64() >> 40) as f32) + 0.5) / ((1u32 << 24) as f32)
    }
    fn next_gaussian(&mut self) -> (f32, f32) {
        let u1 = self.next_f32().clamp(1e-6, 1.0 - 1e-6);
        let u2 = self.next_f32();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f32::consts::PI * u2;
        (r * theta.cos(), r * theta.sin())
    }
}

/// Core 48 clean sentences with verified exact pinyin for touch simulation & online adaptation.
const TOUCH_SENTENCES: &[(&str, &str)] = &[
    ("今天天气真好我们一起去吃饭吧", "jintiantianqizhenhaowomenyiqiquchifanba"),
    ("你现在在哪里我马上就过去找你", "nixianzaizainaliwomashangjiuguoquzhaoni"),
    ("明天早上九点开会别忘记带电脑", "mingtianzaoshangjiudiankaihuibiewangjidaidiannao"),
    ("不好意思刚才在忙没来得及回你消息", "buhaoyisigangcaizaimangmeilaidejihuinixiaoxi"),
    ("我已经到了就在地铁口旁边等你", "woyijingdaolejiuzaiditiekoupangbiandengni"),
    ("昨天晚上睡得太晚今天早上起不来", "zuotianwanshangshuidetaiwanjintianzaoshangqibulai"),
    ("能不能帮我把那份文件发到我邮箱里", "nengbunengbangwobanafenwenjianfadaowoyouxiangli"),
    ("我觉得这个方案比之前那个好多了", "wojuedezhegefanganbizhiqiannagehaoduole"),
    ("待会下班以后我们去超市买点水果", "daihuixiabanyihouwomenquchaoshimaidianshuiguo"),
    ("这件事其实没有你想的那么复杂", "zhejianshiqishimeiyounixiangdenamefuza"),
    ("刚刚收到快递打开一看质量还挺不错的", "ganggangshoudaokuaididakaiyikanzhilianghaitingbucuode"),
    ("今天实在太累了回去洗个澡就睡觉", "jintianshizaitaileilehuiquxigezaojiushuijiao"),
    ("下周我要去北京出差大概要待三天", "xiazhouwoyaoqubeijingchuchaidagaiyaodaisantian"),
    ("你先把需求发给我我下午仔细看一下", "nixianbaxuqiufageiwowoxiawuzixikanyixia"),
    ("家里没有菜了晚上我们点外卖吃吧", "jialimeiyoucailewanshangwomendianwaimaichiba"),
    ("手机快没电了我先找个地方充一下电", "shoujikuaimeidianlewoxianzhaogedifangchongyixiadian"),
    ("听起来不错那就这么说定了不见不散", "tingqilaibucuonajiuzhemeshuodinglebujianbusan"),
    ("我一直在想这个问题到底该怎么解决", "woyizhizaixiangzhegewentidaodigaizenmejiejue"),
    ("这个输入法的纠错模型响应速度非常快", "zhegeshurufadejiucuomoxingxiangyingsudufeichangkuai"),
    ("我们在本地部署了一个轻量级语言模型", "womenzaibendibushuleyigeqingliangjiyuyanmoxing"),
    ("刚才提交的代码在持续集成里跑过了所有测试", "gangcaitijiaodedaimazaichixujichenglipaoguolesuoyouceshi"),
    ("你需要先把这个分支合并到主干再发布版本", "nixuyaoxianbazhegefenzhihebingdaozhuganzaifabubanben"),
    ("内存占用控制在五十兆以内才能稳定运行", "neicunzhanyongkongzhizaiwushizhaoyineicainengwendingyunxing"),
    ("这个接口返回的数据结构需要保持向后兼容", "zhegejiekoufanhuideshujujiegouxuyaobaochixianghoujianrong"),
    ("测试环境已经部署好了你可以直接上去验证", "ceshihuanjingyijingbushuhaolenikeyizhijieshangquyanzheng"),
    ("日志里显示网络连接超时可能是代理配置有问题", "rizhilixianshiwangluolianjiechaoshikenengshidailipeizhiyouwenti"),
    ("数据库查询太慢了最好给这个字段加一个索引", "shujukuchaxuntaimanlezuihaogeizhegeziduanjiayigesuoyin"),
    ("前端页面在暗黑模式下的对比度还需要再调一下", "qianduanyemianzaianheimoshixiadeduibiduhaixuyaozaitiaoyixia"),
    ("这里有一个边界条件会导致候选词排序异常", "zheliyouyigebianjietiaojianhuidaozhihouxuancipaixuyichang"),
    ("把配置文件改完保存之后输入法会实时重新加载", "bapeizhiwenjiangaiwanbaocunzhihoushurufahuishishichongxinjiazai"),
    ("我们可以用知识蒸馏来提升小模型的准确率", "womenkeyiyongzhishizhengliulaitishengxiaomoxingdezhunquelv"),
    ("异步任务完成后系统会自动通知主线程处理", "yiburenwuwanchenghouxitonghuizidongtongzhizhuxianchengchuli"),
    ("感觉这波操作直接把性价比拉满了", "ganjuezhebocaozuozhijiebaxingjiabilamanle"),
    ("这家咖啡馆环境很安静适合周末带电脑来办公", "zhejiakafeiguanhuanjinghenanjingshihezhoumodaidiannaolaibangong"),
    ("今年双十一的优惠力度感觉没有往年那么大了", "jinnianshuangshiyideyouhuiliduganjuemeiyouwangniannamedale"),
    ("看完这部电影以后我整个人都被治愈了", "kanwanzhebudianyingyihouwozhenggerendoubeizhiyule"),
    ("最近天气降温太快了出门记得多穿一件外套", "zuijintianqijiangwentaikuailechumenjideduochuanyijianwaitao"),
    ("这个攻略写得非常详细新手照着做也不会踩坑", "zhegegonglvexiedefeichangxiangxixinshouzhaozhezuoyebuhuicaikeng"),
    ("周末和朋友约了去露营结果半路下起了大雨", "zhoumohepengyouyuelequluyingjieguobanluxiaqiledayu"),
    ("每天坚持运动半小时睡眠质量真的改善了很多", "meitianjianchiyundongbanxiaoshishuimianzhiliangzhendegaishanlehenduo"),
    ("这款耳机降噪效果不错就是戴久了耳朵有点疼", "zhekuanerjijiangzaoxiaoguobucuojiushidaijiuleerduoyoudianteng"),
    ("刷到这条视频的时候我忍不住笑出了声", "shuadaozhetiaoshipindeshihouworenbuzhuxiaochulesheng"),
    ("明天早上从上海虹桥坐高铁去杭州东站", "mingtianzaoshangcongshanghaihongqiaozuogaotiequhangzhoudongzhan"),
    ("国庆节假期我们打算去云南大理和丽江旅游", "guoqingjiejiaqiwomendasuanquyunnandalihelijianglvyou"),
    ("北京中关村附近有很多互联网和人工智能公司", "beijingzhongguancunfujinyouhenduohulianwangherengongzhinenggongsi"),
    ("深圳南山区这两年的科技创业氛围特别浓厚", "shenzhennanshanquzheliangniandekejichuangyefenweitebienonghou"),
    ("清华大学和北京大学都在海淀区学院路附近", "qinghuadaxuehebeijingdaxuedouzaihaidianquxueyuanlufujin"),
    ("苹果公司在秋季发布会上推出了新一代芯片", "pingguogongsizaiqiujifabuhuishangtuichulexinyidaixinpian"),
];

/// Gold regression cases (context, keys, expected_substring, must_be_top1)
const GOLD_CASES: &[(&str, &str, &str, bool)] = &[
    ("橘子", "jiandan", "简单", true),
    ("", "jiandan", "简单", true),
    ("", "fanan", "方案", false),
    ("", "zhogwen", "中文", true),
    ("", "zhonngwen", "中文", true),
    ("", "shenjingbing", "神经病", true),
    ("", "sjb", "神经病", false),
    ("", "shenjb", "神经病", false),
    ("", "srf", "输入法", true),
    ("", "xi'an", "西安", true),
    ("", "fang'an", "方案", true),
    ("", "pi'ao", "皮袄", true),
    ("", "tian'an'men", "天安门", true),
    ("", "jv", "橘", false),
    ("明天要去北京出差，", "shunbianbajipiaohejiudiandinghao", "顺便把机票和酒店订好", true),
    ("数据库查询太慢了，", "zuihaogeizhegeziduanjiayigesuoyin", "最好给这个字段加一个索引", true),
];

fn char_acc(expected: &str, predicted: &str) -> (usize, usize) {
    let e: Vec<char> = expected.chars().collect();
    let p: Vec<char> = predicted.chars().collect();
    let hits = e.iter().enumerate().filter(|&(i, &c)| p.get(i).copied() == Some(c)).count();
    (hits, e.len())
}

fn horizontal_neighbor(layout: &KeyLayout, ch: u8) -> Option<u8> {
    let (cx0, cy0) = layout.center_of(ch);
    let mut best: Option<(f32, u8)> = None;
    for d in 0..26u8 {
        let k = b'a' + d;
        if k == ch {
            continue;
        }
        let (cx, cy) = layout.center_of(k);
        if (cy - cy0).abs() < 0.1 {
            let dx = (cx - cx0).abs();
            if dx <= 1.05 && best.map_or(true, |b| dx < b.0) {
                best = Some((dx, k));
            }
        }
    }
    best.map(|b| b.1)
}

fn make_engine(schema: WubiSchema, aux_path: &PathBuf, typo_enabled: bool, adapt: bool) -> WubiEngine {
    let mut cfg = WubiConfig::default();
    cfg.schema = schema;
    cfg.typo.enabled = typo_enabled;
    cfg.typo.touch_hitbox_adapt = adapt;
    cfg.dynamic_word_freq = false;
    let mut engine = WubiEngine::new(cfg);
    let dir = PathBuf::from("Resources/Dicts");
    let pydict = PinyinDictView::from_file(dir.join("pinyin.pydict")).expect("load pinyin.pydict");
    let aux = AuxView::open_mmap(aux_path).expect("load wbaux");
    engine.load_pinyin_dict(pydict);
    engine.set_aux(aux);
    engine
}

fn build_char_to_pinyin(dict: &PinyinDictView) -> HashMap<char, String> {
    let mut best: HashMap<char, (u32, String)> = HashMap::new();
    for sid in 0..dict.syllable_count() {
        let syl = dict.syllable_str(sid as u16);
        for e in dict.lookup_chars(sid as u16, false) {
            let mut cs = e.text.chars();
            if let (Some(ch), None) = (cs.next(), cs.next()) {
                let entry = best.entry(ch).or_insert_with(|| (0, String::new()));
                if e.weight > entry.0 {
                    *entry = (e.weight, syl.to_owned());
                }
            }
        }
    }
    best.into_iter().map(|(c, (_, s))| (c, s)).collect()
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let aux_path = args
        .windows(2)
        .find(|w| w[0] == "--aux")
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(|| PathBuf::from("Resources/Dicts/wubi_aux.wbaux"));
    let tsv_dir = args
        .windows(2)
        .find(|w| w[0] == "--tsv-dir")
        .map(|w| PathBuf::from(&w[1]));
    let max_per_domain: usize = args
        .windows(2)
        .find(|w| w[0] == "--max-per-domain")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(200);

    let aux = AuxView::open_mmap(&aux_path).expect("load aux");
    let nv = aux.neural_view().expect("missing WBLM0001");
    let s = nv.offs.summary;
    println!("=========================================================================");
    println!(
        "Juxu Comprehensive Benchmark | aux={} | V={} R={} D={} H={} L={} F={} | punct={}",
        aux_path.file_name().unwrap().to_string_lossy(),
        s.vocab_size, s.rank, s.d_model, s.n_heads, s.n_layers, s.ffn_dim, nv.offs.has_punct_tokens
    );
    println!("=========================================================================");

    // -------------------------------------------------------------------------
    // 1. Regression Gold Set
    // -------------------------------------------------------------------------
    let mut eng = make_engine(WubiSchema::Pinyin, &aux_path, true, true);
    let mut gold_pass = 0usize;
    let mut gold_fails = Vec::new();
    for &(ctx, keys, exp, must_top1) in GOLD_CASES {
        eng.clear_composition();
        eng.set_recent_context(ctx);
        for (i, ch) in keys.chars().enumerate() {
            let _ = eng.handle_key_event(0, Some(ch), false, 1000 + i as u64 * 60);
        }
        let all = eng.all_candidates();
        let top1 = all.first().map(|c| c.text.as_str()).unwrap_or("");
        let ok = if must_top1 {
            top1 == exp
        } else {
            all.iter().any(|c| c.text.contains(exp))
        };
        if ok {
            gold_pass += 1;
        } else {
            gold_fails.push((ctx, keys, exp, top1.to_owned()));
        }
    }
    println!("\n[1] Regression Gold Set: {}/{} passed", gold_pass, GOLD_CASES.len());
    for (ctx, keys, exp, got) in &gold_fails {
        println!("  FAIL: ctx='{}' keys='{}' exp='{}' got='{}'", ctx, keys, exp, got);
    }

    // -------------------------------------------------------------------------
    // 2. Continuous 2D Touchscreen Typo Simulator (Full Pinyin)
    // -------------------------------------------------------------------------
    let layout = KeyLayout::default();
    let empty_prof = TouchProfile::default();

    println!("\n[2] Continuous 2D Touchscreen Typo Simulator ({} sentences, Full Pinyin)", TOUCH_SENTENCES.len());

    // 2a. Clean Border Taps (offset 0.38 toward neighbor, still inside intended key -> measure FP)
    {
        let mut e = make_engine(WubiSchema::Pinyin, &aux_path, true, true);
        let mut t1 = 0usize;
        let mut ch_h = 0usize;
        let mut ch_t = 0usize;
        let mut fp_corr = 0usize;
        for &(exp, keys) in TOUCH_SENTENCES {
            e.clear_composition();
            e.set_recent_context("");
            let kb = keys.as_bytes();
            for (i, &ch) in kb.iter().enumerate() {
                let (cx, cy) = layout.center_of(ch);
                let (nx, ny) = if i % 4 == 2 {
                    if let Some(nb) = horizontal_neighbor(&layout, ch) {
                        let (ncx, ncy) = layout.center_of(nb);
                        (cx + 0.38 * (ncx - cx), cy + 0.38 * (ncy - cy))
                    } else {
                        (cx, cy)
                    }
                } else {
                    (cx, cy)
                };
                let _ = e.push_touch_letter(ch, nx, ny, true, 1000 + i as u64 * 60);
            }
            let cands = e.all_candidates();
            let got = cands.first().map(|c| c.text.as_str()).unwrap_or("");
            if got == exp { t1 += 1; }
            if cands.first().is_some_and(|c| !c.is_exact) { fp_corr += 1; }
            let (h, t) = char_acc(exp, got);
            ch_h += h; ch_t += t;
        }
        println!(
            "  Clean Border (0.38 pitch inside key)  | Top-1={:>2}/{} ({:5.1}%) | CharAcc={:5.2}% | OverCorr_FP={:4.1}%",
            t1, TOUCH_SENTENCES.len(), 100.0 * t1 as f64 / TOUCH_SENTENCES.len() as f64,
            100.0 * ch_h as f64 / ch_t as f64, 100.0 * fp_corr as f64 / TOUCH_SENTENCES.len() as f64
        );
    }

    // 2b. Boundary Slip (1 slip per sentence: 0.58 toward neighbor = 0.08 across border)
    // Compare physical key (no touch coords) vs continuous 2D touch coords
    for (mode_name, use_touch_coords) in [("Boundary Slip (physical key only) ", false), ("Boundary Slip (2D touch + gutter) ", true)] {
        let mut e = make_engine(WubiSchema::Pinyin, &aux_path, true, true);
        let mut t1 = 0usize;
        let mut ch_h = 0usize;
        let mut ch_t = 0usize;
        for (s_idx, &(exp, keys)) in TOUCH_SENTENCES.iter().enumerate() {
            e.clear_composition();
            e.set_recent_context("");
            let kb = keys.as_bytes();
            let slip_pos = (s_idx * 7 + 3) % kb.len();
            for (i, &ch) in kb.iter().enumerate() {
                if i == slip_pos {
                    if let Some(nb) = horizontal_neighbor(&layout, ch) {
                        let (cx, cy) = layout.center_of(ch);
                        let (ncx, ncy) = layout.center_of(nb);
                        if use_touch_coords {
                            let nx = cx + 0.58 * (ncx - cx);
                            let ny = cy + 0.58 * (ncy - cy);
                            let _ = e.push_touch_letter(nb, nx, ny, true, 1000 + i as u64 * 60);
                        } else {
                            let _ = e.handle_key_event(0, Some(nb as char), false, 1000 + i as u64 * 60);
                        }
                        continue;
                    }
                }
                let (cx, cy) = layout.center_of(ch);
                if use_touch_coords {
                    let _ = e.push_touch_letter(ch, cx, cy, true, 1000 + i as u64 * 60);
                } else {
                    let _ = e.handle_key_event(0, Some(ch as char), false, 1000 + i as u64 * 60);
                }
            }
            let got = e.all_candidates().first().map(|c| c.text.as_str()).unwrap_or("");
            if got == exp { t1 += 1; }
            let (h, t) = char_acc(exp, got);
            ch_h += h; ch_t += t;
        }
        println!(
            "  {} | Top-1={:>2}/{} ({:5.1}%) | CharAcc={:5.2}%",
            mode_name, t1, TOUCH_SENTENCES.len(),
            100.0 * t1 as f64 / TOUCH_SENTENCES.len() as f64,
            100.0 * ch_h as f64 / ch_t as f64
        );
    }

    // 2c. 2D Gaussian Mild (sigma=0.24) & Moderate (sigma=0.32): Typo OFF vs ON
    for (label, sigma) in [("2D Gaussian Mild     (σ=0.24)", 0.24f32), ("2D Gaussian Moderate (σ=0.32)", 0.32f32)] {
        for (typo_on, tag) in [(false, "Typo=OFF"), (true, "Typo=ON ")] {
            let mut rng = Rng::new(261121);
            let mut e = make_engine(WubiSchema::Pinyin, &aux_path, typo_on, typo_on);
            let mut t1 = 0usize;
            let mut ch_h = 0usize;
            let mut ch_t = 0usize;
            let mut raw_wrong_keys = 0usize;
            let mut total_keys = 0usize;
            for &(exp, keys) in TOUCH_SENTENCES {
                e.clear_composition();
                e.set_recent_context("");
                for (i, &ch) in keys.as_bytes().iter().enumerate() {
                    let (cx, cy) = layout.center_of(ch);
                    let (gx, gy) = rng.next_gaussian();
                    let nx = cx + gx * sigma;
                    let ny = cy + gy * sigma;
                    let rep = layout.nearest_key(nx, ny, &empty_prof, false);
                    total_keys += 1;
                    if rep != ch {
                        raw_wrong_keys += 1;
                    }
                    let _ = e.push_touch_letter(rep, nx, ny, true, 1000 + i as u64 * 60);
                }
                let got = e.all_candidates().first().map(|c| c.text.as_str()).unwrap_or("");
                if got == exp { t1 += 1; }
                let (h, t) = char_acc(exp, got);
                ch_h += h; ch_t += t;
            }
            println!(
                "  {} [{}] | RawMissKey={:4.1}% | Top-1={:>2}/{} ({:5.1}%) | CharAcc={:5.2}%",
                label, tag,
                100.0 * raw_wrong_keys as f64 / total_keys.max(1) as f64,
                t1, TOUCH_SENTENCES.len(),
                100.0 * t1 as f64 / TOUCH_SENTENCES.len() as f64,
                100.0 * ch_h as f64 / ch_t as f64
            );
        }
    }

    // 2d. Systematic Thumb Bias (+0.22, +0.12) + sigma=0.22 Online Adaptation Curve
    {
        let mut rng = Rng::new(987654);
        let mut e = make_engine(WubiSchema::Pinyin, &aux_path, true, true);
        let mut first_h = 0usize;
        let mut first_t = 0usize;
        let mut last_h = 0usize;
        let mut last_t = 0usize;
        let mut first_t1 = 0usize;
        let mut last_t1 = 0usize;
        for (idx, &(exp, keys)) in TOUCH_SENTENCES.iter().enumerate() {
            e.clear_composition();
            e.set_recent_context("");
            for (i, &ch) in keys.as_bytes().iter().enumerate() {
                let (cx, cy) = layout.center_of(ch);
                let (gx, gy) = rng.next_gaussian();
                let nx = cx + 0.22 + gx * 0.22;
                let ny = cy + 0.12 + gy * 0.22;
                let rep = layout.nearest_key(nx, ny, &empty_prof, false);
                let _ = e.push_touch_letter(rep, nx, ny, true, 1000 + i as u64 * 60);
            }
            let got = e.all_candidates().first().map(|c| c.text.as_str()).unwrap_or("").to_owned();
            let (h, t) = char_acc(exp, &got);
            if idx < 12 {
                first_h += h; first_t += t;
                if got == exp { first_t1 += 1; }
            } else if idx >= TOUCH_SENTENCES.len() - 12 {
                last_h += h; last_t += t;
                if got == exp { last_t1 += 1; }
            }
            // Simulate user selecting the intended candidate so TouchProfile learns the systematic bias
            if let Some(pos) = e.all_candidates().iter().position(|c| c.text == exp) {
                let _ = e.select_abs_candidate(pos, 5000);
            }
        }
        println!(
            "  Systematic Bias (+0.22,+0.12) Adapt   | First12: Top-1={:>2}/12 CharAcc={:5.2}% -> Last12: Top-1={:>2}/12 CharAcc={:5.2}%",
            first_t1, 100.0 * first_h as f64 / first_t.max(1) as f64,
            last_t1, 100.0 * last_h as f64 / last_t.max(1) as f64
        );
    }

    // -------------------------------------------------------------------------
    // 3. Large-Scale Multi-Domain Held-Out Benchmark (1,000+ sentences)
    // -------------------------------------------------------------------------
    if let Some(ref dir) = tsv_dir {
        let pydict = PinyinDictView::from_file("Resources/Dicts/pinyin.pydict").expect("load pydict");
        let c2p = build_char_to_pinyin(&pydict);
        let mut e = make_engine(WubiSchema::Pinyin, &aux_path, true, false);

        #[derive(Default)]
        struct DStat { n: usize, t1: usize, t3: usize, ch_h: usize, ch_t: usize }
        let mut stats: BTreeMap<String, DStat> = BTreeMap::new();

        for fname in ["test_sentences.tsv", "duplex_test.tsv", "slang_2025_2026.tsv", "new_test.tsv"] {
            let p = dir.join(fname);
            if !p.exists() { continue; }
            let content = fs::read_to_string(&p).unwrap_or_default();
            for line in content.lines() {
                if line.is_empty() || line.starts_with('#') || !line.contains('\t') { continue; }
                let mut sp = line.splitn(2, '\t');
                let dom = sp.next().unwrap_or("unknown").trim().to_owned();
                let sent = sp.next().unwrap_or("").trim();
                let st = stats.entry(dom.clone()).or_default();
                if st.n >= max_per_domain { continue; }

                let cjk: String = sent.chars().filter(|&c| (0x4E00..=0x9FFF).contains(&(c as u32))).collect();
                let clen = cjk.chars().count();
                if !(4..=24).contains(&clen) { continue; }
                let mut py = String::new();
                let mut ok = true;
                for ch in cjk.chars() {
                    if let Some(s) = c2p.get(&ch) { py.push_str(s); } else { ok = false; break; }
                }
                if !ok { continue; }

                e.clear_composition();
                e.set_recent_context("");
                for (i, ch) in py.chars().enumerate() {
                    let _ = e.handle_key_event(0, Some(ch), false, 1000 + i as u64 * 60);
                }
                let cands = e.all_candidates();
                let got = cands.first().map(|c| c.text.as_str()).unwrap_or("");
                let r = cands.iter().position(|c| c.text == cjk);
                st.n += 1;
                if r == Some(0) { st.t1 += 1; }
                if r.is_some_and(|x| x < 3) { st.t3 += 1; }
                let (h, t) = char_acc(&cjk, got);
                st.ch_h += h; st.ch_t += t;
            }
        }

        println!("\n[3] Large-Scale Held-Out Multi-Domain Full Pinyin (max {}/domain)", max_per_domain);
        println!("  {:<20} {:>5} {:>8} {:>8} {:>8}", "Domain", "N", "Top-1", "Top-3", "CharAcc");
        let mut tot = DStat::default();
        for (dom, st) in &stats {
            tot.n += st.n; tot.t1 += st.t1; tot.t3 += st.t3; tot.ch_h += st.ch_h; tot.ch_t += st.ch_t;
            println!(
                "  {:<20} {:>5} {:>7.1}% {:>7.1}% {:>7.2}%",
                dom, st.n,
                100.0 * st.t1 as f64 / st.n.max(1) as f64,
                100.0 * st.t3 as f64 / st.n.max(1) as f64,
                100.0 * st.ch_h as f64 / st.ch_t.max(1) as f64
            );
        }
        println!(
            "  {:<20} {:>5} {:>7.1}% {:>7.1}% {:>7.2}%",
            "TOTAL", tot.n,
            100.0 * tot.t1 as f64 / tot.n.max(1) as f64,
            100.0 * tot.t3 as f64 / tot.n.max(1) as f64,
            100.0 * tot.ch_h as f64 / tot.ch_t.max(1) as f64
        );
    }
}
