//! Comprehensive end-to-end Full Pinyin + Cross-Clause RWKV evaluation suite.
//!
//! Covers 3 evaluation layers:
//! - L1 (Model calibration & cross-clause NLL): exact softmax NLL vs self-normalized `|logZ|` drift,
//!   and real punctuated cross-clause context gain (`with_punct_ctx` vs `cold_bos`).
//! - L2 (End-to-end Rust decoder): multi-domain full pinyin sentences, initial abbreviations & apostrophes,
//!   4 typo classes (missing -g, transposition, double-tap insertion, physical neighbor) + FP rate,
//!   plus automatic error attribution (`lm_rank_miss` vs `beam_or_dict_miss`).
//! - L3 (Latency & size budget): `step_token` µs, full-sentence `p50/p95/p99` ms, `.wbaux` size.

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use wubi_core::config::WubiConfig;
use wubi_core::format::AuxView;
use wubi_core::neural::{NeuralContextTracker, NeuralView};
use wubi_core::pinyin::{decode_pinyin, PinyinDecoder, PinyinStream};
use wubi_core::pinyin_dict::PinyinDictView;
use wubi_core::typo::{KeyLayout, KeyObs};
use wubi_core::user_data::UserStore;

/// (domain, expected_chinese, exact_pinyin_keys)
const FULL_SENTENCES: &[(&str, &str, &str)] = &[
    // --- Daily IM & chat ---
    ("im_chat", "今天天气真好我们一起去吃饭吧", "jintiantianqizhenhaowomenyiqiquchifanba"),
    ("im_chat", "你现在在哪里我马上就过去找你", "nixianzaizainaliwomashangjiuguoquzhaoni"),
    ("im_chat", "明天早上九点开会别忘记带电脑", "mingtianzaoshangjiudiankaihuibiewangjidaidiannao"),
    ("im_chat", "不好意思刚才在忙没来得及回你消息", "buhaoyisigangcaizaimangmeilaidejihuiniXiaoxi"),
    ("im_chat", "我已经到了就在地铁口旁边等你", "woyijingdaolejiuzaiditiekoupangbiandengni"),
    ("im_chat", "昨天晚上睡得太晚今天早上起不来", "zuotianwanshangshuidetaiwanjintianzaoshangqibulai"),
    ("im_chat", "能不能帮我把那份文件发到我邮箱里", "nengbunengbangwobanafenwenjianfadaowoyouxiangli"),
    ("im_chat", "我觉得这个方案比之前那个好多了", "wojuedezhegefanganbizhiqiannagehaoduole"),
    ("im_chat", "待会下班以后我们去超市买点水果", "daihuixiabanyihouwomenquchaoshimaidianshuiguo"),
    ("im_chat", "这件事其实没有你想的那么复杂", "zhejianshiqishimeiyounixiangdenamefuza"),
    ("im_chat", "刚刚收到快递打开一看质量还挺不错的", "ganggangshoudaokuaididakaiyikanzhilianghaitingbucuode"),
    ("im_chat", "今天实在太累了回去洗个澡就睡觉", "jintianshizaitaileilehuiquxigezaojiushuijiao"),
    ("im_chat", "下周我要去北京出差大概要待三天", "xiazhouwoyaoqubeijingchuchaidagaiyaodaisantian"),
    ("im_chat", "你先把需求发给我我下午仔细看一下", "nixianbaxuqiufageiwowoxiawuzixikanyixia"),
    ("im_chat", "家里没有菜了晚上我们点外卖吃吧", "jialimeiyoucailewanshangwomendianwaimaichiba"),
    ("im_chat", "手机快没电了我先找个地方充一下电", "shoujikuaimeidianlewoxianzhaogedifangchongyixiadian"),
    ("im_chat", "听起来不错那就这么说定了不见不散", "tingqilaibucuonajiuzhemeshuodinglebujianbusan"),
    ("im_chat", "我一直在想这个问题到底该怎么解决", "woyizhizaixiangzhegewentidaodigaizenmejiejue"),
    // --- Tech & work ---
    ("tech_work", "这个输入法的纠错模型响应速度非常快", "zhegeshurufadejiucuomoxingxiangyingsudufeichangkuai"),
    ("tech_work", "我们在本地部署了一个轻量级语言模型", "womenzaibendibushuleyigeqingliangjiyuyanmoxing"),
    ("tech_work", "刚才提交的代码在持续集成里跑过了所有测试", "gangcaitijiaodedaimazaichixujichenglipaoguolesuoyouceshi"),
    ("tech_work", "你需要先把这个分支合并到主干再发布版本", "nixuyaoxianbazhegefenzhihebingdaozhuganzaifabubanben"),
    ("tech_work", "内存占用控制在五十兆以内才能稳定运行", "neicunzhanyongkongzhizaiwushizhaoyineicainengwendingyunxing"),
    ("tech_work", "这个接口返回的数据结构需要保持向后兼容", "zhegejiekoufanhuideshujujiegouxuyaobaochixianghoujianrong"),
    ("tech_work", "测试环境已经部署好了你可以直接上去验证", "ceshihuanjingyijingbushuhaolenikeyizhijieshangquyanzheng"),
    ("tech_work", "日志里显示网络连接超时可能是代理配置有问题", "rizhilixianshiwangluolianjiechaoshikenengshidailipeizhiyouwenti"),
    ("tech_work", "数据库查询太慢了最好给这个字段加一个索引", "shujukuchaxuntaimanlezuihaogeizhegeziduanjiayigesuoyin"),
    ("tech_work", "前端页面在暗黑模式下的对比度还需要再调一下", concat!("qianduanyemianzaianheimoshixiade", "duibidu", "haixuyaozaitiaoyixia")),
    ("tech_work", "这里有一个边界条件会导致候选词排序异常", "zheliyouyigebianjietiaojianhuidaozhihouxuancipaixuyichang"),
    ("tech_work", "把配置文件改完保存之后输入法会实时重新加载", "bapeizhiwenjiangaiwanbaocunzhihoushurufahuishishichongxinjiazai"),
    ("tech_work", "我们可以用知识蒸馏来提升小模型的准确率", "womenkeyiyongzhishizhengliulaitishengxiaomoxingdezhunquelv"),
    ("tech_work", "异步任务完成后系统会自动通知主线程处理", "yiburenwuwanchenghouxitonghuizidongtongzhizhuxianchengchuli"),
    // --- Social & slang ---
    ("slang_social", "感觉这波操作直接把性价比拉满了", "ganjuezhebocaozuozhijiebaxingjiabilamanle"),
    ("slang_social", "这家咖啡馆环境很安静适合周末带电脑来办公", "zhejiakafeiguanhuanjinghenanjingshihezhoumodaidiannaolaibangong"),
    ("slang_social", "今年双十一的优惠力度感觉没有往年那么大了", "jinnianshuangshiyideyouhuiliduganjuemeiyouwangniannamedale"),
    ("slang_social", "看完这部电影以后我整个人都被治愈了", "kanwanzhebudianyingyihouwozhenggerendoubeizhiyule"),
    ("slang_social", "最近天气降温太快了出门记得多穿一件外套", "zuijintianqijiangwentaikuailechumenjideduochuanyijianwaitao"),
    ("slang_social", "这个攻略写得非常详细新手照着做也不会踩坑", "zhegegonglvexiedefeichangxiangxixinshouzhaozhezuoyebuhuicaikeng"),
    ("slang_social", "周末和朋友约了去露营结果半路下起了大雨", "zhoumohepengyouyuelequluyingjieguobanluxiaqiledayu"),
    ("slang_social", "每天坚持运动半小时睡眠质量真的改善了很多", "meitianjianchiyundongbanxiaoshishuimianzhiliangzhendegaishanlehenduo"),
    ("slang_social", "这款耳机降噪效果不错就是戴久了耳朵有点疼", "zhekuanerjijiangzaoxiaoguobucuojiushidaijiuleerduoyoudianteng"),
    ("slang_social", "刷到这条视频的时候我忍不住笑出了声", "shuadaozhetiaoshipindeshihouworenbuzhuxiaochulesheng"),
    // --- Entities & proper nouns ---
    ("entities", "明天早上从上海虹桥坐高铁去杭州东站", "mingtianzaoshangcongshanghaihongqiaozuogaotiequhangzhoudongzhan"),
    ("entities", "国庆节假期我们打算去云南大理和丽江旅游", "guoqingjiejiaqiwomendasuanquyunnandalihelijianglvyou"),
    ("entities", "北京中关村附近有很多互联网和人工智能公司", "beijingzhongguancunfujinyouhenduohulianwangherengongzhinenggongsi"),
    ("entities", "深圳南山区这两年的科技创业氛围特别浓厚", "shenzhennanshanquzheliangniandekejichuangyefenweitebienonghou"),
    ("entities", "我准备在周末去国家博物馆看最新的历史展览", "wozhunbeizaizhoumoquguojiabowuguankanzuixindelishizhanlan"),
    ("entities", "清华大学和北京大学都在海淀区学院路附近", "qinghuadaxuehebeijingdaxuedouzaihaidianquxueyuanlufujin"),
    ("entities", "从广州白云机场飞往成都天府机场大约两小时", "congguangzhoubaiyunjichangfeiwangchengdutianfujichangdayueliangxiaoshi"),
    ("entities", "长江三角洲和珠江三角洲是中国经济最活跃的地区", "changjiangsanjiaozhouhezhujiangsanjiaozhoushizhongguojingjizuihuoyuedediqu"),
    ("entities", "故宫博物院每天的门票都需要提前在网上预约", "gugongbowuyuanmeitiandemenpiaodouxuyaotiqianzaiwangshangyuyue"),
    ("entities", "苹果公司在秋季发布会上推出了新一代芯片", "pingguogongsizaiqiujifabuhuishangtuichulexinyidaixinpian"),
    // --- High-ambiguity homophones & polyphones ---
    ("homophone_hard", "他决定在会议上正式提出这个方案", "tajuedingzaihuiyishangzhengshitichuzhegefangan"),
    ("homophone_hard", "我们需要把这件事的优先级再提高一点", "womenxuyaobazhejianshideyouxianjizaitigaoyidian"),
    ("homophone_hard", "银行工作人员提醒大家注意防范电信诈骗", "yinhanggongzuorenyuantixingdajiazhuyifangfandianxinzhapian"),
    ("homophone_hard", "他在长途旅行后终于和老朋友重逢了", "tazaichangtulvxinghouzhongyuhelaopengyouchongfengle"),
    ("homophone_hard", "这件事非常重要请大家务必认真对待", "zhejianshifeichangzhongyaoqingdajiawubirenzhenduidai"),
    ("homophone_hard", "事实证明他们当时的判断是完全正确的", "shishizhengmingtamendangshidepanduanshiwanquanzhengquede"),
    ("homophone_hard", "请帮我确认一下明天的日程安排有没有冲突", "qingbangwoquerenyixiamingtianderichenganpaiyoumeiyouchongtu"),
    ("homophone_hard", "他以优异的成绩考入了理想的大学", "tayiyouyidechengjikaorulelixiangdedaxue"),
    ("homophone_hard", "为了保证数据安全所有密码都必须加密存储", "weilebaozhengshujuanquansuoyoumimadoubixujiamicunchu"),
    ("homophone_hard", "这篇论文的实验结果非常具有说服力", "zhepianlunwendeshiyanjieguofeichangjuyoushuofuli"),
];

/// Cross-clause punctuated context test pairs: (preceding_context_with_punct, target_clause, pinyin)
const CROSS_CLAUSE_CASES: &[(&str, &str, &str)] = &[
    ("明天要去北京出差，", "顺便把机票和酒店订好", "shunbianbajipiaohejiudiandinghao"),
    ("手机快没电了，", "先找个地方充一下电", "xianzhaogedifangchongyixiadian"),
    ("数据库查询太慢了，", "最好给这个字段加一个索引", "zuihaogeizhegeziduanjiayigesuoyin"),
    ("刚才提交的代码报错了，", "我看一下持续集成的日志", "wokanyixiachixujichengderizhi"),
    ("外面风有点大，", "麻烦你把窗户关一下", "mafannibachuanghuguanyixia"),
    ("昨晚睡得太晚了，", "今天早上根本起不来", "jintianzaoshanggenbenqibulai"),
    ("家里没有菜了，", "晚上我们点外卖吃吧", "wanshangwomendianwaimaichiba"),
    ("今天实在太累了，", "回去洗个澡就早点睡觉", "huiquxigezaojiuzaodianshuijiao"),
    ("周末去爬山吧？", "记得带上水和防晒霜", "jidedaishangshuihefangshaishuang"),
    ("这个方案有风险。", "我们需要准备一套备选方案", "womenxuyaozhunbeiyitaobeixuanfangan"),
    ("会议推迟到下午三点了，", "记得通知一下其他同事", "jidetongzhiyixiaqitatongshi"),
    ("快递已经送到驿站了，", "下班路上顺便去取一下", "xiabanlushangshunbianququyixia"),
    ("天气预报说下午会下雨，", "出门的时候别忘了带伞", "chumendeshihoubiewangledaisan"),
    ("这家店的咖啡味道不错，", "就是排队的时间有点长", "jiushipaiduideshijianyoudianchang"),
    ("内存占用还是偏高，", "排查一下是不是有内存泄漏", "paichayixiashibushiyouneicunxielou"),
    ("你先把需求文档发给我，", "我下午仔细评估一下工期", "woxiawuzixipingguyixiagongqi"),
    ("刚刚收到银行的短信提醒，", "说有一笔境外消费记录", "shuoyouyibijingwaixiaofeijilu"),
    ("高铁马上就要检票了，", "咱们快点往检票口走吧", "zanmenkuaidianwangjianpiaokouzouba"),
    ("这台电脑的风扇声音好大，", "是不是后台有程序卡死了", "shibushihoutaiyouchengxukasile"),
    ("代码已经合并到主干了。", "接下来准备打标签发布版本", "jiexialaizhunbeidabiaoqianfabubanben"),
    ("电影还有十分钟开场，", "我们先去买桶爆米花吧", "womenxianqumaitongbaomihuaba"),
    ("最近嗓子有点不舒服，", "多喝点温水少吃辛辣食物", "duohedianwenshuishaochixinlashiwu"),
    ("文件已经上传到网盘了，", "提取码我等会儿私信发你", "tiqumawodenghuirsixinfani"),
    ("这个模型体积只有六兆，", "在手机端运行也非常流畅", "zaishoujiduanyunxingyefeichangliuchang"),
];

/// Abbreviations & apostrophe disambiguation cases: (kind, expected, keys)
const ABBR_AND_APOS_CASES: &[(&str, &str, &str)] = &[
    ("abbr", "输入法", "srf"),
    ("abbr", "人工智能", "rgzn"),
    ("abbr", "不好意思", "bhys"),
    ("abbr", "为什么", "wsm"),
    ("abbr", "怎么样", "zmy"),
    ("abbr", "没关系", "mgx"),
    ("abbr", "小朋友", "xpy"),
    ("abbr", "数据库", "sjk"),
    ("abbr", "没问题", "mwt"),
    ("abbr", "差不多", "cbd"),
    ("abbr", "计算机", "jsj"),
    ("abbr", "输入法", "shrf"),
    ("abbr", "输入法", "shurf"),
    ("apos", "西安", "xi'an"),
    ("apos", "方案", "fang'an"),
    ("apos", "海鸥", "hai'ou"),
    ("apos", "皮袄", "pi'ao"),
    ("apos", "天安门", "tian'an'men"),
    ("apos", "长安", "chang'an"),
    ("apos", "档案", "dang'an"),
    ("apos", "平安", "ping'an"),
    ("apos", "激昂", "ji'ang"),
    ("apos", "酷爱", "ku'ai"),
    ("apos", "先", "xian"),
];

/// Curated typo rescue cases: (typo_kind, expected, typo_keys)
const TYPO_CASES: &[(&str, &str, &str)] = &[
    // Missing trailing -g (-ng -> -n)
    ("omit_g", "中国", "zhonguo"),
    ("omit_g", "上班", "shanban"),
    ("omit_g", "明天", "mintian"),
    ("omit_g", "朋友", "penyou"),
    ("omit_g", "应该", "yingai"),
    ("omit_g", "工程", "goncheng"),
    ("omit_g", "方法", "fanfa"),
    ("omit_g", "生活", "shenhuo"),
    // Adjacent transposition
    ("transpose", "我们", "wmoen"),
    ("transpose", "你好", "nhiao"),
    ("transpose", "今天", "jnitain"),
    ("transpose", "中国", "zhnogguo"),
    ("transpose", "输入法", "suhrufa"),
    ("transpose", "工作", "gnogzuo"),
    ("transpose", "时间", "shjiian"),
    ("transpose", "问题", "wneti"),
    // Double-tap extra letter insertion
    ("insert_dup", "中文", "zhonngwen"),
    ("insert_dup", "今天", "jinntian"),
    ("insert_dup", "朋友", "penngyou"),
    ("insert_dup", "开发", "kaiffa"),
    ("insert_dup", "方案", "fanngan"),
    ("insert_dup", "模型", "moxxing"),
    // Neighbor key substitution (single mistyped key in sentence/word)
    ("neighbor", "今天天气", "jintiantianqo"),
    ("neighbor", "输入法", "shurufs"),
    ("neighbor", "数据库", "shujukj"),
    ("neighbor", "人工智能", "rengongzhinebg"),
    ("neighbor", "测试环境", "ceshihuanjinh"),
    ("neighbor", "明天早上", "mingtianzsoshang"),
];

fn make_obs(keys: &str, layout: &KeyLayout) -> Vec<KeyObs> {
    keys.bytes()
        .enumerate()
        .map(|(i, b)| {
            let ch = b.to_ascii_lowercase();
            let (cx, cy) = if ch.is_ascii_lowercase() {
                layout.center_of(ch)
            } else {
                (0.0, 0.0)
            };
            KeyObs {
                ch,
                norm_x: cx,
                norm_y: cy,
                has_touch: false,
                timestamp_ms: 1000 + (i as u64) * 65,
            }
        })
        .collect()
}

fn char_accuracy(expected: &str, predicted: &str) -> (usize, usize) {
    let e: Vec<char> = expected.chars().collect();
    let p: Vec<char> = predicted.chars().collect();
    let mut hits = 0usize;
    for (i, &ec) in e.iter().enumerate() {
        if p.get(i).copied() == Some(ec) {
            hits += 1;
        }
    }
    (hits, e.len())
}

fn percentile_ms(sorted_us: &[u128], p: f64) -> f64 {
    if sorted_us.is_empty() {
        return 0.0;
    }
    let idx = (((sorted_us.len() - 1) as f64) * p).round() as usize;
    sorted_us[idx.min(sorted_us.len() - 1)] as f64 / 1000.0
}

fn eval_exact_nll_and_calibration(
    nv: &NeuralView<'_>,
    context: &str,
    target: &str,
) -> (f64, usize, Vec<f32>) {
    let mut tracker = NeuralContextTracker::default();
    if context.is_empty() {
        tracker.init_with_model(nv);
    } else {
        tracker.set_context_str(nv, context);
    }
    let mut nll_sum = 0.0f64;
    let mut count = 0usize;
    let mut abs_logz = Vec::new();
    for ch in target.chars() {
        let u = ch as u32;
        if (0x3400..0xA000).contains(&u) {
            let (self_sc, log_z) = nv.eval_next_char_calibration(&tracker.active, ch);
            let exact_lp = (self_sc - log_z) as f64;
            nll_sum += -exact_lp;
            count += 1;
            abs_logz.push(log_z.abs());
            let tok = nv.char_to_token(ch);
            nv.step_token(tok, &mut tracker.active);
        }
    }
    (nll_sum, count, abs_logz)
}

/// Build reverse char -> highest-weight syllable map from `PinyinDictView` for external TSV evaluation.
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
    let verbose = args.iter().any(|a| a == "--verbose");
    let tsv_path = args
        .windows(2)
        .find(|w| w[0] == "--tsv")
        .map(|w| PathBuf::from(&w[1]));
    let max_tsv: usize = args
        .windows(2)
        .find(|w| w[0] == "--max-tsv")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(200);
    let aux_override = args
        .windows(2)
        .find(|w| w[0] == "--aux")
        .map(|w| PathBuf::from(&w[1]));

    let dir = PathBuf::from("Resources/Dicts");
    let pydict_path = dir.join("pinyin.pydict");
    let aux_path = aux_override.unwrap_or_else(|| dir.join("wubi_aux.wbaux"));

    let dict = PinyinDictView::from_file(&pydict_path)
        .unwrap_or_else(|e| panic!("failed to load {}: {}", pydict_path.display(), e));
    let aux = AuxView::open_mmap(&aux_path)
        .unwrap_or_else(|e| panic!("failed to load {}: {}", aux_path.display(), e));
    let aux_bytes = fs::metadata(&aux_path).map(|m| m.len()).unwrap_or(0);
    let nv = aux.neural_view().expect("wubi_aux.wbaux missing WBLM0001 neural model");
    let summary = nv.offs.summary;

    println!("=========================================================================");
    println!(
        "Juxu Full-Pinyin & Cross-Clause Evaluation | aux={:.2} MB | V={} R={} D={} H={} L={} F={} | punct_ctx={}",
        aux_bytes as f64 / (1024.0 * 1024.0),
        summary.vocab_size,
        summary.rank,
        summary.d_model,
        summary.n_heads,
        summary.n_layers,
        summary.ffn_dim,
        nv.offs.has_punct_tokens
    );
    println!("=========================================================================");

    let layout = KeyLayout::default();
    let config = WubiConfig::default();
    let user = UserStore::default();
    let mut bos_tracker = NeuralContextTracker::default();
    bos_tracker.init_with_model(&nv);

    // -------------------------------------------------------------------------
    // Module A: Multi-domain Full Pinyin Sentences (L2 + Error Attribution)
    // -------------------------------------------------------------------------
    #[derive(Default)]
    struct DomainStat {
        total: usize,
        top1: usize,
        top3: usize,
        in_beam: usize,
        char_hits: usize,
        char_total: usize,
        mrr_sum: f64,
        fp_corrected: usize,
        lm_rank_miss: usize,
        beam_or_dict_miss: usize,
    }

    let mut by_domain: BTreeMap<&str, DomainStat> = BTreeMap::new();
    let mut all_lat_us: Vec<u128> = Vec::new();
    let mut misses: Vec<(&str, &str, String, String, Option<usize>)> = Vec::new();
    let mut all_abs_logz: Vec<f32> = Vec::new();
    let mut total_nll = 0.0f64;
    let mut total_nll_chars = 0usize;

    for &(domain, expected, raw_keys) in FULL_SENTENCES {
        let keys_lower = raw_keys.to_ascii_lowercase();
        let obs = make_obs(&keys_lower, &layout);
        let decoder = PinyinDecoder {
            dict: &dict,
            aux: Some(&aux),
            model: Some(aux.neural_view().unwrap()),
            config: &config,
            user: &user,
            context: &[],
            layout: &layout,
        };

        let t0 = Instant::now();
        let (cands, paths) = decode_pinyin(&obs, &keys_lower, &decoder, &bos_tracker.active);
        all_lat_us.push(t0.elapsed().as_micros());

        let (nll_s, n_c, mut lz) = eval_exact_nll_and_calibration(&nv, "", expected);
        total_nll += nll_s;
        total_nll_chars += n_c;
        all_abs_logz.append(&mut lz);

        let st = by_domain.entry(domain).or_default();
        st.total += 1;
        let top1_text = cands.first().map(|c| c.candidate.text.as_str()).unwrap_or("");
        let (ch_h, ch_t) = char_accuracy(expected, top1_text);
        st.char_hits += ch_h;
        st.char_total += ch_t;
        if paths.first().is_some_and(|p| p.any_corrected) {
            st.fp_corrected += 1;
        }

        let rank_in_cands = cands.iter().position(|c| c.candidate.text == expected);
        let rank_in_paths = paths.iter().position(|p| p.text == expected);
        if let Some(r) = rank_in_cands {
            st.mrr_sum += 1.0 / ((r + 1) as f64);
            if r == 0 {
                st.top1 += 1;
            }
            if r < 3 {
                st.top3 += 1;
            }
        }
        if rank_in_paths.is_some() {
            st.in_beam += 1;
        }
        if rank_in_cands != Some(0) {
            if rank_in_paths.is_some() {
                st.lm_rank_miss += 1;
            } else {
                st.beam_or_dict_miss += 1;
            }
            misses.push((domain, expected, top1_text.to_owned(), keys_lower, rank_in_paths));
        }
    }

    println!("\n[A] Full-Pinyin Multi-Domain Sentences ({} sentences)", FULL_SENTENCES.len());
    println!(
        "  {:<15} {:>4} {:>8} {:>8} {:>8} {:>8} {:>7} {:>6} {:>9} {:>10}",
        "Domain", "N", "Top-1", "Top-3", "InBeam", "CharAcc", "MRR", "FP%", "LM_Miss", "Beam_Miss"
    );
    let mut tot = DomainStat::default();
    for (dom, s) in &by_domain {
        tot.total += s.total;
        tot.top1 += s.top1;
        tot.top3 += s.top3;
        tot.in_beam += s.in_beam;
        tot.char_hits += s.char_hits;
        tot.char_total += s.char_total;
        tot.mrr_sum += s.mrr_sum;
        tot.fp_corrected += s.fp_corrected;
        tot.lm_rank_miss += s.lm_rank_miss;
        tot.beam_or_dict_miss += s.beam_or_dict_miss;
        println!(
            "  {:<15} {:>4} {:>7.1}% {:>7.1}% {:>7.1}% {:>7.2}% {:>7.3} {:>5.1}% {:>9} {:>10}",
            dom,
            s.total,
            100.0 * s.top1 as f64 / s.total as f64,
            100.0 * s.top3 as f64 / s.total as f64,
            100.0 * s.in_beam as f64 / s.total as f64,
            100.0 * s.char_hits as f64 / s.char_total.max(1) as f64,
            s.mrr_sum / s.total as f64,
            100.0 * s.fp_corrected as f64 / s.total as f64,
            s.lm_rank_miss,
            s.beam_or_dict_miss,
        );
    }
    println!(
        "  {:<15} {:>4} {:>7.1}% {:>7.1}% {:>7.1}% {:>7.2}% {:>7.3} {:>5.1}% {:>9} {:>10}",
        "TOTAL",
        tot.total,
        100.0 * tot.top1 as f64 / tot.total as f64,
        100.0 * tot.top3 as f64 / tot.total as f64,
        100.0 * tot.in_beam as f64 / tot.total as f64,
        100.0 * tot.char_hits as f64 / tot.char_total.max(1) as f64,
        tot.mrr_sum / tot.total as f64,
        100.0 * tot.fp_corrected as f64 / tot.total as f64,
        tot.lm_rank_miss,
        tot.beam_or_dict_miss,
    );
    if verbose || !misses.is_empty() {
        let show_n = if verbose { misses.len() } else { misses.len().min(12) };
        println!("  Sample misses (showing {}/{}):", show_n, misses.len());
        for (dom, exp, got, keys, r_beam) in misses.iter().take(show_n) {
            let reason = match r_beam {
                Some(r) => format!("LM_RANK(#{} in beam)", r + 1),
                None => "BEAM_OR_DICT_MISS".to_owned(),
            };
            println!("    [{}] {} | exp={} | got={} | keys={}", dom, reason, exp, got, keys);
        }
    }

    // -------------------------------------------------------------------------
    // Module B: True Punctuated Cross-Clause Context (cold_bos vs with_punct_ctx)
    // -------------------------------------------------------------------------
    let mut cc_cold_top1 = 0usize;
    let mut cc_ctx_top1 = 0usize;
    let mut cc_cold_ch = 0usize;
    let mut cc_ctx_ch = 0usize;
    let mut cc_total_ch = 0usize;
    let mut cc_cold_nll = 0.0f64;
    let mut cc_ctx_nll = 0.0f64;
    let mut cc_nll_chars = 0usize;

    for &(ctx_str, target, raw_keys) in CROSS_CLAUSE_CASES {
        let keys_lower = raw_keys.to_ascii_lowercase();
        let obs = make_obs(&keys_lower, &layout);

        // Cold BOS
        let (nll0, nc0, _) = eval_exact_nll_and_calibration(&nv, "", target);
        cc_cold_nll += nll0;
        cc_nll_chars += nc0;
        let dec0 = PinyinDecoder {
            dict: &dict,
            aux: Some(&aux),
            model: Some(aux.neural_view().unwrap()),
            config: &config,
            user: &user,
            context: &[],
            layout: &layout,
        };
        let (c0, _) = decode_pinyin(&obs, &keys_lower, &dec0, &bos_tracker.active);
        let g0 = c0.first().map(|c| c.candidate.text.as_str()).unwrap_or("");
        if g0 == target {
            cc_cold_top1 += 1;
        }
        let (h0, tc) = char_accuracy(target, g0);
        cc_cold_ch += h0;
        cc_total_ch += tc;

        // With punctuated cross-clause context
        let (nll1, _, _) = eval_exact_nll_and_calibration(&nv, ctx_str, target);
        cc_ctx_nll += nll1;
        let mut ctx_tracker = NeuralContextTracker::default();
        ctx_tracker.set_context_str(&nv, ctx_str);
        let ctx_chars: Vec<char> = ctx_str.chars().collect();
        let dec1 = PinyinDecoder {
            dict: &dict,
            aux: Some(&aux),
            model: Some(aux.neural_view().unwrap()),
            config: &config,
            user: &user,
            context: &ctx_chars,
            layout: &layout,
        };
        let (c1, _) = decode_pinyin(&obs, &keys_lower, &dec1, &ctx_tracker.active);
        let g1 = c1.first().map(|c| c.candidate.text.as_str()).unwrap_or("");
        if g1 == target {
            cc_ctx_top1 += 1;
        }
        let (h1, _) = char_accuracy(target, g1);
        cc_ctx_ch += h1;
        if verbose && g0 != g1 {
            println!("    [CC diff] ctx={} | exp={} | cold={} | punct_ctx={}", ctx_str, target, g0, g1);
        }
    }
    let cc_n = CROSS_CLAUSE_CASES.len();
    let cold_avg_nll = cc_cold_nll / cc_nll_chars.max(1) as f64;
    let ctx_avg_nll = cc_ctx_nll / cc_nll_chars.max(1) as f64;
    println!("\n[B] Cross-Clause Punctuated Context ({} pairs)", cc_n);
    println!(
        "  Cold BOS       | NLL={:.4} | Top-1={:>2}/{} ({:5.1}%) | CharAcc={:5.2}%",
        cold_avg_nll,
        cc_cold_top1,
        cc_n,
        100.0 * cc_cold_top1 as f64 / cc_n as f64,
        100.0 * cc_cold_ch as f64 / cc_total_ch.max(1) as f64
    );
    println!(
        "  Punct Context  | NLL={:.4} (Δ={:+.4}) | Top-1={:>2}/{} ({:5.1}%, Δ={:+}) | CharAcc={:5.2}%",
        ctx_avg_nll,
        ctx_avg_nll - cold_avg_nll,
        cc_ctx_top1,
        cc_n,
        100.0 * cc_ctx_top1 as f64 / cc_n as f64,
        cc_ctx_top1 as isize - cc_cold_top1 as isize,
        100.0 * cc_ctx_ch as f64 / cc_total_ch.max(1) as f64
    );

    // -------------------------------------------------------------------------
    // Module C: Abbreviations & Apostrophes
    // -------------------------------------------------------------------------
    let mut ab_stat: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for &(kind, expected, raw_keys) in ABBR_AND_APOS_CASES {
        let obs = make_obs(raw_keys, &layout);
        let dec = PinyinDecoder {
            dict: &dict,
            aux: Some(&aux),
            model: Some(aux.neural_view().unwrap()),
            config: &config,
            user: &user,
            context: &[],
            layout: &layout,
        };
        let (cands, _) = decode_pinyin(&obs, raw_keys, &dec, &bos_tracker.active);
        let e = ab_stat.entry(kind).or_default();
        e.0 += 1;
        let r = cands.iter().position(|c| c.candidate.text == expected);
        if r == Some(0) {
            e.1 += 1;
        }
        if r.is_some_and(|x| x < 3) {
            e.2 += 1;
        }
        if verbose && r != Some(0) {
            println!(
                "    [{}] miss: keys={} exp={} got={}",
                kind,
                raw_keys,
                expected,
                cands.first().map(|c| c.candidate.text.as_str()).unwrap_or("")
            );
        }
    }
    println!("\n[C] Initial Abbreviations & Apostrophes");
    for (k, (n, t1, t3)) in &ab_stat {
        println!(
            "  {:<8} | N={:>2} | Top-1={:>2}/{} ({:5.1}%) | Top-3={:>2}/{} ({:5.1}%)",
            k,
            n,
            t1,
            n,
            100.0 * *t1 as f64 / *n as f64,
            t3,
            n,
            100.0 * *t3 as f64 / *n as f64
        );
    }

    // -------------------------------------------------------------------------
    // Module D: Full-Pinyin Typo Rescue
    // -------------------------------------------------------------------------
    let mut ty_stat: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for &(kind, expected, raw_keys) in TYPO_CASES {
        let obs = make_obs(raw_keys, &layout);
        let dec = PinyinDecoder {
            dict: &dict,
            aux: Some(&aux),
            model: Some(aux.neural_view().unwrap()),
            config: &config,
            user: &user,
            context: &[],
            layout: &layout,
        };
        let (cands, _) = decode_pinyin(&obs, raw_keys, &dec, &bos_tracker.active);
        let e = ty_stat.entry(kind).or_default();
        e.0 += 1;
        let r = cands.iter().position(|c| c.candidate.text == expected);
        if r == Some(0) {
            e.1 += 1;
        }
        if r.is_some_and(|x| x < 3) {
            e.2 += 1;
        }
        if verbose && r != Some(0) {
            println!(
                "    [typo:{}] miss: keys={} exp={} got={}",
                kind,
                raw_keys,
                expected,
                cands.first().map(|c| c.candidate.text.as_str()).unwrap_or("")
            );
        }
    }
    println!("\n[D] Full-Pinyin Typo Rescue (clean FP={:.1}%)", 100.0 * tot.fp_corrected as f64 / tot.total as f64);
    for (k, (n, t1, t3)) in &ty_stat {
        println!(
            "  {:<12} | N={:>2} | Top-1={:>2}/{} ({:5.1}%) | Top-3={:>2}/{} ({:5.1}%)",
            k,
            n,
            t1,
            n,
            100.0 * *t1 as f64 / *n as f64,
            t3,
            n,
            100.0 * *t3 as f64 / *n as f64
        );
    }

    // -------------------------------------------------------------------------
    // Optional External TSV Large-Scale Held-Out Evaluation
    // -------------------------------------------------------------------------
    if let Some(ref path) = tsv_path {
        let content = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
        let c2p = build_char_to_pinyin(&dict);
        let mut tsv_n = 0usize;
        let mut tsv_t1 = 0usize;
        let mut tsv_t3 = 0usize;
        let mut tsv_ch_h = 0usize;
        let mut tsv_ch_t = 0usize;
        for line in content.lines() {
            if tsv_n >= max_tsv {
                break;
            }
            let sent = line.split('\t').last().unwrap_or("").trim();
            let cjk: String = sent.chars().filter(|&c| (0x4E00..=0x9FFF).contains(&(c as u32))).collect();
            let clen = cjk.chars().count();
            if !(6..=18).contains(&clen) {
                continue;
            }
            let mut py = String::new();
            let mut ok = true;
            for ch in cjk.chars() {
                if let Some(s) = c2p.get(&ch) {
                    py.push_str(s);
                } else {
                    ok = false;
                    break;
                }
            }
            if !ok {
                continue;
            }
            let obs = make_obs(&py, &layout);
            let dec = PinyinDecoder {
                dict: &dict,
                aux: Some(&aux),
                model: Some(aux.neural_view().unwrap()),
                config: &config,
                user: &user,
                context: &[],
                layout: &layout,
            };
            let (cands, _) = decode_pinyin(&obs, &py, &dec, &bos_tracker.active);
            tsv_n += 1;
            let got = cands.first().map(|c| c.candidate.text.as_str()).unwrap_or("");
            let r = cands.iter().position(|c| c.candidate.text == cjk);
            if r == Some(0) {
                tsv_t1 += 1;
            }
            if r.is_some_and(|x| x < 3) {
                tsv_t3 += 1;
            }
            let (h, t) = char_accuracy(&cjk, got);
            tsv_ch_h += h;
            tsv_ch_t += t;
        }
        println!(
            "\n[TSV] {} ({} auto-romanized sentences) | Top-1={:5.1}% | Top-3={:5.1}% | CharAcc={:5.2}%",
            path.display(),
            tsv_n,
            100.0 * tsv_t1 as f64 / tsv_n.max(1) as f64,
            100.0 * tsv_t3 as f64 / tsv_n.max(1) as f64,
            100.0 * tsv_ch_h as f64 / tsv_ch_t.max(1) as f64
        );
    }

    // -------------------------------------------------------------------------
    // Module E: L1 Self-Norm Calibration & L3 Latency Budget Gate
    // -------------------------------------------------------------------------
    all_abs_logz.sort_by(|a, b| a.total_cmp(b));
    let mean_lz: f32 = all_abs_logz.iter().sum::<f32>() / all_abs_logz.len().max(1) as f32;
    let p95_lz = all_abs_logz
        .get(((all_abs_logz.len().saturating_sub(1) as f64) * 0.95).round() as usize)
        .copied()
        .unwrap_or(0.0);
    let max_lz = all_abs_logz.last().copied().unwrap_or(0.0);

    // Benchmark `step_token` (500 steps) & streaming per-keystroke latency
    let mut bench_st = bos_tracker.active.clone();
    let t_step0 = Instant::now();
    for i in 0..500usize {
        nv.step_token(4 + (i % 1000), &mut bench_st);
    }
    let step_us = t_step0.elapsed().as_micros() as f64 / 500.0;

    // Benchmark incremental PinyinStream per-keystroke latency across 10 long sentences
    let mut keystroke_us: Vec<u128> = Vec::new();
    for &(_, _, raw_keys) in FULL_SENTENCES.iter().take(12) {
        let keys_lower = raw_keys.to_ascii_lowercase();
        let obs = make_obs(&keys_lower, &layout);
        let dec = PinyinDecoder {
            dict: &dict,
            aux: Some(&aux),
            model: Some(aux.neural_view().unwrap()),
            config: &config,
            user: &user,
            context: &[],
            layout: &layout,
        };
        let mut stream = PinyinStream::default();
        for len in 1..=obs.len() {
            let t0 = Instant::now();
            let _ = stream.update_and_decode(&obs[..len], &keys_lower[..len], &dec, &bos_tracker.active);
            keystroke_us.push(t0.elapsed().as_micros());
        }
    }
    all_lat_us.sort_unstable();
    keystroke_us.sort_unstable();

    println!("\n[E] L1 Calibration & L3 Latency Budget Gate");
    println!(
        "  Self-Norm |logZ|   | mean={:.4} | p95={:.4} | max={:.4} | exact_NLL={:.4} ({} chars)",
        mean_lz,
        p95_lz,
        max_lz,
        total_nll / total_nll_chars.max(1) as f64,
        total_nll_chars
    );
    println!("  step_token         | {:.1} µs/step", step_us);
    println!(
        "  Stream keystroke   | p50={:.2} ms | p95={:.2} ms | p99={:.2} ms (120Hz budget: 8.33 ms)",
        percentile_ms(&keystroke_us, 0.50),
        percentile_ms(&keystroke_us, 0.95),
        percentile_ms(&keystroke_us, 0.99),
    );
    println!(
        "  Full-sent cold     | p50={:.2} ms | p95={:.2} ms | p99={:.2} ms",
        percentile_ms(&all_lat_us, 0.50),
        percentile_ms(&all_lat_us, 0.95),
        percentile_ms(&all_lat_us, 0.99),
    );
}
