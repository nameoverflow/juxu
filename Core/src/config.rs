use std::fs;
use std::io;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum InputSchema {
    Wubi86 = 86,
    Wubi98 = 98,
    Wubi06 = 6,
    Xiaohe = 10,
    Pinyin = 20,
    XiaoheShuangpin = 21,
    Cangjie3 = 33,
    Cangjie5 = 35,
    Cangjie6 = 36,
}


impl InputSchema {
    pub fn from_id(id: u32) -> Self {
        match id {
            98 => Self::Wubi98,
            6 | 2006 => Self::Wubi06,
            10 => Self::Xiaohe,
            20 => Self::Pinyin,
            21 => Self::XiaoheShuangpin,
            33 => Self::Cangjie3,
            35 => Self::Cangjie5,
            36 => Self::Cangjie6,
            _ => Self::Wubi86,
        }
    }

    pub fn from_str_tag(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "98" | "wubi98" => Self::Wubi98,
            "06" | "2006" | "wubi06" | "new_century" | "xinshiji" => Self::Wubi06,
            "10" | "xiaohe" | "flypy" | "xh" => Self::Xiaohe,
            "20" | "pinyin" | "quanpin" | "qp" => Self::Pinyin,
            "21" | "xiaohe_sp" | "shuangpin" | "sp" | "flypy_sp" | "xhsp" => Self::XiaoheShuangpin,
            "33" | "cj3" | "cangjie3" | "cangjie_3" => Self::Cangjie3,
            "35" | "cj5" | "cangjie5" | "cangjie_5" | "cangjie" | "cj" => Self::Cangjie5,
            "36" | "cj6" | "cangjie6" | "cangjie_6" => Self::Cangjie6,
            _ => Self::Wubi86,
        }
    }

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::Wubi86 => "86",
            Self::Wubi98 => "98",
            Self::Wubi06 => "06",
            Self::Xiaohe => "xiaohe",
            Self::Pinyin => "pinyin",
            Self::XiaoheShuangpin => "xiaohe_sp",
            Self::Cangjie3 => "cangjie3",
            Self::Cangjie5 => "cangjie5",
            Self::Cangjie6 => "cangjie6",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Wubi86 => "86版五笔",
            Self::Wubi98 => "98版五笔",
            Self::Wubi06 => "新世纪五笔",
            Self::Xiaohe => "小鹤音形",
            Self::Pinyin => "全拼",
            Self::XiaoheShuangpin => "小鹤双拼",
            Self::Cangjie3 => "仓颉三代",
            Self::Cangjie5 => "仓颉五代",
            Self::Cangjie6 => "仓颉六代",
        }
    }

    pub fn slot_index(self) -> usize {
        match self {
            Self::Wubi86 => 0,
            Self::Wubi98 => 1,
            Self::Wubi06 => 2,
            Self::Xiaohe => 3,
            Self::Cangjie3 => 4,
            Self::Cangjie5 => 5,
            Self::Cangjie6 => 6,
            Self::Pinyin => 7,
            Self::XiaoheShuangpin => 8,
        }
    }

    #[inline]
    pub fn is_wubi(self) -> bool {
        matches!(self, Self::Wubi86 | Self::Wubi98 | Self::Wubi06)
    }

    #[inline]
    pub fn is_xiaohe(self) -> bool {
        matches!(self, Self::Xiaohe)
    }

    #[inline]
    pub fn is_cangjie(self) -> bool {
        matches!(self, Self::Cangjie3 | Self::Cangjie5 | Self::Cangjie6)
    }

    #[inline]
    pub fn is_pinyin(self) -> bool {
        matches!(self, Self::Pinyin | Self::XiaoheShuangpin)
    }

    #[inline]
    pub fn is_shuangpin(self) -> bool {
        matches!(self, Self::XiaoheShuangpin)
    }

    #[inline]
    pub fn max_code_len(self) -> usize {
        if self.is_cangjie() { 5 } else { 4 }
    }

    #[inline]
    pub fn allows_z_code(self) -> bool {
        !self.is_wubi()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FourthEmptyBehavior {
    /// 阻断无效的第 4 码，保留前 3 码并发出提示（主流五笔默认习惯）
    Block,
    /// 第 4 码空码时自动清空重输
    Clear,
}

impl FourthEmptyBehavior {
    pub fn from_tag(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "clear" => Self::Clear,
            _ => Self::Block,
        }
    }
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Clear => "clear",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShiftToggleMode {
    /// Shift 切换英文时将未上屏编码作为英文直接上屏（苹果原生手感）
    CommitCode,
    /// Shift 切换英文时将当前首选候选词上屏
    CommitFirst,
    /// Shift 切换英文时直接丢弃未上屏编码
    Clear,
}

impl ShiftToggleMode {
    pub fn from_tag(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "commit_first" | "first" => Self::CommitFirst,
            "clear" => Self::Clear,
            _ => Self::CommitCode,
        }
    }
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::CommitCode => "commit_code",
            Self::CommitFirst => "commit_first",
            Self::Clear => "clear",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateOrientation {
    Horizontal,
    Vertical,
}

impl CandidateOrientation {
    pub fn from_tag(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "vertical" | "vert" => Self::Vertical,
            _ => Self::Horizontal,
        }
    }
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypoConfig {
    /// 是否开启统计与触控误触纠正
    pub enabled: bool,
    /// 触屏端个人触摸热区动态自适应（EMA 微调每键判定中心）
    pub touch_hitbox_adapt: bool,
    /// 空码救回 / 键边缘滑偏时直接升为首选的后验置信度阈值 (0.50..=0.99)
    pub promote_threshold: f32,
    /// 精确码有效时在次选位展示纠错候选的最低置信度阈值 (0.15..=0.80)
    pub suggest_threshold: f32,
    /// 纠错救回项是否抑制「四码唯一自动上屏」，防止误触把错字自动顶上屏
    pub suppress_auto_commit_on_correction: bool,
    /// 漏空格切分纠错（基于语言模型识别三码/二码/一码字后漏按空格，自动顶屏或给出双字切分候选）
    pub missed_space_correction: bool,
}

impl Default for TypoConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            touch_hitbox_adapt: true,
            promote_threshold: 0.82,
            suggest_threshold: 0.30,
            suppress_auto_commit_on_correction: true,
            missed_space_correction: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct JuxuConfig {
    pub schema: InputSchema,
    /// 四码唯一自动上屏
    pub fourth_code_auto_commit: bool,
    /// 第五码自动顶字上屏（四码重码时敲第 5 码将首选上屏并开启新一轮首码）
    pub fifth_code_push_commit: bool,
    /// 四码空码处理方式
    pub fourth_empty_behavior: FourthEmptyBehavior,
    /// 最短编码（简码）优先置顶
    pub shortest_code_first: bool,
    /// 出简不出全（有唯一简码的单字在输入全码时后置，培养简码肌肉记忆）
    pub strict_shortcode: bool,
    /// 流式无空格连打（需要带语言模型的辅助词库）
    pub spaceless: bool,
    /// 全码输入时提示存在更短简码（如 ⚡q）
    pub show_shortcode_hint: bool,
    /// 词组动态调频
    pub dynamic_word_freq: bool,
    /// 单字动态调频（默认关闭以固定单字重码序）
    pub dynamic_char_freq: bool,
    /// 连打历史自动组词
    pub auto_phrase: bool,
    /// 连打自动组词触发次数阈值（1 或 2，默认 2 次防噪音成词）
    pub auto_phrase_threshold: u8,
    /// z 键万能通配查询 + 拼音反查 + zz 符号
    pub z_omnipotent: bool,
    /// 分号/引号快捷选 2、3 候选词
    pub semicolon_quote_select: bool,
    /// 中文标点与成对引号智能配对
    pub smart_punctuation: bool,
    /// 中英文之间自动加空格
    pub auto_space_zh_en: bool,
    /// 数字与中文之间也自动加空格
    pub auto_space_digits: bool,
    /// 扩展生僻大字集开关
    pub enable_rare_charset: bool,
    /// 全拼声母简拼开关（如 srf -> 输入法）
    pub pinyin_initial_abbr: bool,
    /// Shift 中英切换策略
    pub shift_toggle_mode: ShiftToggleMode,
    /// 每页候选词个数 (3..=9)
    pub page_size: u8,
    /// 候选栏方向
    pub orientation: CandidateOrientation,
    /// 本地输入诊断与评测日志（记录手选非首选、纠错、退格重打等用于制作 eval set）
    pub eval_logging: bool,
    /// 统计 + 触控热区误触纠正配置
    pub typo: TypoConfig,
}

impl Default for JuxuConfig {
    fn default() -> Self {
        Self {
            schema: InputSchema::Wubi86,
            fourth_code_auto_commit: true,
            fifth_code_push_commit: true,
            fourth_empty_behavior: FourthEmptyBehavior::Block,
            shortest_code_first: true,
            strict_shortcode: false,
            spaceless: false,
            show_shortcode_hint: true,
            dynamic_word_freq: true,
            dynamic_char_freq: false,
            auto_phrase: true,
            auto_phrase_threshold: 2,
            z_omnipotent: true,
            semicolon_quote_select: false,
            smart_punctuation: true,
            auto_space_zh_en: true,
            auto_space_digits: true,
            enable_rare_charset: false,
            pinyin_initial_abbr: true,
            shift_toggle_mode: ShiftToggleMode::CommitCode,
            page_size: 5,
            orientation: CandidateOrientation::Horizontal,
            eval_logging: true,
            typo: TypoConfig::default(),
        }
    }
}

impl JuxuConfig {
    pub fn to_toml_string(&self) -> String {
        format!(
            concat!(
                "# 句序 (Juxu) 配置文件 — macOS 与 iOS 通用内核\n",
                "# 修改保存后将实时生效\n\n",
                "[general]\n",
                "# 输入方案: \"86\" | \"98\" | \"06\" (新世纪) | \"xiaohe\" (小鹤音形) | \"pinyin\" (全拼) | \"xiaohe_sp\" (小鹤双拼) | \"cangjie3\" | \"cangjie5\" | \"cangjie6\"\n",
                "schema = \"{}\"\n",
                "page_size = {}\n",
                "# 候选栏方向: \"horizontal\" | \"vertical\"\n",
                "orientation = \"{}\"\n",
                "# Shift 切换中英行为: \"commit_code\" | \"commit_first\" | \"clear\"\n",
                "shift_toggle_mode = \"{}\"\n",
                "smart_punctuation = {}\n",
                "semicolon_quote_select = {}\n",
                "auto_space_zh_en = {}\n",
                "auto_space_digits = {}\n",
                "eval_logging = {}\n\n",
                "[typing]\n",
                "fourth_code_auto_commit = {}\n",
                "fifth_code_push_commit = {}\n",
                "# 四码空码行为: \"block\" | \"clear\"\n",
                "fourth_empty_behavior = \"{}\"\n",
                "shortest_code_first = {}\n",
                "strict_shortcode = {}\n",
                "spaceless = {}\n",
                "show_shortcode_hint = {}\n",
                "dynamic_word_freq = {}\n",
                "dynamic_char_freq = {}\n",
                "auto_phrase = {}\n",
                "auto_phrase_threshold = {}\n",
                "z_omnipotent = {}\n",
                "enable_rare_charset = {}\n",
                "pinyin_initial_abbr = {}\n\n",
                "[typo_correction]\n",
                "enabled = {}\n",
                "touch_hitbox_adapt = {}\n",
                "promote_threshold = {:.2}\n",
                "suggest_threshold = {:.2}\n",
                "suppress_auto_commit_on_correction = {}\n",
                "missed_space_correction = {}\n"
            ),
            self.schema.as_tag(),
            self.page_size,
            self.orientation.as_tag(),
            self.shift_toggle_mode.as_tag(),
            self.smart_punctuation,
            self.semicolon_quote_select,
            self.auto_space_zh_en,
            self.auto_space_digits,
            self.eval_logging,
            self.fourth_code_auto_commit,
            self.fifth_code_push_commit,
            self.fourth_empty_behavior.as_tag(),
            self.shortest_code_first,
            self.strict_shortcode,
            self.spaceless,
            self.show_shortcode_hint,
            self.dynamic_word_freq,
            self.dynamic_char_freq,
            self.auto_phrase,
            self.auto_phrase_threshold,
            self.z_omnipotent,
            self.enable_rare_charset,
            self.pinyin_initial_abbr,
            self.typo.enabled,
            self.typo.touch_hitbox_adapt,
            self.typo.promote_threshold,
            self.typo.suggest_threshold,
            self.typo.suppress_auto_commit_on_correction,
            self.typo.missed_space_correction,
        )
    }

    pub fn from_toml_str(src: &str) -> Self {
        let mut cfg = Self::default();
        let mut section = String::new();
        for raw_line in src.lines() {
            let line = match raw_line.find('#') {
                Some(idx) => &raw_line[..idx],
                None => raw_line,
            }
            .trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_ascii_lowercase();
                continue;
            }
            let Some(eq) = line.find('=') else {
                continue;
            };
            let key = line[..eq].trim();
            let val = line[eq + 1..].trim().trim_matches('"').trim_matches('\'');
            let parse_bool = |v: &str| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes");

            match (section.as_str(), key) {
                (_, "schema") => cfg.schema = InputSchema::from_str_tag(val),
                (_, "page_size") => {
                    if let Ok(n) = val.parse::<u8>() {
                        cfg.page_size = n.clamp(3, 9);
                    }
                }
                (_, "orientation") => cfg.orientation = CandidateOrientation::from_tag(val),
                (_, "shift_toggle_mode") => cfg.shift_toggle_mode = ShiftToggleMode::from_tag(val),
                (_, "smart_punctuation") => cfg.smart_punctuation = parse_bool(val),
                (_, "semicolon_quote_select") => cfg.semicolon_quote_select = parse_bool(val),
                (_, "auto_space_zh_en") => cfg.auto_space_zh_en = parse_bool(val),
                (_, "auto_space_digits") => cfg.auto_space_digits = parse_bool(val),
                (_, "eval_logging") => cfg.eval_logging = parse_bool(val),
                (_, "fourth_code_auto_commit") => cfg.fourth_code_auto_commit = parse_bool(val),
                (_, "fifth_code_push_commit") => cfg.fifth_code_push_commit = parse_bool(val),
                (_, "fourth_empty_behavior") => {
                    cfg.fourth_empty_behavior = FourthEmptyBehavior::from_tag(val)
                }
                (_, "shortest_code_first") => cfg.shortest_code_first = parse_bool(val),
                (_, "strict_shortcode") => cfg.strict_shortcode = parse_bool(val),
                (_, "spaceless") => cfg.spaceless = parse_bool(val),
                (_, "show_shortcode_hint") => cfg.show_shortcode_hint = parse_bool(val),
                (_, "dynamic_word_freq") => cfg.dynamic_word_freq = parse_bool(val),
                (_, "dynamic_char_freq") => cfg.dynamic_char_freq = parse_bool(val),
                (_, "auto_phrase") => cfg.auto_phrase = parse_bool(val),
                (_, "auto_phrase_threshold") => {
                    if let Ok(n) = val.parse::<u8>() {
                        cfg.auto_phrase_threshold = n.clamp(1, 5);
                    }
                }
                (_, "z_omnipotent") => cfg.z_omnipotent = parse_bool(val),
                (_, "enable_rare_charset") => cfg.enable_rare_charset = parse_bool(val),
                (_, "pinyin_initial_abbr") => cfg.pinyin_initial_abbr = parse_bool(val),
                ("typo_correction", "enabled") | (_, "typo_enabled") => {
                    cfg.typo.enabled = parse_bool(val)
                }
                (_, "touch_hitbox_adapt") => cfg.typo.touch_hitbox_adapt = parse_bool(val),
                (_, "promote_threshold") => {
                    if let Ok(f) = val.parse::<f32>() {
                        cfg.typo.promote_threshold = f.clamp(0.50, 0.99);
                    }
                }
                (_, "suggest_threshold") => {
                    if let Ok(f) = val.parse::<f32>() {
                        cfg.typo.suggest_threshold = f.clamp(0.10, 0.85);
                    }
                }
                (_, "suppress_auto_commit_on_correction") => {
                    cfg.typo.suppress_auto_commit_on_correction = parse_bool(val)
                }
                (_, "missed_space_correction") => {
                    cfg.typo.missed_space_correction = parse_bool(val)
                }
                _ => {}
            }
        }
        cfg
    }

    pub fn load_file<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let content = fs::read_to_string(path)?;
        Ok(Self::from_toml_str(&content))
    }

    pub fn save_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, self.to_toml_string())
    }
}
