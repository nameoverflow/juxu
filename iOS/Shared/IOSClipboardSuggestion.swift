import Foundation

/// A single native-style clipboard suggestion rendered centered in the idle candidate ribbon.
public struct IOSClipboardSuggestion: Equatable, Sendable {
    public let changeCount: Int
    /// Exact text inserted into `UITextDocumentProxy` on tap (raw clipboard text or extracted OTP digits).
    public let insertText: String
    /// Compact single-line display string for the candidate bar.
    public let displayText: String
    /// Small caption shown above the primary text (e.g. "来自剪贴板" or "剪贴板验证码").
    public let subtitle: String
    /// True when the suggestion represents a verification / one-time code.
    public let isOTP: Bool

    public init(
        changeCount: Int,
        insertText: String,
        displayText: String,
        subtitle: String,
        isOTP: Bool
    ) {
        self.changeCount = changeCount
        self.insertText = insertText
        self.displayText = displayText
        self.subtitle = subtitle
        self.isOTP = isOTP
    }
}

/// Parses clipboard strings into clean candidate-bar previews and extracts OTP codes from copied SMS/email messages.
public enum IOSClipboardParser {
    private static let maxPreviewScanChars = 240
    private static let maxPreviewDisplayChars = 80
    private static let maxOTPScanChars = 600

    private static let zhOTPKeywords: [String] = [
        "验证码", "校验码", "动态码", "动态密码", "确认码", "安全码",
        "认证码", "短信码", "登录码", "授权码", "取件码", "临时密码", "口令",
    ]

    private static let enOTPKeywords: [String] = [
        "verification code", "security code", "confirmation code", "confirm code",
        "login code", "auth code", "passcode", "one-time", "one time",
        "otp", "code is", "code:",
    ]

    private static let excludedLeadingTokens: [String] = [
        "尾号", "末四位", "后四位", "卡号", "订单", "单号", "满", "¥", "￥", "$",
    ]

    private static let excludedTrailingChars: Set<Character> = [
        "年", "月", "日", "元", "块", "分", "秒", "%", "％",
    ]

    public static func parse(
        raw: String,
        changeCount: Int,
        prefersOTP: Bool = false
    ) -> IOSClipboardSuggestion? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }

        // Never treat a single URL as an OTP message even if its path contains digits and "code".
        let isSingleURL = !trimmed.contains(where: \.isWhitespace)
            && (trimmed.lowercased().hasPrefix("http://") || trimmed.lowercased().hasPrefix("https://"))

        if !isSingleURL {
            // Case 1: Pure 4–8 digit numeric code
            if trimmed.count >= 4, trimmed.count <= 8, trimmed.allSatisfy(\.isASCII), trimmed.allSatisfy(\.isNumber) {
                return IOSClipboardSuggestion(
                    changeCount: changeCount,
                    insertText: trimmed,
                    displayText: trimmed,
                    subtitle: prefersOTP ? "剪贴板验证码" : "来自剪贴板",
                    isOTP: prefersOTP
                )
            }

            // Case 2: Copied SMS / email text containing an OTP keyword + 4–8 digit code
            if trimmed.count <= maxOTPScanChars, let otp = extractOTPCode(from: trimmed) {
                return IOSClipboardSuggestion(
                    changeCount: changeCount,
                    insertText: otp,
                    displayText: otp,
                    subtitle: "剪贴板验证码",
                    isOTP: true
                )
            }
        }

        let display = makeSingleLinePreview(from: raw)
        guard !display.isEmpty else { return nil }
        return IOSClipboardSuggestion(
            changeCount: changeCount,
            insertText: raw,
            displayText: display,
            subtitle: "来自剪贴板",
            isOTP: false
        )
    }

    public static func makeSingleLinePreview(from raw: String) -> String {
        let slice = String(raw.prefix(maxPreviewScanChars))
        var out = ""
        out.reserveCapacity(min(slice.count, maxPreviewDisplayChars + 2))
        var prevSpace = true

        for ch in slice {
            if ch.isWhitespace || ch.isNewline {
                if !prevSpace {
                    out.append(" ")
                    prevSpace = true
                }
            } else {
                out.append(ch)
                prevSpace = false
            }
            if out.count >= maxPreviewDisplayChars + 1 {
                break
            }
        }

        let cleaned = out.trimmingCharacters(in: .whitespaces)
        if cleaned.count > maxPreviewDisplayChars {
            return String(cleaned.prefix(maxPreviewDisplayChars)) + "…"
        }
        return cleaned
    }

    /// Extracts a 4–8 digit verification code from a copied SMS or email message when OTP context keywords exist.
    public static func extractOTPCode(from text: String) -> String? {
        let lower = text.lowercased()
        var keywordPositions: [(start: Int, end: Int)] = []

        let chars = Array(text)
        let lowerChars = Array(lower)

        for kw in zhOTPKeywords {
            appendMatches(of: Array(kw), in: chars, into: &keywordPositions)
        }
        for kw in enOTPKeywords {
            appendMatches(of: Array(kw), in: lowerChars, into: &keywordPositions)
        }
        guard !keywordPositions.isEmpty else { return nil }

        // Collect all standalone 4–8 digit candidates (plus 3+3 grouped codes like "904 281" or "482-916").
        struct Candidate {
            let digits: String
            let start: Int
            let end: Int // exclusive
        }

        var candidates: [Candidate] = []
        var i = 0
        while i < chars.count {
            if isASCIIDigit(chars[i]) {
                let start = i
                while i < chars.count && isASCIIDigit(chars[i]) {
                    i += 1
                }
                let end = i
                let len = end - start

                // Check for 3+3 grouped OTP like "904 281" or "482-916"
                if len == 3,
                   end + 3 < chars.count,
                   (chars[end] == " " || chars[end] == "-"),
                   isASCIIDigit(chars[end + 1]),
                   isASCIIDigit(chars[end + 2]),
                   isASCIIDigit(chars[end + 3]),
                   (end + 4 == chars.count || !isASCIIDigit(chars[end + 4])) {
                    let groupedEnd = end + 4
                    let digits = String(chars[start..<end]) + String(chars[(end + 1)..<groupedEnd])
                    if !isExcludedContext(chars: chars, start: start, end: groupedEnd) {
                        candidates.append(Candidate(digits: digits, start: start, end: groupedEnd))
                    }
                    i = groupedEnd
                    continue
                }

                if len >= 4 && len <= 8 {
                    let digits = String(chars[start..<end])
                    if !isExcludedContext(chars: chars, start: start, end: end) {
                        candidates.append(Candidate(digits: digits, start: start, end: end))
                    }
                }
            } else {
                i += 1
            }
        }

        guard !candidates.isEmpty else { return nil }

        var bestCode: String?
        var bestScore = Double.greatestFiniteMagnitude

        for cand in candidates {
            for kw in keywordPositions {
                let dist: Double
                let isAfter: Bool
                if cand.start >= kw.end {
                    dist = Double(cand.start - kw.end)
                    isAfter = true
                } else if kw.start >= cand.end {
                    dist = Double(kw.start - cand.end)
                    isAfter = false
                } else {
                    dist = 0
                    isAfter = true
                }

                // Require the code to appear reasonably close to the OTP keyword.
                guard dist <= 36 else { continue }

                // Slightly favor codes immediately following the keyword and standard 6-digit codes.
                let directionPenalty = isAfter ? 0.0 : 4.0
                let lengthBonus: Double = (cand.digits.count == 6) ? -1.5 : (cand.digits.count == 4 ? -0.5 : 0.0)
                let score = dist + directionPenalty + lengthBonus
                if score < bestScore {
                    bestScore = score
                    bestCode = cand.digits
                }
            }
        }

        return bestCode
    }

    private static func isASCIIDigit(_ ch: Character) -> Bool {
        guard let scalar = ch.unicodeScalars.first, ch.unicodeScalars.count == 1 else { return false }
        return scalar.value >= 48 && scalar.value <= 57
    }

    private static func appendMatches(
        of pattern: [Character],
        in haystack: [Character],
        into out: inout [(start: Int, end: Int)]
    ) {
        let pLen = pattern.count
        guard pLen > 0, haystack.count >= pLen else { return }
        var i = 0
        while i <= haystack.count - pLen {
            var matched = true
            for j in 0..<pLen {
                if haystack[i + j] != pattern[j] {
                    matched = false
                    break
                }
            }
            if matched {
                // For pure-ASCII short keywords like "otp", require word boundaries.
                let isShortASCII = pLen <= 4 && pattern.allSatisfy(\.isASCII)
                let leftOK = !isShortASCII || i == 0 || !haystack[i - 1].isLetter
                let rightOK = !isShortASCII || (i + pLen == haystack.count) || !haystack[i + pLen].isLetter
                if leftOK && rightOK {
                    out.append((start: i, end: i + pLen))
                }
                i += pLen
            } else {
                i += 1
            }
        }
    }

    private static func isExcludedContext(chars: [Character], start: Int, end: Int) -> Bool {
        if end < chars.count && excludedTrailingChars.contains(chars[end]) {
            return true
        }
        // Exclude digits embedded inside a URL token (e.g. "https://example.com/otp/123456")
        var tokStart = start
        while tokStart > 0 && !chars[tokStart - 1].isWhitespace && !chars[tokStart - 1].isNewline {
            tokStart -= 1
        }
        let tokenPrefix = String(chars[tokStart..<end]).lowercased()
        if tokenPrefix.contains("http://") || tokenPrefix.contains("https://") {
            return true
        }
        let prefixStart = max(0, start - 6)
        let prefixStr = String(chars[prefixStart..<start]).trimmingCharacters(in: .whitespaces)
        for token in excludedLeadingTokens {
            if prefixStr.hasSuffix(token) {
                return true
            }
        }
        return false
    }
}

/// Persistence abstraction for `IOSClipboardSuggestionStore` (backed by `UserDefaults` in the keyboard extension).
public protocol IOSClipboardKeyValueStore: AnyObject {
    func integer(forKey key: String) -> Int
    func double(forKey key: String) -> Double
    func string(forKey key: String) -> String?
    func object(forKey key: String) -> Any?
    func set(_ value: Int, forKey key: String)
    func set(_ value: Double, forKey key: String)
    func set(_ value: Any?, forKey key: String)
    func removeObject(forKey key: String)
}

extension UserDefaults: IOSClipboardKeyValueStore {}

/// Tracks `UIPasteboard.general.changeCount` so the keyboard reads the system pasteboard at most ONCE per copy,
/// caches the suggestion across text-field switches within its freshness window, and hides immediately once pasted or dismissed.
public final class IOSClipboardSuggestionStore {
    public static let defaultTTLSeconds: TimeInterval = 300.0

    private enum Keys {
        static let handledChangeCount = "priswubi.clip.handledChangeCount"
        static let seenChangeCount = "priswubi.clip.seenChangeCount"
        static let seenTimestamp = "priswubi.clip.seenTimestamp"
        static let cachedRawText = "priswubi.clip.cachedRawText"
    }

    private let store: IOSClipboardKeyValueStore
    public let ttlSeconds: TimeInterval

    public enum LookupResult: Equatable, Sendable {
        case none
        case cached(IOSClipboardSuggestion)
        case needsRead(changeCount: Int)
    }

    public init(
        store: IOSClipboardKeyValueStore = UserDefaults.standard,
        ttlSeconds: TimeInterval = IOSClipboardSuggestionStore.defaultTTLSeconds
    ) {
        self.store = store
        self.ttlSeconds = ttlSeconds
    }

    private func optionalInt(forKey key: String) -> Int? {
        store.object(forKey: key) != nil ? store.integer(forKey: key) : nil
    }

    public var seenChangeCount: Int? {
        optionalInt(forKey: Keys.seenChangeCount)
    }

    public var handledChangeCount: Int? {
        optionalInt(forKey: Keys.handledChangeCount)
    }

    /// Non-blocking cache check: returns `.cached` if this `changeCount` was already read and is fresh,
    /// `.none` if disabled/handled/empty/expired, or `.needsRead` when the caller should fetch
    /// `UIPasteboard.general.string` asynchronously off the main thread.
    public func lookup(
        hasFullAccess: Bool,
        enabled: Bool,
        hasStrings: Bool,
        changeCount: Int,
        prefersOTP: Bool,
        now: TimeInterval
    ) -> LookupResult {
        guard enabled, hasFullAccess else {
            return .none
        }

        if optionalInt(forKey: Keys.handledChangeCount) == changeCount {
            return .none
        }

        let seen = optionalInt(forKey: Keys.seenChangeCount)

        guard hasStrings else {
            if seen != changeCount {
                store.set(changeCount, forKey: Keys.seenChangeCount)
                store.set(now, forKey: Keys.seenTimestamp)
                store.removeObject(forKey: Keys.cachedRawText)
            }
            return .none
        }

        if seen != changeCount {
            return .needsRead(changeCount: changeCount)
        }

        let firstSeen = store.double(forKey: Keys.seenTimestamp)
        let age = now - firstSeen
        if age > ttlSeconds || age < -60.0 {
            return .none
        }

        guard let cached = store.string(forKey: Keys.cachedRawText), !cached.isEmpty else {
            return .none
        }
        guard let parsed = IOSClipboardParser.parse(raw: cached, changeCount: changeCount, prefersOTP: prefersOTP) else {
            return .none
        }
        return .cached(parsed)
    }

    /// Records the result of an asynchronous `UIPasteboard.general.string` read for `changeCount`.
    /// Passing `nil` (e.g. user tapped "不允许粘贴") still marks `changeCount` as seen so we never re-prompt.
    @discardableResult
    public func recordRead(
        changeCount: Int,
        rawText: String?,
        prefersOTP: Bool,
        now: TimeInterval
    ) -> IOSClipboardSuggestion? {
        if optionalInt(forKey: Keys.handledChangeCount) == changeCount {
            return nil
        }
        store.set(changeCount, forKey: Keys.seenChangeCount)
        store.set(now, forKey: Keys.seenTimestamp)
        if let rawText, !rawText.isEmpty {
            store.set(rawText, forKey: Keys.cachedRawText)
            return IOSClipboardParser.parse(raw: rawText, changeCount: changeCount, prefersOTP: prefersOTP)
        } else {
            store.removeObject(forKey: Keys.cachedRawText)
            return nil
        }
    }

    public func refresh(
        hasFullAccess: Bool,
        enabled: Bool,
        hasStrings: Bool,
        changeCount: Int,
        prefersOTP: Bool,
        now: TimeInterval,
        readString: () -> String?
    ) -> IOSClipboardSuggestion? {
        switch lookup(
            hasFullAccess: hasFullAccess,
            enabled: enabled,
            hasStrings: hasStrings,
            changeCount: changeCount,
            prefersOTP: prefersOTP,
            now: now
        ) {
        case .none:
            return nil
        case .cached(let suggestion):
            return suggestion
        case .needsRead(let cc):
            let fetched = readString()
            return recordRead(changeCount: cc, rawText: fetched, prefersOTP: prefersOTP, now: now)
        }
    }

    public func consume(changeCount: Int) {
        markHandled(changeCount)
    }

    public func dismiss(changeCount: Int) {
        markHandled(changeCount)
    }

    private func markHandled(_ changeCount: Int) {
        store.set(changeCount, forKey: Keys.handledChangeCount)
        store.set(changeCount, forKey: Keys.seenChangeCount)
        store.removeObject(forKey: Keys.cachedRawText)
    }
}

/// Controls session-level visibility of the clipboard pill:
/// only shows right after a fresh copy (`changeCount` change) or right after opening/switching to a text field
/// before any typing/editing occurs; hides for the remainder of that typing session once the user inputs text.
public struct IOSClipboardVisibilityGate: Equatable, Sendable {
    private var lastObservedChangeCount: Int? = nil
    public private(set) var isSuppressedByTyping: Bool = false

    public init() {}

    public mutating func resetForKeyboardAppearance() {
        isSuppressedByTyping = false
    }

    public mutating func markUserTyped() {
        isSuppressedByTyping = true
    }

    public mutating func shouldAllowDisplay(for changeCount: Int) -> Bool {
        if lastObservedChangeCount != changeCount {
            lastObservedChangeCount = changeCount
            isSuppressedByTyping = false
        }
        return !isSuppressedByTyping
    }
}
