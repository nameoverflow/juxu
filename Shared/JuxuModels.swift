import Foundation

public struct JuxuCandidateItem: Codable, Identifiable, Equatable, Sendable {
    public let index: Int
    public let text: String
    public let comment: String
    public let badge: UInt8
    public let confidence: Float
    public let isExact: Bool

    public var id: Int { index }

    public init(
        index: Int,
        text: String,
        comment: String,
        badge: UInt8,
        confidence: Float,
        isExact: Bool
    ) {
        self.index = index
        self.text = text
        self.comment = comment
        self.badge = badge
        self.confidence = confidence
        self.isExact = isExact
    }

    public enum BadgeKind: Equatable, Sendable {
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

    public init(key: String, dx: Float, dy: Float, samples: UInt32) {
        self.key = key
        self.dx = dx
        self.dy = dy
        self.samples = samples
    }
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

    public init(
        totalEvents: Int,
        commits: Int,
        top1Hits: Int,
        manualSelects: Int,
        lmOverruled: Int,
        typoTriggered: Int,
        backspaceAfterCommit: Int,
        abandons: Int
    ) {
        self.totalEvents = totalEvents
        self.commits = commits
        self.top1Hits = top1Hits
        self.manualSelects = manualSelects
        self.lmOverruled = lmOverruled
        self.typoTriggered = typoTriggered
        self.backspaceAfterCommit = backspaceAfterCommit
        self.abandons = abandons
    }

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
    public let hasDecodedTouchOffsets: Bool
    public let hasDecodedT9TouchOffsets: Bool

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
    public var macOSPreedit: String { activeInlinePreedit }
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
        candidates: [JuxuCandidateItem],
        hasDecodedTouchOffsets: Bool = true,
        hasDecodedT9TouchOffsets: Bool = true
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
        self.hasDecodedTouchOffsets = hasDecodedTouchOffsets
        self.hasDecodedT9TouchOffsets = hasDecodedT9TouchOffsets
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
        let decodedTouch = try c.decodeIfPresent([Float].self, forKey: .touchOffsets)
        self.hasDecodedTouchOffsets = (decodedTouch != nil)
        self.touchOffsets = decodedTouch ?? Array(repeating: 0, count: 52)
        self.t9Mode = try c.decodeIfPresent(Bool.self, forKey: .t9Mode) ?? false
        self.t9ValidNextMask = try c.decodeIfPresent(UInt8.self, forKey: .t9ValidNextMask) ?? 0xFF
        self.t9NextKeyPriors = try c.decodeIfPresent([UInt8].self, forKey: .t9NextKeyPriors) ?? Array(repeating: 100, count: 8)
        let decodedT9Touch = try c.decodeIfPresent([Float].self, forKey: .t9TouchOffsets)
        self.hasDecodedT9TouchOffsets = (decodedT9Touch != nil)
        self.t9TouchOffsets = decodedT9Touch ?? Array(repeating: 0, count: 16)
        self.t9LockedPrefix = try c.decodeIfPresent(String.self, forKey: .t9LockedPrefix) ?? ""
        self.t9Syllables = try c.decodeIfPresent([String].self, forKey: .t9Syllables) ?? []
        self.candidates = try c.decode([JuxuCandidateItem].self, forKey: .candidates)
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(consumed, forKey: .consumed)
        try c.encode(bell, forKey: .bell)
        try c.encode(asciiMode, forKey: .asciiMode)
        try c.encode(schemaId, forKey: .schemaId)
        try c.encode(page, forKey: .page)
        try c.encode(totalPages, forKey: .totalPages)
        try c.encode(vertical, forKey: .vertical)
        try c.encode(schemaName, forKey: .schemaName)
        try c.encode(commitText, forKey: .commitText)
        try c.encode(preedit, forKey: .preedit)
        try c.encode(segmentedCode, forKey: .segmentedCode)
        try c.encode(notice, forKey: .notice)
        try c.encode(userPhraseCount, forKey: .userPhraseCount)
        try c.encode(calibratedKeyCount, forKey: .calibratedKeyCount)
        try c.encode(validNextMask, forKey: .validNextMask)
        try c.encode(nextKeyPriors, forKey: .nextKeyPriors)
        try c.encode(touchOffsets, forKey: .touchOffsets)
        try c.encode(t9Mode, forKey: .t9Mode)
        try c.encode(t9ValidNextMask, forKey: .t9ValidNextMask)
        try c.encode(t9NextKeyPriors, forKey: .t9NextKeyPriors)
        try c.encode(t9TouchOffsets, forKey: .t9TouchOffsets)
        try c.encode(t9LockedPrefix, forKey: .t9LockedPrefix)
        try c.encode(t9Syllables, forKey: .t9Syllables)
        try c.encode(candidates, forKey: .candidates)
    }

    /// Preserves `touchOffsets` and `t9TouchOffsets` from `previous` whenever a non-committing
    /// keystroke JSON omits those arrays.
    public func inheritingMissingTouchOffsets(from previous: JuxuActionState) -> JuxuActionState {
        if hasDecodedTouchOffsets && hasDecodedT9TouchOffsets {
            return self
        }
        return JuxuActionState(
            consumed: consumed,
            bell: bell,
            asciiMode: asciiMode,
            schemaId: schemaId,
            page: page,
            totalPages: totalPages,
            vertical: vertical,
            schemaName: schemaName,
            commitText: commitText,
            preedit: preedit,
            segmentedCode: segmentedCode,
            notice: notice,
            userPhraseCount: userPhraseCount,
            calibratedKeyCount: calibratedKeyCount,
            validNextMask: validNextMask,
            nextKeyPriors: nextKeyPriors,
            touchOffsets: hasDecodedTouchOffsets ? touchOffsets : previous.touchOffsets,
            t9Mode: t9Mode,
            t9ValidNextMask: t9ValidNextMask,
            t9NextKeyPriors: t9NextKeyPriors,
            t9TouchOffsets: hasDecodedT9TouchOffsets ? t9TouchOffsets : previous.t9TouchOffsets,
            t9LockedPrefix: t9LockedPrefix,
            t9Syllables: t9Syllables,
            candidates: candidates,
            hasDecodedTouchOffsets: hasDecodedTouchOffsets || previous.hasDecodedTouchOffsets,
            hasDecodedT9TouchOffsets: hasDecodedT9TouchOffsets || previous.hasDecodedT9TouchOffsets
        )
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

    public init(text: String, code: String, count: UInt32, auto: Bool) {
        self.text = text
        self.code = code
        self.count = count
        self.auto = auto
    }
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

    public init(
        id: UInt32,
        category: InputSchemaCategory,
        title: String,
        shortTitle: String,
        tag: String,
        desc: String
    ) {
        self.id = id
        self.category = category
        self.title = title
        self.shortTitle = shortTitle
        self.tag = tag
        self.desc = desc
    }

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

public struct JuxuPreferences: Codable, Equatable, Sendable {
    public static let appGroupIdentifier = "group.im.juxu.shared"

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
    public var spaceCommaKey: Bool = false
    public var keyLongPressAlternates: Bool = false
    public var backspaceSwipeClear: Bool = false
    public var keyboardHoldCursor: Bool = false
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

    private enum CodingKeys: String, CodingKey {
        case schemaId, pageSize, vertical, shiftToggleMode, smartPunctuation, semicolonQuoteSelect
        case autoSpaceZhEn, autoSpaceDigits, clipboardSuggestion, cangjieKeycapRadicals, wubiKeycapRadicals
        case keySwipeUpInput, spaceCommaKey, keyLongPressAlternates, backspaceSwipeClear, keyboardHoldCursor
        case evalLogging, emojiCandidates
        case fourthCodeAutoCommit, fifthCodePushCommit, fourthEmptyBehavior
        case shortestCodeFirst, strictShortcode, spaceless, showShortcodeHint
        case dynamicWordFreq, dynamicCharFreq, autoPhrase, autoPhraseThreshold
        case zOmnipotent, enableRareCharset, pinyinInitialAbbr, pinyinT9Layout
        case typoEnabled, touchHitboxAdapt, promoteThreshold, suggestThreshold
        case suppressAutoCommitOnCorrection, missedSpaceCorrection
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.schemaId = try c.decodeIfPresent(UInt32.self, forKey: .schemaId) ?? 86
        self.pageSize = try c.decodeIfPresent(Int.self, forKey: .pageSize) ?? 5
        self.vertical = try c.decodeIfPresent(Bool.self, forKey: .vertical) ?? false
        self.shiftToggleMode = try c.decodeIfPresent(String.self, forKey: .shiftToggleMode) ?? "commit_code"
        self.smartPunctuation = try c.decodeIfPresent(Bool.self, forKey: .smartPunctuation) ?? true
        self.semicolonQuoteSelect = try c.decodeIfPresent(Bool.self, forKey: .semicolonQuoteSelect) ?? false
        self.autoSpaceZhEn = try c.decodeIfPresent(Bool.self, forKey: .autoSpaceZhEn) ?? true
        self.autoSpaceDigits = try c.decodeIfPresent(Bool.self, forKey: .autoSpaceDigits) ?? true
        self.clipboardSuggestion = try c.decodeIfPresent(Bool.self, forKey: .clipboardSuggestion) ?? true
        self.cangjieKeycapRadicals = try c.decodeIfPresent(Bool.self, forKey: .cangjieKeycapRadicals) ?? true
        self.wubiKeycapRadicals = try c.decodeIfPresent(Bool.self, forKey: .wubiKeycapRadicals) ?? false
        self.keySwipeUpInput = try c.decodeIfPresent(Bool.self, forKey: .keySwipeUpInput) ?? false
        self.spaceCommaKey = try c.decodeIfPresent(Bool.self, forKey: .spaceCommaKey) ?? false
        self.keyLongPressAlternates = try c.decodeIfPresent(Bool.self, forKey: .keyLongPressAlternates) ?? false
        self.backspaceSwipeClear = try c.decodeIfPresent(Bool.self, forKey: .backspaceSwipeClear) ?? false
        self.keyboardHoldCursor = try c.decodeIfPresent(Bool.self, forKey: .keyboardHoldCursor) ?? false
        self.evalLogging = try c.decodeIfPresent(Bool.self, forKey: .evalLogging) ?? true
        self.emojiCandidates = try c.decodeIfPresent(Bool.self, forKey: .emojiCandidates) ?? false

        self.fourthCodeAutoCommit = try c.decodeIfPresent(Bool.self, forKey: .fourthCodeAutoCommit) ?? true
        self.fifthCodePushCommit = try c.decodeIfPresent(Bool.self, forKey: .fifthCodePushCommit) ?? true
        self.fourthEmptyBehavior = try c.decodeIfPresent(String.self, forKey: .fourthEmptyBehavior) ?? "block"
        self.shortestCodeFirst = try c.decodeIfPresent(Bool.self, forKey: .shortestCodeFirst) ?? true
        self.strictShortcode = try c.decodeIfPresent(Bool.self, forKey: .strictShortcode) ?? false
        self.spaceless = try c.decodeIfPresent(Bool.self, forKey: .spaceless) ?? false
        self.showShortcodeHint = try c.decodeIfPresent(Bool.self, forKey: .showShortcodeHint) ?? true
        self.dynamicWordFreq = try c.decodeIfPresent(Bool.self, forKey: .dynamicWordFreq) ?? true
        self.dynamicCharFreq = try c.decodeIfPresent(Bool.self, forKey: .dynamicCharFreq) ?? false
        self.autoPhrase = try c.decodeIfPresent(Bool.self, forKey: .autoPhrase) ?? true
        self.autoPhraseThreshold = try c.decodeIfPresent(Int.self, forKey: .autoPhraseThreshold) ?? 2
        self.zOmnipotent = try c.decodeIfPresent(Bool.self, forKey: .zOmnipotent) ?? true
        self.enableRareCharset = try c.decodeIfPresent(Bool.self, forKey: .enableRareCharset) ?? false
        self.pinyinInitialAbbr = try c.decodeIfPresent(Bool.self, forKey: .pinyinInitialAbbr) ?? true
        self.pinyinT9Layout = try c.decodeIfPresent(Bool.self, forKey: .pinyinT9Layout) ?? false

        self.typoEnabled = try c.decodeIfPresent(Bool.self, forKey: .typoEnabled) ?? true
        self.touchHitboxAdapt = try c.decodeIfPresent(Bool.self, forKey: .touchHitboxAdapt) ?? true
        self.promoteThreshold = try c.decodeIfPresent(Double.self, forKey: .promoteThreshold) ?? 0.82
        self.suggestThreshold = try c.decodeIfPresent(Double.self, forKey: .suggestThreshold) ?? 0.30
        self.suppressAutoCommitOnCorrection = try c.decodeIfPresent(Bool.self, forKey: .suppressAutoCommitOnCorrection) ?? true
        self.missedSpaceCorrection = try c.decodeIfPresent(Bool.self, forKey: .missedSpaceCorrection) ?? true
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(schemaId, forKey: .schemaId)
        try c.encode(pageSize, forKey: .pageSize)
        try c.encode(vertical, forKey: .vertical)
        try c.encode(shiftToggleMode, forKey: .shiftToggleMode)
        try c.encode(smartPunctuation, forKey: .smartPunctuation)
        try c.encode(semicolonQuoteSelect, forKey: .semicolonQuoteSelect)
        try c.encode(autoSpaceZhEn, forKey: .autoSpaceZhEn)
        try c.encode(autoSpaceDigits, forKey: .autoSpaceDigits)
        try c.encode(clipboardSuggestion, forKey: .clipboardSuggestion)
        try c.encode(cangjieKeycapRadicals, forKey: .cangjieKeycapRadicals)
        try c.encode(wubiKeycapRadicals, forKey: .wubiKeycapRadicals)
        try c.encode(keySwipeUpInput, forKey: .keySwipeUpInput)
        try c.encode(spaceCommaKey, forKey: .spaceCommaKey)
        try c.encode(keyLongPressAlternates, forKey: .keyLongPressAlternates)
        try c.encode(backspaceSwipeClear, forKey: .backspaceSwipeClear)
        try c.encode(keyboardHoldCursor, forKey: .keyboardHoldCursor)
        try c.encode(evalLogging, forKey: .evalLogging)
        try c.encode(emojiCandidates, forKey: .emojiCandidates)

        try c.encode(fourthCodeAutoCommit, forKey: .fourthCodeAutoCommit)
        try c.encode(fifthCodePushCommit, forKey: .fifthCodePushCommit)
        try c.encode(fourthEmptyBehavior, forKey: .fourthEmptyBehavior)
        try c.encode(shortestCodeFirst, forKey: .shortestCodeFirst)
        try c.encode(strictShortcode, forKey: .strictShortcode)
        try c.encode(spaceless, forKey: .spaceless)
        try c.encode(showShortcodeHint, forKey: .showShortcodeHint)
        try c.encode(dynamicWordFreq, forKey: .dynamicWordFreq)
        try c.encode(dynamicCharFreq, forKey: .dynamicCharFreq)
        try c.encode(autoPhrase, forKey: .autoPhrase)
        try c.encode(autoPhraseThreshold, forKey: .autoPhraseThreshold)
        try c.encode(zOmnipotent, forKey: .zOmnipotent)
        try c.encode(enableRareCharset, forKey: .enableRareCharset)
        try c.encode(pinyinInitialAbbr, forKey: .pinyinInitialAbbr)
        try c.encode(pinyinT9Layout, forKey: .pinyinT9Layout)

        try c.encode(typoEnabled, forKey: .typoEnabled)
        try c.encode(touchHitboxAdapt, forKey: .touchHitboxAdapt)
        try c.encode(promoteThreshold, forKey: .promoteThreshold)
        try c.encode(suggestThreshold, forKey: .suggestThreshold)
        try c.encode(suppressAutoCommitOnCorrection, forKey: .suppressAutoCommitOnCorrection)
        try c.encode(missedSpaceCorrection, forKey: .missedSpaceCorrection)
    }

    public func toTOML() -> String {
        guard let data = try? JSONEncoder().encode(self),
              let json = String(data: data, encoding: .utf8) else {
            return ""
        }
        return json.withCString { cJson in
            guard let ptr = juxu_config_json_to_toml(cJson) else { return "" }
            defer { juxu_string_free(ptr) }
            return String(cString: ptr)
        }
    }

    public static func fromTOML(_ toml: String) -> JuxuPreferences {
        let json = toml.withCString { cToml -> String in
            guard let ptr = juxu_config_toml_to_json(cToml) else { return "" }
            defer { juxu_string_free(ptr) }
            return String(cString: ptr)
        }
        guard let data = json.data(using: .utf8),
              let decoded = try? JSONDecoder().decode(JuxuPreferences.self, from: data) else {
            return JuxuPreferences()
        }
        return decoded
    }
}
