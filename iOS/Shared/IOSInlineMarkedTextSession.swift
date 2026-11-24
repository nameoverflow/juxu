import Foundation

@MainActor
public protocol IOSMarkedTextDocumentSink: AnyObject {
    var documentContextBeforeInput: String? { get }
    var documentContextAfterInput: String? { get }
    var selectedText: String? { get }
    func setMarkedText(_ markedText: String, selectedRange: NSRange)
    func unmarkText()
    func insertText(_ text: String)
    func adjustTextPosition(byCharacterOffset offset: Int)
}

public extension IOSMarkedTextDocumentSink {
    var documentContextAfterInput: String? { nil }
    var selectedText: String? { nil }
    func adjustTextPosition(byCharacterOffset offset: Int) {}
}

/// Coordinates inline `markedText` composition and committed text insertion on iOS `UITextDocumentProxy`.
///
/// Why we cannot use `insertText(_:)` or same-turn `unmarkText() + setMarkedText(_:)`:
/// 1. Custom `UITextInput` views such as `YYTextView` (Bilibili, Weibo, etc.) implement `insertText(_:)`
///    as `replaceRange(_selectedTextRange, withText:)` without replacing or clearing `_markedTextRange`.
///    The ONLY `UITextInput` operation that replaces `_markedTextRange` in `YYTextView` is
///    `setMarkedText(committed, selectedRange: (committed.utf16.count, 0))` followed by `unmarkText()`.
/// 2. In UIKit's `UIInputViewController`, `textDocumentProxy` coalesces calls within a single runloop turn
///    into `TIKeyboardOutput` (`insertionText` -> last `setMarkedText` -> `unmarkIfNecessary`). Calling
///    `unmarkText()` and a new `setMarkedText(nextPreedit)` in the SAME turn causes `unmarkIfNecessary`
///    to unmark `nextPreedit` in place! Therefore, streaming/partial commits flush
///    `setMarkedText(committed) + unmarkText()` in Turn 1, and apply `setMarkedText(nextPreedit)` on the
///    next main-runloop turn via `flushTurnBoundary(in:)`.
@MainActor
public struct IOSInlineMarkedTextSession {
    public var markedCode: String {
        deferredInlinePreedit ?? hostMarkedCode
    }
    /// Process-wide orphaned `markedText` state so keyboard dismiss -> `KeyboardViewController`
    /// deallocation + re-instantiation inside the extension process preserves pending cleanup.
    private static var sharedHasOrphanedMarked = false
    private static var sharedNeedsAppearanceSweep = false
    private static var sharedOrphanedExpectedBeforeTail = ""

    private var hostMarkedCode: String = ""
    private var turnUnmarkedCommit: String?
    public private(set) var deferredInlinePreedit: String?

    public var autoSpaceTracker = AutoSpaceContextTracker()
    public private(set) var compositionBaseContext: String?

    public var hasOrphanedMarkedInSession: Bool { Self.sharedHasOrphanedMarked }
    public var needsAppearanceSweep: Bool { Self.sharedNeedsAppearanceSweep }

    public init() {}

    public static func resetSharedOrphanedState() {
        sharedHasOrphanedMarked = false
        sharedNeedsAppearanceSweep = false
        sharedOrphanedExpectedBeforeTail = ""
    }

    public mutating func reset() {
        hostMarkedCode = ""
        turnUnmarkedCommit = nil
        deferredInlinePreedit = nil
        autoSpaceTracker.invalidate()
        compositionBaseContext = nil
    }

    public mutating func resetForAppearance() {
        hostMarkedCode = ""
        if turnUnmarkedCommit == nil {
            deferredInlinePreedit = nil
        }
        autoSpaceTracker.invalidate()
        compositionBaseContext = nil
    }

    /// Records that input focus left the composing field before `markedText` was committed.
    /// Because UIKit resigns the outgoing responder before notifying `UIInputViewController`,
    /// the outgoing field retains an orphaned `markedTextRange` until refocused and swept.
    @discardableResult
    public mutating func recordFocusLoss() -> Bool {
        let hadMarked = !markedCode.isEmpty
        if hadMarked {
            Self.sharedHasOrphanedMarked = true
            Self.sharedOrphanedExpectedBeforeTail = String((compositionBaseContext ?? autoSpaceTracker.currentShadowBefore).suffix(32))
        }
        reset()
        return hadMarked
    }

    /// Records `viewWillDisappear`: UIKit deallocates `KeyboardViewController` on dismiss and creates
    /// a new instance on reopen, so `sharedNeedsAppearanceSweep` persists at process scope.
    public mutating func recordKeyboardDisappear() {
        if !markedCode.isEmpty {
            Self.sharedHasOrphanedMarked = true
            Self.sharedOrphanedExpectedBeforeTail = String((compositionBaseContext ?? autoSpaceTracker.currentShadowBefore).suffix(32))
        }
        if Self.sharedHasOrphanedMarked {
            Self.sharedNeedsAppearanceSweep = true
        }
        reset()
    }

    /// Clears any orphaned `markedTextRange` left in a refocused field from a prior aborted composition,
    /// without disturbing active range selections (`selectedText`).
    @discardableResult
    public mutating func sweepOrphanedMarkedText(
        in sink: any IOSMarkedTextDocumentSink
    ) -> (needsAsyncFlush: Bool, expectedBeforeTail: String) {
        guard Self.sharedHasOrphanedMarked else { return (false, "") }
        guard hostMarkedCode.isEmpty && deferredInlinePreedit == nil && turnUnmarkedCommit == nil else {
            return (false, "")
        }
        let tail = Self.sharedOrphanedExpectedBeforeTail
        Self.sharedHasOrphanedMarked = false
        Self.sharedNeedsAppearanceSweep = false
        Self.sharedOrphanedExpectedBeforeTail = ""
        if (sink.selectedText ?? "").isEmpty {
            turnUnmarkedCommit = ""
            sink.setMarkedText("", selectedRange: NSRange(location: 0, length: 0))
            sink.unmarkText()
            return (true, tail)
        } else {
            sink.unmarkText()
            return (false, "")
        }
    }

    /// Restores the caret after `sweepOrphanedMarkedText` on iOS 26 TextKit 2 (`UITextField`/`UITextView`),
    /// which resets `selectedTextRange` to `{0, 0}` when `setMarkedText("", ...)` clears an orphaned range.
    public mutating func restoreCaretAfterOrphanedSweepIfNeeded(
        expectedBeforeTail: String,
        in sink: any IOSMarkedTextDocumentSink
    ) {
        guard !expectedBeforeTail.isEmpty,
              hostMarkedCode.isEmpty,
              deferredInlinePreedit == nil else { return }
        let before = sink.documentContextBeforeInput ?? ""
        guard before.isEmpty else { return }
        let after = sink.documentContextAfterInput ?? ""
        guard let range = after.range(of: expectedBeforeTail) else { return }
        let offset = after.distance(from: after.startIndex, to: range.upperBound)
        if offset > 0 {
            sink.adjustTextPosition(byCharacterOffset: offset)
            autoSpaceTracker.reconcileExternalContext(hostBefore: String(after[..<range.upperBound]))
        }
    }

    public mutating func reconcileExternalContext(hostBefore: String?) {
        guard hostMarkedCode.isEmpty && deferredInlinePreedit == nil && turnUnmarkedCommit == nil else { return }
        autoSpaceTracker.reconcileExternalContext(hostBefore: hostBefore)
    }

    public mutating func flushTurnBoundary(in sink: any IOSMarkedTextDocumentSink) {
        turnUnmarkedCommit = nil
        guard let pending = deferredInlinePreedit else { return }
        deferredInlinePreedit = nil
        guard !pending.isEmpty else { return }
        sink.setMarkedText(
            pending,
            selectedRange: NSRange(location: pending.utf16.count, length: 0)
        )
        hostMarkedCode = pending
    }

    public mutating func cancelMarkedCode(in sink: any IOSMarkedTextDocumentSink) {
        deferredInlinePreedit = nil
        if turnUnmarkedCommit == nil && !hostMarkedCode.isEmpty {
            sink.setMarkedText("", selectedRange: NSRange(location: 0, length: 0))
            sink.unmarkText()
            hostMarkedCode = ""
        }
        compositionBaseContext = nil
    }

    private mutating func prepareFormattedCommit(
        _ text: String,
        splitInternal: Bool,
        overrideBefore: String?,
        options: AutoSpaceOptions,
        in sink: any IOSMarkedTextDocumentSink
    ) -> String {
        autoSpaceTracker.prepareCommit(
            text,
            hostBefore: sink.documentContextBeforeInput,
            hostBeforeIsAuthoritative: false,
            overrideBefore: overrideBefore,
            splitInternal: splitInternal,
            options: options
        )
    }

    private mutating func takeSameTurnPrefix() -> String? {
        turnUnmarkedCommit
    }

    private mutating func commitReplacingMarkedIfNeeded(
        _ formatted: String,
        in sink: any IOSMarkedTextDocumentSink
    ) -> Bool {
        if let prev = takeSameTurnPrefix() {
            let combined = prev + formatted
            turnUnmarkedCommit = combined
            deferredInlinePreedit = nil
            sink.setMarkedText(
                combined,
                selectedRange: NSRange(location: combined.utf16.count, length: 0)
            )
            sink.unmarkText()
            hostMarkedCode = ""
            return true
        } else if !hostMarkedCode.isEmpty {
            turnUnmarkedCommit = formatted
            deferredInlinePreedit = nil
            sink.setMarkedText(
                formatted,
                selectedRange: NSRange(location: formatted.utf16.count, length: 0)
            )
            sink.unmarkText()
            hostMarkedCode = ""
            return true
        } else {
            sink.insertText(formatted)
            return false
        }
    }

    @discardableResult
    public mutating func insertTextWithAutoSpace(
        _ text: String,
        splitInternal: Bool,
        overrideBefore: String? = nil,
        options: AutoSpaceOptions,
        in sink: any IOSMarkedTextDocumentSink
    ) -> Bool {
        guard !text.isEmpty else { return false }
        let effectiveOverride = overrideBefore ?? (turnUnmarkedCommit != nil ? autoSpaceTracker.currentShadowBefore : nil)
        let formatted = prepareFormattedCommit(
            text,
            splitInternal: splitInternal,
            overrideBefore: effectiveOverride,
            options: options,
            in: sink
        )
        if let prev = takeSameTurnPrefix() {
            let combined = prev + formatted
            turnUnmarkedCommit = combined
            sink.setMarkedText(
                combined,
                selectedRange: NSRange(location: combined.utf16.count, length: 0)
            )
            sink.unmarkText()
            return true
        } else {
            sink.insertText(formatted)
            return false
        }
    }

    @discardableResult
    public mutating func insertPassthrough(
        _ text: String,
        in sink: any IOSMarkedTextDocumentSink
    ) -> Bool {
        guard !text.isEmpty else { return false }
        let sameTurnBase = turnUnmarkedCommit != nil ? autoSpaceTracker.currentShadowBefore : nil
        autoSpaceTracker.recordPassthrough(
            text,
            hostBefore: sink.documentContextBeforeInput,
            hostBeforeIsAuthoritative: false,
            overrideBefore: sameTurnBase
        )
        if let prev = takeSameTurnPrefix() {
            let combined = prev + text
            turnUnmarkedCommit = combined
            sink.setMarkedText(
                combined,
                selectedRange: NSRange(location: combined.utf16.count, length: 0)
            )
            sink.unmarkText()
            return true
        } else {
            sink.insertText(text)
            return false
        }
    }

    @discardableResult
    public mutating func commitPendingFirst(
        _ rawText: String,
        options: AutoSpaceOptions,
        in sink: any IOSMarkedTextDocumentSink
    ) -> Bool {
        guard !rawText.isEmpty else {
            cancelMarkedCode(in: sink)
            return false
        }
        let hadMarked = !markedCode.isEmpty
        let base = hadMarked ? compositionBaseContext : nil
        let formatted = prepareFormattedCommit(
            rawText,
            splitInternal: true,
            overrideBefore: base,
            options: options,
            in: sink
        )
        let usedTurnUnmark = commitReplacingMarkedIfNeeded(formatted, in: sink)
        compositionBaseContext = nil
        return usedTurnUnmark
    }

    @discardableResult
    public mutating func applyEngineState(
        commitText: String,
        inlinePreedit: String,
        options: AutoSpaceOptions,
        in sink: any IOSMarkedTextDocumentSink
    ) -> (didCommit: Bool, needsAsyncFlush: Bool) {
        var didCommit = false
        var needsAsyncFlush = false
        var committedShadow: String?

        if !commitText.isEmpty {
            let hadMarked = !markedCode.isEmpty
            let base = hadMarked ? compositionBaseContext : nil
            let formatted = prepareFormattedCommit(
                commitText,
                splitInternal: true,
                overrideBefore: base,
                options: options,
                in: sink
            )
            if commitReplacingMarkedIfNeeded(formatted, in: sink) {
                needsAsyncFlush = true
            }
            committedShadow = autoSpaceTracker.currentShadowBefore
            didCommit = true
        }

        if inlinePreedit.isEmpty {
            cancelMarkedCode(in: sink)
        } else if turnUnmarkedCommit != nil {
            compositionBaseContext = committedShadow ?? compositionBaseContext ?? autoSpaceTracker.currentShadowBefore
            deferredInlinePreedit = inlinePreedit
            needsAsyncFlush = true
        } else if inlinePreedit != hostMarkedCode {
            if hostMarkedCode.isEmpty {
                compositionBaseContext = committedShadow ?? autoSpaceTracker.effectiveBefore(
                    hostBefore: sink.documentContextBeforeInput,
                    hostBeforeIsAuthoritative: false
                )
            }
            sink.setMarkedText(
                inlinePreedit,
                selectedRange: NSRange(location: inlinePreedit.utf16.count, length: 0)
            )
            hostMarkedCode = inlinePreedit
        }

        return (didCommit, needsAsyncFlush)
    }
}
