use crate::json::push_json_str;
use std::fmt::Write as _;
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
    #[inline]
    pub fn id(self) -> u32 {
        self as u32
    }

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
            "21" | "xiaohe_sp" | "shuangpin" | "sp" | "flypy_sp" | "xhsp" => {
                Self::XiaoheShuangpin
            }
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
        if self.is_cangjie() {
            5
        } else {
            4
        }
    }

    #[inline]
    pub fn allows_z_code(self) -> bool {
        !self.is_wubi()
    }

    #[inline]
    pub fn supports_z_pinyin(self) -> bool {
        self.is_wubi() || self.is_cangjie()
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
            "commit_cand" | "commit_first" | "first" => Self::CommitFirst,
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
    pub fn as_json_tag(self) -> &'static str {
        match self {
            Self::CommitCode => "commit_code",
            Self::CommitFirst => "commit_cand",
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
    /// 精确码有效时在次选位展示纠错候选的最低置信度阈值 (0.10..=0.85)
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
    pub fourth_code_auto_commit: bool,
    pub fifth_code_push_commit: bool,
    pub fourth_empty_behavior: FourthEmptyBehavior,
    pub shortest_code_first: bool,
    pub strict_shortcode: bool,
    pub spaceless: bool,
    pub show_shortcode_hint: bool,
    pub dynamic_word_freq: bool,
    pub dynamic_char_freq: bool,
    pub auto_phrase: bool,
    pub auto_phrase_threshold: u8,
    pub z_omnipotent: bool,
    pub semicolon_quote_select: bool,
    pub smart_punctuation: bool,
    pub auto_space_zh_en: bool,
    pub auto_space_digits: bool,
    pub clipboard_suggestion: bool,
    pub cangjie_keycap_radicals: bool,
    pub wubi_keycap_radicals: bool,
    pub key_swipe_up_input: bool,
    pub space_comma_key: bool,
    pub key_long_press_alternates: bool,
    pub backspace_swipe_clear: bool,
    pub keyboard_hold_cursor: bool,
    pub enable_rare_charset: bool,
    pub pinyin_initial_abbr: bool,
    pub pinyin_t9_layout: bool,
    pub shift_toggle_mode: ShiftToggleMode,
    pub page_size: u8,
    pub orientation: CandidateOrientation,
    pub eval_logging: bool,
    pub emoji_candidates: bool,
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
            clipboard_suggestion: true,
            cangjie_keycap_radicals: true,
            wubi_keycap_radicals: false,
            key_swipe_up_input: false,
            space_comma_key: false,
            key_long_press_alternates: false,
            backspace_swipe_clear: false,
            keyboard_hold_cursor: false,
            enable_rare_charset: false,
            pinyin_initial_abbr: true,
            pinyin_t9_layout: false,
            shift_toggle_mode: ShiftToggleMode::CommitCode,
            page_size: 5,
            orientation: CandidateOrientation::Horizontal,
            eval_logging: true,
            emoji_candidates: false,
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
                "clipboard_suggestion = {}\n",
                "cangjie_keycap_radicals = {}\n",
                "wubi_keycap_radicals = {}\n",
                "key_swipe_up_input = {}\n",
                "space_comma_key = {}\n",
                "key_long_press_alternates = {}\n",
                "backspace_swipe_clear = {}\n",
                "keyboard_hold_cursor = {}\n",
                "emoji_candidates = {}\n",
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
                "pinyin_initial_abbr = {}\n",
                "pinyin_t9_layout = {}\n\n",
                "[typo_correction]\n",
                "enabled = {}\n",
                "touch_hitbox_adapt = {}\n",
                "promote_threshold = {:.2}\n",
                "suggest_threshold = {:.2}\n",
                "suppress_auto_commit_on_correction = {}\n",
                "missed_space_correction = {}\n"
            ),
            self.schema.as_tag(),
            self.page_size.clamp(3, 9),
            self.orientation.as_tag(),
            self.shift_toggle_mode.as_tag(),
            self.smart_punctuation,
            self.semicolon_quote_select,
            self.auto_space_zh_en,
            self.auto_space_digits,
            self.clipboard_suggestion,
            self.cangjie_keycap_radicals,
            self.wubi_keycap_radicals,
            self.key_swipe_up_input,
            self.space_comma_key,
            self.key_long_press_alternates,
            self.backspace_swipe_clear,
            self.keyboard_hold_cursor,
            self.emoji_candidates,
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
            self.auto_phrase_threshold.clamp(1, 5),
            self.z_omnipotent,
            self.enable_rare_charset,
            self.pinyin_initial_abbr,
            self.pinyin_t9_layout,
            self.typo.enabled,
            self.typo.touch_hitbox_adapt,
            self.typo.promote_threshold.clamp(0.50, 0.99),
            self.typo.suggest_threshold.clamp(0.10, 0.85),
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
            let parse_bool =
                |v: &str| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes");

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
                (_, "clipboard_suggestion") => cfg.clipboard_suggestion = parse_bool(val),
                (_, "cangjie_keycap_radicals") => cfg.cangjie_keycap_radicals = parse_bool(val),
                (_, "wubi_keycap_radicals") => cfg.wubi_keycap_radicals = parse_bool(val),
                (_, "key_swipe_up_input") => cfg.key_swipe_up_input = parse_bool(val),
                (_, "space_comma_key") => cfg.space_comma_key = parse_bool(val),
                (_, "key_long_press_alternates") => cfg.key_long_press_alternates = parse_bool(val),
                (_, "backspace_swipe_clear") => cfg.backspace_swipe_clear = parse_bool(val),
                (_, "keyboard_hold_cursor") => cfg.keyboard_hold_cursor = parse_bool(val),
                (_, "eval_logging") => cfg.eval_logging = parse_bool(val),
                (_, "emoji_candidates") => cfg.emoji_candidates = parse_bool(val),
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
                (_, "pinyin_t9_layout") => cfg.pinyin_t9_layout = parse_bool(val),
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
                (_, "missed_space_correction") => cfg.typo.missed_space_correction = parse_bool(val),
                _ => {}
            }
        }
        cfg
    }

    pub fn to_json_string(&self) -> String {
        let mut out = String::with_capacity(1024);
        out.push('{');
        let _ = write!(
            out,
            "\"schemaId\":{},\"pageSize\":{},\"vertical\":{},\"shiftToggleMode\":",
            self.schema.id(),
            (self.page_size as usize).clamp(3, 9),
            self.orientation == CandidateOrientation::Vertical,
        );
        push_json_str(&mut out, self.shift_toggle_mode.as_json_tag());
        let _ = write!(
            out,
            ",\"smartPunctuation\":{},\"semicolonQuoteSelect\":{},\"autoSpaceZhEn\":{},\"autoSpaceDigits\":{},\"clipboardSuggestion\":{},\"cangjieKeycapRadicals\":{},\"wubiKeycapRadicals\":{},\"keySwipeUpInput\":{},\"spaceCommaKey\":{},\"keyLongPressAlternates\":{},\"backspaceSwipeClear\":{},\"keyboardHoldCursor\":{},\"evalLogging\":{},\"emojiCandidates\":{},\"fourthCodeAutoCommit\":{},\"fifthCodePushCommit\":{},\"fourthEmptyBehavior\":",
            self.smart_punctuation,
            self.semicolon_quote_select,
            self.auto_space_zh_en,
            self.auto_space_digits,
            self.clipboard_suggestion,
            self.cangjie_keycap_radicals,
            self.wubi_keycap_radicals,
            self.key_swipe_up_input,
            self.space_comma_key,
            self.key_long_press_alternates,
            self.backspace_swipe_clear,
            self.keyboard_hold_cursor,
            self.eval_logging,
            self.emoji_candidates,
            self.fourth_code_auto_commit,
            self.fifth_code_push_commit,
        );
        push_json_str(&mut out, self.fourth_empty_behavior.as_tag());
        let _ = write!(
            out,
            ",\"shortestCodeFirst\":{},\"strictShortcode\":{},\"spaceless\":{},\"showShortcodeHint\":{},\"dynamicWordFreq\":{},\"dynamicCharFreq\":{},\"autoPhrase\":{},\"autoPhraseThreshold\":{},\"zOmnipotent\":{},\"enableRareCharset\":{},\"pinyinInitialAbbr\":{},\"pinyinT9Layout\":{},\"typoEnabled\":{},\"touchHitboxAdapt\":{},\"promoteThreshold\":{:.2},\"suggestThreshold\":{:.2},\"suppressAutoCommitOnCorrection\":{},\"missedSpaceCorrection\":{}}}",
            self.shortest_code_first,
            self.strict_shortcode,
            self.spaceless,
            self.show_shortcode_hint,
            self.dynamic_word_freq,
            self.dynamic_char_freq,
            self.auto_phrase,
            (self.auto_phrase_threshold as u32).clamp(1, 5),
            self.z_omnipotent,
            self.enable_rare_charset,
            self.pinyin_initial_abbr,
            self.pinyin_t9_layout,
            self.typo.enabled,
            self.typo.touch_hitbox_adapt,
            self.typo.promote_threshold.clamp(0.50, 0.99),
            self.typo.suggest_threshold.clamp(0.10, 0.85),
            self.typo.suppress_auto_commit_on_correction,
            self.typo.missed_space_correction,
        );
        out
    }

    pub fn from_json_str(json: &str) -> Self {
        let mut cfg = Self::default();
        let bytes = json.as_bytes();
        let mut i = 0usize;
        let skip_ws = |idx: &mut usize| {
            while *idx < bytes.len() && bytes[*idx].is_ascii_whitespace() {
                *idx += 1;
            }
        };
        let parse_string = |idx: &mut usize| -> Option<String> {
            skip_ws(idx);
            if *idx >= bytes.len() || bytes[*idx] != b'"' {
                return None;
            }
            *idx += 1;
            let mut s = String::new();
            while *idx < bytes.len() {
                let b = bytes[*idx];
                *idx += 1;
                if b == b'"' {
                    return Some(s);
                }
                if b == b'\\' {
                    if *idx >= bytes.len() {
                        break;
                    }
                    let esc = bytes[*idx];
                    *idx += 1;
                    match esc {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' if *idx + 4 <= bytes.len() => {
                            if let Ok(hex) = std::str::from_utf8(&bytes[*idx..*idx + 4]) {
                                if let Ok(cp) = u32::from_str_radix(hex, 16) {
                                    if let Some(ch) = char::from_u32(cp) {
                                        s.push(ch);
                                    }
                                }
                            }
                            *idx += 4;
                        }
                        other => s.push(other as char),
                    }
                } else {
                    // Preserve UTF-8 multi-byte characters cleanly
                    let start = *idx - 1;
                    if b < 0x80 {
                        s.push(b as char);
                    } else if let Ok(rest) = std::str::from_utf8(&bytes[start..]) {
                        if let Some(ch) = rest.chars().next() {
                            s.push(ch);
                            *idx = start + ch.len_utf8();
                        }
                    }
                }
            }
            None
        };
        let parse_raw_token = |idx: &mut usize| -> Option<&str> {
            skip_ws(idx);
            let start = *idx;
            while *idx < bytes.len() {
                let b = bytes[*idx];
                if b == b',' || b == b'}' || b.is_ascii_whitespace() {
                    break;
                }
                *idx += 1;
            }
            (start < *idx).then(|| std::str::from_utf8(&bytes[start..*idx]).ok()).flatten()
        };

        skip_ws(&mut i);
        if i >= bytes.len() || bytes[i] != b'{' {
            return cfg;
        }
        i += 1;

        while i < bytes.len() {
            skip_ws(&mut i);
            if i >= bytes.len() || bytes[i] == b'}' {
                break;
            }
            if bytes[i] == b',' {
                i += 1;
                continue;
            }
            let Some(key) = parse_string(&mut i) else {
                break;
            };
            skip_ws(&mut i);
            if i >= bytes.len() || bytes[i] != b':' {
                break;
            }
            i += 1;
            skip_ws(&mut i);
            if i >= bytes.len() {
                break;
            }

            if bytes[i] == b'"' {
                let Some(val) = parse_string(&mut i) else {
                    break;
                };
                match key.as_str() {
                    "shiftToggleMode" => cfg.shift_toggle_mode = ShiftToggleMode::from_tag(&val),
                    "fourthEmptyBehavior" => {
                        cfg.fourth_empty_behavior = FourthEmptyBehavior::from_tag(&val)
                    }
                    _ => {}
                }
            } else if bytes[i] == b'{' || bytes[i] == b'[' {
                // Skip nested JSON container if any future caller passes extra fields
                let open = bytes[i];
                let close = if open == b'{' { b'}' } else { b']' };
                let mut depth = 1usize;
                i += 1;
                while i < bytes.len() && depth > 0 {
                    if bytes[i] == b'"' {
                        let _ = parse_string(&mut i);
                        continue;
                    }
                    if bytes[i] == open {
                        depth += 1;
                    } else if bytes[i] == close {
                        depth -= 1;
                    }
                    i += 1;
                }
            } else {
                let Some(tok) = parse_raw_token(&mut i) else {
                    break;
                };
                let as_bool = match tok {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                };
                match key.as_str() {
                    "schemaId" => {
                        if let Ok(id) = tok.parse::<u32>() {
                            cfg.schema = InputSchema::from_id(id);
                        }
                    }
                    "pageSize" => {
                        if let Ok(n) = tok.parse::<usize>() {
                            cfg.page_size = n.clamp(3, 9) as u8;
                        }
                    }
                    "vertical" => {
                        if let Some(v) = as_bool {
                            cfg.orientation = if v {
                                CandidateOrientation::Vertical
                            } else {
                                CandidateOrientation::Horizontal
                            };
                        }
                    }
                    "smartPunctuation" => {
                        if let Some(b) = as_bool {
                            cfg.smart_punctuation = b;
                        }
                    }
                    "semicolonQuoteSelect" => {
                        if let Some(b) = as_bool {
                            cfg.semicolon_quote_select = b;
                        }
                    }
                    "autoSpaceZhEn" => {
                        if let Some(b) = as_bool {
                            cfg.auto_space_zh_en = b;
                        }
                    }
                    "autoSpaceDigits" => {
                        if let Some(b) = as_bool {
                            cfg.auto_space_digits = b;
                        }
                    }
                    "clipboardSuggestion" => {
                        if let Some(b) = as_bool {
                            cfg.clipboard_suggestion = b;
                        }
                    }
                    "cangjieKeycapRadicals" => {
                        if let Some(b) = as_bool {
                            cfg.cangjie_keycap_radicals = b;
                        }
                    }
                    "wubiKeycapRadicals" => {
                        if let Some(b) = as_bool {
                            cfg.wubi_keycap_radicals = b;
                        }
                    }
                    "keySwipeUpInput" => {
                        if let Some(b) = as_bool {
                            cfg.key_swipe_up_input = b;
                        }
                    }
                    "spaceCommaKey" => {
                        if let Some(b) = as_bool {
                            cfg.space_comma_key = b;
                        }
                    }
                    "keyLongPressAlternates" => {
                        if let Some(b) = as_bool {
                            cfg.key_long_press_alternates = b;
                        }
                    }
                    "backspaceSwipeClear" => {
                        if let Some(b) = as_bool {
                            cfg.backspace_swipe_clear = b;
                        }
                    }
                    "keyboardHoldCursor" => {
                        if let Some(b) = as_bool {
                            cfg.keyboard_hold_cursor = b;
                        }
                    }
                    "evalLogging" => {
                        if let Some(b) = as_bool {
                            cfg.eval_logging = b;
                        }
                    }
                    "emojiCandidates" => {
                        if let Some(b) = as_bool {
                            cfg.emoji_candidates = b;
                        }
                    }
                    "fourthCodeAutoCommit" => {
                        if let Some(b) = as_bool {
                            cfg.fourth_code_auto_commit = b;
                        }
                    }
                    "fifthCodePushCommit" => {
                        if let Some(b) = as_bool {
                            cfg.fifth_code_push_commit = b;
                        }
                    }
                    "shortestCodeFirst" => {
                        if let Some(b) = as_bool {
                            cfg.shortest_code_first = b;
                        }
                    }
                    "strictShortcode" => {
                        if let Some(b) = as_bool {
                            cfg.strict_shortcode = b;
                        }
                    }
                    "spaceless" => {
                        if let Some(b) = as_bool {
                            cfg.spaceless = b;
                        }
                    }
                    "showShortcodeHint" => {
                        if let Some(b) = as_bool {
                            cfg.show_shortcode_hint = b;
                        }
                    }
                    "dynamicWordFreq" => {
                        if let Some(b) = as_bool {
                            cfg.dynamic_word_freq = b;
                        }
                    }
                    "dynamicCharFreq" => {
                        if let Some(b) = as_bool {
                            cfg.dynamic_char_freq = b;
                        }
                    }
                    "autoPhrase" => {
                        if let Some(b) = as_bool {
                            cfg.auto_phrase = b;
                        }
                    }
                    "autoPhraseThreshold" => {
                        if let Ok(n) = tok.parse::<u32>() {
                            cfg.auto_phrase_threshold = n.clamp(1, 5) as u8;
                        }
                    }
                    "zOmnipotent" => {
                        if let Some(b) = as_bool {
                            cfg.z_omnipotent = b;
                        }
                    }
                    "enableRareCharset" => {
                        if let Some(b) = as_bool {
                            cfg.enable_rare_charset = b;
                        }
                    }
                    "pinyinInitialAbbr" => {
                        if let Some(b) = as_bool {
                            cfg.pinyin_initial_abbr = b;
                        }
                    }
                    "pinyinT9Layout" => {
                        if let Some(b) = as_bool {
                            cfg.pinyin_t9_layout = b;
                        }
                    }
                    "typoEnabled" => {
                        if let Some(b) = as_bool {
                            cfg.typo.enabled = b;
                        }
                    }
                    "touchHitboxAdapt" => {
                        if let Some(b) = as_bool {
                            cfg.typo.touch_hitbox_adapt = b;
                        }
                    }
                    "promoteThreshold" => {
                        if let Ok(f) = tok.parse::<f32>() {
                            if f.is_finite() {
                                cfg.typo.promote_threshold = f.clamp(0.50, 0.99);
                            }
                        }
                    }
                    "suggestThreshold" => {
                        if let Ok(f) = tok.parse::<f32>() {
                            if f.is_finite() {
                                cfg.typo.suggest_threshold = f.clamp(0.10, 0.85);
                            }
                        }
                    }
                    "suppressAutoCommitOnCorrection" => {
                        if let Some(b) = as_bool {
                            cfg.typo.suppress_auto_commit_on_correction = b;
                        }
                    }
                    "missedSpaceCorrection" => {
                        if let Some(b) = as_bool {
                            cfg.typo.missed_space_correction = b;
                        }
                    }
                    _ => {}
                }
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
