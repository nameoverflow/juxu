import Foundation

// MARK: - Chinese Punctuation Pairing & Tracker

/// Chinese punctuation pairing: typing an opener also inserts its closer with the cursor in between.
public typealias IOSPunctuationPairing = PunctuationPairing

public enum PunctuationPairing {
    public enum Action: Equatable, Sendable {
        case plain
        case insertPair(close: String)
        case skipClose
    }

    private static let closers: [String: String] = [
        "“": "”",
        "‘": "’",
        "（": "）",
        "《": "》",
        "〈": "〉",
        "【": "】",
        "〔": "〕",
        "「": "」",
        "『": "』",
    ]
    private static let closerSet = Set(closers.values)
    /// Auto-closing is only safe when the cursor is not directly before existing text.
    private static let safeFollowers = Set("，。！？；：、）》〉】〕」』”’,.!?;:)]}")

    public static func closer(for open: String) -> String? {
        closers[open]
    }

    public static func action(for symbol: String, after: String?, openClosers: [String]) -> Action {
        if let close = closers[symbol] {
            guard let next = after?.first else { return .insertPair(close: close) }
            return (next.isWhitespace || safeFollowers.contains(next)) ? .insertPair(close: close) : .plain
        }
        if closerSet.contains(symbol), openClosers.last == symbol, after?.first.map(String.init) == symbol {
            return .skipClose
        }
        return .plain
    }

    /// Backspace right after an untouched auto-inserted pair removes both halves.
    public static func shouldDeletePair(before: String?, after: String?, openClosers: [String]) -> Bool {
        guard let close = openClosers.last,
              let open = before?.last.map(String.init),
              closers[open] == close else { return false }
        return after?.first.map(String.init) == close
    }

    /// Forget tracked closers once the text right of the cursor no longer starts with them.
    public static func pruned(_ openClosers: [String], after: String?) -> [String] {
        guard let last = openClosers.last, after?.first.map(String.init) != last else { return openClosers }
        return []
    }

    /// True while the text right of the cursor is still the tracked closers (innermost first) followed by
    /// what was there when the first pair opened. Compared over the visible window, which the host may trim.
    public static func cursorStaysInPairs(_ openClosers: [String], tail: String, after: String?) -> Bool {
        let expected = openClosers.reversed().joined() + tail
        let visible = after ?? ""
        let length = min(expected.count, visible.count)
        return expected.prefix(length) == visible.prefix(length) && visible.count >= openClosers.count
    }
}

/// Shared state machine tracking auto-inserted closer stack (`openClosers`) and surrounding right-side
/// context (`openPairTail`) across both macOS `JuxuInputController` and iOS `KeyboardViewController`.
public struct PunctuationPairingTracker: Equatable, Sendable {
    public private(set) var openClosers: [String] = []
    public private(set) var openPairTail: String = ""

    public init() {}

    public mutating func reset() {
        openClosers.removeAll()
        openPairTail = ""
    }

    public mutating func pruneOnCursorChange(after: String?) {
        openClosers = PunctuationPairing.pruned(openClosers, after: after)
        if !PunctuationPairing.cursorStaysInPairs(openClosers, tail: openPairTail, after: after) {
            openClosers.removeAll()
        }
        if openClosers.isEmpty {
            openPairTail = ""
        }
    }

    public mutating func handleSymbol(_ symbol: String, after: String?) -> PunctuationPairing.Action {
        pruneOnCursorChange(after: after)
        let act = PunctuationPairing.action(for: symbol, after: after, openClosers: openClosers)
        switch act {
        case .insertPair(let close):
            if openClosers.isEmpty {
                openPairTail = after ?? ""
            }
            openClosers.append(close)
        case .skipClose:
            if !openClosers.isEmpty {
                openClosers.removeLast()
            }
            if openClosers.isEmpty {
                openPairTail = ""
            }
        case .plain:
            break
        }
        return act
    }

    public mutating func consumeBackspacePairIfNeeded(before: String?, after: String?) -> Bool {
        guard !openClosers.isEmpty else { return false }
        pruneOnCursorChange(after: after)
        guard PunctuationPairing.shouldDeletePair(before: before, after: after, openClosers: openClosers) else {
            return false
        }
        openClosers.removeLast()
        if openClosers.isEmpty {
            openPairTail = ""
        }
        return true
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

    public var currentShadowBefore: String { shadowBefore }

    public mutating func prepareCommit(
        _ text: String,
        hostBefore: String?,
        hostBeforeIsAuthoritative: Bool,
        overrideBefore: String? = nil,
        splitInternal: Bool,
        options: AutoSpaceOptions
    ) -> String {
        guard !text.isEmpty else { return text }
        let before: String
        if let overrideBefore {
            before = overrideBefore
        } else {
            before = effectiveBefore(
                hostBefore: hostBefore,
                hostBeforeIsAuthoritative: hostBeforeIsAuthoritative
            )
        }
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
        hostBeforeIsAuthoritative: Bool,
        overrideBefore: String? = nil
    ) {
        guard !text.isEmpty else { return }
        let before: String
        if let overrideBefore {
            before = overrideBefore
        } else {
            before = effectiveBefore(
                hostBefore: hostBefore,
                hostBeforeIsAuthoritative: hostBeforeIsAuthoritative
            )
        }
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
        if (!optimisticInserted.isEmpty || optimisticDeleteCount > 0) && current == prevHost {
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
