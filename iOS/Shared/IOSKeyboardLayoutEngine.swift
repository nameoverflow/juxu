import CoreGraphics
import Foundation

public enum IOSKeyPlane: Equatable {
    case letters
    case numbers
    case symbols
}

public enum IOSShiftState: Equatable {
    case lower
    case upperOnce
    case capsLocked

    public var isUppercase: Bool {
        self != .lower
    }
}

public enum IOSReturnKeyRole: Equatable {
    case normal
    case go
    case search
    case send
    case next
    case done
    case join
    case continueRole

    public var title: String {
        switch self {
        case .normal: return ""
        case .go: return "前往"
        case .search: return "搜索"
        case .send: return "发送"
        case .next: return "下一项"
        case .done: return "完成"
        case .join: return "加入"
        case .continueRole: return "继续"
        }
    }

    public var isBlueProminent: Bool {
        self != .normal
    }
}

public enum IOSKeyKind: Equatable {
    case letter(UInt8) // 'a'..'z'
    case t9Key(UInt8) // '1'..'9' ('1' = separator, '2'..'9' = T9 letter groups)
    case t9Clear // 重输
    case symbol(String)
    case shift
    case backspace
    case planeSwitch(IOSKeyPlane, label: String)
    case globe
    case modeToggle // 中/英切换
    case space
    case returnKey
}

/// Swipe-up secondary input on letter keys: digits on the top row, common punctuation below.
public enum IOSSwipeUp {
    private static let chinese: [UInt8: String] = table(
        "qwertyuiopasdfghjklzxcvbnm",
        ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0",
         "-", "/", "：", "；", "（", "）", "～", "“", "”",
         "@", "·", "#", "`", "？", "！", "…"]
    )
    private static let ascii: [UInt8: String] = table(
        "qwertyuiopasdfghjklzxcvbnm",
        ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0",
         "-", "/", ":", ";", "(", ")", "~", "'", "\"",
         "@", "_", "#", "&", "?", "!", "..."]
    )

    private static func table(_ keys: String, _ values: [String]) -> [UInt8: String] {
        Dictionary(uniqueKeysWithValues: zip(Array(keys.utf8), values))
    }

    public static func symbol(for letter: UInt8, asciiMode: Bool) -> String? {
        (asciiMode ? ascii : chinese)[letter]
    }

    /// Upward travel needed to arm the gesture; light thumb roll during normal typing stays below it.
    public static func armDistance(keyHeight: CGFloat) -> CGFloat {
        max(26.0, keyHeight * 0.60)
    }

    /// Mostly-vertical upward drag past the arm distance, with hysteresis so sliding back down cancels it.
    public static func isActive(
        from start: CGPoint,
        to current: CGPoint,
        keyHeight: CGFloat,
        wasActive: Bool
    ) -> Bool {
        let up = start.y - current.y
        let threshold = armDistance(keyHeight: keyHeight) * (wasActive ? 0.6 : 1.0)
        return up >= threshold && abs(current.x - start.x) <= up * 0.55
    }
}

/// Plans the backspace swipe-up: delete leftward from the cursor, stopping at the current line's start.
public enum IOSLineClear {
    public struct Plan: Equatable {
        public let deleteCount: Int
        /// True when this batch finishes the line (a line break is visible, or nothing is left to delete).
        public let reachesLineStart: Bool
    }

    public static func plan(before: String, maxBatch: Int) -> Plan {
        let lineLength = before.reversed().prefix(while: { !$0.isNewline }).count
        let sawLineBreak = lineLength < before.count
        if lineLength > maxBatch {
            return Plan(deleteCount: maxBatch, reachesLineStart: false)
        }
        return Plan(deleteCount: lineLength, reachesLineStart: sawLineBreak)
    }

    /// What the host context must look like before the next batch may run.
    public struct Expected: Equatable {
        let previous: String
        let remaining: String

        public init(previous: String, removed: Int) {
            self.previous = previous
            self.remaining = String(previous.dropLast(removed))
        }

        public func isSatisfied(by before: String) -> Bool {
            if remaining.isEmpty {
                return !previous.hasPrefix(before)
            }
            return before.hasSuffix(remaining)
        }
    }
}

/// Undo history for backspace line clears. A record is only restorable in the same document with the
/// cursor back where the clear left it, so typing or moving elsewhere can never insert text in the wrong place.
public struct IOSLineClearUndo: Equatable {
    public struct Record: Equatable {
        public let text: String
        public let contextAfter: String
        public let documentID: UUID?
    }

    public static let maxRecords = 8
    public private(set) var records: [Record] = []

    public init() {}

    public mutating func push(text: String, contextBefore: String, documentID: UUID?) {
        guard !text.isEmpty, let documentID else { return }
        records.append(Record(text: text, contextAfter: String(contextBefore.suffix(32)), documentID: documentID))
        if records.count > Self.maxRecords {
            records.removeFirst(records.count - Self.maxRecords)
        }
    }

    public func restorableIndex(before: String, documentID: UUID?) -> Int? {
        guard let documentID else { return nil }
        return records.lastIndex { record in
            guard record.documentID == documentID else { return false }
            return record.contextAfter.isEmpty ? before.isEmpty : before.hasSuffix(record.contextAfter)
        }
    }

    public mutating func take(before: String, documentID: UUID?) -> String? {
        guard let index = restorableIndex(before: before, documentID: documentID) else { return nil }
        return records.remove(at: index).text
    }

    public mutating func removeAll() {
        records.removeAll()
    }
}

/// Long-press character strip: tone-marked letters, uppercase, the swipe-up symbol and lowercase.
public enum IOSKeyAlternates {
    private static let accents: [UInt8: [String]] = [
        UInt8(ascii: "a"): ["ā", "á", "ǎ", "à"],
        UInt8(ascii: "e"): ["ē", "é", "ě", "è"],
        UInt8(ascii: "i"): ["ī", "í", "ǐ", "ì"],
        UInt8(ascii: "o"): ["ō", "ó", "ǒ", "ò"],
        UInt8(ascii: "u"): ["ū", "ú", "ǔ", "ù"],
        UInt8(ascii: "v"): ["ǖ", "ǘ", "ǚ", "ǜ", "ü"],
    ]

    /// `defaultIndex` points at the digit/symbol, which is what a plain long-press release enters.
    public static func options(for letter: UInt8, asciiMode: Bool) -> (items: [String], defaultIndex: Int) {
        let lower = String(UnicodeScalar(letter))
        var items = accents[letter] ?? []
        items.append(lower.uppercased())
        var defaultIndex = items.count - 1
        if let symbol = IOSSwipeUp.symbol(for: letter, asciiMode: asciiMode) {
            items.append(symbol)
            defaultIndex = items.count - 1
        }
        items.append(lower)
        return (items, defaultIndex)
    }

    private static let symbolAlternates: [String: [String]] = [
        "0": ["0", "°"],
        "-": ["-", "–", "—", "•"],
        "/": ["/", "\\"],
        ":": [":", "："],
        "：": ["：", ":"],
        ";": [";", "；"],
        "；": ["；", ";"],
        "(": ["(", "（", "[", "{"],
        "（": ["（", "(", "「", "『", "【", "{"],
        ")": [")", "）", "]", "}"],
        "）": ["）", ")", "」", "』", "】", "}"],
        "$": ["$", "￥", "€", "£", "¥", "₩", "¢"],
        "￥": ["￥", "$", "€", "£", "¥", "₩", "¢"],
        "&": ["&", "§"],
        "\"": ["\"", "“", "”", "„", "«", "»"],
        "“": ["“", "\"", "”", "„", "«", "»"],
        "”": ["”", "\"", "“", "„", "«", "»"],
        ".": [".", "…", "。"],
        "。": ["。", ".", "…", "．"],
        ",": [",", "，"],
        "，": ["，", ",", "、"],
        "、": ["、", "\\", "|"],
        "?": ["?", "¿", "？"],
        "？": ["？", "?", "¿"],
        "!": ["!", "¡", "！"],
        "！": ["！", "!", "¡"],
        "'": ["'", "‘", "’", "`"],
        "‘": ["‘", "'", "’", "`"],
        "’": ["’", "'", "‘", "`"],
        "[": ["[", "【", "「", "『", "〔"],
        "]": ["]", "】", "」", "』", "〕"],
        "{": ["{", "「", "『", "【"],
        "}": ["}", "」", "』", "】"],
        "%": ["%", "‰"],
        "=": ["=", "≠", "≈"],
        "+": ["+", "±"],
        "*": ["*", "×", "•"],
        "~": ["~", "～"],
        "～": ["～", "~"],
        "<": ["<", "《", "〈", "≤"],
        ">": [">", "》", "〉", "≥"],
        "《": ["《", "〈", "<", "«"],
        "》": ["》", "〉", ">", "»"],
        "〈": ["〈", "《", "<"],
        "〉": ["〉", "》", ">"],
        "【": ["【", "「", "『", "[", "〔"],
        "】": ["】", "」", "』", "]", "〕"],
        "…": ["…", "……", "..."],
        "·": ["·", "•", "°"],
        "•": ["•", "·", "°"],
        "——": ["——", "—", "–", "-", "_"],
        "_": ["_", "——", "—", "–"],
        "€": ["€", "$", "￥", "£", "¥"],
        "£": ["£", "$", "￥", "€", "¥"],
        "¥": ["¥", "￥", "$", "€", "£"],
    ]

    /// Long-press alternates for symbol/number keys (matching native iOS keyboard).
    public static func symbolOptions(for symbol: String, asciiMode: Bool) -> (items: [String], defaultIndex: Int)? {
        if symbol == "-", !asciiMode {
            return (["-", "－", "—", "–", "•"], 0)
        }
        guard let items = symbolAlternates[symbol] else { return nil }
        return (items, 0)
    }

    public static func cellWidth(count: Int, containerWidth: CGFloat) -> CGFloat {
        min(42.0, (containerWidth - 8.0) / CGFloat(max(count, 1)))
    }

    /// Strip frame centered on the default cell over the key, clamped inside the keyboard.
    public static func stripFrame(
        keyFrame: CGRect,
        count: Int,
        defaultIndex: Int,
        containerWidth: CGFloat,
        height: CGFloat = 50.0
    ) -> CGRect {
        let cell = cellWidth(count: count, containerWidth: containerWidth)
        let width = cell * CGFloat(count)
        let idealX = keyFrame.midX - cell * (CGFloat(defaultIndex) + 0.5)
        let x = min(max(4.0, idealX), max(4.0, containerWidth - width - 4.0))
        return CGRect(x: x, y: max(2.0, keyFrame.minY - height - 5.0), width: width, height: height)
    }

    public static func index(atX x: CGFloat, in strip: CGRect, count: Int) -> Int {
        guard count > 0, strip.width > 0 else { return 0 }
        let raw = Int(floor((x - strip.minX) / (strip.width / CGFloat(count))))
        return min(max(0, raw), count - 1)
    }
}

public struct IOSKeySpec: Identifiable, Equatable {
    public let id: String
    public let kind: IOSKeyKind
    public let frame: CGRect
    public let primaryLabel: String
    /// Secondary muted inline suffix (e.g. `"/英"` or `"/中"` on the mode switch key).
    public let secondaryInlineLabel: String?
    /// Corner subscript label (e.g. `"五"` in bottom-right of iOS 26 native Wubi spacebar).
    public let cornerSubscript: String?
    /// Compact 1–2 line radical mnemonic block drawn beneath the letter on Wubi radical keycaps.
    public let keycapRadicals: String?
    public let systemSymbol: String?
    public let isBlueAccent: Bool
    /// Character entered by swiping up on this key (drawn as a small hint unless radicals occupy the cap).
    public let swipeUpSymbol: String?

    public init(
        id: String,
        kind: IOSKeyKind,
        frame: CGRect,
        primaryLabel: String,
        secondaryInlineLabel: String? = nil,
        cornerSubscript: String? = nil,
        keycapRadicals: String? = nil,
        systemSymbol: String? = nil,
        isBlueAccent: Bool = false,
        swipeUpSymbol: String? = nil
    ) {
        self.id = id
        self.kind = kind
        self.frame = frame
        self.primaryLabel = primaryLabel
        self.secondaryInlineLabel = secondaryInlineLabel
        self.cornerSubscript = cornerSubscript
        self.keycapRadicals = keycapRadicals
        self.systemSymbol = systemSymbol
        self.isBlueAccent = isBlueAccent
        self.swipeUpSymbol = swipeUpSymbol
    }
}

/// Exact iOS 26 native Wubi keyboard geometric & state engine, calibrated 1:1 from iOS 26 metrics:
/// - Uniform pure-white rounded-squircle keycaps (`cornerRadius = 7.8pt`, continuous) in Light mode
/// - Lowercase key legends (`q w e r t y...`) by default, uppercase only when Shift/CapsLock active
/// - Ergonomic thumb downward bias (`2.6pt`) + Wubi dictionary valid-next-key dynamic hitbox expansion
public struct IOSKeyboardLayoutEngine: Equatable {
    public static let keyCornerRadius: CGFloat = 7.8
    public static let trayTopCornerRadius: CGFloat = 26.0
    public static let candidateBarHeight: CGFloat = 53.0
    public static let landscapeCandidateBarHeight: CGFloat = 40.0
    /// Thumbs naturally strike ~2.6pt below visual keycap center on handheld phones.
    public static let ergonomicThumbYBias: CGFloat = 2.6

    public var plane: IOSKeyPlane = .letters
    public var shiftState: IOSShiftState = .lower
    public var asciiMode: Bool = false
    public var schemaId: UInt32 = 86
    public var pinyinT9Layout: Bool = false
    public var cangjieKeycapRadicals: Bool = true
    public var wubiKeycapRadicals: Bool = false
    public var swipeUpInputEnabled: Bool = false
    /// Temporarily show Latin `a..z` keycaps during Cangjie `z`+Pinyin reverse lookup.
    public var isZLookupActive: Bool = false
    /// True only on Home-button devices where system does not draw its own bottom Globe bar.
    public var showInlineGlobeKey: Bool = false
    public var returnRole: IOSReturnKeyRole = .normal
    public var isComposing: Bool = false
    public var isTrackpadActive: Bool = false
    /// 26-bit mask of valid next Wubi letters (`0x03FF_FFFF` when unconstrained).
    public var validNextMask: UInt32 = 0x03FF_FFFF
    /// 26-element continuous neural prior weights (`0` = dead, `100` = neutral, `50..180` = RWKV weighted).
    public var nextKeyPriors: [UInt8] = Array(repeating: 100, count: 26)
    /// 52-element per-key adaptive touch offsets `[dx0, dy0, ..., dx25, dy25]` in normalized key-pitch units.
    public var touchOffsets: [Float] = Array(repeating: 0, count: 52)
    /// 8-bit mask of valid next T9 keys (`'2'..='9'`, `0xFF` when unconstrained).
    public var t9ValidNextMask: UInt8 = 0xFF
    /// 8-element continuous neural prior weights for T9 keys `'2'..='9'`.
    public var t9NextKeyPriors: [UInt8] = Array(repeating: 100, count: 8)
    /// 16-element per-key adaptive touch offsets `[dx2, dy2, ..., dx9, dy9]` in normalized T9 key-pitch units.
    public var t9TouchOffsets: [Float] = Array(repeating: 0, count: 16)

    public var isT9PinyinActive: Bool {
        plane == .letters && !asciiMode && schemaId == 20 && pinyinT9Layout
    }

    private var lastShiftTapTime: TimeInterval = 0

    public init() {}

    public mutating func tapShift(at timestamp: TimeInterval) {
        switch shiftState {
        case .lower:
            shiftState = .upperOnce
            lastShiftTapTime = timestamp
        case .upperOnce:
            if timestamp - lastShiftTapTime <= 0.34 {
                shiftState = .capsLocked
            } else {
                shiftState = .lower
            }
            lastShiftTapTime = 0
        case .capsLocked:
            shiftState = .lower
            lastShiftTapTime = 0
        }
    }

    public mutating func consumeOneShotShiftIfNeeded() {
        if shiftState == .upperOnce {
            shiftState = .lower
        }
    }

    public mutating func switchPlane(_ target: IOSKeyPlane) {
        plane = target
        if target != .letters && shiftState == .upperOnce {
            shiftState = .lower
        }
    }

    /// Subtle bottom-right subscript on the spacebar reflecting active input schema.
    public var spaceCornerTag: String? {
        if isTrackpadActive {
            return nil
        }
        if asciiMode {
            return "EN"
        }
        switch schemaId {
        case 20: return pinyinT9Layout ? "九" : "拼"
        case 21: return "双"
        case 10: return "鹤"
        case 33, 35, 36: return "仓"
        default: return "五"
        }
    }

    public struct LayoutSnapshot {
        public let bounds: CGRect
        public let keys: [IOSKeySpec]
        public let centers52: [Float]
        public let originX: Float
        public let originY: Float
        public let pitchX: Float
        public let pitchY: Float
        public let bottomRowMinY: CGFloat
        public let isT9Layout: Bool
        public let t9SidebarFrame: CGRect
        public let t9Centers16: [Float]
        public let t9OriginX: Float
        public let t9OriginY: Float
        public let t9PitchX: Float
        public let t9PitchY: Float
    }

    public func computeLayout(in size: CGSize) -> LayoutSnapshot {
        let w = max(280, size.width)
        let isLandscape = w > 540
        // Guard against transient full-screen container heights (e.g. 903pt before XPC heightConstraint settles
        // or while a system paste-permission modal interrupts layout) so Row 0 never stretches to fill the tray.
        let maxReasonableH: CGFloat = isLandscape ? 220.0 : 225.0
        let h = min(max(168, size.height), maxReasonableH)

        // Exact iOS 26 proportions (`440pt` -> outerMargin `6.67pt`, gapX `6.00pt`, keyW `37.27pt`, pitchX `43.27pt`,
        // `keyH = 45.0pt`, `rowGap = 11.0pt`, total `53 + 220 = 273pt` matching native `608..881pt` tray).
        // `bottomPad = 7.0pt` preserves generous bottom catch-margin inside the `220pt` XPC key canvas.
        let outerX: CGFloat = isLandscape ? 8.0 : round(w * (20.0 / 1320.0) * 3.0) / 3.0
        let gapX: CGFloat = isLandscape ? 6.5 : round(w * (18.0 / 1320.0) * 3.0) / 3.0
        let topPad: CGFloat = isLandscape ? 2.0 : 0.0
        let bottomPad: CGFloat = isLandscape ? 4.5 : 7.0
        let rowGap: CGFloat = isLandscape ? 8.0 : 11.0

        let usableH = h - topPad - bottomPad - rowGap * 3.0
        let keyH = max(34.0, floor(usableH / 4.0))
        let pitchY = keyH + rowGap

        // 10 keys + 9 gaps = w - 2 * outerX
        let stdKeyW = (w - outerX * 2.0 - gapX * 9.0) / 10.0
        let pitchX = stdKeyW + gapX

        let rowY: [CGFloat] = [
            topPad,
            topPad + pitchY,
            topPad + pitchY * 2.0,
            topPad + pitchY * 3.0,
        ]

        if isT9PinyinActive {
            return computeT9Layout(
                w: w,
                h: h,
                outerX: outerX,
                gapX: gapX,
                rowGap: rowGap,
                keyH: keyH,
                pitchY: pitchY,
                rowY: rowY
            )
        }

        // Precompute exact Row 1 (`a..l`) x-origins because Row 2 (`z..m`) and Row 3 (`Space`/`Return`) lock to them!
        let row1StartX = outerX + pitchX * 0.5
        let colRow1X = (0..<9).map { i in row1StartX + CGFloat(i) * pitchX }

        var specs: [IOSKeySpec] = []

        switch plane {
        case .letters:
            // Row 0: q w e r t y u i o p
            for (c, b) in Array("qwertyuiop".utf8).enumerated() {
                let x = outerX + CGFloat(c) * pitchX
                specs.append(
                    IOSKeySpec(
                        id: "L_\(b)",
                        kind: .letter(b),
                        frame: CGRect(x: x, y: rowY[0], width: stdKeyW, height: keyH),
                        primaryLabel: formatLetter(b),
                        keycapRadicals: formatWubiRadicals(b),
                        swipeUpSymbol: (isTrackpadActive || !swipeUpInputEnabled) ? nil : IOSSwipeUp.symbol(for: b, asciiMode: asciiMode)
                    )
                )
            }

            // Row 1: a s d f g h j k l
            for (c, b) in Array("asdfghjkl".utf8).enumerated() {
                specs.append(
                    IOSKeySpec(
                        id: "L_\(b)",
                        kind: .letter(b),
                        frame: CGRect(x: colRow1X[c], y: rowY[1], width: stdKeyW, height: keyH),
                        primaryLabel: formatLetter(b),
                        keycapRadicals: formatWubiRadicals(b),
                        swipeUpSymbol: (isTrackpadActive || !swipeUpInputEnabled) ? nil : IOSSwipeUp.symbol(for: b, asciiMode: asciiMode)
                    )
                )
            }

            // Row 2: [⇧] + z x c v b n m (aligned with colRow1X[1..7] = s..k!) + [⌫]
            let sideKeyW = stdKeyW * 1.342 // 49.33pt on 440pt width
            let shiftSym: String
            switch shiftState {
            case .lower: shiftSym = "shift"
            case .upperOnce: shiftSym = "shift.fill"
            case .capsLocked: shiftSym = "capslock.fill"
            }
            specs.append(
                IOSKeySpec(
                    id: "K_shift",
                    kind: .shift,
                    frame: CGRect(x: outerX, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: "",
                    systemSymbol: isTrackpadActive ? nil : shiftSym
                )
            )

            for (c, b) in Array("zxcvbnm".utf8).enumerated() {
                specs.append(
                    IOSKeySpec(
                        id: "L_\(b)",
                        kind: .letter(b),
                        frame: CGRect(x: colRow1X[c + 1], y: rowY[2], width: stdKeyW, height: keyH),
                        primaryLabel: formatLetter(b),
                        keycapRadicals: formatWubiRadicals(b),
                        swipeUpSymbol: (isTrackpadActive || !swipeUpInputEnabled) ? nil : IOSSwipeUp.symbol(for: b, asciiMode: asciiMode)
                    )
                )
            }

            specs.append(
                IOSKeySpec(
                    id: "K_backspace",
                    kind: .backspace,
                    frame: CGRect(x: w - outerX - sideKeyW, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: "",
                    systemSymbol: isTrackpadActive ? nil : "delete.backward"
                )
            )

        case .numbers:
            let r0 = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]
            appendTenRow(r0, rowIdx: 0, rowY: rowY[0], outerX: outerX, pitchX: pitchX, keyW: stdKeyW, keyH: keyH, into: &specs)

            let r1 = asciiMode
                ? ["-", "/", ":", ";", "(", ")", "$", "&", "@", "\""]
                : ["-", "/", "：", "；", "（", "）", "￥", "@", "“", "”"]
            appendTenRow(r1, rowIdx: 1, rowY: rowY[1], outerX: outerX, pitchX: pitchX, keyW: stdKeyW, keyH: keyH, into: &specs)

            let sideKeyW = stdKeyW * 1.342
            specs.append(
                IOSKeySpec(
                    id: "P_symbols",
                    kind: .planeSwitch(.symbols, label: "#+="),
                    frame: CGRect(x: outerX, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : "#+="
                )
            )
            let puncMid = asciiMode
                ? [".", ",", "?", "!", "'"]
                : ["。", "，", "、", "？", "！", "."]
            appendSpanSymbols(
                puncMid,
                rowY: rowY[2],
                minX: colRow1X[1],
                maxX: colRow1X[7] + stdKeyW,
                gapX: gapX,
                keyH: keyH,
                into: &specs
            )
            specs.append(
                IOSKeySpec(
                    id: "K_backspace",
                    kind: .backspace,
                    frame: CGRect(x: w - outerX - sideKeyW, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: "",
                    systemSymbol: isTrackpadActive ? nil : "delete.backward"
                )
            )

        case .symbols:
            let r0 = ["[", "]", "{", "}", "#", "%", "^", "*", "+", "="]
            appendTenRow(r0, rowIdx: 0, rowY: rowY[0], outerX: outerX, pitchX: pitchX, keyW: stdKeyW, keyH: keyH, into: &specs)

            let r1 = asciiMode
                ? ["_", "\\", "|", "~", "<", ">", "€", "£", "¥", "•"]
                : ["——", "\\", "|", "～", "《", "》", "【", "】", "…", "·"]
            appendTenRow(r1, rowIdx: 1, rowY: rowY[1], outerX: outerX, pitchX: pitchX, keyW: stdKeyW, keyH: keyH, into: &specs)

            let sideKeyW = stdKeyW * 1.342
            specs.append(
                IOSKeySpec(
                    id: "P_numbers",
                    kind: .planeSwitch(.numbers, label: "123"),
                    frame: CGRect(x: outerX, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : "123"
                )
            )
            let symMid = ["…", "‘", "’", "〈", "〉"]
            appendSpanSymbols(
                symMid,
                rowY: rowY[2],
                minX: colRow1X[1],
                maxX: colRow1X[7] + stdKeyW,
                gapX: gapX,
                keyH: keyH,
                into: &specs
            )
            specs.append(
                IOSKeySpec(
                    id: "K_backspace",
                    kind: .backspace,
                    frame: CGRect(x: w - outerX - sideKeyW, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: "",
                    systemSymbol: isTrackpadActive ? nil : "delete.backward"
                )
            )
        }

        // Row 3: Exact iOS 26 4-Key Grid!
        let leftPairSpan = (colRow1X[1] + stdKeyW) - outerX
        let leftEachW = (leftPairSpan - gapX) * 0.5

        let targetPlane: IOSKeyPlane = (plane == .letters) ? .numbers : .letters
        let planeLabel = (plane == .letters) ? "123" : "ABC"
        specs.append(
            IOSKeySpec(
                id: "B_plane",
                kind: .planeSwitch(targetPlane, label: planeLabel),
                frame: CGRect(x: outerX, y: rowY[3], width: leftEachW, height: keyH),
                primaryLabel: isTrackpadActive ? "" : planeLabel
            )
        )

        let secondLeftX = outerX + leftEachW + gapX
        if showInlineGlobeKey {
            specs.append(
                IOSKeySpec(
                    id: "B_globe",
                    kind: .globe,
                    frame: CGRect(x: secondLeftX, y: rowY[3], width: leftEachW, height: keyH),
                    primaryLabel: "",
                    systemSymbol: isTrackpadActive ? nil : "globe"
                )
            )
        } else {
            // Crisp typographic 中/英 mode indicator instead of jarring smiley icon
            specs.append(
                IOSKeySpec(
                    id: "B_mode",
                    kind: .modeToggle,
                    frame: CGRect(x: secondLeftX, y: rowY[3], width: leftEachW, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : (asciiMode ? "EN" : "中"),
                    secondaryInlineLabel: isTrackpadActive ? nil : (asciiMode ? "/中" : "/英")
                )
            )
        }

        // Spacebar: `colRow1X[2] .. (colRow1X[6] + stdKeyW)` (`d.minX .. j.maxX`)!
        let spaceMinX = colRow1X[2]
        let spaceMaxX = colRow1X[6] + stdKeyW
        specs.append(
            IOSKeySpec(
                id: "B_space",
                kind: .space,
                frame: CGRect(x: spaceMinX, y: rowY[3], width: spaceMaxX - spaceMinX, height: keyH),
                primaryLabel: "",
                cornerSubscript: spaceCornerTag
            )
        )

        // Return Key: `colRow1X[7] .. (w - outerX)` (`k.minX .. p.maxX`)!
        let returnMinX = colRow1X[7]
        let returnMaxX = w - outerX
        let useBlue = returnRole.isBlueProminent
        specs.append(
            IOSKeySpec(
                id: "B_return",
                kind: .returnKey,
                frame: CGRect(x: returnMinX, y: rowY[3], width: returnMaxX - returnMinX, height: keyH),
                primaryLabel: isTrackpadActive ? "" : (useBlue ? returnRole.title : ""),
                systemSymbol: isTrackpadActive ? nil : (useBlue ? nil : "return.left"),
                isBlueAccent: useBlue
            )
        )

        // Compute exact 26-key normalized coordinates matching our actual Row 0/1/2 geometric centers.
        // Ergonomic thumb Y-bias is applied consistently via `compensatedLetterPoint(_:in:)` before
        // both UI `hitTest` and Rust `pushTouch`, keeping frontend and backend coordinates 1:1 aligned.
        let qMidX = outerX + stdKeyW * 0.5
        let qMidY = rowY[0] + keyH * 0.5
        var centers52 = [Float](repeating: 0, count: 52)
        for (c, b) in Array("qwertyuiop".utf8).enumerated() {
            let idx = Int(b - UInt8(ascii: "a"))
            centers52[idx * 2] = Float(c)
            centers52[idx * 2 + 1] = 0.0
        }
        for (c, b) in Array("asdfghjkl".utf8).enumerated() {
            let idx = Int(b - UInt8(ascii: "a"))
            centers52[idx * 2] = Float(c) + 0.5
            centers52[idx * 2 + 1] = 1.0
        }
        for (c, b) in Array("zxcvbnm".utf8).enumerated() {
            let idx = Int(b - UInt8(ascii: "a"))
            centers52[idx * 2] = Float(c) + 1.5
            centers52[idx * 2 + 1] = 2.0
        }

        return LayoutSnapshot(
            bounds: CGRect(origin: .zero, size: CGSize(width: w, height: h)),
            keys: specs,
            centers52: centers52,
            originX: Float(qMidX),
            originY: Float(qMidY),
            pitchX: Float(pitchX),
            pitchY: Float(pitchY),
            bottomRowMinY: rowY[3],
            isT9Layout: false,
            t9SidebarFrame: .zero,
            t9Centers16: Self.canonicalT9Centers16,
            t9OriginX: 0,
            t9OriginY: 0,
            t9PitchX: 1,
            t9PitchY: 1
        )
    }

    private static let canonicalT9Centers16: [Float] = [
        1, 0, // 2: ABC
        2, 0, // 3: DEF
        0, 1, // 4: GHI
        1, 1, // 5: JKL
        2, 1, // 6: MNO
        0, 2, // 7: PQRS
        1, 2, // 8: TUV
        2, 2, // 9: WXYZ
    ]

    /// Native iOS 9-key T9 Pinyin 5-column × 4-row grid inside the fixed 220pt key canvas:
    /// - Col 0 (left): Row 0..2 vertical punctuation/syllable filter sidebar (`t9SidebarFrame`), Row 3 `123`
    /// - Col 1..3 (center 3x3): `1(分词)` `2(ABC)` `3(DEF)` / `4(GHI)` `5(JKL)` `6(MNO)` / `7(PQRS)` `8(TUV)` `9(WXYZ)`
    ///   and Row 3: `中/英` (or `Globe`) + wide `空格`
    /// - Col 4 (right): Row 0 `⌫`, Row 1 `重输`, Row 2..3 tall 2-row `Return`
    private func computeT9Layout(
        w: CGFloat,
        h: CGFloat,
        outerX: CGFloat,
        gapX: CGFloat,
        rowGap: CGFloat,
        keyH: CGFloat,
        pitchY: CGFloat,
        rowY: [CGFloat]
    ) -> LayoutSnapshot {
        let usableW = w - outerX * 2.0 - gapX * 4.0
        let sideW = floor(usableW * 0.175)
        let midW = (usableW - sideW * 2.0) / 3.0
        let col0X = outerX
        let col1X = col0X + sideW + gapX
        let col2X = col1X + midW + gapX
        let col3X = col2X + midW + gapX
        let col4X = col3X + midW + gapX
        let midPitchX = midW + gapX

        let sidebarFrame = CGRect(
            x: col0X,
            y: rowY[0],
            width: sideW,
            height: rowY[2] + keyH - rowY[0]
        )

        var specs: [IOSKeySpec] = []

        // Center 3x3 T9 keypad
        let t9Grid: [(UInt8, String, String, CGFloat, CGFloat)] = [
            (UInt8(ascii: "1"), "分词", "", col1X, rowY[0]),
            (UInt8(ascii: "2"), "ABC", "2", col2X, rowY[0]),
            (UInt8(ascii: "3"), "DEF", "3", col3X, rowY[0]),
            (UInt8(ascii: "4"), "GHI", "4", col1X, rowY[1]),
            (UInt8(ascii: "5"), "JKL", "5", col2X, rowY[1]),
            (UInt8(ascii: "6"), "MNO", "6", col3X, rowY[1]),
            (UInt8(ascii: "7"), "PQRS", "7", col1X, rowY[2]),
            (UInt8(ascii: "8"), "TUV", "8", col2X, rowY[2]),
            (UInt8(ascii: "9"), "WXYZ", "9", col3X, rowY[2]),
        ]
        for (digit, label, sub, x, y) in t9Grid {
            specs.append(
                IOSKeySpec(
                    id: "T9_\(digit)",
                    kind: .t9Key(digit),
                    frame: CGRect(x: x, y: y, width: midW, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : label,
                    cornerSubscript: isTrackpadActive || sub.isEmpty ? nil : sub
                )
            )
        }

        // Right column: Backspace (Row 0), Clear/重输 (Row 1), Return (Row 2..3 tall)
        specs.append(
            IOSKeySpec(
                id: "K_backspace",
                kind: .backspace,
                frame: CGRect(x: col4X, y: rowY[0], width: sideW, height: keyH),
                primaryLabel: "",
                systemSymbol: isTrackpadActive ? nil : "delete.backward"
            )
        )
        specs.append(
            IOSKeySpec(
                id: "T9_clear",
                kind: .t9Clear,
                frame: CGRect(x: col4X, y: rowY[1], width: sideW, height: keyH),
                primaryLabel: isTrackpadActive ? "" : "重输"
            )
        )
        let useBlue = returnRole.isBlueProminent
        specs.append(
            IOSKeySpec(
                id: "B_return",
                kind: .returnKey,
                frame: CGRect(x: col4X, y: rowY[2], width: sideW, height: keyH * 2.0 + rowGap),
                primaryLabel: isTrackpadActive ? "" : (useBlue ? returnRole.title : ""),
                systemSymbol: isTrackpadActive ? nil : (useBlue ? nil : "return.left"),
                isBlueAccent: useBlue
            )
        )

        // Bottom row: [123] [中/英 or Globe] [Space spanning Col 2..3]
        specs.append(
            IOSKeySpec(
                id: "B_plane",
                kind: .planeSwitch(.numbers, label: "123"),
                frame: CGRect(x: col0X, y: rowY[3], width: sideW, height: keyH),
                primaryLabel: isTrackpadActive ? "" : "123"
            )
        )
        if showInlineGlobeKey {
            specs.append(
                IOSKeySpec(
                    id: "B_globe",
                    kind: .globe,
                    frame: CGRect(x: col1X, y: rowY[3], width: midW, height: keyH),
                    primaryLabel: "",
                    systemSymbol: isTrackpadActive ? nil : "globe"
                )
            )
        } else {
            specs.append(
                IOSKeySpec(
                    id: "B_mode",
                    kind: .modeToggle,
                    frame: CGRect(x: col1X, y: rowY[3], width: midW, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : "中",
                    secondaryInlineLabel: isTrackpadActive ? nil : "/英"
                )
            )
        }
        specs.append(
            IOSKeySpec(
                id: "B_space",
                kind: .space,
                frame: CGRect(x: col2X, y: rowY[3], width: midW * 2.0 + gapX, height: keyH),
                primaryLabel: "",
                cornerSubscript: spaceCornerTag
            )
        )

        let k1MidX = col1X + midW * 0.5
        let k1MidY = rowY[0] + keyH * 0.5
        return LayoutSnapshot(
            bounds: CGRect(origin: .zero, size: CGSize(width: w, height: h)),
            keys: specs,
            centers52: [Float](repeating: 0, count: 52),
            originX: Float(k1MidX),
            originY: Float(k1MidY),
            pitchX: Float(midPitchX),
            pitchY: Float(pitchY),
            bottomRowMinY: rowY[3],
            isT9Layout: true,
            t9SidebarFrame: sidebarFrame,
            t9Centers16: Self.canonicalT9Centers16,
            t9OriginX: Float(k1MidX),
            t9OriginY: Float(k1MidY),
            t9PitchX: Float(midPitchX),
            t9PitchY: Float(pitchY)
        )
    }

    /// Apply consistent row-aware ergonomic upward thumb compensation for letter taps,
    /// shared by both frontend `hitTest` and backend `pushTouch` coordinate normalization.
    public static func compensatedLetterPoint(_ point: CGPoint, in snapshot: LayoutSnapshot) -> CGPoint {
        var qPt = point
        let row0MidY = CGFloat(snapshot.originY)
        if point.y > row0MidY && point.y < snapshot.bottomRowMinY - 5.0 {
            qPt.y -= ergonomicThumbYBias
        }
        return qPt
    }

    public static func hitTest(
        point: CGPoint,
        in snapshot: LayoutSnapshot,
        validNextMask: UInt32 = 0x03FF_FFFF,
        nextKeyPriors: [UInt8] = [],
        touchOffsets: [Float] = [],
        t9ValidNextMask: UInt8 = 0xFF,
        t9NextKeyPriors: [UInt8] = [],
        t9TouchOffsets: [Float] = []
    ) -> IOSKeySpec? {
        guard snapshot.bounds.insetBy(dx: -18, dy: -28).contains(point) else {
            return nil
        }

        // 1. Functional keys (`Space`, `⌫`, `⇧`, `123`, `中/英`, `Globe`, `Return`, `重输`) are checked against
        //    unbiased `point` first so top-edge taps on functional keys are never pulled up into letters/T9.
        for k in snapshot.keys {
            switch k.kind {
            case .letter, .t9Key, .symbol:
                continue
            default:
                if k.frame.contains(point) {
                    return k
                }
            }
        }

        // 2. Apply ergonomic upward compensation strictly inside the 3 letter/symbol rows (never near Row 3
        //    `Space`/`123`/`Return`, so top-edge Space taps never get pulled into `c`/`v`/`b`/`n`!).
        let qPt = compensatedLetterPoint(point, in: snapshot)

        let hasMaskWeights = snapshot.isT9Layout
            ? (t9ValidNextMask != 0xFF && t9ValidNextMask != 0)
            : ((validNextMask != 0x03FF_FFFF) && (validNextMask != 0))
        let hasNeuralPriors = snapshot.isT9Layout
            ? ((t9NextKeyPriors.count == 8) && t9NextKeyPriors.contains(where: { $0 != 100 }))
            : ((nextKeyPriors.count == 26) && nextKeyPriors.contains(where: { $0 != 100 }))
        let hasTouchOffsets = snapshot.isT9Layout
            ? ((t9TouchOffsets.count == 16) && t9TouchOffsets.contains(where: { abs($0) > 1e-4 }))
            : ((touchOffsets.count == 52) && touchOffsets.contains(where: { abs($0) > 1e-4 }))
        let hasDynamicWeights = hasMaskWeights || hasNeuralPriors || hasTouchOffsets

        let personalShift: (UInt8) -> (CGFloat, CGFloat) = { ascii in
            guard hasTouchOffsets, ascii >= 97, ascii <= 122 else { return (0, 0) }
            let idx = Int(ascii - 97)
            let dx = CGFloat(max(-0.20, min(0.20, touchOffsets[idx * 2]))) * CGFloat(snapshot.pitchX)
            let dy = CGFloat(max(-0.20, min(0.20, touchOffsets[idx * 2 + 1]))) * CGFloat(snapshot.pitchY)
            return (dx, dy)
        }
        let personalT9Shift: (UInt8) -> (CGFloat, CGFloat) = { digit in
            guard hasTouchOffsets, digit >= UInt8(ascii: "2"), digit <= UInt8(ascii: "9") else { return (0, 0) }
            let idx = Int(digit - UInt8(ascii: "2"))
            let dx = CGFloat(max(-0.20, min(0.20, t9TouchOffsets[idx * 2]))) * CGFloat(snapshot.t9PitchX)
            let dy = CGFloat(max(-0.20, min(0.20, t9TouchOffsets[idx * 2 + 1]))) * CGFloat(snapshot.t9PitchY)
            return (dx, dy)
        }

        if !hasDynamicWeights {
            if let direct = snapshot.keys.first(where: { $0.frame.contains(qPt) }) {
                return direct
            }
        } else {
            // Sacred Keycap Core: any tap landing cleanly inside the visual interior (`>= 3.0pt` from X bevel,
            // `>= 2.6pt` from Y bevel, shifted gently by personal TouchProfile) ALWAYS belongs to that keycap
            // even if `validNextMask` marks it 0.
            // Dynamic weighting only arbitrates border/bevel (`0..3pt`) & inter-key gutter taps!
            for k in snapshot.keys {
                switch k.kind {
                case .letter(let ascii):
                    let (px, py) = personalShift(ascii)
                    let core = k.frame.offsetBy(dx: px * 0.5, dy: py * 0.5).insetBy(dx: 3.0, dy: 2.6)
                    if core.contains(qPt) {
                        return k
                    }
                case .t9Key(let digit):
                    let (px, py) = personalT9Shift(digit)
                    let core = k.frame.offsetBy(dx: px * 0.5, dy: py * 0.5).insetBy(dx: 4.5, dy: 2.8)
                    if core.contains(qPt) {
                        return k
                    }
                case .symbol:
                    if k.frame.contains(qPt) {
                        return k
                    }
                default:
                    break
                }
            }
        }

        var best: IOSKeySpec?
        var bestScore = CGFloat.greatestFiniteMagnitude
        let maxLetterBottomY = snapshot.bottomRowMinY - 6.0

        for k in snapshot.keys {
            var effRect = k.frame
            var deadKeyPenalty: CGFloat = 0
            var evalPt = qPt

            switch k.kind {
            case .space:
                // Evaluate Row 3 against raw `point` and give Spacebar generous top (`4.5pt`), side (`2.2pt`),
                // and bottom (`10pt`) cushions so light grazing thumb taps on any rim of Spacebar never miss!
                evalPt = point
                effRect = CGRect(
                    x: k.frame.minX - 2.2,
                    y: k.frame.minY - 4.5,
                    width: k.frame.width + 4.4,
                    height: k.frame.height + 14.5
                )
            case .planeSwitch(_, let lbl) where k.frame.minY >= snapshot.bottomRowMinY - 1.0 && lbl != "#+=":
                evalPt = point
                effRect = CGRect(
                    x: k.frame.minX - 2.0,
                    y: k.frame.minY - 3.0,
                    width: k.frame.width + 2.0,
                    height: k.frame.height + 12.0
                )
            case .modeToggle, .globe, .returnKey:
                evalPt = point
                effRect = CGRect(
                    x: k.frame.minX,
                    y: k.frame.minY - 3.0,
                    width: k.frame.width,
                    height: k.frame.height + 12.0
                )
            case .shift, .backspace, .t9Clear:
                evalPt = point
            case .t9Key(let digit) where hasDynamicWeights && digit >= UInt8(ascii: "2") && digit <= UInt8(ascii: "9"):
                let (px, py) = personalT9Shift(digit)
                effRect = effRect.offsetBy(dx: px, dy: py)
                let idx = Int(digit - UInt8(ascii: "2"))
                let bit = UInt8(1) << UInt8(idx)
                let pVal = (t9NextKeyPriors.count == 8) ? Int(t9NextKeyPriors[idx]) : ((t9ValidNextMask & bit) != 0 ? 100 : 0)
                if (hasMaskWeights && (t9ValidNextMask & bit) == 0) || pVal == 0 {
                    effRect = effRect.insetBy(dx: 3.2, dy: 2.4)
                    deadKeyPenalty = 4.2
                } else {
                    let bias = CGFloat(pVal - 100) / 25.0
                    let expX: CGFloat = hasMaskWeights
                        ? max(1.0, min(5.2, 3.6 + bias * 1.0))
                        : max(-1.5, min(3.4, bias * 1.3))
                    let expY: CGFloat = hasMaskWeights
                        ? max(0.8, min(3.8, 2.8 + bias * 0.7))
                        : max(-1.0, min(2.4, bias * 0.9))
                    deadKeyPenalty = -bias * (hasMaskWeights ? 1.05 : 0.8)
                    effRect = effRect.insetBy(dx: -expX, dy: -expY)
                    if effRect.maxY > maxLetterBottomY {
                        effRect.size.height = max(k.frame.height, maxLetterBottomY - effRect.minY)
                    }
                }
            case .letter(let ascii) where hasDynamicWeights && ascii >= 97 && ascii <= 122:
                let (px, py) = personalShift(ascii)
                effRect = effRect.offsetBy(dx: px, dy: py)
                let idx = Int(ascii - 97)
                let bit = UInt32(1) << UInt32(idx)
                let pVal = (nextKeyPriors.count == 26) ? Int(nextKeyPriors[idx]) : ((validNextMask & bit) != 0 ? 100 : 0)
                if (hasMaskWeights && (validNextMask & bit) == 0) || pVal == 0 {
                    effRect = effRect.insetBy(dx: 2.8, dy: 2.2)
                    deadKeyPenalty = 4.2
                } else {
                    // Modulate gutter/bevel reach continuously by `Micro-RWKV-7` next-key log-prior (`pVal = 54..176`, `100` = neutral)
                    let bias = CGFloat(pVal - 100) / 25.0
                    let expX: CGFloat
                    let expY: CGFloat
                    if !hasMaskWeights {
                        // Key #1 predictive gutter mode (`validNextMask == ALL_26`)
                        expX = max(-1.6, min(3.8, bias * 1.45))
                        expY = max(-1.0, min(2.4, bias * 0.95))
                        deadKeyPenalty = -bias * 0.85
                    } else {
                        // Keys #2..#4 valid continuation mode (breaks valid-vs-valid gutter ties!)
                        expX = max(1.2, min(6.2, 4.8 + bias * 1.15))
                        expY = max(0.8, min(4.0, 3.2 + bias * 0.75))
                        deadKeyPenalty = -bias * 1.15
                    }
                    effRect = effRect.insetBy(dx: -expX, dy: -expY)
                    if effRect.maxY > maxLetterBottomY {
                        effRect.size.height = max(k.frame.height, maxLetterBottomY - effRect.minY)
                    }
                }
            default:
                break
            }

            let dx = max(effRect.minX - evalPt.x, 0, evalPt.x - effRect.maxX)
            let dy = max(effRect.minY - evalPt.y, 0, evalPt.y - effRect.maxY)
            let score: CGFloat
            if dx == 0 && dy == 0 {
                // Point is inside `effRect`: negative signed depth to nearest edge breaks ties cleanly!
                let innerDepth = min(
                    evalPt.x - effRect.minX,
                    effRect.maxX - evalPt.x,
                    evalPt.y - effRect.minY,
                    effRect.maxY - evalPt.y
                )
                score = -innerDepth + deadKeyPenalty * 0.25
            } else {
                score = hypot(dx, dy) + deadKeyPenalty
            }

            if score < bestScore {
                bestScore = score
                best = k
            }
        }
        return best
    }

    /// Native iOS 26 Chinese keyboard stay-vs-return rules for `123` and `#+=` planes:
    /// - On `123`: digits (`0-9`) and numeric/time/currency/path connectors (`- / : ： . ￥ ¥ $`) stay on `123`;
    ///   sentence punctuation, quotes, brackets, `@`, `&`, `;` return to `.letters`.
    /// - On `#+=`: `~`, `～`, `…` stay on `#+=`; brackets, math, and punctuation symbols return to `.letters`.
    public static func shouldReturnToLettersAfterSymbol(_ sym: String, on plane: IOSKeyPlane) -> Bool {
        if sym.allSatisfy(\.isNumber) {
            return false
        }
        switch plane {
        case .numbers:
            let staySet: Set<String> = ["-", "/", ":", "：", ".", "￥", "¥", "$"]
            return !staySet.contains(sym)
        case .symbols:
            let staySet: Set<String> = ["~", "～", "…"]
            return !staySet.contains(sym)
        case .letters:
            return false
        }
    }

    public static let cangjieRadicalTable: [String] = [
        "日", "月", "金", "木", "水", "火", "土", "竹", "戈", "十", "大", "中", "一",
        "弓", "人", "心", "手", "口", "尸", "廿", "山", "女", "田", "難", "卜", "重",
    ]

    public static func cangjieRadical(for ascii: UInt8) -> String? {
        let idx: Int
        if ascii >= UInt8(ascii: "a") && ascii <= UInt8(ascii: "z") {
            idx = Int(ascii - UInt8(ascii: "a"))
        } else if ascii >= UInt8(ascii: "A") && ascii <= UInt8(ascii: "Z") {
            idx = Int(ascii - UInt8(ascii: "A"))
        } else {
            return nil
        }
        return cangjieRadicalTable[idx]
    }

    public var showsCangjieRadicals: Bool {
        cangjieKeycapRadicals
            && !asciiMode
            && !shiftState.isUppercase
            && !isZLookupActive
            && (schemaId == 33 || schemaId == 35 || schemaId == 36)
    }

    public static let wubi86RadicalTable: [String] = [
        "工戈艹\n匚七廿", // a
        "子耳了\n也阝",   // b
        "又巴马\n厶予",   // c
        "大犬三\n古石厂", // d
        "月彡乃\n用豕",   // e
        "土士二\n干十寸", // f
        "王戋\n五一",     // g
        "目上止\n卜皮",   // h
        "水氵小\n氺",     // i
        "日早虫\n曰刂",   // j
        "口川",           // k
        "田甲四\n车力囗", // l
        "山由贝\n骨几",   // m
        "已己尸\n心羽",   // n
        "火灬米\n业",     // o
        "之宀冖\n辶礻",   // p
        "金钅勹\n夕儿",   // q
        "白手扌\n斤丘",   // r
        "木丁西",         // s
        "禾竹彳\n攵夂",   // t
        "立辛六\n门疒",   // u
        "女刀九\n臼彐",   // v
        "人亻八",         // w
        "纟幺弓\n匕母",   // x
        "言讠文\n方广",   // y
        "万能",           // z
    ]

    public static let wubi98RadicalTable: [String] = [
        "工戈艹\n匚七廿", // a
        "子耳了\n也皮",   // b
        "又巴马\n厶予",   // c
        "大犬三\n古石厂", // d
        "月彡用\n力且",   // e
        "土士二\n干十寸", // f
        "王戋\n五一",     // g
        "目上止\n卜具",   // h
        "水氵小\n氺",     // i
        "日早虫\n曰刂",   // j
        "口川",           // k
        "田甲四\n车囗皿", // l
        "山由贝\n骨冂",   // m
        "已己尸\n心羽",   // n
        "火灬米\n业广",   // o
        "之宀冖\n辶礻",   // p
        "金钅勹\n夕儿",   // q
        "白手扌\n斤气",   // r
        "木丁西",         // s
        "禾竹彳\n攵夂",   // t
        "立辛六\n门疒",   // u
        "女刀九\n彐巛",   // v
        "人亻八\n几",     // w
        "纟幺弓\n匕母",   // x
        "言讠文\n方主",   // y
        "万能",           // z
    ]

    public static let wubi06RadicalTable: [String] = [
        "工戈艹\n匚七廿", // a
        "子耳了\n也阝",   // b
        "又巴马\n厶予",   // c
        "大犬三\n古石厂", // d
        "月彡用\n力且",   // e
        "土士二\n干十寸", // f
        "王五一\n青",     // g
        "目上止\n卜皮",   // h
        "水氵小\n氺",     // i
        "日早虫\n曰刂",   // j
        "口川",           // k
        "田甲四\n车囗皿", // l
        "山由贝\n骨冂",   // m
        "已己尸\n心羽",   // n
        "火灬米\n业广",   // o
        "之宀冖\n辶礻",   // p
        "金钅勹\n夕儿",   // q
        "白手扌\n斤气",   // r
        "木丁西",         // s
        "禾竹彳\n攵夂",   // t
        "立辛门\n疒丬",   // u
        "女刀九\n彐巛",   // v
        "人亻八\n几",     // w
        "纟幺弓\n匕母",   // x
        "言讠文\n方六",   // y
        "万能",           // z
    ]

    public static func wubiRadicals(for ascii: UInt8, schemaId: UInt32) -> String? {
        let idx: Int
        if ascii >= UInt8(ascii: "a") && ascii <= UInt8(ascii: "z") {
            idx = Int(ascii - UInt8(ascii: "a"))
        } else if ascii >= UInt8(ascii: "A") && ascii <= UInt8(ascii: "Z") {
            idx = Int(ascii - UInt8(ascii: "A"))
        } else {
            return nil
        }
        switch schemaId {
        case 86: return wubi86RadicalTable[idx]
        case 98: return wubi98RadicalTable[idx]
        case 6: return wubi06RadicalTable[idx]
        default: return nil
        }
    }

    public var showsWubiRadicals: Bool {
        wubiKeycapRadicals
            && !asciiMode
            && !shiftState.isUppercase
            && (schemaId == 86 || schemaId == 98 || schemaId == 6)
    }

    private func formatWubiRadicals(_ ascii: UInt8) -> String? {
        if isTrackpadActive || !showsWubiRadicals {
            return nil
        }
        return Self.wubiRadicals(for: ascii, schemaId: schemaId)
    }

    private func formatLetter(_ ascii: UInt8) -> String {
        if isTrackpadActive {
            return ""
        }
        if showsCangjieRadicals, let rad = Self.cangjieRadical(for: ascii) {
            return rad
        }
        let s = String(UnicodeScalar(ascii))
        return shiftState.isUppercase ? s.uppercased() : s.lowercased()
    }

    private func appendTenRow(
        _ labels: [String],
        rowIdx: Int,
        rowY: CGFloat,
        outerX: CGFloat,
        pitchX: CGFloat,
        keyW: CGFloat,
        keyH: CGFloat,
        into specs: inout [IOSKeySpec]
    ) {
        for (c, s) in labels.enumerated() {
            let x = outerX + CGFloat(c) * pitchX
            specs.append(
                IOSKeySpec(
                    id: "S_\(rowIdx)_\(c)",
                    kind: .symbol(s),
                    frame: CGRect(x: x, y: rowY, width: keyW, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : s
                )
            )
        }
    }

    private func appendSpanSymbols(
        _ labels: [String],
        rowY: CGFloat,
        minX: CGFloat,
        maxX: CGFloat,
        gapX: CGFloat,
        keyH: CGFloat,
        into specs: inout [IOSKeySpec]
    ) {
        guard !labels.isEmpty else { return }
        let n = CGFloat(labels.count)
        let kw = ((maxX - minX) - gapX * (n - 1)) / n
        for (i, s) in labels.enumerated() {
            let x = minX + CGFloat(i) * (kw + gapX)
            specs.append(
                IOSKeySpec(
                    id: "SM_\(i)",
                    kind: .symbol(s),
                    frame: CGRect(x: x, y: rowY, width: kw, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : s
                )
            )
        }
    }
}

public struct SpaceTrackpadAccumulator {
    public private(set) var isActive: Bool = false
    private var anchorX: CGFloat = 0
    private var anchorY: CGFloat = 0
    private var lastUpdateTimestamp: TimeInterval = 0
    public let stepPoints: CGFloat = 9.5
    public let lineStepPoints: CGFloat = 24.0

    public init() {}

    public mutating func begin(at x: CGFloat) {
        begin(at: CGPoint(x: x, y: 0))
    }

    public mutating func begin(at point: CGPoint, timestamp: TimeInterval = 0) {
        isActive = true
        anchorX = point.x
        anchorY = point.y
        lastUpdateTimestamp = timestamp
    }

    public mutating func update(x: CGFloat) -> Int {
        update(point: CGPoint(x: x, y: anchorY)).dxSteps
    }

    public mutating func update(point: CGPoint, timestamp: TimeInterval = 0) -> (dxSteps: Int, dySteps: Int) {
        guard isActive else { return (0, 0) }
        let diffX = point.x - anchorX
        let diffY = point.y - anchorY

        var rawXSteps = Int((diffX / stepPoints).rounded(.towardZero))
        if rawXSteps != 0 {
            anchorX += CGFloat(rawXSteps) * stepPoints
            // Gentle velocity boost when sweeping fast horizontally across the trackpad
            if timestamp > 0 && lastUpdateTimestamp > 0 {
                let dt = max(0.004, timestamp - lastUpdateTimestamp)
                let speed = abs(diffX) / CGFloat(dt)
                if speed > 520 && abs(rawXSteps) == 1 {
                    rawXSteps *= 2
                }
            }
        }

        let dySteps = Int((diffY / lineStepPoints).rounded(.towardZero))
        if dySteps != 0 {
            anchorY += CGFloat(dySteps) * lineStepPoints
        }

        if rawXSteps != 0 || dySteps != 0 {
            lastUpdateTimestamp = timestamp
        }
        return (rawXSteps, dySteps)
    }

    public mutating func end() {
        isActive = false
    }
}
