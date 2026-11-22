#if os(iOS)
import Security

// The Personal Team profile grants shared keychain groups for this team's apps.
private enum IOSSharedSettings {
    static let preferencesAccount = "input-settings"
    static let touchProfileAccount = "touch-profile"
    static let evalLogAccount = "eval-log-ring"

    private static func query(account: String, groupKey: String = "JuxuSettingsGroup", service: String = "Juxu") -> [String: Any]? {
        guard let group = Bundle.main.object(forInfoDictionaryKey: groupKey) as? String else { return nil }
        return [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
                kSecAttrAccount as String: account, kSecAttrAccessGroup as String: group]
    }

    static func read(account: String) -> Data? {
        for (groupKey, service) in [("JuxuSettingsGroup", "Juxu"), ("PrisWubiSettingsGroup", "PrisWubi")] {
            if var q = query(account: account, groupKey: groupKey, service: service) {
                q[kSecReturnData as String] = true
                var result: CFTypeRef?
                if SecItemCopyMatching(q as CFDictionary, &result) == errSecSuccess, let data = result as? Data {
                    if groupKey != "JuxuSettingsGroup" {
                        write(data, account: account)
                    }
                    return data
                }
            }
        }
        return nil
    }

    @discardableResult
    static func write(_ data: Data, account: String) -> Bool {
        guard var query = query(account: account) else { return false }
        let update = [kSecValueData as String: data]
        let status = SecItemUpdate(query as CFDictionary, update as CFDictionary)
        if status == errSecItemNotFound {
            query[kSecValueData as String] = data
            query[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
            return SecItemAdd(query as CFDictionary, nil) == errSecSuccess
        }
        return status == errSecSuccess
    }
}
#endif
import Foundation

public struct JuxuCandidateItem: Codable, Identifiable, Equatable, Sendable {
    public let index: Int
    public let text: String
    public let comment: String
    public let badge: UInt8
    public let confidence: Float
    public let isExact: Bool

    public var id: Int { index }

    public enum BadgeKind {
        case none
        case shortcode
        case typoCorrected
        case wildcard
        case pinyinLookup
        case userPhrase
        case emoji
    }

    public var badgeKind: BadgeKind {
        switch badge {
        case 1: return .shortcode
        case 2: return .typoCorrected
        case 3: return .wildcard
        case 4: return .pinyinLookup
        case 5: return .userPhrase
        case 6: return .emoji
        default: return .none
        }
    }
}

public struct TouchKeyCalibrationItem: Codable, Identifiable, Equatable, Sendable {
    public let key: String
    public let dx: Float
    public let dy: Float
    public let samples: UInt32
    public var id: String { key }
}

public struct EvalLogSummary: Equatable, Sendable {
    public let totalEvents: Int
    public let commits: Int
    public let top1Hits: Int
    public let manualSelects: Int
    public let lmOverruled: Int
    public let typoTriggered: Int
    public let backspaceAfterCommit: Int
    public let abandons: Int

    public var top1RatePercent: Double {
        commits > 0 ? (Double(top1Hits) * 100.0 / Double(commits)) : 0.0
    }

    public static let empty = EvalLogSummary(
        totalEvents: 0,
        commits: 0,
        top1Hits: 0,
        manualSelects: 0,
        lmOverruled: 0,
        typoTriggered: 0,
        backspaceAfterCommit: 0,
        abandons: 0
    )
}

public struct JuxuActionState: Codable, Equatable, Sendable {
    public let consumed: Bool
    public let bell: Bool
    public let asciiMode: Bool
    public let schemaId: UInt32
    public let page: Int
    public let totalPages: Int
    public let vertical: Bool
    public let schemaName: String
    public let commitText: String
    public let preedit: String
    public let segmentedCode: String
    public let notice: String
    public let userPhraseCount: Int
    public let calibratedKeyCount: Int
    public let validNextMask: UInt32
    public let nextKeyPriors: [UInt8]
    public let touchOffsets: [Float]
    public let t9Mode: Bool
    public let t9ValidNextMask: UInt8
    public let t9NextKeyPriors: [UInt8]
    public let t9TouchOffsets: [Float]
    public let t9LockedPrefix: String
    public let t9Syllables: [String]
    public let candidates: [JuxuCandidateItem]

    /// Active inline preedit shown in the host text field (`markedText`):
    /// uses the current uncommitted segment after `" │ "` when segmented code is present.
    public var activeInlinePreedit: String {
        guard !preedit.isEmpty else { return "" }
        guard !segmentedCode.isEmpty else { return preedit }
        if let boundary = segmentedCode.range(of: " │ ") {
            return String(segmentedCode[boundary.upperBound...])
        }
        return segmentedCode
    }

    #if os(macOS)
    var macOSPreedit: String { activeInlinePreedit }
    #endif

    public init(
        consumed: Bool,
        bell: Bool,
        asciiMode: Bool,
        schemaId: UInt32,
        page: Int,
        totalPages: Int,
        vertical: Bool,
        schemaName: String,
        commitText: String,
        preedit: String,
        segmentedCode: String = "",
        notice: String,
        userPhraseCount: Int,
        calibratedKeyCount: Int,
        validNextMask: UInt32 = 0x03FF_FFFF,
        nextKeyPriors: [UInt8] = Array(repeating: 100, count: 26),
        touchOffsets: [Float] = Array(repeating: 0, count: 52),
        t9Mode: Bool = false,
        t9ValidNextMask: UInt8 = 0xFF,
        t9NextKeyPriors: [UInt8] = Array(repeating: 100, count: 8),
        t9TouchOffsets: [Float] = Array(repeating: 0, count: 16),
        t9LockedPrefix: String = "",
        t9Syllables: [String] = [],
        candidates: [JuxuCandidateItem]
    ) {
        self.consumed = consumed
        self.bell = bell
        self.asciiMode = asciiMode
        self.schemaId = schemaId
        self.page = page
        self.totalPages = totalPages
        self.vertical = vertical
        self.schemaName = schemaName
        self.commitText = commitText
        self.preedit = preedit
        self.segmentedCode = segmentedCode
        self.notice = notice
        self.userPhraseCount = userPhraseCount
        self.calibratedKeyCount = calibratedKeyCount
        self.validNextMask = validNextMask
        self.nextKeyPriors = nextKeyPriors
        self.touchOffsets = touchOffsets
        self.t9Mode = t9Mode
        self.t9ValidNextMask = t9ValidNextMask
        self.t9NextKeyPriors = t9NextKeyPriors
        self.t9TouchOffsets = t9TouchOffsets
        self.t9LockedPrefix = t9LockedPrefix
        self.t9Syllables = t9Syllables
        self.candidates = candidates
    }

    private enum CodingKeys: String, CodingKey {
        case consumed, bell, asciiMode, schemaId, page, totalPages, vertical
        case schemaName, commitText, preedit, segmentedCode, notice, userPhraseCount, calibratedKeyCount
        case validNextMask, nextKeyPriors, touchOffsets
        case t9Mode, t9ValidNextMask, t9NextKeyPriors, t9TouchOffsets, t9LockedPrefix, t9Syllables
        case candidates
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.consumed = try c.decode(Bool.self, forKey: .consumed)
        self.bell = try c.decode(Bool.self, forKey: .bell)
        self.asciiMode = try c.decode(Bool.self, forKey: .asciiMode)
        self.schemaId = try c.decode(UInt32.self, forKey: .schemaId)
        self.page = try c.decode(Int.self, forKey: .page)
        self.totalPages = try c.decode(Int.self, forKey: .totalPages)
        self.vertical = try c.decode(Bool.self, forKey: .vertical)
        self.schemaName = try c.decode(String.self, forKey: .schemaName)
        self.commitText = try c.decode(String.self, forKey: .commitText)
        self.preedit = try c.decode(String.self, forKey: .preedit)
        self.segmentedCode = try c.decodeIfPresent(String.self, forKey: .segmentedCode) ?? ""
        self.notice = try c.decode(String.self, forKey: .notice)
        self.userPhraseCount = try c.decode(Int.self, forKey: .userPhraseCount)
        self.calibratedKeyCount = try c.decode(Int.self, forKey: .calibratedKeyCount)
        self.validNextMask = try c.decodeIfPresent(UInt32.self, forKey: .validNextMask) ?? 0x03FF_FFFF
        self.nextKeyPriors = try c.decodeIfPresent([UInt8].self, forKey: .nextKeyPriors) ?? Array(repeating: 100, count: 26)
        self.touchOffsets = try c.decodeIfPresent([Float].self, forKey: .touchOffsets) ?? Array(repeating: 0, count: 52)
        self.t9Mode = try c.decodeIfPresent(Bool.self, forKey: .t9Mode) ?? false
        self.t9ValidNextMask = try c.decodeIfPresent(UInt8.self, forKey: .t9ValidNextMask) ?? 0xFF
        self.t9NextKeyPriors = try c.decodeIfPresent([UInt8].self, forKey: .t9NextKeyPriors) ?? Array(repeating: 100, count: 8)
        self.t9TouchOffsets = try c.decodeIfPresent([Float].self, forKey: .t9TouchOffsets) ?? Array(repeating: 0, count: 16)
        self.t9LockedPrefix = try c.decodeIfPresent(String.self, forKey: .t9LockedPrefix) ?? ""
        self.t9Syllables = try c.decodeIfPresent([String].self, forKey: .t9Syllables) ?? []
        self.candidates = try c.decode([JuxuCandidateItem].self, forKey: .candidates)
    }

    public static let empty = JuxuActionState(
        consumed: false,
        bell: false,
        asciiMode: false,
        schemaId: 86,
        page: 0,
        totalPages: 0,
        vertical: false,
        schemaName: "86版五笔",
        commitText: "",
        preedit: "",
        notice: "",
        userPhraseCount: 0,
        calibratedKeyCount: 0,
        validNextMask: 0x03FF_FFFF,
        candidates: []
    )
}

public struct UserPhraseRecord: Codable, Identifiable, Equatable, Sendable {
    public let text: String
    public let code: String
    public let count: UInt32
    public let auto: Bool
    public var id: String { text }
}

public enum InputSchemaCategory: String, CaseIterable, Identifiable, Sendable {
    case pinyin = "拼音"
    case wubi = "五笔"
    case cangjie = "仓颉"

    public var id: String { rawValue }

    public var subtitle: String {
        switch self {
        case .pinyin: return "全拼 · 小鹤双拼 · 小鹤音形"
        case .wubi: return "86 版 · 98 版 · 新世纪"
        case .cangjie: return "仓颉三代 · 五代 · 六代"
        }
    }

    public var symbolName: String {
        switch self {
        case .pinyin: return "character.phonetic"
        case .wubi: return "square.grid.2x2"
        case .cangjie: return "character.book.closed"
        }
    }

    public var defaultSchemaId: UInt32 {
        switch self {
        case .pinyin: return 20
        case .wubi: return 86
        case .cangjie: return 35
        }
    }

    public static func category(for schemaId: UInt32) -> InputSchemaCategory {
        switch schemaId {
        case 20, 21, 10: return .pinyin
        case 33, 35, 36: return .cangjie
        default: return .wubi
        }
    }
}

public struct InputSchemaOption: Identifiable, Equatable, Sendable {
    public let id: UInt32
    public let category: InputSchemaCategory
    public let title: String
    public let shortTitle: String
    public let tag: String
    public let desc: String

    public static let all: [InputSchemaOption] = [
        InputSchemaOption(id: 20, category: .pinyin, title: "全拼", shortTitle: "全拼", tag: "整句简拼", desc: "整句连打 · 声母简拼 · 触点纠错"),
        InputSchemaOption(id: 21, category: .pinyin, title: "小鹤双拼", shortTitle: "小鹤双拼", tag: "双拼整句", desc: "小鹤双拼键位 · 整句连打 · 触点纠错"),
        InputSchemaOption(id: 10, category: .pinyin, title: "小鹤音形", shortTitle: "小鹤音形", tag: "双拼+鹤形", desc: "双拼 + 鹤形辅助码 · 定长/整句"),
        InputSchemaOption(id: 86, category: .wubi, title: "86 版五笔", shortTitle: "86 版", tag: "经典五笔", desc: "王永民经典版 · 通用度最广"),
        InputSchemaOption(id: 98, category: .wubi, title: "98 版五笔", shortTitle: "98 版", tag: "规范码元", desc: "码元规范化 · 拆分更科学"),
        InputSchemaOption(id: 6, category: .wubi, title: "新世纪五笔", shortTitle: "新世纪", tag: "06 国标", desc: "2006 第三代国家规范码表"),
        InputSchemaOption(id: 33, category: .cangjie, title: "仓颉三代", shortTitle: "三代", tag: "经典三代", desc: "传统三代码表 · 港澳台经典通用"),
        InputSchemaOption(id: 35, category: .cangjie, title: "仓颉五代", shortTitle: "五代", tag: "标准五代", desc: "朱邦复第五代标准 · 完整字集"),
        InputSchemaOption(id: 36, category: .cangjie, title: "仓颉六代", shortTitle: "六代", tag: "苍颉六代", desc: "第六代苍颉检字法 · 扩展全字集"),
    ]

    public static func options(for category: InputSchemaCategory) -> [InputSchemaOption] {
        all.filter { $0.category == category }
    }
}

public struct JuxuPreferences: Equatable, Sendable {
    public static let appGroupIdentifier = "group.com.hcyue.juxu.shared"

    public var schemaId: UInt32 = 86
    public var pageSize: Int = 5
    public var vertical: Bool = false
    public var shiftToggleMode: String = "commit_code"
    public var smartPunctuation: Bool = true
    public var semicolonQuoteSelect: Bool = false
    public var autoSpaceZhEn: Bool = true
    public var autoSpaceDigits: Bool = true
    public var clipboardSuggestion: Bool = true
    public var cangjieKeycapRadicals: Bool = true
    public var wubiKeycapRadicals: Bool = false
    public var keySwipeUpInput: Bool = false
    public var keyLongPressAlternates: Bool = false
    public var backspaceSwipeClear: Bool = false
    public var evalLogging: Bool = true
    public var emojiCandidates: Bool = false

    public var fourthCodeAutoCommit: Bool = true
    public var fifthCodePushCommit: Bool = true
    public var fourthEmptyBehavior: String = "block"
    public var shortestCodeFirst: Bool = true
    public var strictShortcode: Bool = false
    public var spaceless: Bool = false
    public var showShortcodeHint: Bool = true
    public var dynamicWordFreq: Bool = true
    public var dynamicCharFreq: Bool = false
    public var autoPhrase: Bool = true
    public var autoPhraseThreshold: Int = 2
    public var zOmnipotent: Bool = true
    public var enableRareCharset: Bool = false
    public var pinyinInitialAbbr: Bool = true
    public var pinyinT9Layout: Bool = false

    public var typoEnabled: Bool = true
    public var touchHitboxAdapt: Bool = true
    public var promoteThreshold: Double = 0.82
    public var suggestThreshold: Double = 0.30
    public var suppressAutoCommitOnCorrection: Bool = true
    public var missedSpaceCorrection: Bool = true

    public var isPinyinFamily: Bool {
        schemaId == 20 || schemaId == 21
    }

    public var isCangjieFamily: Bool {
        schemaId == 33 || schemaId == 35 || schemaId == 36
    }

    public var isWubiFamily: Bool {
        schemaId == 86 || schemaId == 98 || schemaId == 6
    }

    public var schemaCategory: InputSchemaCategory {
        InputSchemaCategory.category(for: schemaId)
    }

    public init() {}

    public func toTOML() -> String {
        let schemaTag: String
        switch schemaId {
        case 98: schemaTag = "98"
        case 6: schemaTag = "06"
        case 10: schemaTag = "xiaohe"
        case 20: schemaTag = "pinyin"
        case 21: schemaTag = "xiaohe_sp"
        case 33: schemaTag = "cangjie3"
        case 35: schemaTag = "cangjie5"
        case 36: schemaTag = "cangjie6"
        default: schemaTag = "86"
        }
        return """
        # 句序 (Juxu) 配置文件 — macOS 与 iOS 通用内核
        # 修改保存后将实时生效

        [general]
        schema = "\(schemaTag)"
        page_size = \(pageSize)
        orientation = "\(vertical ? "vertical" : "horizontal")"
        shift_toggle_mode = "\(shiftToggleMode)"
        smart_punctuation = \(smartPunctuation)
        semicolon_quote_select = \(semicolonQuoteSelect)
        auto_space_zh_en = \(autoSpaceZhEn)
        auto_space_digits = \(autoSpaceDigits)
        clipboard_suggestion = \(clipboardSuggestion)
        cangjie_keycap_radicals = \(cangjieKeycapRadicals)
        wubi_keycap_radicals = \(wubiKeycapRadicals)
        key_swipe_up_input = \(keySwipeUpInput)
        key_long_press_alternates = \(keyLongPressAlternates)
        backspace_swipe_clear = \(backspaceSwipeClear)
        emoji_candidates = \(emojiCandidates)
        eval_logging = \(evalLogging)

        [typing]
        fourth_code_auto_commit = \(fourthCodeAutoCommit)
        fifth_code_push_commit = \(fifthCodePushCommit)
        fourth_empty_behavior = "\(fourthEmptyBehavior)"
        shortest_code_first = \(shortestCodeFirst)
        strict_shortcode = \(strictShortcode)
        spaceless = \(spaceless)
        show_shortcode_hint = \(showShortcodeHint)
        dynamic_word_freq = \(dynamicWordFreq)
        dynamic_char_freq = \(dynamicCharFreq)
        auto_phrase = \(autoPhrase)
        auto_phrase_threshold = \(autoPhraseThreshold)
        z_omnipotent = \(zOmnipotent)
        enable_rare_charset = \(enableRareCharset)
        pinyin_initial_abbr = \(pinyinInitialAbbr)
        pinyin_t9_layout = \(pinyinT9Layout)

        [typo_correction]
        enabled = \(typoEnabled)
        touch_hitbox_adapt = \(touchHitboxAdapt)
        promote_threshold = \(String(format: "%.2f", promoteThreshold))
        suggest_threshold = \(String(format: "%.2f", suggestThreshold))
        suppress_auto_commit_on_correction = \(suppressAutoCommitOnCorrection)
        missed_space_correction = \(missedSpaceCorrection)
        """
    }

    public static func fromTOML(_ toml: String) -> JuxuPreferences {
        var p = JuxuPreferences()
        var section = ""
        for rawLine in toml.split(separator: "\n", omittingEmptySubsequences: false) {
            let clean = rawLine.split(separator: "#", maxSplits: 1, omittingEmptySubsequences: false).first ?? ""
            let line = clean.trimmingCharacters(in: .whitespacesAndNewlines)
            if line.isEmpty { continue }
            if line.hasPrefix("[") && line.hasSuffix("]") {
                section = String(line.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces).lowercased()
                continue
            }
            let parts = line.split(separator: "=", maxSplits: 1)
            guard parts.count == 2 else { continue }
            let key = parts[0].trimmingCharacters(in: .whitespaces)
            let val = parts[1].trimmingCharacters(in: .whitespaces)
                .trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
            let boolVal = ["true", "1", "yes"].contains(val.lowercased())

            switch (section, key) {
            case (_, "schema"):
                let lower = val.lowercased()
                if lower == "98" { p.schemaId = 98 }
                else if lower == "06" || lower == "2006" { p.schemaId = 6 }
                else if lower == "xiaohe" || lower == "flypy" || lower == "10" { p.schemaId = 10 }
                else if lower == "pinyin" || lower == "quanpin" || lower == "20" { p.schemaId = 20 }
                else if lower == "xiaohe_sp" || lower == "shuangpin" || lower == "sp" || lower == "flypy_sp" || lower == "xhsp" || lower == "21" { p.schemaId = 21 }
                else if lower == "cangjie3" || lower == "cangjie_3" || lower == "cj3" || lower == "33" { p.schemaId = 33 }
                else if lower == "cangjie5" || lower == "cangjie_5" || lower == "cj5" || lower == "cangjie" || lower == "cj" || lower == "35" { p.schemaId = 35 }
                else if lower == "cangjie6" || lower == "cangjie_6" || lower == "cj6" || lower == "36" { p.schemaId = 36 }
                else { p.schemaId = 86 }
            case (_, "page_size"):
                p.pageSize = min(9, max(3, Int(val) ?? 5))
            case (_, "orientation"):
                p.vertical = (val.lowercased() == "vertical")
            case (_, "shift_toggle_mode"):
                p.shiftToggleMode = val
            case (_, "smart_punctuation"):
                p.smartPunctuation = boolVal
            case (_, "semicolon_quote_select"):
                p.semicolonQuoteSelect = boolVal
            case (_, "auto_space_zh_en"):
                p.autoSpaceZhEn = boolVal
            case (_, "auto_space_digits"):
                p.autoSpaceDigits = boolVal
            case (_, "clipboard_suggestion"):
                p.clipboardSuggestion = boolVal
            case (_, "cangjie_keycap_radicals"):
                p.cangjieKeycapRadicals = boolVal
            case (_, "wubi_keycap_radicals"):
                p.wubiKeycapRadicals = boolVal
            case (_, "key_swipe_up_input"):
                p.keySwipeUpInput = boolVal
            case (_, "key_long_press_alternates"):
                p.keyLongPressAlternates = boolVal
            case (_, "backspace_swipe_clear"):
                p.backspaceSwipeClear = boolVal
            case (_, "emoji_candidates"):
                p.emojiCandidates = boolVal
            case (_, "eval_logging"):
                p.evalLogging = boolVal
            case (_, "fourth_code_auto_commit"):
                p.fourthCodeAutoCommit = boolVal
            case (_, "fifth_code_push_commit"):
                p.fifthCodePushCommit = boolVal
            case (_, "fourth_empty_behavior"):
                p.fourthEmptyBehavior = val
            case (_, "shortest_code_first"):
                p.shortestCodeFirst = boolVal
            case (_, "strict_shortcode"):
                p.strictShortcode = boolVal
            case (_, "spaceless"):
                p.spaceless = boolVal
            case (_, "show_shortcode_hint"):
                p.showShortcodeHint = boolVal
            case (_, "dynamic_word_freq"):
                p.dynamicWordFreq = boolVal
            case (_, "dynamic_char_freq"):
                p.dynamicCharFreq = boolVal
            case (_, "auto_phrase"):
                p.autoPhrase = boolVal
            case (_, "auto_phrase_threshold"):
                p.autoPhraseThreshold = min(5, max(1, Int(val) ?? 2))
            case (_, "z_omnipotent"):
                p.zOmnipotent = boolVal
            case (_, "enable_rare_charset"):
                p.enableRareCharset = boolVal
            case (_, "pinyin_initial_abbr"):
                p.pinyinInitialAbbr = boolVal
            case (_, "pinyin_t9_layout"):
                p.pinyinT9Layout = boolVal
            case ("typo_correction", "enabled"):
                p.typoEnabled = boolVal
            case (_, "touch_hitbox_adapt"):
                p.touchHitboxAdapt = boolVal
            case (_, "promote_threshold"):
                p.promoteThreshold = min(0.99, max(0.50, Double(val) ?? 0.82))
            case (_, "suggest_threshold"):
                p.suggestThreshold = min(0.85, max(0.10, Double(val) ?? 0.30))
            case (_, "suppress_auto_commit_on_correction"):
                p.suppressAutoCommitOnCorrection = boolVal
            case (_, "missed_space_correction"):
                p.missedSpaceCorrection = boolVal
            default:
                break
            }
        }
        return p
    }
}

@MainActor
public final class JuxuBridge: ObservableObject {
    public static let shared = JuxuBridge()

    nonisolated(unsafe) private var handle: OpaquePointer?
    private let decoder = JSONDecoder()
    private var resolvedDictsDir: URL?
    private var emojiTableLoaded = false
    private var loadedDictSchemas: Set<UInt32> = []
    private var pinyinDictLoaded: Bool = false
    private var auxDictLoaded: Bool = false
    private var lastLoadedUserDataMtime: Date?
    #if os(iOS)
    private let inMemoryOnly: Bool
    private var lastSharedTouchProfile: [TouchKeyCalibrationItem]?
    #endif

    @Published public private(set) var state: JuxuActionState = .empty
    @Published public var prefs: JuxuPreferences = JuxuPreferences()
    @Published public private(set) var userPhrases: [UserPhraseRecord] = []
    @Published public private(set) var touchCalibrations: [TouchKeyCalibrationItem] = []
    @Published public private(set) var evalLogSummary: EvalLogSummary = .empty
    /// When true (e.g. password / OTP / numeric field), queued eval events are drained and discarded.
    public var suppressEvalLogging: Bool = false

    public let configURL: URL
    public let userDataURL: URL
    public let evalLogURL: URL
    public let hasSharedContainerAccess: Bool
    private let bridgeInMemoryOnly: Bool
    #if os(iOS)
    private var iosEvalRingLoaded: Bool = false
    private var iosEvalRingLines: [String] = []
    #endif

    public init(customDictsDir: URL? = nil, inMemoryOnly: Bool = false) {
        self.bridgeInMemoryOnly = inMemoryOnly
        #if os(iOS)
        self.inMemoryOnly = inMemoryOnly
        #endif
        #if os(macOS)
        let home = URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true)
        let dotfile = home.appendingPathComponent(".juxu.toml")
        let legacyDotfile = home.appendingPathComponent(".priswubi.toml")
        let appSupport = home
            .appendingPathComponent("Library/Application Support/Juxu", isDirectory: true)
        let legacyAppSupport = home
            .appendingPathComponent("Library/Application Support/PrisWubi", isDirectory: true)
        if !inMemoryOnly {
            let fm = FileManager.default
            if !fm.fileExists(atPath: appSupport.path), fm.fileExists(atPath: legacyAppSupport.path) {
                try? fm.createDirectory(at: appSupport, withIntermediateDirectories: true)
                for file in ["config.toml", "user_data.tsv"] {
                    let src = legacyAppSupport.appendingPathComponent(file)
                    let dst = appSupport.appendingPathComponent(file)
                    if fm.fileExists(atPath: src.path), !fm.fileExists(atPath: dst.path) {
                        try? fm.copyItem(at: src, to: dst)
                    }
                }
            } else {
                try? fm.createDirectory(at: appSupport, withIntermediateDirectories: true)
            }
        }
        if FileManager.default.fileExists(atPath: dotfile.path) {
            self.configURL = dotfile
        } else if FileManager.default.fileExists(atPath: legacyDotfile.path) {
            self.configURL = legacyDotfile
        } else {
            self.configURL = appSupport.appendingPathComponent("config.toml")
        }
        self.userDataURL = appSupport.appendingPathComponent("user_data.tsv")
        self.evalLogURL = appSupport.appendingPathComponent("eval_log.jsonl")
        self.hasSharedContainerAccess = true
        #else
        if let groupURL = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: JuxuPreferences.appGroupIdentifier
        ) {
            self.configURL = groupURL.appendingPathComponent("config.toml")
            self.userDataURL = groupURL.appendingPathComponent("user_data.tsv")
            self.evalLogURL = groupURL.appendingPathComponent("eval_log.jsonl")
            self.hasSharedContainerAccess = true
        } else {
            let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first
                ?? URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            self.configURL = docs.appendingPathComponent("config.toml")
            self.userDataURL = docs.appendingPathComponent("user_data.tsv")
            self.evalLogURL = docs.appendingPathComponent("eval_log.jsonl")
            self.hasSharedContainerAccess = false
        }
        #endif

        self.handle = juxu_engine_new()
        loadDictionaries(overrideDir: customDictsDir)
        if !inMemoryOnly {
            loadConfigAndUser()
        } else {
            ensureDictionaryLoaded(for: prefs.schemaId)
            refreshState()
            refreshTouchProfile()
        }
    }

    deinit {
        if let h = handle {
            juxu_engine_free(h)
        }
    }

    public func loadDictionaries(overrideDir: URL? = nil) {
        guard let h = handle else { return }
        let candidateDirs: [URL] = [
            overrideDir,
            // On iOS App Extensions (`JuxuApp.app/PlugIns/JuxuKeyboard.appex`), prefer the
            // containing app's single `JuxuApp.app/Dicts` copy first so stale extension copies
            // can never shadow freshly bundled dictionaries.
            Bundle.main.bundleURL
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("Dicts", isDirectory: true),
            Bundle.main.resourceURL?.appendingPathComponent("Dicts", isDirectory: true),
            Bundle.main.resourceURL,
            URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
                .appendingPathComponent("Resources/Dicts", isDirectory: true),
        ].compactMap { $0 }

        for dir in candidateDirs {
            let p86 = dir.appendingPathComponent("wubi86.wbdict").path
            if FileManager.default.fileExists(atPath: p86) {
                resolvedDictsDir = dir
                if !auxDictLoaded {
                    let pAux = dir.appendingPathComponent("wubi_aux.wbaux").path
                    if pAux.withCString({ juxu_engine_load_aux_mmap(h, $0) }) {
                        auxDictLoaded = true
                    }
                }
                let pEn = dir.appendingPathComponent("english.endict").path
                if FileManager.default.fileExists(atPath: pEn) {
                    _ = pEn.withCString { juxu_engine_load_english_dict_mmap(h, $0) }
                }
                #if os(macOS)
                for sid: UInt32 in [86, 98, 6, 10, 33, 35, 36] {
                    ensureDictionaryLoaded(for: sid)
                }
                ensureDictionaryLoaded(for: 20)
                #endif
                break
            }
        }
    }

    public func ensureDictionaryLoaded(for schemaId: UInt32) {
        guard let h = handle else { return }
        if resolvedDictsDir == nil {
            loadDictionaries()
        }
        guard let dir = resolvedDictsDir else { return }
        if !auxDictLoaded {
            let pAux = dir.appendingPathComponent("wubi_aux.wbaux").path
            if pAux.withCString({ juxu_engine_load_aux_mmap(h, $0) }) {
                auxDictLoaded = true
            }
        }
        if schemaId == 20 || schemaId == 21 {
            #if os(iOS)
            if !loadedDictSchemas.isEmpty {
                juxu_engine_retain_only_schema_dict(h, schemaId)
                loadedDictSchemas.removeAll()
            }
            #endif
            if !pinyinDictLoaded || !juxu_engine_has_active_dict(h) {
                let pPy = dir.appendingPathComponent("pinyin.pydict").path
                if pPy.withCString({ juxu_engine_load_pinyin_dict_mmap(h, $0) }) {
                    pinyinDictLoaded = true
                }
            }
            return
        }
        #if os(iOS)
        // Release the 67 MB Pinyin dictionary and other unused dicts BEFORE mapping the new `.wbdict`
        // so iOS keyboard extension never peaks above its memory ceiling during a schema switch.
        if pinyinDictLoaded || loadedDictSchemas.contains(where: { $0 != schemaId }) {
            juxu_engine_retain_only_schema_dict(h, schemaId)
            pinyinDictLoaded = false
            loadedDictSchemas = loadedDictSchemas.contains(schemaId) ? [schemaId] : []
        }
        #endif
        if !loadedDictSchemas.contains(schemaId) || (juxu_engine_get_schema(h) == schemaId && !juxu_engine_has_active_dict(h)) {
            let fileName: String
            switch schemaId {
            case 98: fileName = "wubi98.wbdict"
            case 6: fileName = "wubi06.wbdict"
            case 10: fileName = "xiaohe.wbdict"
            case 33: fileName = "cangjie3.wbdict"
            case 35: fileName = "cangjie5.wbdict"
            case 36: fileName = "cangjie6.wbdict"
            default: fileName = "wubi86.wbdict"
            }
            let path = dir.appendingPathComponent(fileName).path
            if path.withCString({ juxu_engine_load_dict_mmap(h, schemaId, $0) }) {
                loadedDictSchemas.insert(schemaId)
            }
        }
    }

    public func loadConfigAndUser(refreshPublishedLists: Bool = true) {
        guard let h = handle else { return }
        var uiPrefsSourceTOML: String? = nil
        if FileManager.default.fileExists(atPath: configURL.path) {
            _ = configURL.path.withCString { juxu_engine_load_config(h, $0) }
            uiPrefsSourceTOML = try? String(contentsOf: configURL, encoding: .utf8)
        } else {
            _ = configURL.path.withCString { juxu_engine_save_config(h, $0) }
        }
        #if os(iOS)
        if let data = IOSSharedSettings.read(account: IOSSharedSettings.preferencesAccount),
           let shared = String(data: data, encoding: .utf8) {
            shared.withCString { juxu_engine_set_config_toml(h, $0) }
            uiPrefsSourceTOML = shared
        }
        loadSharedTouchProfile()
        #endif
        let toml = takeString(juxu_engine_get_config_toml(h))
        var loaded = JuxuPreferences.fromTOML(toml)
        if let uiTOML = uiPrefsSourceTOML {
            let uiPrefs = JuxuPreferences.fromTOML(uiTOML)
            loaded.clipboardSuggestion = uiPrefs.clipboardSuggestion
            loaded.cangjieKeycapRadicals = uiPrefs.cangjieKeycapRadicals
            loaded.wubiKeycapRadicals = uiPrefs.wubiKeycapRadicals
            loaded.keySwipeUpInput = uiPrefs.keySwipeUpInput
            loaded.keyLongPressAlternates = uiPrefs.keyLongPressAlternates
            loaded.backspaceSwipeClear = uiPrefs.backspaceSwipeClear
        }
        self.prefs = loaded
        ensureDictionaryLoaded(for: loaded.schemaId)
        ensureEmojiTableLoaded()
        if FileManager.default.fileExists(atPath: userDataURL.path) {
            let mtime = (try? FileManager.default.attributesOfItem(atPath: userDataURL.path)[.modificationDate]) as? Date
            if lastLoadedUserDataMtime == nil || mtime == nil || mtime != lastLoadedUserDataMtime {
                if userDataURL.path.withCString({ juxu_engine_load_user_data(h, $0) }) {
                    lastLoadedUserDataMtime = mtime
                }
            }
        }
        refreshState()
        if refreshPublishedLists {
            refreshUserPhrases()
            refreshTouchProfile()
        }
    }

    /// The emoji table is a separate read-only file, mapped only once the feature is switched on.
    private func ensureEmojiTableLoaded() {
        guard prefs.emojiCandidates, !emojiTableLoaded, let h = handle, let dir = resolvedDictsDir else { return }
        let path = dir.appendingPathComponent("emoji.tsv").path
        emojiTableLoaded = path.withCString { juxu_engine_load_emoji_table_mmap(h, $0) }
    }

    public func applyAndSavePreferences(_ newPrefs: JuxuPreferences, persist: Bool = true) {
        guard let h = handle else { return }
        self.prefs = newPrefs
        ensureDictionaryLoaded(for: newPrefs.schemaId)
        ensureEmojiTableLoaded()
        let toml = newPrefs.toTOML()
        toml.withCString { juxu_engine_set_config_toml(h, $0) }
        if persist {
            #if os(iOS)
            IOSSharedSettings.write(Data(toml.utf8), account: IOSSharedSettings.preferencesAccount)
            #endif
            _ = configURL.path.withCString { juxu_engine_save_config(h, $0) }
            try? toml.write(to: configURL, atomically: true, encoding: .utf8)
        }
        refreshState()
        refreshUserPhrases()
    }

    public func switchSchema(_ schemaId: UInt32, persist: Bool = true) {
        var updated = prefs
        updated.schemaId = schemaId
        applyAndSavePreferences(updated, persist: persist)
    }

    public func setKeyLayout(
        centers52: [Float],
        originX: Float,
        originY: Float,
        pitchX: Float,
        pitchY: Float
    ) {
        guard let h = handle, centers52.count == 52 else { return }
        centers52.withUnsafeBufferPointer { buf in
            juxu_engine_set_key_layout(h, buf.baseAddress, originX, originY, pitchX, pitchY)
        }
    }

    public func setT9Layout(
        centers16: [Float],
        originX: Float,
        originY: Float,
        pitchX: Float,
        pitchY: Float
    ) {
        guard let h = handle, centers16.count == 16 else { return }
        centers16.withUnsafeBufferPointer { buf in
            juxu_engine_set_t9_layout(h, buf.baseAddress, originX, originY, pitchX, pitchY)
        }
    }

    public func flushUserDataIfNeeded(refreshPublishedLists: Bool = true) {
        guard let h = handle else { return }
        if juxu_engine_is_user_dirty(h) {
            #if os(iOS)
            publishTouchProfile()
            #endif
            _ = userDataURL.path.withCString { juxu_engine_save_user_data(h, $0) }
            lastLoadedUserDataMtime = (try? FileManager.default.attributesOfItem(atPath: userDataURL.path)[.modificationDate]) as? Date
            if refreshPublishedLists {
                refreshUserPhrases()
                refreshTouchProfile()
            }
        }
    }

    public func handleKey(
        keyCode: UInt16,
        scalar: UInt32,
        hasCmdOrCtrl: Bool,
        timestampMs: UInt64
    ) -> JuxuActionState {
        guard let h = handle else { return .empty }
        if !juxu_engine_has_active_dict(h) {
            ensureDictionaryLoaded(for: juxu_engine_get_schema(h))
        }
        let json = takeString(juxu_engine_handle_key_json(h, keyCode, scalar, hasCmdOrCtrl, timestampMs))
        let decoded = decodeState(json)
        self.state = decoded
        drainEvalLogIfNeeded()
        return decoded
    }

    public func pushTouch(
        reportedAscii: UInt8,
        x: Float,
        y: Float,
        isNormalized: Bool,
        timestampMs: UInt64
    ) -> JuxuActionState {
        guard let h = handle else { return .empty }
        if !juxu_engine_has_active_dict(h) {
            ensureDictionaryLoaded(for: juxu_engine_get_schema(h))
        }
        let json = takeString(juxu_engine_push_touch_json(h, reportedAscii, x, y, isNormalized, timestampMs))
        let decoded = decodeState(json)
        self.state = decoded
        drainEvalLogIfNeeded()
        return decoded
    }

    public func pushT9Touch(
        reportedDigit: UInt8,
        x: Float,
        y: Float,
        isNormalized: Bool,
        timestampMs: UInt64
    ) -> JuxuActionState {
        guard let h = handle else { return .empty }
        if !juxu_engine_has_active_dict(h) {
            ensureDictionaryLoaded(for: juxu_engine_get_schema(h))
        }
        let json = takeString(juxu_engine_push_t9_touch_json(h, reportedDigit, x, y, isNormalized, timestampMs))
        let decoded = decodeState(json)
        self.state = decoded
        drainEvalLogIfNeeded()
        return decoded
    }

    public func lockT9Syllable(_ syllable: String) -> JuxuActionState {
        guard let h = handle else { return .empty }
        let json = syllable.withCString { cStr in
            takeString(juxu_engine_lock_t9_syllable_json(h, cStr))
        }
        let decoded = decodeState(json)
        self.state = decoded
        return decoded
    }

    public func selectCandidate(pageRelIndex: Int, timestampMs: UInt64) -> JuxuActionState {
        guard let h = handle else { return .empty }
        let json = takeString(juxu_engine_select_candidate_json(h, UInt32(max(0, pageRelIndex)), timestampMs))
        let decoded = decodeState(json)
        self.state = decoded
        drainEvalLogIfNeeded()
        return decoded
    }

    public func selectAbsCandidate(absIndex: Int, timestampMs: UInt64) -> JuxuActionState {
        guard let h = handle else { return .empty }
        let json = takeString(juxu_engine_select_abs_candidate_json(h, UInt32(max(0, absIndex)), timestampMs))
        let decoded = decodeState(json)
        self.state = decoded
        drainEvalLogIfNeeded()
        return decoded
    }

    public func allCandidates(maxCount: Int = 60) -> [JuxuCandidateItem] {
        guard let h = handle else { return [] }
        let json = takeString(juxu_engine_all_candidates_json(h, UInt32(max(1, maxCount))))
        if let data = json.data(using: .utf8),
           let list = try? decoder.decode([JuxuCandidateItem].self, from: data) {
            return list
        }
        return []
    }

    public func turnPage(delta: Int32) -> JuxuActionState {
        guard let h = handle else { return .empty }
        let json = takeString(juxu_engine_page_turn_json(h, delta))
        let decoded = decodeState(json)
        self.state = decoded
        return decoded
    }

    public func toggleAsciiMode() -> JuxuActionState {
        guard let h = handle else { return .empty }
        let json = takeString(juxu_engine_toggle_ascii_json(h))
        let decoded = decodeState(json)
        self.state = decoded
        return decoded
    }

    public func clearComposition() {
        guard let h = handle else { return }
        juxu_engine_clear(h)
        refreshState()
    }

    public func setContext(_ text: String) {
        guard let h = handle else { return }
        text.withCString { juxu_engine_set_context(h, $0) }
    }

    public func acceptPrediction(_ text: String, timestampMs: UInt64) {
        guard let h = handle else { return }
        text.withCString { juxu_engine_accept_prediction(h, $0, timestampMs) }
    }

    public func predictNextWords(maxCount: Int = 8) -> [String] {
        guard let h = handle else { return [] }
        let json = takeString(juxu_engine_predict_next_json(h, UInt32(max(1, maxCount))))
        if let data = json.data(using: .utf8),
           let list = try? decoder.decode([String].self, from: data) {
            return list
        }
        return []
    }

    @discardableResult
    public func addUserPhrase(_ text: String, persist: Bool = true) -> String {
        guard let h = handle else { return "" }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.count >= 2 else { return "" }
        let code = trimmed.withCString { takeString(juxu_engine_add_user_phrase(h, $0)) }
        if !code.isEmpty && persist {
            flushUserDataIfNeeded()
        }
        refreshState()
        refreshUserPhrases()
        return code
    }

    public func removeUserPhrase(_ text: String, persist: Bool = true) {
        guard let h = handle else { return }
        _ = text.withCString { juxu_engine_remove_user_phrase(h, $0) }
        if persist {
            flushUserDataIfNeeded()
        }
        refreshState()
        refreshUserPhrases()
    }

    public func resetTouchCalibration(persist: Bool = true) {
        guard let h = handle else { return }
        juxu_engine_reset_touch_profile(h)
        if persist {
            #if os(iOS)
            publishTouchProfile(isReset: true)
            #endif
            flushUserDataIfNeeded()
        }
        refreshState()
        refreshTouchProfile()
    }

    public func refreshUserPhrases() {
        guard let h = handle else { return }
        let json = takeString(juxu_engine_list_user_phrases_json(h))
        if let data = json.data(using: .utf8),
           let list = try? decoder.decode([UserPhraseRecord].self, from: data) {
            self.userPhrases = list
        }
    }

    public func refreshTouchProfile() {
        touchCalibrations = readTouchProfile()
    }

    private func readTouchProfile() -> [TouchKeyCalibrationItem] {
        guard let h = handle else { return [] }
        let json = takeString(juxu_engine_touch_profile_json(h))
        return (try? decoder.decode([TouchKeyCalibrationItem].self, from: Data(json.utf8))) ?? []
    }

    #if os(iOS)
    public func reloadTouchCalibration() {
        loadSharedTouchProfile()
        refreshTouchProfile()
    }

    private func readSharedTouchProfile() -> [TouchKeyCalibrationItem]? {
        guard let data = IOSSharedSettings.read(account: IOSSharedSettings.touchProfileAccount),
              let items = try? decoder.decode([TouchKeyCalibrationItem].self, from: data),
              items.map(\.key) == "abcdefghijklmnopqrstuvwxyz".map(String.init) else { return nil }
        return items
    }

    private func loadSharedTouchProfile() {
        guard !inMemoryOnly else { return }
        if let profile = readSharedTouchProfile() {
            adoptSharedTouchProfile(profile)
        } else {
            // Migrate calibration from the keyboard's existing local user store.
            publishTouchProfile()
        }
    }

    private func adoptSharedTouchProfile(_ profile: [TouchKeyCalibrationItem]) {
        lastSharedTouchProfile = profile
        guard let h = handle, profile != readTouchProfile() else { return }
        let offsets = profile.flatMap { [$0.dx, $0.dy] }
        let counts = profile.map(\.samples)
        offsets.withUnsafeBufferPointer { xy in
            counts.withUnsafeBufferPointer { samples in
                juxu_engine_set_touch_profile(h, xy.baseAddress, samples.baseAddress)
            }
        }
    }

    private func publishTouchProfile(isReset: Bool = false) {
        guard !inMemoryOnly else { return }
        let profile = readTouchProfile()
        guard profile.count == 26,
              isReset || (profile != lastSharedTouchProfile && profile.contains { $0.samples > 0 })
        else { return }

        // A running keyboard must not overwrite a reset made in the host app.
        if !isReset, let current = readSharedTouchProfile(), current != lastSharedTouchProfile {
            adoptSharedTouchProfile(current)
            return
        }
        if let data = try? JSONEncoder().encode(profile),
           IOSSharedSettings.write(data, account: IOSSharedSettings.touchProfileAccount) {
            lastSharedTouchProfile = profile
        }
    }
    #endif

    public func refreshState() {
        guard let h = handle else { return }
        let json = takeString(juxu_engine_state_json(h))
        self.state = decodeState(json)
    }

    private func decodeState(_ json: String) -> JuxuActionState {
        guard let data = json.data(using: .utf8),
              let s = try? decoder.decode(JuxuActionState.self, from: data) else {
            return .empty
        }
        return s
    }

    private func takeString(_ ptr: UnsafeMutablePointer<CChar>?) -> String {
        guard let p = ptr else { return "" }
        defer { juxu_string_free(p) }
        return String(cString: p)
    }

    // MARK: - Local Input Eval Log (macOS JSONL + iOS Shared Keychain Ring)

    private func drainEvalLogIfNeeded() {
        guard let h = handle else { return }
        let batch = takeString(juxu_engine_drain_eval_log_jsonl(h))
        guard !bridgeInMemoryOnly, !suppressEvalLogging, !batch.isEmpty else { return }
        appendEvalLogLines(batch)
    }

    private func appendEvalLogLines(_ batch: String) {
        let payload = Data((batch + "\n").utf8)
        let fm = FileManager.default
        if let attrs = try? fm.attributesOfItem(atPath: evalLogURL.path),
           let size = attrs[.size] as? UInt64,
           size > 8 * 1024 * 1024 {
            let prevURL = evalLogURL.deletingLastPathComponent().appendingPathComponent("eval_log.prev.jsonl")
            try? fm.removeItem(at: prevURL)
            try? fm.moveItem(at: evalLogURL, to: prevURL)
        }
        if !fm.fileExists(atPath: evalLogURL.path) {
            fm.createFile(
                atPath: evalLogURL.path,
                contents: payload,
                attributes: [.posixPermissions: NSNumber(value: Int16(0o600))]
            )
        } else if let fh = try? FileHandle(forWritingTo: evalLogURL) {
            defer { try? fh.close() }
            _ = try? fh.seekToEnd()
            try? fh.write(contentsOf: payload)
        }

        #if os(iOS)
        syncIOSEvalLogRing(newBatch: batch)
        #endif
    }

    #if os(iOS)
    private static func decodeSharedEvalRing() -> [String] {
        guard let compressed = IOSSharedSettings.read(account: IOSSharedSettings.evalLogAccount) else {
            return []
        }
        let rawData: Data
        if let inflated = try? (compressed as NSData).decompressed(using: .zlib) as Data {
            rawData = inflated
        } else {
            rawData = compressed
        }
        guard let text = String(data: rawData, encoding: .utf8) else { return [] }
        return text.split(separator: "\n", omittingEmptySubsequences: true).map(String.init)
    }

    private func syncIOSEvalLogRing(newBatch: String) {
        if !iosEvalRingLoaded {
            iosEvalRingLines = Self.decodeSharedEvalRing()
            iosEvalRingLoaded = true
        }
        let added = newBatch.split(separator: "\n", omittingEmptySubsequences: true).map(String.init)
        guard !added.isEmpty else { return }
        iosEvalRingLines.append(contentsOf: added)
        if iosEvalRingLines.count > 400 {
            iosEvalRingLines.removeFirst(iosEvalRingLines.count - 400)
        }
        let joined = iosEvalRingLines.joined(separator: "\n")
        if let compressed = try? (Data(joined.utf8) as NSData).compressed(using: .zlib) as Data {
            _ = IOSSharedSettings.write(compressed, account: IOSSharedSettings.evalLogAccount)
        }
    }
    #endif

    public func readEvalLogText() -> String {
        #if os(iOS)
        let ring = Self.decodeSharedEvalRing()
        if !ring.isEmpty {
            return ring.joined(separator: "\n") + "\n"
        }
        #endif
        return (try? String(contentsOf: evalLogURL, encoding: .utf8)) ?? ""
    }

    public func reloadEvalLogSummary() {
        let text = readEvalLogText()
        var totalEvents = 0
        var commits = 0
        var top1Hits = 0
        var manualSelects = 0
        var lmOverruled = 0
        var typoTriggered = 0
        var backspaceAfterCommit = 0
        var abandons = 0

        for line in text.split(separator: "\n", omittingEmptySubsequences: true) {
            let s = String(line)
            guard s.hasPrefix("{") else { continue }
            totalEvents += 1
            if s.contains("\"event\":\"commit\"") || s.contains("\"event\":\"partial_commit\"") {
                commits += 1
                if s.contains("\"rank\":0,") {
                    top1Hits += 1
                } else {
                    manualSelects += 1
                }
                if s.contains("\"lm_overruled\":true") {
                    lmOverruled += 1
                }
                if !s.contains("\"typo\":\"none\"") && s.contains("\"typo\":\"") {
                    typoTriggered += 1
                }
            } else if s.contains("\"event\":\"backspace_after_commit\"") {
                backspaceAfterCommit += 1
            } else if s.contains("\"event\":\"abandon\"") {
                abandons += 1
            }
        }
        self.evalLogSummary = EvalLogSummary(
            totalEvents: totalEvents,
            commits: commits,
            top1Hits: top1Hits,
            manualSelects: manualSelects,
            lmOverruled: lmOverruled,
            typoTriggered: typoTriggered,
            backspaceAfterCommit: backspaceAfterCommit,
            abandons: abandons
        )
    }

    public func exportEvalLogFileURL() -> URL? {
        #if os(macOS)
        return FileManager.default.fileExists(atPath: evalLogURL.path) ? evalLogURL : nil
        #else
        let text = readEvalLogText()
        guard !text.isEmpty else { return nil }
        let out = FileManager.default.temporaryDirectory.appendingPathComponent("juxu_ios_eval_log.jsonl")
        try? Data(text.utf8).write(to: out, options: .atomic)
        return out
        #endif
    }

    public func clearEvalLog() {
        try? FileManager.default.removeItem(at: evalLogURL)
        let prevURL = evalLogURL.deletingLastPathComponent().appendingPathComponent("eval_log.prev.jsonl")
        try? FileManager.default.removeItem(at: prevURL)
        #if os(iOS)
        iosEvalRingLines.removeAll()
        iosEvalRingLoaded = true
        if let emptyCompressed = try? (Data() as NSData).compressed(using: .zlib) as Data {
            _ = IOSSharedSettings.write(emptyCompressed, account: IOSSharedSettings.evalLogAccount)
        }
        #endif
        self.evalLogSummary = .empty
    }
}

// MARK: - Chinese / English Auto-Spacing Engine

public struct AutoSpaceOptions: Equatable, Sendable {
    public var enabled: Bool
    public var spaceDigits: Bool

    public init(enabled: Bool = true, spaceDigits: Bool = true) {
        self.enabled = enabled
        self.spaceDigits = spaceDigits
    }

    public init(prefs: JuxuPreferences) {
        self.enabled = prefs.autoSpaceZhEn
        self.spaceDigits = prefs.autoSpaceDigits
    }
}

public enum AutoSpaceFormatter {
    private enum CharClass {
        case cjk
        case asciiLetter
        case asciiDigit
        case whitespace
        case other
    }

    private static func classify(_ scalar: Unicode.Scalar) -> CharClass {
        let v = scalar.value
        if scalar.properties.isWhitespace || v == 0x200B || v == 0x200C || v == 0x200D || v == 0xFEFF {
            return .whitespace
        }
        if v >= 0x30 && v <= 0x39 {
            return .asciiDigit
        }
        if (v >= 0x41 && v <= 0x5A)
            || (v >= 0x61 && v <= 0x7A)
            || (v >= 0x00C0 && v <= 0x00D6)
            || (v >= 0x00D8 && v <= 0x00F6)
            || (v >= 0x00F8 && v <= 0x024F)
            || (v >= 0x1E00 && v <= 0x1EFF)
        {
            return .asciiLetter
        }
        if (v >= 0x4E00 && v <= 0x9FFF)
            || (v >= 0x3400 && v <= 0x4DBF)
            || (v >= 0x20000 && v <= 0x2A6DF)
            || (v >= 0x2A700 && v <= 0x2EBEF)
            || (v >= 0x30000 && v <= 0x323AF)
            || (v >= 0xF900 && v <= 0xFAFF)
            || (v >= 0x2F800 && v <= 0x2FA1F)
            || (v >= 0x2E80 && v <= 0x2EFF)
            || (v >= 0x2F00 && v <= 0x2FDF)
            || (v >= 0x3100 && v <= 0x312F)
            || (v >= 0x31A0 && v <= 0x31BF)
            || v == 0x3007
        {
            return .cjk
        }
        return .other
    }

    public static func isAutoSpaceRightTrigger(_ text: String, options: AutoSpaceOptions) -> Bool {
        guard options.enabled, let first = text.unicodeScalars.first else { return false }
        let c = classify(first)
        if c == .cjk || c == .asciiLetter { return true }
        if c == .asciiDigit {
            if options.spaceDigits { return true }
            return leadingAlnumContainsLetter(text)
        }
        return false
    }

    public static func shouldInsertLeadingSpace(
        before: String,
        inserting: String,
        options: AutoSpaceOptions
    ) -> Bool {
        guard options.enabled else { return false }
        guard let left = before.unicodeScalars.last,
              let right = inserting.unicodeScalars.first else {
            return false
        }
        let lc = classify(left)
        let rc = classify(right)
        if lc == .whitespace || rc == .whitespace {
            return false
        }
        if (rc == .asciiLetter || rc == .asciiDigit) && isInsideCJKURL(before: before) {
            return false
        }
        if lc == .cjk && rc == .asciiLetter {
            return true
        }
        if lc == .asciiLetter && rc == .cjk {
            return true
        }
        if lc == .cjk && rc == .asciiDigit {
            return options.spaceDigits || leadingAlnumContainsLetter(inserting)
        }
        if lc == .asciiDigit && rc == .cjk {
            return options.spaceDigits || trailingAlnumContainsLetter(before)
        }
        if rc == .cjk && lc == .other {
            if endsWithSpacedLanguageToken(before) {
                return true
            }
            if options.spaceDigits && endsWithNumberPercent(before) {
                return true
            }
        }
        return false
    }

    public static func format(
        before: String,
        inserting: String,
        splitInternal: Bool,
        suppressLeading: Bool = false,
        options: AutoSpaceOptions
    ) -> (text: String, lastAutoSpaceAnchor: String?) {
        guard !inserting.isEmpty else { return (inserting, nil) }
        if !options.enabled {
            return (inserting, nil)
        }
        if !splitInternal {
            let addLead = !suppressLeading && shouldInsertLeadingSpace(before: before, inserting: inserting, options: options)
            if addLead {
                return (" " + inserting, before)
            }
            return (inserting, nil)
        }

        var acc = tailScalars(of: before, maxScalars: 64)
        var out = ""
        out.reserveCapacity(inserting.count + 2)
        var lastAnchor: String? = nil

        for (idx, scalar) in inserting.unicodeScalars.enumerated() {
            let s = String(scalar)
            let suppressHere = (idx == 0) && suppressLeading
            if !suppressHere && shouldInsertLeadingSpace(before: acc, inserting: s, options: options) {
                lastAnchor = acc
                out.append(" ")
                acc.append(" ")
            }
            out.append(s)
            acc.append(s)
        }
        return (out, lastAnchor)
    }

    private static func trailingAlnumContainsLetter(_ s: String) -> Bool {
        var hasLetter = false
        for scalar in s.unicodeScalars.reversed() {
            let c = classify(scalar)
            if c == .asciiLetter {
                hasLetter = true
            } else if c != .asciiDigit {
                break
            }
        }
        return hasLetter
    }

    private static func leadingAlnumContainsLetter(_ s: String) -> Bool {
        for scalar in s.unicodeScalars {
            let c = classify(scalar)
            if c == .asciiLetter {
                return true
            } else if c != .asciiDigit {
                break
            }
        }
        return false
    }

    private static func isInsideCJKURL(before: String) -> Bool {
        var tokenReversed: [Unicode.Scalar] = []
        for scalar in before.unicodeScalars.reversed() {
            if classify(scalar) == .whitespace {
                break
            }
            tokenReversed.append(scalar)
        }
        guard !tokenReversed.isEmpty else { return false }
        let token = String(String.UnicodeScalarView(tokenReversed.reversed()))
        guard let range = token.range(of: "://") else { return false }
        let scheme = token[..<range.lowerBound]
        guard !scheme.isEmpty else { return false }
        let schemeValid = scheme.unicodeScalars.allSatisfy { s in
            let v = s.value
            return (v >= 0x41 && v <= 0x5A)
                || (v >= 0x61 && v <= 0x7A)
                || (v >= 0x30 && v <= 0x39)
                || v == 0x2B || v == 0x2D || v == 0x2E
        }
        guard schemeValid else { return false }
        let afterScheme = token[range.upperBound...]
        return afterScheme.unicodeScalars.contains { classify($0) == .cjk }
    }

    private static func endsWithSpacedLanguageToken(_ before: String) -> Bool {
        let scalars = Array(before.unicodeScalars)
        let n = scalars.count
        if n >= 3 && scalars[n - 1] == "+" && scalars[n - 2] == "+" {
            if classify(scalars[n - 3]) == .asciiLetter {
                return true
            }
        }
        if n >= 2 && scalars[n - 1] == "#" {
            let prev = scalars[n - 2]
            if prev == "C" || prev == "c" || prev == "F" || prev == "f" {
                if n == 2 { return true }
                let beforePrev = scalars[n - 3]
                let c = classify(beforePrev)
                if c != .asciiLetter && c != .asciiDigit && beforePrev != "#" {
                    return true
                }
            }
        }
        return false
    }

    private static func endsWithNumberPercent(_ before: String) -> Bool {
        let scalars = Array(before.unicodeScalars)
        guard scalars.last == "%" else { return false }
        var i = scalars.count - 2
        var sawDigit = false
        while i >= 0 {
            let s = scalars[i]
            if classify(s) == .asciiDigit {
                sawDigit = true
            } else if s == "." && sawDigit {
                // Allow decimals like 99.9%
            } else {
                // Reject %50% patterns
                if s == "%" { return false }
                break
            }
            i -= 1
        }
        return sawDigit
    }

    fileprivate static func tailScalars(of s: String, maxScalars: Int) -> String {
        let view = s.unicodeScalars
        if view.count <= maxScalars {
            return s
        }
        let start = view.index(view.endIndex, offsetBy: -maxScalars)
        return String(view[start...])
    }
}

public struct AutoSpaceContextTracker: Sendable {
    private var shadowBefore: String = ""
    private var lastObservedHostBefore: String? = nil
    private var optimisticInserted: String = ""
    private var optimisticDeleteCount: Int = 0

    private var lastAutoSpaceAnchorTail: String? = nil
    private var pendingAutoSpaceActive: Bool = false
    private var commitsSinceAutoSpace: Int = 0
    private var suppressedAnchorTail: String? = nil

    public init() {}

    public mutating func invalidate() {
        shadowBefore = ""
        lastObservedHostBefore = nil
        optimisticInserted = ""
        optimisticDeleteCount = 0
        lastAutoSpaceAnchorTail = nil
        pendingAutoSpaceActive = false
        commitsSinceAutoSpace = 0
        suppressedAnchorTail = nil
    }

    public mutating func effectiveBefore(
        hostBefore: String?,
        hostBeforeIsAuthoritative: Bool
    ) -> String {
        if hostBeforeIsAuthoritative {
            if let host = hostBefore {
                shadowBefore = AutoSpaceFormatter.tailScalars(of: host, maxScalars: 64)
                return host
            }
            return shadowBefore
        }
        let currentHost = hostBefore ?? ""
        if let prevHost = lastObservedHostBefore {
            if currentHost == prevHost {
                return applyOptimisticDelta(to: currentHost)
            }
            let expected = applyOptimisticDelta(to: prevHost)
            if currentHost == expected {
                lastObservedHostBefore = currentHost
                optimisticInserted = ""
                optimisticDeleteCount = 0
                shadowBefore = AutoSpaceFormatter.tailScalars(of: currentHost, maxScalars: 64)
                return currentHost
            }
            if optimisticDeleteCount == 0, currentHost.hasPrefix(prevHost) {
                let caughtUp = String(currentHost.dropFirst(prevHost.count))
                if !caughtUp.isEmpty && optimisticInserted.hasPrefix(caughtUp) {
                    optimisticInserted = String(optimisticInserted.dropFirst(caughtUp.count))
                    lastObservedHostBefore = currentHost
                    let merged = applyOptimisticDelta(to: currentHost)
                    shadowBefore = AutoSpaceFormatter.tailScalars(of: merged, maxScalars: 64)
                    return merged
                }
            }
        }
        lastObservedHostBefore = currentHost
        optimisticInserted = ""
        optimisticDeleteCount = 0
        shadowBefore = AutoSpaceFormatter.tailScalars(of: currentHost, maxScalars: 64)
        return currentHost
    }

    public mutating func prepareCommit(
        _ text: String,
        hostBefore: String?,
        hostBeforeIsAuthoritative: Bool,
        overrideBefore: String? = nil,
        splitInternal: Bool,
        options: AutoSpaceOptions
    ) -> String {
        guard !text.isEmpty else { return text }
        let syncedBefore = effectiveBefore(
            hostBefore: hostBefore,
            hostBeforeIsAuthoritative: hostBeforeIsAuthoritative
        )
        let before = overrideBefore ?? syncedBefore
        let suppressLeading: Bool
        if let sup = suppressedAnchorTail, !sup.isEmpty, before.hasSuffix(sup) {
            suppressLeading = true
        } else {
            suppressLeading = false
        }

        let (formatted, anchor) = AutoSpaceFormatter.format(
            before: before,
            inserting: text,
            splitInternal: splitInternal,
            suppressLeading: suppressLeading,
            options: options
        )
        suppressedAnchorTail = nil

        if let anchor {
            lastAutoSpaceAnchorTail = AutoSpaceFormatter.tailScalars(of: anchor, maxScalars: 16)
            pendingAutoSpaceActive = true
            commitsSinceAutoSpace = 1
        } else if pendingAutoSpaceActive {
            if text.unicodeScalars.first?.properties.isWhitespace == true {
                pendingAutoSpaceActive = false
                lastAutoSpaceAnchorTail = nil
                commitsSinceAutoSpace = 0
            } else {
                commitsSinceAutoSpace += 1
                if commitsSinceAutoSpace > 2 {
                    pendingAutoSpaceActive = false
                    lastAutoSpaceAnchorTail = nil
                }
            }
        }

        shadowBefore = AutoSpaceFormatter.tailScalars(of: before + formatted, maxScalars: 64)
        if !hostBeforeIsAuthoritative {
            if lastObservedHostBefore == nil {
                lastObservedHostBefore = hostBefore ?? ""
            }
            optimisticInserted.append(formatted)
        }
        return formatted
    }

    public mutating func recordPassthrough(
        _ text: String,
        hostBefore: String?,
        hostBeforeIsAuthoritative: Bool
    ) {
        guard !text.isEmpty else { return }
        let before = effectiveBefore(
            hostBefore: hostBefore,
            hostBeforeIsAuthoritative: hostBeforeIsAuthoritative
        )
        suppressedAnchorTail = nil
        if pendingAutoSpaceActive {
            if text.unicodeScalars.first?.properties.isWhitespace == true {
                pendingAutoSpaceActive = false
                lastAutoSpaceAnchorTail = nil
                commitsSinceAutoSpace = 0
            } else {
                commitsSinceAutoSpace += 1
                if commitsSinceAutoSpace > 2 {
                    pendingAutoSpaceActive = false
                    lastAutoSpaceAnchorTail = nil
                }
            }
        }
        shadowBefore = AutoSpaceFormatter.tailScalars(of: before + text, maxScalars: 64)
        if !hostBeforeIsAuthoritative {
            if lastObservedHostBefore == nil {
                lastObservedHostBefore = hostBefore ?? ""
            }
            optimisticInserted.append(text)
        }
    }

    public mutating func willDeleteBackward(
        hostBefore: String?,
        hostBeforeIsAuthoritative: Bool
    ) {
        let before = effectiveBefore(
            hostBefore: hostBefore,
            hostBeforeIsAuthoritative: hostBeforeIsAuthoritative
        )
        guard let last = before.unicodeScalars.last else {
            suppressedAnchorTail = nil
            pendingAutoSpaceActive = false
            return
        }
        let remaining = String(before.unicodeScalars.dropLast())
        if last == " ",
           pendingAutoSpaceActive,
           commitsSinceAutoSpace <= 2,
           let anchor = lastAutoSpaceAnchorTail,
           !anchor.isEmpty,
           remaining.hasSuffix(anchor)
        {
            suppressedAnchorTail = anchor
            pendingAutoSpaceActive = false
            lastAutoSpaceAnchorTail = nil
            commitsSinceAutoSpace = 0
        } else {
            suppressedAnchorTail = nil
        }

        shadowBefore = AutoSpaceFormatter.tailScalars(of: remaining, maxScalars: 64)
        if !hostBeforeIsAuthoritative {
            if lastObservedHostBefore == nil {
                lastObservedHostBefore = hostBefore ?? ""
            }
            if !optimisticInserted.isEmpty {
                optimisticInserted = String(optimisticInserted.unicodeScalars.dropLast())
            } else {
                optimisticDeleteCount += 1
            }
        }
    }

    public mutating func reconcileExternalContext(hostBefore: String?) {
        let current = hostBefore ?? ""
        guard let prevHost = lastObservedHostBefore else {
            lastObservedHostBefore = current
            shadowBefore = AutoSpaceFormatter.tailScalars(of: current, maxScalars: 64)
            return
        }
        let expected = applyOptimisticDelta(to: prevHost)
        if current == expected {
            lastObservedHostBefore = current
            optimisticInserted = ""
            optimisticDeleteCount = 0
            shadowBefore = AutoSpaceFormatter.tailScalars(of: current, maxScalars: 64)
            return
        }
        if optimisticDeleteCount == 0, current.hasPrefix(prevHost) {
            let caughtUp = String(current.dropFirst(prevHost.count))
            if !caughtUp.isEmpty && optimisticInserted.hasPrefix(caughtUp) {
                optimisticInserted = String(optimisticInserted.dropFirst(caughtUp.count))
                lastObservedHostBefore = current
                let merged = applyOptimisticDelta(to: current)
                shadowBefore = AutoSpaceFormatter.tailScalars(of: merged, maxScalars: 64)
                return
            }
        }
        if optimisticInserted.isEmpty && optimisticDeleteCount == 0 && current != prevHost {
            lastAutoSpaceAnchorTail = nil
            pendingAutoSpaceActive = false
            commitsSinceAutoSpace = 0
            suppressedAnchorTail = nil
        }
        lastObservedHostBefore = current
        optimisticInserted = ""
        optimisticDeleteCount = 0
        shadowBefore = AutoSpaceFormatter.tailScalars(of: current, maxScalars: 64)
    }

    private func applyOptimisticDelta(to base: String) -> String {
        var result = base
        if optimisticDeleteCount > 0 {
            let scalars = result.unicodeScalars
            let dropCount = min(optimisticDeleteCount, scalars.count)
            result = String(scalars.dropLast(dropCount))
        }
        if !optimisticInserted.isEmpty {
            result.append(optimisticInserted)
        }
        return result
    }
}
