import CoreGraphics
import Foundation

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
    /// During active composition (`isComposing == true`), require a longer, steeper deliberate drag
    /// so rapid ballistic thumb lift-rolls toward upper rows never break an ongoing Pinyin/Wubi stream.
    public static func armDistance(keyHeight: CGFloat, isComposing: Bool = false) -> CGFloat {
        if isComposing {
            return max(35.0, keyHeight * 0.80)
        }
        return max(26.0, keyHeight * 0.60)
    }

    /// Mostly-vertical upward drag past the arm distance, with hysteresis so sliding back down cancels it.
    public static func isActive(
        from start: CGPoint,
        to current: CGPoint,
        keyHeight: CGFloat,
        wasActive: Bool,
        isComposing: Bool = false
    ) -> Bool {
        let up = start.y - current.y
        let threshold = armDistance(keyHeight: keyHeight, isComposing: isComposing) * (wasActive ? 0.6 : 1.0)
        let maxLateralRatio: CGFloat = isComposing ? 0.42 : 0.55
        return up >= threshold && abs(current.x - start.x) <= up * maxLateralRatio
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

/// Computes accelerated hold-delete chunk sizes without crossing line breaks or overshooting punctuation boundaries.
public enum IOSBackspaceChunkDelete {
    public static func acceleratedDeleteCount(before: String) -> Int {
        guard !before.isEmpty else { return 0 }
        if before.last == "\n" {
            return 1
        }
        var count = 0
        for ch in before.reversed() {
            if ch == "\n" || count >= 3 { break }
            count += 1
            if count >= 2 && (ch == " " || ch == "，" || ch == "。" || ch == "," || ch == ".") {
                break
            }
        }
        return max(1, count)
    }
}

/// Computes character-offset cursor jumps across lines in multi-line text views from `UITextDocumentProxy` context.
public enum IOSTextCursorNavigation {
    public static func verticalMoveOffset(before: String, after: String, lines: Int) -> Int? {
        guard lines != 0 else { return nil }

        let currentCol: Int
        if let lastNL = before.lastIndex(of: "\n") {
            currentCol = before.distance(from: before.index(after: lastNL), to: before.endIndex)
        } else {
            currentCol = before.count
        }

        if lines < 0 {
            guard let lastNL = before.lastIndex(of: "\n") else {
                return before.isEmpty ? nil : -before.count
            }
            let prevPrefix = before[..<lastNL]
            let prevLineStart = prevPrefix.lastIndex(of: "\n").map { prevPrefix.index(after: $0) } ?? prevPrefix.startIndex
            let prevLineLen = prevPrefix.distance(from: prevLineStart, to: prevPrefix.endIndex)
            let targetCol = min(currentCol, prevLineLen)
            return -(currentCol + 1 + (prevLineLen - targetCol))
        } else {
            guard let nextNL = after.firstIndex(of: "\n") else {
                return after.isEmpty ? nil : after.count
            }
            let toEndOfCurLine = after.distance(from: after.startIndex, to: nextNL)
            let rem = after[after.index(after: nextNL)...]
            let nextLineEnd = rem.firstIndex(of: "\n") ?? rem.endIndex
            let nextLineLen = rem.distance(from: rem.startIndex, to: nextLineEnd)
            let targetCol = min(currentCol, nextLineLen)
            return toEndOfCurLine + 1 + targetCol
        }
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

/// Whole-keyboard long-press trackpad policy (opt-in).
/// When enabled, holding any non-system key enters trackpad cursor mode first; continuing to hold
/// still on a key with alternates promotes to its alternates popup, whereas moving the finger stays
/// in cursor mode and cancels the popup.
public enum IOSKeyboardTrackpadHold {
    /// Hold duration before trackpad cursor control activates when whole-keyboard hold is on.
    public static let trackpadDelay: TimeInterval = 0.35
    /// Spacebar hold duration when whole-keyboard hold is off.
    public static let defaultSpaceHoldDelay: TimeInterval = 0.42
    /// Default alternates long-press delay when whole-keyboard hold is off.
    public static let defaultAlternatesDelay: TimeInterval = 0.40
    /// Total hold duration before an idle trackpad touch promotes to the key's alternates popup.
    public static let alternatesPromotionDelay: TimeInterval = 0.78
    /// Movement threshold (in points) that cancels pending timers or commits to cursor movement.
    public static let moveCancelDistance: CGFloat = 12.0

    public static func supportsTrackpadHold(for kind: IOSKeyKind, wholeKeyboardEnabled: Bool) -> Bool {
        if kind == .space { return true }
        guard wholeKeyboardEnabled else { return false }
        switch kind {
        case .backspace, .globe, .planeSwitch:
            return false
        default:
            return true
        }
    }

    public static func trackpadHoldDelay(wholeKeyboardEnabled: Bool) -> TimeInterval {
        wholeKeyboardEnabled ? trackpadDelay : defaultSpaceHoldDelay
    }

    public static func alternatesDelay(wholeKeyboardEnabled: Bool) -> TimeInterval {
        wholeKeyboardEnabled ? alternatesPromotionDelay : defaultAlternatesDelay
    }

    public static func shouldCancelAlternatesOnMove(from start: CGPoint, to current: CGPoint) -> Bool {
        hypot(current.x - start.x, current.y - start.y) > moveCancelDistance
    }

    public static func shouldCancelAlternatesInTrackpad(
        from start: CGPoint,
        to current: CGPoint,
        dx: Int,
        dy: Int
    ) -> Bool {
        dx != 0 || dy != 0 || shouldCancelAlternatesOnMove(from: start, to: current)
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
