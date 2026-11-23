#if os(iOS)
import CoreHaptics
import UIKit

/// Low-latency Taptic feedback controller combining `CoreHaptics` crisp high-sharpness transients
/// (`playsHapticsOnly = true`, zero audio-session ducking) with a pre-warmed `.rigid` fallback ring.
@MainActor
final class IOSKeyboardFeedbackController {
    private var engine: CHHapticEngine?
    private var keyTapPlayer: CHHapticPatternPlayer?
    private var softTickPlayer: CHHapticPatternPlayer?
    private var engineRunning = false

    private var rigidRing: [UIImpactFeedbackGenerator] = []
    private var ringIndex = 0
    private let selectionGenerator = UISelectionFeedbackGenerator()

    init(attachingTo view: UIView? = nil) {
        for _ in 0..<3 {
            if #available(iOS 17.5, *), let view {
                rigidRing.append(UIImpactFeedbackGenerator(style: .rigid, view: view))
            } else {
                rigidRing.append(UIImpactFeedbackGenerator(style: .rigid))
            }
        }
        setupCoreHapticsIfSupported()
    }

    private func setupCoreHapticsIfSupported() {
        guard CHHapticEngine.capabilitiesForHardware().supportsHaptics else { return }
        do {
            let eng = try CHHapticEngine()
            eng.playsHapticsOnly = true
            // Allow CoreHaptics to power-gate the Taptic hardware channel after ~2s idle while the keyboard
            // stays visible on screen (eliminating static idle battery drain), auto-waking on `player.start`.
            eng.isAutoShutdownEnabled = true
            eng.stoppedHandler = { [weak self] reason in
                Task { @MainActor in
                    guard let self else { return }
                    if reason != .idleTimeout {
                        self.engineRunning = false
                    }
                }
            }
            eng.resetHandler = { [weak self] in
                Task { @MainActor in
                    self?.rebuildPlayersAndStart()
                }
            }
            self.engine = eng
            rebuildPlayersAndStart()
        } catch {
            self.engine = nil
            self.engineRunning = false
        }
    }

    private func rebuildPlayersAndStart() {
        guard let eng = engine else { return }
        do {
            let keyEvent = CHHapticEvent(
                eventType: .hapticTransient,
                parameters: [
                    CHHapticEventParameter(parameterID: .hapticIntensity, value: 0.45),
                    CHHapticEventParameter(parameterID: .hapticSharpness, value: 0.88),
                ],
                relativeTime: 0
            )
            let softEvent = CHHapticEvent(
                eventType: .hapticTransient,
                parameters: [
                    CHHapticEventParameter(parameterID: .hapticIntensity, value: 0.31),
                    CHHapticEventParameter(parameterID: .hapticSharpness, value: 0.80),
                ],
                relativeTime: 0
            )
            keyTapPlayer = try eng.makePlayer(with: CHHapticPattern(events: [keyEvent], parameters: []))
            softTickPlayer = try eng.makePlayer(with: CHHapticPattern(events: [softEvent], parameters: []))
            try eng.start()
            engineRunning = true
        } catch {
            engineRunning = false
        }
    }

    func prepare() {
        for gen in rigidRing {
            gen.prepare()
        }
        selectionGenerator.prepare()
        if let eng = engine, !engineRunning {
            do {
                try eng.start()
                engineRunning = true
            } catch {
                engineRunning = false
            }
        }
    }

    func suspend() {
        engine?.stop(completionHandler: nil)
        engineRunning = false
    }

    func fireKeyDown() {
        UIDevice.current.playInputClick()
        if let player = keyTapPlayer, let eng = engine {
            if !engineRunning {
                try? eng.start()
                engineRunning = true
            }
            do {
                try player.start(atTime: CHHapticTimeImmediate)
                return
            } catch {
                if (try? eng.start()) != nil {
                    engineRunning = true
                    if (try? player.start(atTime: CHHapticTimeImmediate)) != nil {
                        return
                    }
                }
                engineRunning = false
            }
        }
        let gen = rigidRing[ringIndex % rigidRing.count]
        ringIndex &+= 1
        gen.impactOccurred(intensity: 0.42)
        gen.prepare()
    }

    func fireSoftStep(playClick: Bool) {
        if playClick {
            UIDevice.current.playInputClick()
        }
        if let player = softTickPlayer, let eng = engine {
            if !engineRunning {
                try? eng.start()
                engineRunning = true
            }
            do {
                try player.start(atTime: CHHapticTimeImmediate)
                return
            } catch {
                if (try? eng.start()) != nil {
                    engineRunning = true
                    if (try? player.start(atTime: CHHapticTimeImmediate)) != nil {
                        return
                    }
                }
                engineRunning = false
            }
        }
        selectionGenerator.selectionChanged()
        selectionGenerator.prepare()
    }
}

/// Hardware-composited continuous-squircle (`CALayer`) keycap node:
/// zero `draw(_:)` backing bitmap, explicit `shadowPath` (zero offscreen pass), cached SF Symbols.
@MainActor
private final class IOSKeycapNodeView: UIView {
    private let primaryLabel = UILabel()
    private let radicalLabel = UILabel()
    private let symbolView = UIImageView()
    private let subscriptLabel = UILabel()

    private(set) var currentSpec: IOSKeySpec?
    private var isDarkTheme = false
    private var isPressedState = false
    private var isGlyphCovered = false
    private var hasAppliedPressVisuals = false

    private static var symbolCache: [String: UIImage] = [:]

    private let swipeHintLabel = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        layer.cornerRadius = IOSKeyboardLayoutEngine.keyCornerRadius
        layer.cornerCurve = .continuous

        primaryLabel.textAlignment = .center
        primaryLabel.isUserInteractionEnabled = false

        radicalLabel.textAlignment = .center
        radicalLabel.numberOfLines = 2
        radicalLabel.lineBreakMode = .byClipping
        radicalLabel.isUserInteractionEnabled = false
        radicalLabel.isHidden = true

        symbolView.contentMode = .center
        symbolView.isUserInteractionEnabled = false

        subscriptLabel.font = .systemFont(ofSize: 12.5, weight: .regular)
        subscriptLabel.textAlignment = .right
        subscriptLabel.isUserInteractionEnabled = false

        swipeHintLabel.font = .systemFont(ofSize: 9.5, weight: .regular)
        swipeHintLabel.textAlignment = .center
        swipeHintLabel.isUserInteractionEnabled = false
        swipeHintLabel.isHidden = true

        addSubview(primaryLabel)
        addSubview(radicalLabel)
        addSubview(symbolView)
        addSubview(subscriptLabel)
        addSubview(swipeHintLabel)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func configure(
        spec: IOSKeySpec,
        dark: Bool,
        pressed: Bool,
        glyphCovered: Bool
    ) {
        let frameChanged = (frame != spec.frame)
        if frameChanged {
            UIView.performWithoutAnimation {
                self.frame = spec.frame
                layoutSublayersOfKeycap(spec: spec)
            }
        }

        let specOrThemeChanged = (currentSpec != spec) || (isDarkTheme != dark)
        currentSpec = spec
        isDarkTheme = dark

        if specOrThemeChanged {
            UIView.performWithoutAnimation {
                updateContent(spec: spec, dark: dark)
                layoutSublayersOfKeycap(spec: spec)
            }
        }

        if specOrThemeChanged || !hasAppliedPressVisuals || pressed != isPressedState || glyphCovered != isGlyphCovered {
            applyPressVisuals(pressed: pressed, glyphCovered: glyphCovered, spec: spec, dark: dark, animatedRelease: !pressed && isPressedState)
            hasAppliedPressVisuals = true
            isPressedState = pressed
            isGlyphCovered = glyphCovered
        }
    }

    func updatePressState(pressed: Bool, glyphCovered: Bool, dark: Bool) {
        guard hasAppliedPressVisuals, let spec = currentSpec else { return }
        guard pressed != isPressedState || glyphCovered != isGlyphCovered || dark != isDarkTheme else { return }
        isDarkTheme = dark
        applyPressVisuals(pressed: pressed, glyphCovered: glyphCovered, spec: spec, dark: dark, animatedRelease: !pressed && isPressedState)
        isPressedState = pressed
        isGlyphCovered = glyphCovered
    }

    private func layoutSublayersOfKeycap(spec: IOSKeySpec) {
        let b = bounds
        swipeHintLabel.frame = CGRect(x: 0, y: 1.5, width: b.width, height: 11.0)
        if let rad = spec.keycapRadicals, !rad.isEmpty {
            let letterH = max(13.0, round(b.height * 0.34))
            primaryLabel.frame = CGRect(x: 0, y: 2.0, width: b.width, height: letterH)
            radicalLabel.frame = CGRect(
                x: 1.0,
                y: 2.0 + letterH - 0.5,
                width: max(0, b.width - 2.0),
                height: max(0, b.height - letterH - 3.5)
            )
            symbolView.frame = b
            subscriptLabel.frame = CGRect(x: b.width - 30.0, y: b.height - 20.0, width: 24.0, height: 15.0)
            return
        }
        let yNudge: CGFloat
        switch spec.kind {
        case .letter:
            let first = spec.primaryLabel.first
            let isNonASCII = !(first?.isASCII ?? true)
            let isUpper = first?.isUppercase ?? false
            let hintShift: CGFloat = spec.swipeUpSymbol == nil ? 0.0 : 2.0
            yNudge = (isNonASCII ? -0.5 : (isUpper ? -0.3 : -2.2)) + hintShift
        case .symbol, .planeSwitch:
            yNudge = -0.8
        case .modeToggle:
            yNudge = -0.5
        default:
            yNudge = 0.0
        }
        primaryLabel.frame = b.offsetBy(dx: 0, dy: yNudge)
        radicalLabel.frame = .zero
        symbolView.frame = b
        subscriptLabel.frame = CGRect(x: b.width - 30.0, y: b.height - 20.0, width: 24.0, height: 15.0)
    }

    private func updateContent(spec: IOSKeySpec, dark: Bool) {
        let fgColor: UIColor = spec.isBlueAccent ? .white : (dark ? .white : .black)
        let subColor: UIColor = dark
            ? UIColor(white: 1.0, alpha: 0.30)
            : UIColor(white: 0.0, alpha: 0.20)
        let modeSecondaryColor: UIColor = dark
            ? UIColor(white: 1.0, alpha: 0.45)
            : UIColor(white: 0.0, alpha: 0.38)

        if let symName = spec.systemSymbol {
            primaryLabel.isHidden = true
            radicalLabel.isHidden = true
            symbolView.isHidden = false
            let cacheKey = symName
            let template: UIImage?
            if let cached = Self.symbolCache[cacheKey] {
                template = cached
            } else {
                let cfg = UIImage.SymbolConfiguration(pointSize: 19.5, weight: .regular)
                let img = UIImage(systemName: symName, withConfiguration: cfg)?.withRenderingMode(.alwaysTemplate)
                if let img { Self.symbolCache[cacheKey] = img }
                template = img
            }
            symbolView.image = template
            symbolView.tintColor = fgColor
        } else if !spec.primaryLabel.isEmpty {
            symbolView.isHidden = true
            primaryLabel.isHidden = false
            if let secondary = spec.secondaryInlineLabel, !secondary.isEmpty {
                let attr = NSMutableAttributedString(
                    string: spec.primaryLabel,
                    attributes: [
                        .font: UIFont.systemFont(ofSize: 16.5, weight: .medium),
                        .foregroundColor: fgColor,
                    ]
                )
                attr.append(
                    NSAttributedString(
                        string: secondary,
                        attributes: [
                            .font: UIFont.systemFont(ofSize: 12.0, weight: .regular),
                            .foregroundColor: modeSecondaryColor,
                        ]
                    )
                )
                primaryLabel.attributedText = attr
            } else if let rad = spec.keycapRadicals, !rad.isEmpty {
                primaryLabel.attributedText = nil
                primaryLabel.font = .systemFont(ofSize: 12.5, weight: .semibold)
                primaryLabel.textColor = fgColor
                primaryLabel.text = spec.primaryLabel.uppercased()

                let radColor: UIColor = dark
                    ? UIColor(white: 1.0, alpha: 0.74)
                    : UIColor(white: 0.0, alpha: 0.64)
                let para = NSMutableParagraphStyle()
                para.alignment = .center
                para.lineHeightMultiple = 1.02
                para.lineSpacing = 0.5
                radicalLabel.attributedText = NSAttributedString(
                    string: rad,
                    attributes: [
                        .font: UIFont.systemFont(ofSize: 9.0, weight: .regular),
                        .foregroundColor: radColor,
                        .paragraphStyle: para,
                    ]
                )
                radicalLabel.isHidden = false
            } else {
                radicalLabel.isHidden = true
                primaryLabel.attributedText = nil
                let fontSize: CGFloat
                let weight: UIFont.Weight
                switch spec.kind {
                case .letter:
                    let first = spec.primaryLabel.first
                    let isNonASCII = !(first?.isASCII ?? true)
                    let isUpper = first?.isUppercase ?? false
                    fontSize = isNonASCII ? 20.5 : (isUpper ? 22.5 : 24.5)
                    weight = .regular
                case .t9Key(let digit):
                    fontSize = (digit == UInt8(ascii: "1")) ? 14.5 : 16.5
                    weight = (digit == UInt8(ascii: "1")) ? .regular : .medium
                case .t9Clear:
                    fontSize = 15.0
                    weight = .regular
                case .symbol(let s):
                    fontSize = s.count > 1 ? 16.5 : 21.5
                    weight = .regular
                case .planeSwitch:
                    fontSize = 18.0
                    weight = .regular
                case .returnKey:
                    fontSize = 16.0
                    weight = spec.isBlueAccent ? .semibold : .regular
                default:
                    fontSize = 17.5
                    weight = .regular
                }
                primaryLabel.font = .systemFont(ofSize: fontSize, weight: weight)
                primaryLabel.textColor = fgColor
                primaryLabel.text = spec.primaryLabel
            }
        } else {
            primaryLabel.isHidden = true
            radicalLabel.isHidden = true
            symbolView.isHidden = true
        }

        if let hint = spec.swipeUpSymbol, spec.keycapRadicals?.isEmpty ?? true {
            swipeHintLabel.isHidden = false
            swipeHintLabel.text = hint
            swipeHintLabel.textColor = dark
                ? UIColor(white: 1.0, alpha: 0.48)
                : UIColor(white: 0.0, alpha: 0.42)
        } else {
            swipeHintLabel.isHidden = true
        }

        if let sub = spec.cornerSubscript, !sub.isEmpty {
            subscriptLabel.isHidden = false
            subscriptLabel.text = sub
            subscriptLabel.textColor = subColor
        } else {
            subscriptLabel.isHidden = true
        }
    }

    func applyPressVisuals(
        pressed: Bool,
        glyphCovered: Bool,
        spec: IOSKeySpec,
        dark: Bool,
        animatedRelease: Bool
    ) {
        let normalBg: UIColor = spec.isBlueAccent
            ? .systemBlue
            : (dark ? UIColor(white: 1.0, alpha: 0.175) : .white)
        let pressedBg: UIColor = spec.isBlueAccent
            ? UIColor.systemBlue.withAlphaComponent(0.80)
            : (dark
                ? UIColor(white: 1.0, alpha: 0.30)
                : UIColor(red: 214.0 / 255.0, green: 216.0 / 255.0, blue: 222.0 / 255.0, alpha: 1.0))

        let targetBg = pressed ? pressedBg : normalBg
        let targetTransform: CGAffineTransform = pressed
            ? CGAffineTransform(translationX: 0, y: 0.45)
            : .identity
        let contentAlpha: CGFloat = glyphCovered ? 0.0 : 1.0

        let applyBlock = {
            self.backgroundColor = targetBg
            self.transform = targetTransform
            self.primaryLabel.alpha = contentAlpha
            if !self.radicalLabel.isHidden {
                self.radicalLabel.alpha = contentAlpha
            }
            if !self.swipeHintLabel.isHidden {
                self.swipeHintLabel.alpha = contentAlpha
            }
        }

        if animatedRelease {
            UIView.animate(
                withDuration: 0.065,
                delay: 0,
                options: [.curveEaseOut, .allowUserInteraction, .beginFromCurrentState],
                animations: applyBlock
            )
        } else {
            UIView.performWithoutAnimation(applyBlock)
        }
    }
}

/// 120Hz hardware-composited iOS 26 4-row keyboard canvas with true per-finger rollover tracking
/// and sticky press-lock hysteresis (eliminating lift-off flick drops & cross-key misfires).
final class IOSKeyCanvasView: UIInputView, UIInputViewAudioFeedback {
    var enableInputClicksWhenVisible: Bool { true }

    var onKeyTouchDown: ((IOSKeySpec) -> Void)?
    var onKeySlidePreview: ((IOSKeySpec?) -> Void)?
    var onKeyTouchUp: ((IOSKeySpec, CGPoint) -> Void)?
    var onKeyTouchCancel: (() -> Void)?
    var onTrackpadStateChanged: ((Bool) -> Void)?
    var onRestorePlane: (() -> Void)?
    var onTrackpadMove: ((Int, Int) -> Void)?
    var onKeySwipeUp: ((String) -> Void)?
    var onBackspaceSwipeArmed: ((IOSKeySpec, Bool) -> Void)?
    var onBackspaceSwipeUp: (() -> Void)?
    var onBackspaceSwipeDownArmed: ((IOSKeySpec, Bool) -> Void)?
    var onBackspaceSwipeDown: (() -> Void)?
    /// Set on backspace touch-down: whether a cleared line can be restored at the cursor.
    var canUndoLineClear: Bool = false
    var onAlternatesBegin: ((IOSKeySpec) -> Void)?
    var onAlternatesMove: ((CGPoint) -> Void)?
    var onAlternatesEnd: ((Bool) -> Void)?
    var backspaceSwipeClearEnabled: Bool = false
    var longPressAlternatesEnabled: Bool = false

    private struct TrackedTouch {
        weak var touch: UITouch?
        let beganTimestamp: TimeInterval
        let initialPoint: CGPoint
        var currentPoint: CGPoint
        var prevPoint: CGPoint
        var prevTimestamp: TimeInterval
        var maxMovedDistance: CGFloat = 0
        var maxAbsDeltaY: CGFloat = 0
        var latestStepSpeed: CGFloat = 0
        let initialKey: IOSKeySpec
        var currentKey: IOSKeySpec
        let isPlaneSlide: Bool
        var didSlideToSymbol: Bool = false
        let startedOnSpace: Bool
        var committedByRollover: Bool = false
        var swipeUpActive: Bool = false
        var swipeDownActive: Bool = false
        var alternatesActive: Bool = false
    }

    private(set) var snapshot: IOSKeyboardLayoutEngine.LayoutSnapshot?
    var validNextMask: UInt32 = 0x03FF_FFFF
    var nextKeyPriors: [UInt8] = Array(repeating: 100, count: 26)
    var touchOffsets: [Float] = Array(repeating: 0, count: 52)
    var t9ValidNextMask: UInt8 = 0xFF
    var t9NextKeyPriors: [UInt8] = Array(repeating: 100, count: 8)
    var t9TouchOffsets: [Float] = Array(repeating: 0, count: 16)
    /// ID of Row-0 key whose glyph is seamlessly covered by the docked callout silhouette.
    var coveredCalloutKeyId: String? {
        didSet {
            if oldValue != coveredCalloutKeyId {
                syncKeycapPressStates()
            }
        }
    }

    override var isHidden: Bool {
        didSet {
            if isHidden {
                resetAllTouches()
            }
        }
    }

    private var keycapNodes: [IOSKeycapNodeView] = []
    private var trackedTouches: [ObjectIdentifier: TrackedTouch] = [:]
    private var spaceHoldTimer: Timer?
    private var spaceHoldTouchId: ObjectIdentifier?
    private var longPressTimer: Timer?
    private var longPressTouchId: ObjectIdentifier?
    private var trackpad = SpaceTrackpadAccumulator()
    private var isDark: Bool = false

    init() {
        super.init(frame: .zero, inputViewStyle: .keyboard)
        isMultipleTouchEnabled = true
        // A fully transparent gutter can be skipped during cross-process hit testing,
        // before a touch reaches this canvas. Give the whole key area a faint backing.
        backgroundColor = UIColor.systemBackground.withAlphaComponent(0.02)
        clipsToBounds = false
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    /// Capture high Row-0 (`q..p`) strikes passed through from unoccupied `IOSCandidateRibbonView` regions
    /// (`y = -24..0`) as well as low Row-3 (`Space`/`123`/`Return`) strikes at the bottom dock edge.
    override func point(inside point: CGPoint, with event: UIEvent?) -> Bool {
        CGRect(
            x: bounds.minX - 8.0,
            y: bounds.minY - 24.0,
            width: bounds.width + 16.0,
            height: bounds.height + 34.0
        ).contains(point)
    }

    override func willMove(toWindow newWindow: UIWindow?) {
        super.willMove(toWindow: newWindow)
        if newWindow == nil {
            resetAllTouches()
        }
    }

    func resetAllTouches() {
        cancelLongPressTimer()
        if trackedTouches.values.contains(where: { $0.alternatesActive && !$0.committedByRollover }) {
            onAlternatesEnd?(false)
        }
        spaceHoldTimer?.invalidate()
        spaceHoldTimer = nil
        spaceHoldTouchId = nil
        if trackpad.isActive {
            trackpad.end()
            onTrackpadStateChanged?(false)
        }
        trackedTouches.removeAll()
        coveredCalloutKeyId = nil
        syncKeycapPressStates()
    }

    /// Commits any pending touch-down key before an external control (e.g. candidate ribbon tap) runs,
    /// guaranteeing strict causal ordering across `keyCanvas` and `ribbonView`.
    func flushPendingTouches() {
        purgeEndedOrOrphanTouches()
        let pendingIds = trackedTouches
            .filter {
                let s = $0.value
                return !s.committedByRollover && !s.isPlaneSlide && !(s.startedOnSpace && trackpad.isActive)
            }
            .sorted(by: { $0.value.beganTimestamp < $1.value.beganTimestamp })
            .map(\.key)

        for oldId in pendingIds {
            guard var oldState = trackedTouches[oldId] else { continue }
            if oldState.startedOnSpace && spaceHoldTouchId == oldId {
                spaceHoldTimer?.invalidate()
                spaceHoldTimer = nil
                spaceHoldTouchId = nil
            }
            oldState.committedByRollover = true
            trackedTouches[oldId] = oldState
            commitRolledOver(oldState)
        }
        syncKeycapPressStates()
    }

    /// A later touch landed first: finish this one as its swipe symbol or normal key.
    /// An armed backspace swipe is deliberately not executed here, so rollover can never clear text.
    private func commitRolledOver(_ state: TrackedTouch) {
        cancelLongPressTimer()
        if state.alternatesActive {
            onAlternatesEnd?(true)
        } else if state.initialKey.kind == .backspace {
            if state.swipeUpActive || state.swipeDownActive {
                onKeyTouchCancel?()
            } else {
                onKeyTouchUp?(state.initialKey, state.initialPoint)
            }
        } else if state.swipeUpActive, let symbol = state.initialKey.swipeUpSymbol {
            onKeySwipeUp?(symbol)
        } else {
            let cleanPt = (state.currentKey.id == state.initialKey.id)
                ? state.initialPoint
                : state.currentPoint
            onKeyTouchUp?(state.currentKey, cleanPt)
        }
    }

    private func cancelLongPressTimer(for id: ObjectIdentifier? = nil) {
        if let id, longPressTouchId != id { return }
        longPressTimer?.invalidate()
        longPressTimer = nil
        longPressTouchId = nil
    }

    private func startLongPressTimer(for id: ObjectIdentifier) {
        cancelLongPressTimer()
        longPressTouchId = id
        let timer = Timer(timeInterval: 0.40, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.longPressTouchId == id else { return }
                self.longPressTimer = nil
                self.longPressTouchId = nil
                guard var state = self.trackedTouches[id],
                      !state.committedByRollover,
                      !state.swipeUpActive,
                      state.currentKey.id == state.initialKey.id else { return }
                state.alternatesActive = true
                self.trackedTouches[id] = state
                self.onAlternatesBegin?(state.initialKey)
            }
        }
        longPressTimer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    private func swipePreviewSpec(for key: IOSKeySpec, symbol: String) -> IOSKeySpec {
        IOSKeySpec(id: key.id, kind: .symbol(symbol), frame: key.frame, primaryLabel: symbol)
    }

    private func purgeEndedOrOrphanTouches(referenceTimestamp: TimeInterval? = nil) {
        let stale = trackedTouches.compactMap { id, state -> ObjectIdentifier? in
            guard let t = state.touch else { return id }
            if state.committedByRollover && (t.phase == .ended || t.phase == .cancelled) {
                return id
            }
            if !state.committedByRollover {
                if t.phase == .cancelled {
                    return id
                }
                if t.phase == .ended, let ref = referenceTimestamp, ref - state.beganTimestamp > 0.30 {
                    return id
                }
            }
            return nil
        }
        for id in stale {
            if spaceHoldTouchId == id {
                spaceHoldTimer?.invalidate()
                spaceHoldTimer = nil
                spaceHoldTouchId = nil
            }
            trackedTouches.removeValue(forKey: id)
        }
    }

    func applyTheme(dark: Bool) {
        guard isDark != dark else { return }
        isDark = dark
        rebuildOrSyncKeycapNodes()
    }

    func updateSnapshot(_ snap: IOSKeyboardLayoutEngine.LayoutSnapshot) {
        self.snapshot = snap
        rebuildOrSyncKeycapNodes()
    }

    private func rebuildOrSyncKeycapNodes() {
        guard let snap = snapshot else { return }
        while keycapNodes.count < snap.keys.count {
            let node = IOSKeycapNodeView(frame: .zero)
            addSubview(node)
            keycapNodes.append(node)
        }
        if keycapNodes.count > snap.keys.count {
            for idx in snap.keys.count..<keycapNodes.count {
                keycapNodes[idx].isHidden = true
            }
        }
        let pressedIds = currentPressedKeyIds()
        for (idx, spec) in snap.keys.enumerated() {
            let node = keycapNodes[idx]
            node.isHidden = false
            let isPressed = pressedIds.contains(spec.id)
            let isCovered = isPressed && (coveredCalloutKeyId == spec.id)
            node.configure(spec: spec, dark: isDark, pressed: isPressed, glyphCovered: isCovered)
        }
    }

    private func currentPressedKeyIds() -> Set<String> {
        if trackpad.isActive { return [] }
        var ids = Set<String>()
        for t in trackedTouches.values where !t.committedByRollover {
            ids.insert(t.currentKey.id)
        }
        return ids
    }

    private func syncKeycapPressStates() {
        guard let snap = snapshot else { return }
        let pressedIds = currentPressedKeyIds()
        for (idx, spec) in snap.keys.enumerated() where idx < keycapNodes.count {
            let node = keycapNodes[idx]
            let isPressed = pressedIds.contains(spec.id)
            let isCovered = isPressed && (coveredCalloutKeyId == spec.id)
            node.updatePressState(pressed: isPressed, glyphCovered: isCovered, dark: isDark)
        }
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        let newestTs = touches.map(\.timestamp).max()
        purgeEndedOrOrphanTouches(referenceTimestamp: newestTs)
        let ordered = touches.sorted(by: { $0.timestamp < $1.timestamp })
        for touch in ordered {
            let newTouchId = ObjectIdentifier(touch)
            // 1. Unified Multi-Touch Rollover: commit every earlier uncommitted touch in exact press order
            //    BEFORE hit-testing the new touch (so `validNextMask`, `plane`, and `shiftState` are fresh!).
            let pendingIds = trackedTouches
                .filter {
                    let state = $0.value
                    if $0.key == newTouchId && abs(state.beganTimestamp - touch.timestamp) < 0.0005 {
                        return false
                    }
                    return !state.committedByRollover && !state.isPlaneSlide
                        && !(state.startedOnSpace && trackpad.isActive)
                }
                .sorted(by: { $0.value.beganTimestamp < $1.value.beganTimestamp })
                .map(\.key)

            for oldId in pendingIds {
                guard var oldState = trackedTouches[oldId] else { continue }
                if oldState.startedOnSpace && spaceHoldTouchId == oldId {
                    spaceHoldTimer?.invalidate()
                    spaceHoldTimer = nil
                    spaceHoldTouchId = nil
                }
                oldState.committedByRollover = true
                trackedTouches[oldId] = oldState
                commitRolledOver(oldState)
            }

            // 2. Hit-test new touch against freshly updated snapshot & dynamic validNextMask
            let point = touch.location(in: self)
            guard let snap = snapshot,
                  let hit = IOSKeyboardLayoutEngine.hitTest(
                      point: point,
                      in: snap,
                      validNextMask: validNextMask,
                      nextKeyPriors: nextKeyPriors,
                      touchOffsets: touchOffsets,
                      t9ValidNextMask: t9ValidNextMask,
                      t9NextKeyPriors: t9NextKeyPriors,
                      t9TouchOffsets: t9TouchOffsets
                  ) else {
                continue
            }

            let id = newTouchId
            let isPlane: Bool
            if case .planeSwitch = hit.kind { isPlane = true } else { isPlane = false }
            let isSpace = (hit.kind == .space)

            let state = TrackedTouch(
                touch: touch,
                beganTimestamp: touch.timestamp,
                initialPoint: point,
                currentPoint: point,
                prevPoint: point,
                prevTimestamp: touch.timestamp,
                initialKey: hit,
                currentKey: hit,
                isPlaneSlide: isPlane,
                didSlideToSymbol: false,
                startedOnSpace: isSpace,
                committedByRollover: false
            )
            trackedTouches[id] = state

            if isSpace {
                spaceHoldTimer?.invalidate()
                spaceHoldTouchId = id
                let timer = Timer(timeInterval: 0.42, repeats: false) { [weak self] _ in
                    MainActor.assumeIsolated {
                        guard let self,
                              let activeId = self.spaceHoldTouchId,
                              let st = self.trackedTouches[activeId],
                              !st.committedByRollover else { return }
                        self.trackpad.begin(at: st.currentPoint, timestamp: ProcessInfo.processInfo.systemUptime)
                        self.onTrackpadStateChanged?(true)
                        self.syncKeycapPressStates()
                    }
                }
                spaceHoldTimer = timer
                RunLoop.main.add(timer, forMode: .common)
            }

            syncKeycapPressStates()
            onKeyTouchDown?(hit)
            if longPressAlternatesEnabled, case .letter = hit.kind {
                startLongPressTimer(for: id)
            }
        }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let snap = snapshot else { return }
        for touch in touches {
            let id = ObjectIdentifier(touch)
            guard var state = trackedTouches[id], !state.committedByRollover else { continue }
            let pt = touch.location(in: self)
            let stepDt = touch.timestamp - state.prevTimestamp
            if state.prevTimestamp > state.beganTimestamp && stepDt > 0.001 {
                let stepDist = hypot(pt.x - state.prevPoint.x, pt.y - state.prevPoint.y)
                state.latestStepSpeed = stepDist / CGFloat(stepDt)
            }
            state.prevPoint = pt
            state.prevTimestamp = touch.timestamp
            state.currentPoint = pt
            state.maxMovedDistance = max(
                state.maxMovedDistance,
                hypot(pt.x - state.initialPoint.x, pt.y - state.initialPoint.y)
            )
            state.maxAbsDeltaY = max(state.maxAbsDeltaY, abs(pt.y - state.initialPoint.y))

            if state.alternatesActive {
                trackedTouches[id] = state
                onAlternatesMove?(pt)
                continue
            }
            if state.maxMovedDistance > 12.0 {
                cancelLongPressTimer(for: id)
            }

            if state.initialKey.kind == .backspace {
                guard backspaceSwipeClearEnabled else {
                    trackedTouches[id] = state
                    continue
                }
                // Clearing is destructive, so backspace needs a longer, deliberate swipe than letters.
                let armed = IOSSwipeUp.isActive(
                    from: state.initialPoint,
                    to: pt,
                    keyHeight: state.initialKey.frame.height * 1.15,
                    wasActive: state.swipeUpActive
                )
                if armed != state.swipeUpActive {
                    state.swipeUpActive = armed
                    onBackspaceSwipeArmed?(state.initialKey, armed)
                }
                let armedDown = canUndoLineClear && !armed && IOSSwipeUp.isActive(
                    from: CGPoint(x: state.initialPoint.x, y: -state.initialPoint.y),
                    to: CGPoint(x: pt.x, y: -pt.y),
                    keyHeight: state.initialKey.frame.height * 1.15,
                    wasActive: state.swipeDownActive
                )
                if armedDown != state.swipeDownActive {
                    state.swipeDownActive = armedDown
                    onBackspaceSwipeDownArmed?(state.initialKey, armedDown)
                }
                trackedTouches[id] = state
                continue
            }

            // 1. Hold-and-slide from `123` / `#+=` to pick a number/punctuation and auto-return
            if state.isPlaneSlide {
                if state.initialKey.frame.insetBy(dx: -8, dy: -8).contains(pt) && !state.didSlideToSymbol {
                    trackedTouches[id] = state
                    continue
                }
                if let hit = IOSKeyboardLayoutEngine.hitTest(point: pt, in: snap), hit.id != state.currentKey.id {
                    state.currentKey = hit
                    if case .symbol = hit.kind {
                        state.didSlideToSymbol = true
                    }
                    trackedTouches[id] = state
                    syncKeycapPressStates()
                    onKeySlidePreview?(hit)
                } else {
                    trackedTouches[id] = state
                }
                continue
            }

            // 2. Spacebar 2D trackpad cursor drag
            if state.startedOnSpace {
                trackedTouches[id] = state
                if !trackpad.isActive && hypot(pt.x - state.initialPoint.x, pt.y - state.initialPoint.y) > 14 {
                    if spaceHoldTouchId == id {
                        spaceHoldTimer?.invalidate()
                        spaceHoldTimer = nil
                    }
                }
                if trackpad.isActive {
                    let (dx, dy) = trackpad.update(point: pt, timestamp: touch.timestamp)
                    if dx != 0 || dy != 0 {
                        onTrackpadMove?(dx, dy)
                    }
                }
                continue
            }

            // 3. Functional keys (`Shift`, `中/英`, `Globe`, `Return`) stay locked to their own keycap
            //    so lift-off thumb roll never converts them into adjacent letters or vice versa.
            switch state.initialKey.kind {
            case .letter, .t9Key, .symbol:
                break
            default:
                trackedTouches[id] = state
                continue
            }

            // Swipe up on a letter enters its secondary symbol; sliding back down restores the letter.
            if state.currentKey.id == state.initialKey.id, let symbol = state.initialKey.swipeUpSymbol {
                let active = IOSSwipeUp.isActive(
                    from: state.initialPoint,
                    to: pt,
                    keyHeight: state.initialKey.frame.height,
                    wasActive: state.swipeUpActive
                )
                if active != state.swipeUpActive {
                    state.swipeUpActive = active
                    trackedTouches[id] = state
                    onKeySlidePreview?(active ? swipePreviewSpec(for: state.initialKey, symbol: symbol) : state.initialKey)
                }
                if active {
                    trackedTouches[id] = state
                    continue
                }
            }

            // 4. Sticky Press-Lock Hysteresis: ignore natural thumb lift-roll within expanded keep-zone
            //    around BOTH the keycap frame and `initialPoint` (protecting border/gutter strikes!).
            let keepRect = state.currentKey.frame.insetBy(dx: -10.5, dy: -13.5)
            let nearInitial = (state.currentKey.id == state.initialKey.id)
                && hypot(pt.x - state.initialPoint.x, pt.y - state.initialPoint.y) <= 13.0
            if keepRect.contains(pt) || nearInitial {
                trackedTouches[id] = state
                continue
            }

            // Never switch keys on rapid ballistic taps (< 115ms) or fast 120Hz lift-off flicks (> 380 pt/s)
            // toward the next key; only deliberate hold-and-slide onto another character key switches keys!
            let dt = max(0, touch.timestamp - state.beganTimestamp)
            guard dt >= 0.115 else {
                trackedTouches[id] = state
                continue
            }
            if dt < 0.20 && state.latestStepSpeed > 380.0 {
                trackedTouches[id] = state
                continue
            }

            if pt.y < -72 {
                trackedTouches[id] = state
                onKeySlidePreview?(nil)
                continue
            }
            if pt.y < 0 {
                trackedTouches[id] = state
                continue
            }

            if let hit = IOSKeyboardLayoutEngine.hitTest(
                point: pt,
                in: snap,
                validNextMask: validNextMask,
                nextKeyPriors: nextKeyPriors,
                touchOffsets: touchOffsets,
                t9ValidNextMask: t9ValidNextMask,
                t9NextKeyPriors: t9NextKeyPriors,
                t9TouchOffsets: t9TouchOffsets
            ),
               hit.id != state.currentKey.id {
                // Character touches may slide only onto character keys — never onto `⌫` (which would silently
                // drop the key in `onKeyTouchUp`), `⇧`, `Space`, `123`, `中/英`, or `Return`!
                let isCharacterTarget: Bool
                switch hit.kind {
                case .letter, .t9Key, .symbol: isCharacterTarget = true
                default: isCharacterTarget = false
                }
                if isCharacterTarget {
                    state.currentKey = hit
                    trackedTouches[id] = state
                    syncKeycapPressStates()
                    onKeySlidePreview?(hit)
                } else {
                    trackedTouches[id] = state
                }
            } else {
                trackedTouches[id] = state
            }
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        let ordered = touches.sorted(by: { $0.timestamp < $1.timestamp })
        for touch in ordered {
            let id = ObjectIdentifier(touch)
            guard let state = trackedTouches.removeValue(forKey: id) else { continue }
            let pt = touch.location(in: self)
            cancelLongPressTimer(for: id)

            if spaceHoldTouchId == id {
                spaceHoldTimer?.invalidate()
                spaceHoldTimer = nil
                spaceHoldTouchId = nil
            }

            if state.committedByRollover {
                syncKeycapPressStates()
                continue
            }

            if state.alternatesActive {
                onAlternatesMove?(pt)
                syncKeycapPressStates()
                onAlternatesEnd?(true)
                continue
            }

            if state.startedOnSpace && trackpad.isActive {
                trackpad.end()
                onTrackpadStateChanged?(false)
                syncKeycapPressStates()
                continue
            }

            if state.isPlaneSlide {
                syncKeycapPressStates()
                if !state.didSlideToSymbol {
                    // Normal tap on `123` / `ABC` / `#+=` (already switched in `onKeyTouchDown`): keep new plane!
                    onKeySlidePreview?(nil)
                    continue
                }
                guard pt.y >= -72,
                      let snap = snapshot,
                      let hit = IOSKeyboardLayoutEngine.hitTest(point: pt, in: snap) else {
                    onRestorePlane?()
                    onKeyTouchCancel?()
                    continue
                }
                if case .symbol(let sym) = hit.kind {
                    let activePlane: IOSKeyPlane = snap.keys.contains(where: { $0.id == "P_numbers" }) ? .symbols : .numbers
                    onKeyTouchUp?(hit, pt)
                    if !IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol(sym, on: activePlane) {
                        onRestorePlane?()
                    }
                } else if hit.id == state.initialKey.id {
                    onKeySlidePreview?(nil)
                } else {
                    onRestorePlane?()
                    onKeyTouchCancel?()
                }
                continue
            }

            // Only cancel when deliberately dragged well outside the entire keyboard tray (`y < -72` after >=140ms);
            // rapid thumb flicks into the candidate ribbon (`y = -44..0`) ALWAYS commit cleanly!
            let dt = max(0, touch.timestamp - state.beganTimestamp)
            // An armed swipe-up on a top-row key legitimately travels past the ribbon, so it is not a cancel.
            let isArmedSwipeUp = state.currentKey.id == state.initialKey.id
                && state.initialKey.swipeUpSymbol != nil
                && IOSSwipeUp.isActive(
                    from: state.initialPoint,
                    to: pt,
                    keyHeight: state.initialKey.frame.height,
                    wasActive: state.swipeUpActive
                )
            if pt.y < -72 && dt >= 0.14 && !isArmedSwipeUp {
                syncKeycapPressStates()
                onKeyTouchCancel?()
                continue
            }

            if state.initialKey.kind == .backspace, state.swipeUpActive {
                syncKeycapPressStates()
                onBackspaceSwipeUp?()
                continue
            }
            if state.initialKey.kind == .backspace, state.swipeDownActive {
                syncKeycapPressStates()
                onBackspaceSwipeDown?()
                continue
            }

            // Re-evaluate at lift-off so a quick flick with few move events still counts.
            if state.currentKey.id == state.initialKey.id,
               let symbol = state.initialKey.swipeUpSymbol,
               IOSSwipeUp.isActive(
                   from: state.initialPoint,
                   to: pt,
                   keyHeight: state.initialKey.frame.height,
                   wasActive: state.swipeUpActive
               ) {
                syncKeycapPressStates()
                onKeySwipeUp?(symbol)
                continue
            }

            let commitPt = (state.currentKey.id == state.initialKey.id) ? state.initialPoint : pt
            syncKeycapPressStates()
            onKeyTouchUp?(state.currentKey, commitPt)
        }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        let ordered = touches.sorted(by: { $0.timestamp < $1.timestamp })
        var rescuedAny = false
        for touch in ordered {
            let id = ObjectIdentifier(touch)
            guard let state = trackedTouches.removeValue(forKey: id) else { continue }
            cancelLongPressTimer(for: id)
            if spaceHoldTouchId == id {
                spaceHoldTimer?.invalidate()
                spaceHoldTimer = nil
                spaceHoldTouchId = nil
            }
            if state.alternatesActive && !state.committedByRollover {
                onAlternatesEnd?(false)
                rescuedAny = true
                continue
            }
            if state.isPlaneSlide {
                if state.didSlideToSymbol {
                    onRestorePlane?()
                }
                continue
            }
            if state.startedOnSpace && trackpad.isActive {
                trackpad.end()
                onTrackpadStateChanged?(false)
                continue
            }
            if state.committedByRollover || state.initialKey.kind == .backspace {
                continue
            }
            if state.swipeUpActive, !isHidden, let symbol = state.initialKey.swipeUpSymbol {
                rescuedAny = true
                onKeySwipeUp?(symbol)
                continue
            }

            // Distinguish edge/bottom gesture-gate cancellations (e.g. screen-edge back-swipe gate on `q`/`p`
            // or Home-indicator gate during a fast horizontal thumb sweep across Spacebar) from true vertical
            // scroll/dismiss drags (`maxMovedDistance` tracked from actual `touchesMoved` events, insulated
            // against synthetic XPC cancel coordinates jumping to window origin):
            let rawEndPt = touch.location(in: self)
            let effectiveDrift: CGFloat
            let effectiveAbsDy: CGFloat
            if bounds.insetBy(dx: -48.0, dy: -64.0).contains(rawEndPt) {
                effectiveDrift = max(
                    state.maxMovedDistance,
                    hypot(rawEndPt.x - state.initialPoint.x, rawEndPt.y - state.initialPoint.y)
                )
                effectiveAbsDy = max(state.maxAbsDeltaY, abs(rawEndPt.y - state.initialPoint.y))
            } else {
                effectiveDrift = state.maxMovedDistance
                effectiveAbsDy = state.maxAbsDeltaY
            }
            let dt = max(0, touch.timestamp - state.beganTimestamp)
            let shouldRescue: Bool
            if state.startedOnSpace {
                shouldRescue = !isHidden && effectiveAbsDy <= 20.0 && effectiveDrift <= 52.0 && dt <= 0.42
            } else {
                shouldRescue = !isHidden && effectiveDrift <= 20.0 && dt <= 0.38
            }
            if shouldRescue {
                rescuedAny = true
                onKeyTouchUp?(state.initialKey, state.initialPoint)
            }
        }
        syncKeycapPressStates()
        if !rescuedAny && trackedTouches.values.allSatisfy(\.committedByRollover) {
            onKeyTouchCancel?()
        }
    }
}

/// Long-press strip of alternate characters; the finger slides horizontally to choose one.
final class IOSKeyAlternatesView: UIView {
    private var labels: [UILabel] = []
    private let highlight = UIView()
    private var items: [String] = []
    private(set) var selectedIndex: Int = 0

    var selectedText: String? {
        items.indices.contains(selectedIndex) ? items[selectedIndex] : nil
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        isHidden = true
        layer.cornerRadius = 13.0
        layer.cornerCurve = .continuous
        layer.shadowColor = UIColor.black.cgColor
        layer.shadowOffset = CGSize(width: 0, height: 3.5)
        layer.shadowRadius = 11.0
        highlight.backgroundColor = .systemBlue
        highlight.layer.cornerRadius = 9.0
        highlight.layer.cornerCurve = .continuous
        addSubview(highlight)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present(items: [String], defaultIndex: Int, over keyFrame: CGRect, containerWidth: CGFloat, dark: Bool) {
        self.items = items
        labels.forEach { $0.removeFromSuperview() }
        frame = IOSKeyAlternates.stripFrame(
            keyFrame: keyFrame,
            count: items.count,
            defaultIndex: defaultIndex,
            containerWidth: containerWidth
        )
        backgroundColor = dark ? UIColor(red: 0.29, green: 0.29, blue: 0.30, alpha: 1.0) : .white
        layer.shadowOpacity = dark ? 0.34 : 0.16
        let cell = bounds.width / CGFloat(max(items.count, 1))
        labels = items.enumerated().map { idx, text in
            let label = UILabel(frame: CGRect(x: CGFloat(idx) * cell, y: 0, width: cell, height: bounds.height))
            label.text = text
            label.textAlignment = .center
            label.font = .systemFont(ofSize: 23.0)
            label.adjustsFontSizeToFitWidth = true
            label.minimumScaleFactor = 0.5
            addSubview(label)
            return label
        }
        isHidden = false
        isDarkStyle = dark
        select(defaultIndex)
    }

    private var isDarkStyle = false

    /// Returns `true` when the highlighted character changed.
    @discardableResult
    func update(xInSuperview x: CGFloat) -> Bool {
        let idx = IOSKeyAlternates.index(atX: x, in: frame, count: items.count)
        guard idx != selectedIndex else { return false }
        select(idx)
        return true
    }

    private func select(_ idx: Int) {
        selectedIndex = idx
        guard labels.indices.contains(idx) else { return }
        highlight.frame = labels[idx].frame.insetBy(dx: 2.0, dy: 4.0)
        for (i, label) in labels.enumerated() {
            label.textColor = i == idx ? .white : (isDarkStyle ? .white : .black)
        }
    }

    func hide() {
        isHidden = true
        items = []
    }
}

/// Full-size iOS 26 Key Callout Bubble with spring micro-motion:
/// - Row 1 & Row 2: standalone `61.33 x 65.0pt` continuous squircle hovering `4.0pt` above pressed keycap.
/// - Row 0 (`q..p`): full `61.33pt` head (`36.5pt` glyph, zero shrinking!) seamlessly tapering into the
///   pressed keycap base so it neither gets clipped by the XPC viewport nor cuts the underlying glyph in half.
final class IOSKeyCalloutView: UIView {
    private let shapeLayer = CAShapeLayer()
    private let glyphLabel = UILabel()
    private var dismissWorkItem: DispatchWorkItem?
    private(set) var coveredKeyId: String?

    override init(frame: CGRect) {
        super.init(frame: CGRect(x: 0, y: 0, width: 61.33, height: 65.0))
        isOpaque = false
        isUserInteractionEnabled = false
        clipsToBounds = false

        shapeLayer.shadowColor = UIColor.black.cgColor
        shapeLayer.shadowOffset = CGSize(width: 0, height: 3.5)
        shapeLayer.shadowRadius = 11.0
        layer.addSublayer(shapeLayer)

        glyphLabel.font = .systemFont(ofSize: 36.5, weight: .regular)
        glyphLabel.textAlignment = .center
        glyphLabel.adjustsFontSizeToFitWidth = true
        glyphLabel.minimumScaleFactor = 0.45
        addSubview(glyphLabel)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func hideImmediately() {
        dismissWorkItem?.cancel()
        dismissWorkItem = nil
        layer.removeAllAnimations()
        coveredKeyId = nil
        alpha = 1.0
        transform = .identity
        isHidden = true
    }

    func releasePreview(onCleared: (() -> Void)? = nil) {
        dismissWorkItem?.cancel()
        let item = DispatchWorkItem { [weak self] in
            guard let self else { return }
            UIView.animate(
                withDuration: 0.052,
                delay: 0,
                options: [.curveEaseIn, .beginFromCurrentState, .allowUserInteraction],
                animations: {
                    self.alpha = 0.0
                    self.transform = CGAffineTransform(scaleX: 0.94, y: 0.94).translatedBy(x: 0, y: 2.0)
                },
                completion: { finished in
                    if finished {
                        self.coveredKeyId = nil
                        self.isHidden = true
                        self.alpha = 1.0
                        self.transform = .identity
                        onCleared?()
                    }
                }
            )
        }
        dismissWorkItem = item
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.038, execute: item)
    }

    /// Presents the full-size callout and returns `true` when Row 0's docked skirt covers the underlying keycap glyph.
    @discardableResult
    func present(label: String, keyId: String, over keyFrameInSuperview: CGRect, dark: Bool) -> Bool {
        dismissWorkItem?.cancel()
        dismissWorkItem = nil

        let wasHidden = isHidden || alpha < 0.2
        layer.removeAllAnimations()

        let headW: CGFloat = max(61.33, keyFrameInSuperview.width * 1.66)
        let headH: CGFloat = 65.0
        let hoverGap: CGFloat = 4.0
        let superW = superview?.bounds.width ?? 440

        let headMinX = min(
            max(3.5, keyFrameInSuperview.midX - headW * 0.5),
            max(3.5, superW - headW - 3.5)
        )
        let rawFloatingY = keyFrameInSuperview.minY - hoverGap - headH

        let fillCol = dark
            ? UIColor(red: 0.29, green: 0.29, blue: 0.30, alpha: 1.0)
            : UIColor.white
        let strokeCol = dark
            ? UIColor.white.withAlphaComponent(0.14)
            : UIColor.black.withAlphaComponent(0.06)

        CATransaction.begin()
        CATransaction.setDisableActions(true)

        let isDockedRow0 = (rawFloatingY < 4.0)

        if !isDockedRow0 {
            // Row 1 & Row 2: pure standalone floating iOS 26 squircle!
            coveredKeyId = nil
            let targetFrame = CGRect(x: headMinX, y: rawFloatingY, width: headW, height: headH)
            UIView.performWithoutAnimation {
                self.transform = .identity
                self.frame = targetFrame
                let p = UIBezierPath(roundedRect: CGRect(origin: .zero, size: targetFrame.size), cornerRadius: 13.5)
                shapeLayer.frame = bounds
                shapeLayer.path = p.cgPath
                shapeLayer.shadowPath = p.cgPath
                glyphLabel.font = .systemFont(ofSize: 36.5, weight: .regular)
                glyphLabel.frame = CGRect(x: 0, y: -1.0, width: headW, height: headH)
            }
        } else {
            // Row 0 (`q..p`): full-size callout docked seamlessly into exact keycap base!
            // Align outer edge flush with `q`/`p` keycap (`6.67pt`) so it sits cleanly inside the 26pt tray corner,
            // and place waist at `49.0..62.0pt` so descenders (`q`, `y`, `p`) have generous room above the taper.
            coveredKeyId = keyId
            let topY: CGFloat = 6.0
            let botY: CGFloat = keyFrameInSuperview.maxY
            let isLeftEdge = (keyFrameInSuperview.minX < 18.0)
            let isRightEdge = (keyFrameInSuperview.maxX > superW - 18.0)
            let row0HeadW: CGFloat = (isLeftEdge || isRightEdge) ? 55.0 : headW
            let row0HeadMinX: CGFloat
            if isLeftEdge {
                row0HeadMinX = keyFrameInSuperview.minX
            } else if isRightEdge {
                row0HeadMinX = keyFrameInSuperview.maxX - row0HeadW
            } else {
                row0HeadMinX = keyFrameInSuperview.midX - row0HeadW * 0.5
            }
            let row0HeadMaxX = row0HeadMinX + row0HeadW

            let unionMinX = min(row0HeadMinX, keyFrameInSuperview.minX)
            let unionMaxX = max(row0HeadMaxX, keyFrameInSuperview.maxX)
            let totalW = unionMaxX - unionMinX
            let totalH = botY - topY

            let hL = row0HeadMinX - unionMinX
            let hR = row0HeadMaxX - unionMinX
            let kL = keyFrameInSuperview.minX - unionMinX
            let kR = keyFrameInSuperview.maxX - unionMinX

            let waistTop: CGFloat = 49.0
            let waistBot: CGFloat = 62.0
            let path = Self.makeDockedBalloonPath(
                headLeft: hL,
                headRight: hR,
                keyLeft: kL,
                keyRight: kR,
                waistTop: waistTop,
                waistBot: waistBot,
                totalHeight: totalH,
                topRadius: 13.0,
                bottomRadius: IOSKeyboardLayoutEngine.keyCornerRadius
            )

            UIView.performWithoutAnimation {
                self.transform = .identity
                self.frame = CGRect(x: unionMinX, y: topY, width: totalW, height: totalH)
                shapeLayer.frame = bounds
                shapeLayer.path = path
                shapeLayer.shadowPath = path
                glyphLabel.font = .systemFont(ofSize: 35.5, weight: .regular)
                glyphLabel.frame = CGRect(x: hL, y: -1.5, width: row0HeadW, height: 51.0)
            }
        }

        shapeLayer.fillColor = fillCol.cgColor
        shapeLayer.strokeColor = strokeCol.cgColor
        shapeLayer.lineWidth = 0.5
        shapeLayer.shadowOpacity = dark ? 0.34 : 0.14
        CATransaction.commit()

        glyphLabel.text = label
        glyphLabel.textColor = dark ? .white : .black
        alpha = 1.0
        isHidden = false

        if wasHidden {
            transform = CGAffineTransform(scaleX: 0.91, y: 0.91).translatedBy(x: 0, y: 3.5)
            UIView.animate(
                withDuration: 0.072,
                delay: 0,
                usingSpringWithDamping: 0.84,
                initialSpringVelocity: 0.5,
                options: [.beginFromCurrentState, .allowUserInteraction],
                animations: {
                    self.transform = .identity
                }
            )
        }
        return isDockedRow0
    }

    private static func makeDockedBalloonPath(
        headLeft: CGFloat,
        headRight: CGFloat,
        keyLeft: CGFloat,
        keyRight: CGFloat,
        waistTop: CGFloat,
        waistBot: CGFloat,
        totalHeight: CGFloat,
        topRadius rT: CGFloat,
        bottomRadius rB: CGFloat
    ) -> CGPath {
        let p = UIBezierPath()
        let midY = (waistTop + waistBot) * 0.5

        p.move(to: CGPoint(x: headLeft + rT, y: 0))
        p.addLine(to: CGPoint(x: headRight - rT, y: 0))
        p.addQuadCurve(to: CGPoint(x: headRight, y: rT), controlPoint: CGPoint(x: headRight, y: 0))
        p.addLine(to: CGPoint(x: headRight, y: waistTop))

        // Right waist curve into keycap right edge
        p.addCurve(
            to: CGPoint(x: keyRight, y: waistBot),
            controlPoint1: CGPoint(x: headRight, y: midY),
            controlPoint2: CGPoint(x: keyRight, y: midY)
        )
        p.addLine(to: CGPoint(x: keyRight, y: totalHeight - rB))
        p.addQuadCurve(
            to: CGPoint(x: keyRight - rB, y: totalHeight),
            controlPoint: CGPoint(x: keyRight, y: totalHeight)
        )
        p.addLine(to: CGPoint(x: keyLeft + rB, y: totalHeight))
        p.addQuadCurve(
            to: CGPoint(x: keyLeft, y: totalHeight - rB),
            controlPoint: CGPoint(x: keyLeft, y: totalHeight)
        )
        p.addLine(to: CGPoint(x: keyLeft, y: waistBot))

        // Left waist curve back up into head left edge
        p.addCurve(
            to: CGPoint(x: headLeft, y: waistTop),
            controlPoint1: CGPoint(x: keyLeft, y: midY),
            controlPoint2: CGPoint(x: headLeft, y: midY)
        )
        p.addLine(to: CGPoint(x: headLeft, y: rT))
        p.addQuadCurve(to: CGPoint(x: headLeft + rT, y: 0), controlPoint: CGPoint(x: headLeft, y: 0))
        p.close()
        return p.cgPath
    }
}
/// Native iOS T9 left sidebar: vertical scrollable punctuation column when idle,
/// switching seamlessly to candidate Pinyin syllable filters when composing.
@MainActor
final class IOST9SidebarView: UIView {
    var onSelectPunctuation: ((String) -> Void)?
    var onSelectSyllable: ((String) -> Void)?

    private static let idlePunctuations = ["，", "。", "？", "！", "、", "……", "“", "”"]

    private let scrollView = UIScrollView()
    private let stackView = UIStackView()
    private var currentItems: [String] = []
    private var isSyllableMode = false
    private var isDark = false

    override init(frame: CGRect) {
        super.init(frame: frame)
        layer.cornerRadius = IOSKeyboardLayoutEngine.keyCornerRadius
        layer.cornerCurve = .continuous
        clipsToBounds = true

        scrollView.showsVerticalScrollIndicator = false
        scrollView.showsHorizontalScrollIndicator = false
        scrollView.alwaysBounceVertical = true
        scrollView.delaysContentTouches = false
        addSubview(scrollView)

        stackView.axis = .vertical
        stackView.distribution = .fill
        stackView.alignment = .fill
        stackView.spacing = 0
        scrollView.addSubview(stackView)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        scrollView.frame = bounds
        layoutRows()
    }

    func update(syllables: [String], composing: Bool, dark: Bool) {
        let nextMode = composing && !syllables.isEmpty
        let nextItems = nextMode ? syllables : Self.idlePunctuations
        let needsRebuild = (nextMode != isSyllableMode) || (nextItems != currentItems) || (dark != isDark)
        isSyllableMode = nextMode
        currentItems = nextItems
        isDark = dark

        backgroundColor = dark
            ? UIColor(white: 1.0, alpha: 0.13)
            : UIColor(red: 229.0 / 255.0, green: 231.0 / 255.0, blue: 236.0 / 255.0, alpha: 1.0)

        guard needsRebuild else { return }
        for v in stackView.arrangedSubviews {
            stackView.removeArrangedSubview(v)
            v.removeFromSuperview()
        }

        let fg: UIColor = dark ? .white : .black
        let sepColor: UIColor = dark
            ? UIColor(white: 1.0, alpha: 0.12)
            : UIColor(white: 0.0, alpha: 0.09)

        for (idx, item) in nextItems.enumerated() {
            let btn = UIButton(type: .system)
            btn.setTitle(item, for: .normal)
            btn.setTitleColor(fg, for: .normal)
            btn.titleLabel?.font = nextMode
                ? .systemFont(ofSize: 15.0, weight: .medium)
                : .systemFont(ofSize: 18.0, weight: .regular)
            btn.titleLabel?.adjustsFontSizeToFitWidth = true
            btn.titleLabel?.minimumScaleFactor = 0.72
            btn.tag = idx
            btn.addTarget(self, action: #selector(handleRowTap(_:)), for: .touchUpInside)

            if idx > 0 {
                let sep = UIView()
                sep.backgroundColor = sepColor
                sep.isUserInteractionEnabled = false
                sep.translatesAutoresizingMaskIntoConstraints = false
                btn.addSubview(sep)
                NSLayoutConstraint.activate([
                    sep.topAnchor.constraint(equalTo: btn.topAnchor),
                    sep.leadingAnchor.constraint(equalTo: btn.leadingAnchor, constant: 8),
                    sep.trailingAnchor.constraint(equalTo: btn.trailingAnchor, constant: -8),
                    sep.heightAnchor.constraint(equalToConstant: 0.5),
                ])
            }
            stackView.addArrangedSubview(btn)
        }
        scrollView.setContentOffset(.zero, animated: false)
        layoutRows()
    }

    private func layoutRows() {
        let w = bounds.width
        let h = bounds.height
        guard w > 4, h > 4 else { return }
        let rowH: CGFloat = max(36.0, floor(h / 4.0))
        let totalH = max(h, rowH * CGFloat(currentItems.count))
        stackView.frame = CGRect(x: 0, y: 0, width: w, height: totalH)
        for v in stackView.arrangedSubviews {
            v.constraints.filter { $0.firstAttribute == .height && $0.secondItem == nil }.forEach { v.removeConstraint($0) }
            v.heightAnchor.constraint(equalToConstant: rowH).isActive = true
        }
        scrollView.contentSize = CGSize(width: w, height: totalH)
    }

    @objc private func handleRowTap(_ sender: UIButton) {
        guard sender.tag >= 0, sender.tag < currentItems.count else { return }
        let item = currentItems[sender.tag]
        if isSyllableMode {
            onSelectSyllable?(item)
        } else {
            onSelectPunctuation?(item)
        }
    }
}
#endif
