import Foundation

/// Platform-independent modifier bitmask for `ShiftTapDetector`.
public struct ShiftTapModifiers: OptionSet, Sendable {
    public let rawValue: UInt

    public init(rawValue: UInt) {
        self.rawValue = rawValue
    }

    public static let shift = ShiftTapModifiers(rawValue: 1 << 0)
    public static let control = ShiftTapModifiers(rawValue: 1 << 1)
    public static let option = ShiftTapModifiers(rawValue: 1 << 2)
    public static let command = ShiftTapModifiers(rawValue: 1 << 3)
    public static let function = ShiftTapModifiers(rawValue: 1 << 4)
    public static let capsLock = ShiftTapModifiers(rawValue: 1 << 5)
}

/// Detects a bare Shift tap (press and release of a single Shift key with no other modifier,
/// key press, or mouse interaction in between) for seamless Chinese/English toggling on macOS.
public struct ShiftTapDetector: Sendable {
    public static let leftShiftKeyCode: UInt16 = 0x38
    public static let rightShiftKeyCode: UInt16 = 0x3C
    public static let maxTapDuration: TimeInterval = 0.35

    private var activeShiftKeyCode: UInt16? = nil
    private var shiftDownTimestamp: TimeInterval = 0
    private var lastModifiers: ShiftTapModifiers = []

    public init() {}

    public mutating func reset() {
        activeShiftKeyCode = nil
        shiftDownTimestamp = 0
        lastModifiers = []
    }

    public mutating func cancelPendingTap() {
        activeShiftKeyCode = nil
        shiftDownTimestamp = 0
    }

    public mutating func handleKeyDown() {
        cancelPendingTap()
    }

    /// Processes a modifier-flags-changed event and returns `true` iff it completes a bare Shift tap.
    ///
    /// - Parameters:
    ///   - keyCode: Hardware key code (`0x38` Left Shift, `0x3C` Right Shift).
    ///   - modifiers: Currently active modifier flags.
    ///   - timestamp: Event timestamp in seconds (monotonic uptime clock).
    ///   - hadInterveningHIDEvent: Closure taking the Shift-down timestamp and returning `true` if any
    ///     non-modifier keyDown or mouse/scroll event occurred after Shift was pressed (catches
    ///     shortcuts intercepted before IMK such as `Shift+Tab`, `Shift+Click`, or global hotkeys).
    public mutating func handleFlagsChanged(
        keyCode: UInt16,
        modifiers: ShiftTapModifiers,
        timestamp: TimeInterval,
        hadInterveningHIDEvent: (TimeInterval) -> Bool = { _ in false }
    ) -> Bool {
        let previous = lastModifiers
        lastModifiers = modifiers

        let isShiftKey = (keyCode == Self.leftShiftKeyCode || keyCode == Self.rightShiftKeyCode)

        if modifiers == .shift {
            // Only arm when transitioning from no modifiers into a bare Shift press by a Shift key.
            // Releasing another modifier (e.g. releasing Cmd before Shift in Cmd+Shift+S) leaves
            // `modifiers == .shift` with a non-Shift keyCode and non-empty `previous`, so it must not arm.
            if isShiftKey && previous.isEmpty && activeShiftKeyCode == nil {
                activeShiftKeyCode = keyCode
                shiftDownTimestamp = timestamp
            } else {
                cancelPendingTap()
            }
            return false
        }

        if modifiers.isEmpty, let armedKeyCode = activeShiftKeyCode {
            let downTs = shiftDownTimestamp
            cancelPendingTap()
            guard isShiftKey && keyCode == armedKeyCode else { return false }
            let elapsed = timestamp - downTs
            guard elapsed >= 0 && elapsed < Self.maxTapDuration else { return false }
            guard !hadInterveningHIDEvent(downTs) else { return false }
            return true
        }

        cancelPendingTap()
        return false
    }
}
