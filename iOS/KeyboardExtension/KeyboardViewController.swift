#if os(iOS)
import UIKit

@objc(KeyboardViewController)
public final class KeyboardViewController: UIInputViewController {
    private lazy var bridge = JuxuBridge()
    private var layoutEngine = IOSKeyboardLayoutEngine()

    private let glassBackdrop = UIView()
    private let ribbonView = IOSCandidateRibbonView()
    private let keyCanvas = IOSKeyCanvasView()
    private let t9SidebarView = IOST9SidebarView()
    private let expandedGrid = IOSExpandedCandidateView()
    private let calloutView = IOSKeyCalloutView()
    private let alternatesView = IOSKeyAlternatesView()

    private var heightConstraint: NSLayoutConstraint?
    private var ribbonHeightConstraint: NSLayoutConstraint?
    private var saveUserDataTimer: Timer?
    private var backspaceTimer: Timer?
    private var backspaceHoldStart: TimeInterval = 0
    private var lastVisualKeySignature: VisualLayoutSignature?
    private var planeBeforePress: IOSKeyPlane = .letters
    private var markedCode = ""
    private var autoSpaceTracker = AutoSpaceContextTracker()
    private var compositionBaseContext: String?
    private let clipboardStore = IOSClipboardSuggestionStore()
    private var clipboardGate = IOSClipboardVisibilityGate()
    private let clipboardReadQueue = DispatchQueue(label: "com.hcyue.juxu.clipboard", qos: .userInitiated)
    private var activeClipboardSuggestion: IOSClipboardSuggestion?
    private var inFlightClipboardChangeCount: Int?
    private var expandedIdleWords: [String]?
    private var hasAppeared = false

    private lazy var feedback = IOSKeyboardFeedbackController(attachingTo: keyCanvas)

    private struct VisualLayoutSignature: Equatable {
        let plane: IOSKeyPlane
        let shiftState: IOSShiftState
        let asciiMode: Bool
        let schemaId: UInt32
        let pinyinT9Layout: Bool
        let cangjieKeycapRadicals: Bool
        let wubiKeycapRadicals: Bool
        let swipeUpInputEnabled: Bool
        let isZLookupActive: Bool
        let showInlineGlobeKey: Bool
        let returnRole: IOSReturnKeyRole
        let isTrackpadActive: Bool
        let size: CGSize
    }

    public override func viewDidLoad() {
        super.viewDidLoad()
        setupViews()
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handlePasteboardChanged),
            name: UIPasteboard.changedNotification,
            object: nil
        )
        if #available(iOS 17.0, *) {
            registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: KeyboardViewController, _) in
                self.syncProxyTraits()
            }
        }
        syncEngineToUI()
    }

    public override var preferredScreenEdgesDeferringSystemGestures: UIRectEdge {
        .all
    }

    public override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        feedback.prepare()
        bridge.loadConfigAndUser(refreshPublishedLists: false)
        autoPairClosers.removeAll()
        autoSpaceTracker.invalidate()
        compositionBaseContext = nil
        clipboardGate.resetForKeyboardAppearance()
        applyDesiredTrayDimensions()
        syncContextFromProxyIfIdle()
        refreshClipboardSuggestion(allowAsyncRead: false)
        syncEngineToUI()
    }

    public override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        hasAppeared = true
        applyDesiredTrayDimensions()
        refreshKeyboardLayout()
        disableTouchDelayOnContainerGestures()
        // Trigger any uncached pasteboard read AFTER initial 4-row layout is on screen,
        // so an iOS "Allow Paste" system alert never freezes mid-layout!
        DispatchQueue.main.async { [weak self] in
            guard let self, self.hasAppeared else { return }
            self.refreshClipboardSuggestion(allowAsyncRead: true)
            self.syncEngineToUI()
        }
    }

    public override func updateViewConstraints() {
        super.updateViewConstraints()
        applyDesiredTrayDimensions()
    }

    public override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        clearLineGeneration &+= 1
        lineClearUndo.removeAll()
        backspaceTapDeferred = false
        hasAppeared = false
        stopBackspaceRepeat()
        keyCanvas.resetAllTouches()
        calloutView.hideImmediately()
        saveUserDataTimer?.invalidate()
        cancelMarkedCode()
        bridge.clearComposition()
        autoSpaceTracker.invalidate()
        compositionBaseContext = nil
        bridge.flushUserDataIfNeeded(refreshPublishedLists: false)
        feedback.suspend()
    }

    public override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        applyDesiredTrayDimensions()
        refreshKeyboardLayout()
    }

    private func applyDesiredTrayDimensions() {
        let width = view.bounds.width > 10 ? view.bounds.width : UIScreen.main.bounds.width
        let isLandscape = width > 540
        // 53pt candidate ribbon + 220pt 4-row keyboard (`keyH = 45pt`, `rowGap = 11pt`) = 273pt matching iOS 26 native height (`608..881pt`).
        let targetH: CGFloat = isLandscape ? 204 : 273
        let targetRibbonH: CGFloat = isLandscape
            ? IOSKeyboardLayoutEngine.landscapeCandidateBarHeight
            : IOSKeyboardLayoutEngine.candidateBarHeight
        var changed = false
        if heightConstraint?.constant != targetH {
            heightConstraint?.constant = targetH
            changed = true
        }
        if ribbonHeightConstraint?.constant != targetRibbonH {
            ribbonHeightConstraint?.constant = targetRibbonH
            changed = true
        }
        if changed {
            inputView?.setNeedsUpdateConstraints()
        }
    }

    public override func textDidChange(_ textInput: UITextInput?) {
        super.textDidChange(textInput)
        bridge.suppressEvalLogging = isSensitiveEvalProxy()
        syncContextFromProxyIfIdle()
        if markedCode.isEmpty {
            autoSpaceTracker.reconcileExternalContext(hostBefore: textDocumentProxy.documentContextBeforeInput)
        }
        refreshClipboardSuggestion(allowAsyncRead: hasAppeared)
        syncProxyTraits()
    }

    @objc private func handlePasteboardChanged() {
        refreshClipboardSuggestion(allowAsyncRead: hasAppeared)
        syncEngineToUI()
    }

    private func markUserTypedForClipboard() {
        clipboardGate.markUserTyped()
        activeClipboardSuggestion = nil
    }

    private func refreshClipboardSuggestion(allowAsyncRead: Bool) {
        guard self.hasFullAccess, bridge.prefs.clipboardSuggestion else {
            activeClipboardSuggestion = nil
            return
        }
        let pb = UIPasteboard.general
        let changeCount = pb.changeCount
        guard clipboardGate.shouldAllowDisplay(for: changeCount) else {
            activeClipboardSuggestion = nil
            return
        }
        let prefersOTP = (textDocumentProxy.textContentType == .oneTimeCode)
        let now = Date().timeIntervalSince1970
        let result = clipboardStore.lookup(
            hasFullAccess: true,
            enabled: true,
            hasStrings: pb.hasStrings,
            changeCount: changeCount,
            prefersOTP: prefersOTP,
            now: now
        )
        switch result {
        case .none:
            activeClipboardSuggestion = nil
        case .cached(let suggestion):
            activeClipboardSuggestion = suggestion
        case .needsRead(let changeCount):
            activeClipboardSuggestion = nil
            guard allowAsyncRead, inFlightClipboardChangeCount != changeCount else { return }
            inFlightClipboardChangeCount = changeCount
            clipboardReadQueue.async { [weak self] in
                // Perform blocking XPC pasteboard read off the main thread so an iOS system
                // "Allow Paste" modal alert never stalls UIKit keyboard layout or rendering.
                let fetched = UIPasteboard.general.string
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    if self.inFlightClipboardChangeCount == changeCount {
                        self.inFlightClipboardChangeCount = nil
                    }
                    let currentOTP = (self.textDocumentProxy.textContentType == .oneTimeCode)
                    let suggestion = self.clipboardStore.recordRead(
                        changeCount: changeCount,
                        rawText: fetched,
                        prefersOTP: currentOTP,
                        now: Date().timeIntervalSince1970
                    )
                    if UIPasteboard.general.changeCount == changeCount,
                       self.clipboardGate.shouldAllowDisplay(for: changeCount) {
                        self.activeClipboardSuggestion = suggestion
                        self.syncEngineToUI()
                    }
                }
            }
        }
    }

    private func setupViews() {
        guard let inputView = self.inputView else { return }
        inputView.allowsSelfSizing = true
        inputView.clipsToBounds = false
        view.clipsToBounds = false

        glassBackdrop.translatesAutoresizingMaskIntoConstraints = false
        glassBackdrop.layer.cornerRadius = IOSKeyboardLayoutEngine.trayTopCornerRadius
        glassBackdrop.layer.cornerCurve = .continuous
        glassBackdrop.layer.maskedCorners = [.layerMinXMinYCorner, .layerMaxXMinYCorner]
        glassBackdrop.isUserInteractionEnabled = false

        ribbonView.translatesAutoresizingMaskIntoConstraints = false
        ribbonView.accessibilityIdentifier = "JuxuRibbon"
        keyCanvas.translatesAutoresizingMaskIntoConstraints = false
        keyCanvas.accessibilityIdentifier = "JuxuKeyCanvas"
        expandedGrid.translatesAutoresizingMaskIntoConstraints = false

        expandedGrid.isHidden = true
        t9SidebarView.isHidden = true
        calloutView.isHidden = true

        inputView.addSubview(glassBackdrop)
        inputView.addSubview(keyCanvas)
        inputView.addSubview(t9SidebarView)
        inputView.addSubview(ribbonView)
        inputView.addSubview(expandedGrid)
        inputView.addSubview(calloutView)
        inputView.addSubview(alternatesView)

        let hc = inputView.heightAnchor.constraint(equalToConstant: 273)
        hc.priority = UILayoutPriority(999)
        self.heightConstraint = hc
        let rhc = ribbonView.heightAnchor.constraint(equalToConstant: IOSKeyboardLayoutEngine.candidateBarHeight)
        self.ribbonHeightConstraint = rhc

        NSLayoutConstraint.activate([
            hc,
            glassBackdrop.topAnchor.constraint(equalTo: inputView.topAnchor),
            glassBackdrop.leadingAnchor.constraint(equalTo: inputView.leadingAnchor),
            glassBackdrop.trailingAnchor.constraint(equalTo: inputView.trailingAnchor),
            glassBackdrop.bottomAnchor.constraint(equalTo: inputView.bottomAnchor),

            ribbonView.topAnchor.constraint(equalTo: inputView.topAnchor),
            ribbonView.leadingAnchor.constraint(equalTo: inputView.leadingAnchor),
            ribbonView.trailingAnchor.constraint(equalTo: inputView.trailingAnchor),
            rhc,

            keyCanvas.topAnchor.constraint(equalTo: ribbonView.bottomAnchor),
            keyCanvas.leadingAnchor.constraint(equalTo: inputView.leadingAnchor),
            keyCanvas.trailingAnchor.constraint(equalTo: inputView.trailingAnchor),
            keyCanvas.bottomAnchor.constraint(equalTo: inputView.bottomAnchor),

            expandedGrid.topAnchor.constraint(equalTo: ribbonView.bottomAnchor),
            expandedGrid.leadingAnchor.constraint(equalTo: inputView.leadingAnchor),
            expandedGrid.trailingAnchor.constraint(equalTo: inputView.trailingAnchor),
            expandedGrid.bottomAnchor.constraint(equalTo: inputView.bottomAnchor),
        ])

        wireCallbacks()
        syncProxyTraits()
    }

    private var lineClearUndo = IOSLineClearUndo()
    /// Text removed so far by the backspace press in progress.
    private var backspaceSessionDeleted = ""
    /// A tap's deletion waits for release while a swipe-down undo is possible.
    private var backspaceTapDeferred = false
    /// Closers auto-inserted by punctuation pairing that still sit right of the cursor, innermost last.
    private var autoPairClosers: [String] = []
    /// Text right of the cursor when the outermost tracked pair was inserted.
    private var autoPairTail = ""

    /// Bumped to abandon an in-flight line clear as soon as anything else happens.
    private var clearLineGeneration: UInt64 = 0

    private func presentCalloutIfAppropriate(for spec: IOSKeySpec) {
        guard !layoutEngine.isTrackpadActive else {
            calloutView.hideImmediately()
            keyCanvas.coveredCalloutKeyId = nil
            return
        }
        switch spec.kind {
        case .letter:
            let frameInRoot = keyCanvas.convert(spec.frame, to: inputView)
            let isDocked = calloutView.present(
                label: spec.primaryLabel,
                keyId: spec.id,
                over: frameInRoot,
                dark: isDarkAppearance
            )
            keyCanvas.coveredCalloutKeyId = isDocked ? spec.id : nil
        case .symbol(let sym) where sym.count <= 3:
            let frameInRoot = keyCanvas.convert(spec.frame, to: inputView)
            let isDocked = calloutView.present(
                label: spec.primaryLabel,
                keyId: spec.id,
                over: frameInRoot,
                dark: isDarkAppearance
            )
            keyCanvas.coveredCalloutKeyId = isDocked ? spec.id : nil
        default:
            calloutView.hideImmediately()
            keyCanvas.coveredCalloutKeyId = nil
        }
    }

    private func wireCallbacks() {
        keyCanvas.onKeyTouchDown = { [weak self] spec in
            guard let self else { return }
            self.clearLineGeneration &+= 1
            self.feedback.fireKeyDown()
            switch spec.kind {
            case .letter, .symbol:
                self.presentCalloutIfAppropriate(for: spec)
            case .planeSwitch(let plane, _):
                self.calloutView.hideImmediately()
                self.keyCanvas.coveredCalloutKeyId = nil
                self.planeBeforePress = self.layoutEngine.plane
                self.commitPendingFirstIfComposing()
                self.layoutEngine.switchPlane(plane)
                self.syncEngineToUI()
            case .backspace:
                self.calloutView.hideImmediately()
                self.keyCanvas.coveredCalloutKeyId = nil
                self.backspaceSessionDeleted = ""
                // While a clear can be undone here, wait for release so a swipe-down deletes nothing first.
                let canUndo = self.bridge.prefs.backspaceSwipeClear
                    && self.bridge.state.preedit.isEmpty
                    && self.lineClearUndo.restorableIndex(
                        before: self.textDocumentProxy.documentContextBeforeInput ?? "",
                        documentID: self.textDocumentProxy.documentIdentifier
                    ) != nil
                self.keyCanvas.canUndoLineClear = canUndo
                self.backspaceTapDeferred = canUndo
                if !canUndo {
                    self.performBackspace()
                }
                self.startBackspaceRepeat()
            default:
                self.calloutView.hideImmediately()
                self.keyCanvas.coveredCalloutKeyId = nil
            }
        }

        keyCanvas.onKeySlidePreview = { [weak self] spec in
            guard let self else { return }
            if let spec {
                self.feedback.fireSoftStep(playClick: false)
                self.presentCalloutIfAppropriate(for: spec)
            } else {
                self.calloutView.hideImmediately()
                self.keyCanvas.coveredCalloutKeyId = nil
            }
        }

        keyCanvas.onRestorePlane = { [weak self] in
            guard let self else { return }
            self.layoutEngine.switchPlane(self.planeBeforePress)
            self.refreshKeyboardLayout()
        }

        keyCanvas.onKeyTouchCancel = { [weak self] in
            self?.calloutView.hideImmediately()
            self?.keyCanvas.coveredCalloutKeyId = nil
            self?.stopBackspaceRepeat()
            self?.backspaceTapDeferred = false
        }

        keyCanvas.onKeyTouchUp = { [weak self] spec, rawPointInCanvas in
            guard let self else { return }
            self.keyCanvas.coveredCalloutKeyId = nil
            self.calloutView.releasePreview()
            self.stopBackspaceRepeat()
            if spec.kind == .backspace, self.backspaceTapDeferred {
                self.performBackspace()
            }
            self.handleKeyRelease(spec: spec, pointInCanvas: rawPointInCanvas)
        }

        keyCanvas.onKeySwipeUp = { [weak self] symbol in
            guard let self else { return }
            self.keyCanvas.coveredCalloutKeyId = nil
            self.calloutView.releasePreview()
            self.stopBackspaceRepeat()
            self.markUserTypedForClipboard()
            self.insertSymbol(symbol)
            self.layoutEngine.consumeOneShotShiftIfNeeded()
            self.syncEngineToUI()
        }

        keyCanvas.onBackspaceSwipeArmed = { [weak self] spec, armed in
            guard let self else { return }
            if armed {
                self.stopBackspaceRepeat()
                self.feedback.fireSoftStep(playClick: false)
                let frameInRoot = self.keyCanvas.convert(spec.frame, to: self.inputView)
                let isDocked = self.calloutView.present(
                    label: "清空",
                    keyId: spec.id,
                    over: frameInRoot,
                    dark: self.isDarkAppearance
                )
                self.keyCanvas.coveredCalloutKeyId = isDocked ? spec.id : nil
            } else {
                self.calloutView.hideImmediately()
                self.keyCanvas.coveredCalloutKeyId = nil
            }
        }

        keyCanvas.onBackspaceSwipeUp = { [weak self] in
            guard let self else { return }
            self.calloutView.hideImmediately()
            self.keyCanvas.coveredCalloutKeyId = nil
            self.stopBackspaceRepeat()
            self.feedback.fireSoftStep(playClick: true)
            self.backspaceTapDeferred = false
            self.clearCurrentLineBeforeCursor()
        }

        keyCanvas.onBackspaceSwipeDownArmed = { [weak self] spec, armed in
            guard let self else { return }
            if armed {
                self.stopBackspaceRepeat()
                self.feedback.fireSoftStep(playClick: false)
                let frameInRoot = self.keyCanvas.convert(spec.frame, to: self.inputView)
                let isDocked = self.calloutView.present(
                    label: "撤回",
                    keyId: spec.id,
                    over: frameInRoot,
                    dark: self.isDarkAppearance
                )
                self.keyCanvas.coveredCalloutKeyId = isDocked ? spec.id : nil
            } else {
                self.calloutView.hideImmediately()
                self.keyCanvas.coveredCalloutKeyId = nil
            }
        }

        keyCanvas.onBackspaceSwipeDown = { [weak self] in
            guard let self else { return }
            self.calloutView.hideImmediately()
            self.keyCanvas.coveredCalloutKeyId = nil
            self.stopBackspaceRepeat()
            self.backspaceTapDeferred = false
            self.feedback.fireSoftStep(playClick: true)
            self.restoreClearedLine()
        }

        keyCanvas.onAlternatesBegin = { [weak self] spec in
            guard let self, case .letter(let ascii) = spec.kind else { return }
            self.calloutView.hideImmediately()
            self.keyCanvas.coveredCalloutKeyId = nil
            self.feedback.fireSoftStep(playClick: false)
            let options = IOSKeyAlternates.options(for: ascii, asciiMode: self.layoutEngine.asciiMode)
            self.alternatesView.present(
                items: options.items,
                defaultIndex: options.defaultIndex,
                over: self.keyCanvas.convert(spec.frame, to: self.inputView),
                containerWidth: self.inputView?.bounds.width ?? self.keyCanvas.bounds.width,
                dark: self.isDarkAppearance
            )
        }

        keyCanvas.onAlternatesMove = { [weak self] point in
            guard let self else { return }
            let x = self.keyCanvas.convert(point, to: self.inputView).x
            if self.alternatesView.update(xInSuperview: x) {
                self.feedback.fireSoftStep(playClick: false)
            }
        }

        keyCanvas.onAlternatesEnd = { [weak self] commit in
            guard let self else { return }
            let text = self.alternatesView.selectedText
            self.alternatesView.hide()
            self.stopBackspaceRepeat()
            guard commit, let text else { return }
            self.markUserTypedForClipboard()
            self.insertSymbol(text)
            self.layoutEngine.consumeOneShotShiftIfNeeded()
            self.syncEngineToUI()
        }

        keyCanvas.onTrackpadStateChanged = { [weak self] active in
            guard let self else { return }
            self.calloutView.hideImmediately()
            self.keyCanvas.coveredCalloutKeyId = nil
            self.layoutEngine.isTrackpadActive = active
            if active {
                self.commitPendingFirstIfComposing()
                self.syncEngineToUI()
                self.feedback.fireSoftStep(playClick: false)
            }
            self.refreshKeyboardLayout()
        }

        keyCanvas.onTrackpadMove = { [weak self] dxSteps, dySteps in
            guard let self, self.bridge.state.preedit.isEmpty else { return }
            var moved = false
            if dxSteps != 0 {
                self.textDocumentProxy.adjustTextPosition(byCharacterOffset: dxSteps)
                moved = true
            }
            if dySteps != 0 {
                if self.moveCursorVertically(lines: dySteps) {
                    moved = true
                }
            }
            if moved {
                self.autoPairClosers.removeAll()
                self.autoSpaceTracker.invalidate()
                self.compositionBaseContext = nil
                self.feedback.fireSoftStep(playClick: false)
                self.syncContextFromProxyIfIdle()
                self.syncEngineToUI()
            }
        }

        ribbonView.onSelectCandidate = { [weak self] relIdx in
            guard let self else { return }
            self.keyCanvas.flushPendingTouches()
            self.feedback.fireSoftStep(playClick: true)
            self.markUserTypedForClipboard()
            let out = self.bridge.selectCandidate(pageRelIndex: relIdx, timestampMs: Self.nowMs())
            self.applyEngineState(out)
        }

        ribbonView.onSelectStarterWord = { [weak self] word in
            guard let self else { return }
            self.keyCanvas.flushPendingTouches()
            self.feedback.fireSoftStep(playClick: true)
            self.markUserTypedForClipboard()
            self.insertTextWithAutoSpace(word, splitInternal: true)
            self.bridge.acceptPrediction(word, timestampMs: Self.nowMs())
            self.scheduleUserDataFlush()
            self.syncEngineToUI()
        }

        ribbonView.onSelectClipboard = { [weak self] suggestion in
            guard let self else { return }
            self.keyCanvas.flushPendingTouches()
            self.feedback.fireSoftStep(playClick: true)
            self.markUserTypedForClipboard()
            self.insertTextWithAutoSpace(suggestion.insertText, splitInternal: false)
            self.clipboardStore.consume(changeCount: suggestion.changeCount)
            self.syncEngineToUI()
        }

        ribbonView.onDismissClipboard = { [weak self] suggestion in
            guard let self else { return }
            self.keyCanvas.flushPendingTouches()
            self.feedback.fireSoftStep(playClick: false)
            self.clipboardStore.dismiss(changeCount: suggestion.changeCount)
            self.activeClipboardSuggestion = nil
            self.syncEngineToUI()
        }

        ribbonView.onTapChevron = { [weak self] in
            guard let self else { return }
            self.keyCanvas.flushPendingTouches()
            self.feedback.fireSoftStep(playClick: true)
            self.expandedGrid.isHidden.toggle()
            let isExp = !self.expandedGrid.isHidden
            self.keyCanvas.isHidden = isExp
            self.updateT9SidebarVisibility()
            if isExp {
                self.reloadExpandedGrid()
            } else {
                self.expandedIdleWords = nil
            }
            self.ribbonView.setExpanded(isExp)
        }

        t9SidebarView.onSelectPunctuation = { [weak self] sym in
            guard let self else { return }
            self.keyCanvas.flushPendingTouches()
            self.feedback.fireSoftStep(playClick: true)
            self.markUserTypedForClipboard()
            self.insertSymbol(sym)
            self.syncEngineToUI()
        }

        t9SidebarView.onSelectSyllable = { [weak self] syl in
            guard let self else { return }
            self.keyCanvas.flushPendingTouches()
            self.feedback.fireSoftStep(playClick: true)
            let out = self.bridge.lockT9Syllable(syl)
            self.applyEngineState(out)
        }

        expandedGrid.onSelectAbsCandidate = { [weak self] absIdx in
            guard let self else { return }
            self.feedback.fireSoftStep(playClick: true)
            self.markUserTypedForClipboard()
            let idleWords = self.expandedIdleWords
            self.expandedIdleWords = nil
            self.expandedGrid.isHidden = true
            self.keyCanvas.isHidden = false
            self.updateT9SidebarVisibility()
            self.ribbonView.setExpanded(false)
            if let idleWords, absIdx < idleWords.count {
                let word = idleWords[absIdx]
                self.insertTextWithAutoSpace(word, splitInternal: true)
                self.bridge.acceptPrediction(word, timestampMs: Self.nowMs())
                self.scheduleUserDataFlush()
                self.syncEngineToUI()
            } else {
                let out = self.bridge.selectAbsCandidate(absIndex: absIdx, timestampMs: Self.nowMs())
                self.applyEngineState(out)
            }
        }

        expandedGrid.onCollapse = { [weak self] in
            self?.feedback.fireSoftStep(playClick: true)
            self?.expandedIdleWords = nil
            self?.expandedGrid.isHidden = true
            self?.keyCanvas.isHidden = false
            self?.updateT9SidebarVisibility()
            self?.ribbonView.setExpanded(false)
        }

        expandedGrid.onBackspace = { [weak self] in
            guard let self else { return }
            let wasComposing = !self.bridge.state.preedit.isEmpty
            self.feedback.fireKeyDown()
            self.performBackspace()
            if !self.expandedGrid.isHidden {
                if wasComposing && self.bridge.state.preedit.isEmpty {
                    self.expandedIdleWords = nil
                    self.expandedGrid.isHidden = true
                    self.keyCanvas.isHidden = false
                    self.ribbonView.setExpanded(false)
                } else {
                    self.reloadExpandedGrid()
                }
            }
        }

        expandedGrid.onClear = { [weak self] in
            self?.feedback.fireSoftStep(playClick: true)
            self?.compositionBaseContext = nil
            self?.cancelMarkedCode()
            self?.bridge.abandonComposition(timestampMs: Self.nowMs())
            self?.expandedIdleWords = nil
            self?.expandedGrid.isHidden = true
            self?.keyCanvas.isHidden = false
            self?.updateT9SidebarVisibility()
            self?.ribbonView.setExpanded(false)
            self?.syncEngineToUI()
        }

        expandedGrid.onOpenSettings = { [weak self] in
            guard let self else { return }
            self.feedback.fireSoftStep(playClick: true)
            self.expandedIdleWords = nil
            self.expandedGrid.isHidden = true
            self.keyCanvas.isHidden = false
            self.updateT9SidebarVisibility()
            self.ribbonView.setExpanded(false)
            self.openHostAppSettings()
        }
    }

    private func reloadExpandedGrid() {
        if bridge.state.preedit.isEmpty {
            let predicted = bridge.state.asciiMode ? [] : bridge.predictNextWords(maxCount: 24)
            let words = predicted.isEmpty && !bridge.state.asciiMode
                ? IOSCandidateRibbonView.starterWords
                : predicted
            expandedIdleWords = words
            let items = words.enumerated().map { idx, text in
                JuxuCandidateItem(
                    index: idx + 1,
                    text: text,
                    comment: "",
                    badge: 0,
                    confidence: 1.0,
                    isExact: true
                )
            }
            expandedGrid.reload(candidates: items, dark: isDarkAppearance)
        } else {
            expandedIdleWords = nil
            expandedGrid.reload(candidates: bridge.allCandidates(maxCount: 300), dark: isDarkAppearance)
        }
    }

    private func openHostAppSettings() {
        guard let url = URL(string: "juxu://settings") else { return }
        var responder: UIResponder? = self
        while let current = responder {
            if let app = current as? UIApplication {
                app.open(url, options: [:], completionHandler: nil)
                return
            }
            responder = current.next
        }
        extensionContext?.open(url, completionHandler: nil)
    }

    private func handleKeyRelease(spec: IOSKeySpec, pointInCanvas: CGPoint) {
        switch spec.kind {
        case .letter(let ascii):
            markUserTypedForClipboard()
            if layoutEngine.asciiMode {
                commitPendingFirstIfComposing()
                let ch = String(UnicodeScalar(ascii))
                let outStr = layoutEngine.shiftState.isUppercase ? ch.uppercased() : ch.lowercased()
                insertTextWithAutoSpace(outStr, splitInternal: false)
                bridge.setContext("")
                layoutEngine.consumeOneShotShiftIfNeeded()
                syncEngineToUI()
            } else if layoutEngine.shiftState.isUppercase {
                // Inline uppercase English anchor: route through Core so mixed sentences (e.g. `woyongRust`)
                // and Wubi English words (`Rust`, `Juxu`) compose without clearing neural context!
                let upperScalar = String(UnicodeScalar(ascii)).uppercased().unicodeScalars.first?.value ?? UInt32(ascii)
                let out = bridge.handleKey(keyCode: 0, scalar: upperScalar, hasCmdOrCtrl: false, timestampMs: Self.nowMs())
                layoutEngine.consumeOneShotShiftIfNeeded()
                applyEngineState(out)
            } else {
                let sendPt: CGPoint
                if let snap = keyCanvas.snapshot {
                    sendPt = IOSKeyboardLayoutEngine.compensatedLetterPoint(pointInCanvas, in: snap)
                } else {
                    sendPt = pointInCanvas
                }
                let out = bridge.pushTouch(
                    reportedAscii: ascii,
                    x: Float(sendPt.x),
                    y: Float(sendPt.y),
                    isNormalized: false,
                    timestampMs: Self.nowMs()
                )
                applyEngineState(out)
            }

        case .t9Key(let digit):
            markUserTypedForClipboard()
            if digit == UInt8(ascii: "1") {
                if !bridge.state.preedit.isEmpty {
                    let out = bridge.pushT9Touch(
                        reportedDigit: UInt8(ascii: "'"),
                        x: 0,
                        y: 0,
                        isNormalized: true,
                        timestampMs: Self.nowMs()
                    )
                    applyEngineState(out)
                }
            } else {
                let sendPt: CGPoint
                if let snap = keyCanvas.snapshot {
                    sendPt = IOSKeyboardLayoutEngine.compensatedLetterPoint(pointInCanvas, in: snap)
                } else {
                    sendPt = pointInCanvas
                }
                let out = bridge.pushT9Touch(
                    reportedDigit: digit,
                    x: Float(sendPt.x),
                    y: Float(sendPt.y),
                    isNormalized: false,
                    timestampMs: Self.nowMs()
                )
                applyEngineState(out)
            }

        case .t9Clear:
            markUserTypedForClipboard()
            if !bridge.state.preedit.isEmpty {
                compositionBaseContext = nil
                cancelMarkedCode()
                bridge.clearComposition()
                syncEngineToUI()
            }

        case .symbol(let sym):
            markUserTypedForClipboard()
            insertSymbol(sym)
            if IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol(sym, on: layoutEngine.plane) {
                layoutEngine.switchPlane(.letters)
            }
            syncEngineToUI()

        case .shift:
            layoutEngine.tapShift(at: ProcessInfo.processInfo.systemUptime)
            refreshKeyboardLayout()

        case .backspace:
            break

        case .planeSwitch(let targetPlane, _):
            commitPendingFirstIfComposing()
            layoutEngine.switchPlane(targetPlane)
            refreshKeyboardLayout()

        case .globe:
            commitPendingFirstIfComposing()
            advanceToNextInputMode()

        case .modeToggle:
            let out = bridge.toggleAsciiMode()
            if layoutEngine.plane != .letters {
                layoutEngine.switchPlane(.letters)
            }
            applyEngineState(out)

        case .space:
            markUserTypedForClipboard()
            if bridge.state.preedit.isEmpty {
                autoSpaceTracker.recordPassthrough(
                    " ",
                    hostBefore: textDocumentProxy.documentContextBeforeInput,
                    hostBeforeIsAuthoritative: false
                )
                textDocumentProxy.insertText(" ")
                bridge.setContext("")
                syncEngineToUI()
            } else {
                let out = bridge.handleKey(keyCode: 0x31, scalar: 32, hasCmdOrCtrl: false, timestampMs: Self.nowMs())
                applyEngineState(out)
            }

        case .returnKey:
            markUserTypedForClipboard()
            if bridge.state.preedit.isEmpty {
                autoSpaceTracker.recordPassthrough(
                    "\n",
                    hostBefore: textDocumentProxy.documentContextBeforeInput,
                    hostBeforeIsAuthoritative: false
                )
                textDocumentProxy.insertText("\n")
                bridge.setContext("")
                syncEngineToUI()
            } else {
                let out = bridge.handleKey(keyCode: 0x24, scalar: 13, hasCmdOrCtrl: false, timestampMs: Self.nowMs())
                applyEngineState(out)
            }
        }
    }

    private var isPunctuationPairingActive: Bool {
        bridge.prefs.smartPunctuation && !layoutEngine.asciiMode
    }

    private func insertSymbol(_ sym: String) {
        commitPendingFirstIfComposing()
        var pairCloser: String?
        if isPunctuationPairingActive {
            let after = textDocumentProxy.documentContextAfterInput
            refreshAutoPairs(after: after)
            switch IOSPunctuationPairing.action(for: sym, after: after, openClosers: autoPairClosers) {
            case .skipClose:
                textDocumentProxy.adjustTextPosition(byCharacterOffset: 1)
                autoPairClosers.removeLast()
                autoSpaceTracker.invalidate()
                bridge.setContext("")
                return
            case .insertPair(let close):
                pairCloser = close
                if autoPairClosers.isEmpty {
                    autoPairTail = after ?? ""
                }
            case .plain:
                break
            }
        } else {
            autoPairClosers.removeAll()
        }
        if AutoSpaceFormatter.isAutoSpaceRightTrigger(sym, options: AutoSpaceOptions(prefs: bridge.prefs)) {
            insertTextWithAutoSpace(sym, splitInternal: false)
        } else {
            autoSpaceTracker.recordPassthrough(
                sym,
                hostBefore: textDocumentProxy.documentContextBeforeInput,
                hostBeforeIsAuthoritative: false
            )
            textDocumentProxy.insertText(sym)
        }
        if let pairCloser {
            textDocumentProxy.insertText(pairCloser)
            textDocumentProxy.adjustTextPosition(byCharacterOffset: -1)
            autoPairClosers.append(pairCloser)
            autoSpaceTracker.invalidate()
        }
        bridge.setContext("")
    }

    /// Forgets tracked pairs once the cursor has been moved away from them.
    private func refreshAutoPairs(after: String?) {
        autoPairClosers = IOSPunctuationPairing.pruned(autoPairClosers, after: after)
        if !IOSPunctuationPairing.cursorStaysInPairs(autoPairClosers, tail: autoPairTail, after: after) {
            autoPairClosers.removeAll()
        }
    }

    /// Remembers what the current backspace press already removed, so a swipe can account for it.
    private func noteBackspaceDeleted(_ text: String) {
        backspaceSessionDeleted = text + backspaceSessionDeleted
    }

    /// Backspace swipe-down: puts back the most recent clear made at this cursor position.
    private func restoreClearedLine() {
        guard bridge.state.preedit.isEmpty,
              let text = lineClearUndo.take(
                  before: textDocumentProxy.documentContextBeforeInput ?? "",
                  documentID: textDocumentProxy.documentIdentifier
              ) else { return }
        clearLineGeneration &+= 1
        autoPairClosers.removeAll()
        autoSpaceTracker.invalidate()
        textDocumentProxy.insertText(text)
        bridge.setContext(String(text.suffix(16)))
        syncEngineToUI()
    }

    /// Backspace swipe-up: drops an active composition, otherwise deletes leftward to the start of the line.
    /// The proxy only exposes a window of context, so long lines are deleted in verified batches.
    private func clearCurrentLineBeforeCursor() {
        markUserTypedForClipboard()
        if !bridge.state.preedit.isEmpty {
            compositionBaseContext = nil
            cancelMarkedCode()
            bridge.clearComposition()
            syncEngineToUI()
            return
        }
        autoPairClosers.removeAll()
        autoSpaceTracker.invalidate()
        // Touch-down at a line start removed only the line break: put it back instead of joining lines.
        // After a longer hold-delete, the text already removed stays removed.
        if backspaceSessionDeleted.count == 1, backspaceSessionDeleted.first?.isNewline == true {
            textDocumentProxy.insertText(backspaceSessionDeleted)
            backspaceSessionDeleted = ""
            bridge.setContext("")
            syncEngineToUI()
            return
        }
        clearLineGeneration &+= 1
        clearLineStep(
            generation: clearLineGeneration,
            deleted: 0,
            waitedTicks: 0,
            expected: nil,
            collected: backspaceSessionDeleted,
            remaining: textDocumentProxy.documentContextBeforeInput ?? ""
        )
        backspaceSessionDeleted = ""
    }

    private func finishLineClear(collected: String, remaining: String) {
        lineClearUndo.push(
            text: collected,
            contextBefore: remaining,
            documentID: textDocumentProxy.documentIdentifier
        )
        bridge.setContext(String(remaining.suffix(32)))
        syncEngineToUI()
    }

    private func clearLineStep(
        generation: UInt64,
        deleted: Int,
        waitedTicks: Int,
        expected: IOSLineClear.Expected?,
        collected: String,
        remaining: String
    ) {
        guard generation == clearLineGeneration else { return }
        let before = textDocumentProxy.documentContextBeforeInput ?? ""
        guard !before.isEmpty, deleted < 50_000 else {
            finishLineClear(collected: collected, remaining: before.isEmpty ? "" : remaining)
            return
        }
        // Never delete from a stale context: an old snapshot could cross the line break or another field.
        if let expected, !expected.isSatisfied(by: before) {
            guard waitedTicks < 8 else {
                finishLineClear(collected: collected, remaining: remaining)
                return
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.024) { [weak self] in
                self?.clearLineStep(
                    generation: generation,
                    deleted: deleted,
                    waitedTicks: waitedTicks + 1,
                    expected: expected,
                    collected: collected,
                    remaining: remaining
                )
            }
            return
        }
        let plan = IOSLineClear.plan(before: before, maxBatch: 128)
        let nowCollected = String(before.suffix(plan.deleteCount)) + collected
        let nowRemaining = String(before.dropLast(plan.deleteCount))
        for _ in 0..<plan.deleteCount {
            textDocumentProxy.deleteBackward()
        }
        guard !plan.reachesLineStart else {
            finishLineClear(collected: nowCollected, remaining: nowRemaining)
            return
        }
        let next = IOSLineClear.Expected(previous: before, removed: plan.deleteCount)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.024) { [weak self] in
            self?.clearLineStep(
                generation: generation,
                deleted: deleted + plan.deleteCount,
                waitedTicks: 0,
                expected: next,
                collected: nowCollected,
                remaining: nowRemaining
            )
        }
    }

    private func commitPendingFirstIfComposing() {
        guard !bridge.state.preedit.isEmpty else { return }
        let rawText = bridge.state.candidates.first?.text ?? bridge.state.preedit
        insertTextWithAutoSpace(rawText, splitInternal: true, overrideBefore: compositionBaseContext)
        compositionBaseContext = nil
        markedCode = ""
        bridge.clearComposition()
    }

    private func performBackspace(acceleratedChunk: Bool = false) {
        backspaceTapDeferred = false
        markUserTypedForClipboard()
        if !bridge.state.preedit.isEmpty {
            let out = bridge.handleKey(keyCode: 0x33, scalar: 0, hasCmdOrCtrl: false, timestampMs: Self.nowMs())
            applyEngineState(out)
            return
        }
        if isPunctuationPairingActive, !autoPairClosers.isEmpty {
            let hostBefore = textDocumentProxy.documentContextBeforeInput
            let hostAfter = textDocumentProxy.documentContextAfterInput
            refreshAutoPairs(after: hostAfter)
            if IOSPunctuationPairing.shouldDeletePair(before: hostBefore, after: hostAfter, openClosers: autoPairClosers) {
                noteBackspaceDeleted(String((hostBefore ?? "").suffix(1)) + (autoPairClosers.last ?? ""))
                textDocumentProxy.adjustTextPosition(byCharacterOffset: 1)
                textDocumentProxy.deleteBackward()
                textDocumentProxy.deleteBackward()
                autoPairClosers.removeLast()
                autoSpaceTracker.invalidate()
                bridge.setContext(String((hostBefore ?? "").dropLast().suffix(16)))
                syncEngineToUI()
                return
            }
        }
        guard acceleratedChunk, let before = textDocumentProxy.documentContextBeforeInput, !before.isEmpty else {
            let before = textDocumentProxy.documentContextBeforeInput ?? ""
            if !before.isEmpty {
                bridge.notifyPostCommitBackspace(timestampMs: Self.nowMs())
            }
            autoSpaceTracker.willDeleteBackward(
                hostBefore: before,
                hostBeforeIsAuthoritative: false
            )
            noteBackspaceDeleted(String(before.suffix(1)))
            textDocumentProxy.deleteBackward()
            let afterDel = before.isEmpty ? "" : String(before.dropLast())
            bridge.setContext(String(afterDel.suffix(16)))
            syncEngineToUI()
            return
        }
        autoSpaceTracker.invalidate()
        // Native-style accelerated hold-delete: erase up to 3 characters without crossing a newline in one jump
        if before.last == "\n" {
            noteBackspaceDeleted("\n")
            textDocumentProxy.deleteBackward()
            let afterDel = String(before.dropLast())
            bridge.setContext(String(afterDel.suffix(16)))
            syncEngineToUI()
            return
        }
        var count = 0
        for ch in before.reversed() {
            if ch == "\n" || count >= 3 { break }
            count += 1
            if count >= 2 && (ch == " " || ch == "，" || ch == "。" || ch == "," || ch == ".") {
                break
            }
        }
        let nDel = max(1, count)
        noteBackspaceDeleted(String(before.suffix(nDel)))
        for _ in 0..<nDel {
            textDocumentProxy.deleteBackward()
        }
        let afterDel = String(before.dropLast(min(nDel, before.count)))
        bridge.setContext(String(afterDel.suffix(16)))
        syncEngineToUI()
    }

    private func startBackspaceRepeat() {
        stopBackspaceRepeat()
        backspaceHoldStart = ProcessInfo.processInfo.systemUptime
        let timer = Timer(timeInterval: 0.068, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                let held = ProcessInfo.processInfo.systemUptime - self.backspaceHoldStart
                guard held >= 0.36 else { return }
                let fastMode = (held >= 1.55) && self.bridge.state.preedit.isEmpty
                self.feedback.fireSoftStep(playClick: true)
                self.performBackspace(acceleratedChunk: fastMode)
            }
        }
        backspaceTimer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    private func stopBackspaceRepeat() {
        backspaceTimer?.invalidate()
        backspaceTimer = nil
    }

    /// Move cursor vertically across lines in multi-line text views using `UITextDocumentProxy` context.
    private func moveCursorVertically(lines: Int) -> Bool {
        guard lines != 0 else { return false }
        let before = textDocumentProxy.documentContextBeforeInput ?? ""
        let after = textDocumentProxy.documentContextAfterInput ?? ""

        // Current column from most recent newline in `before`
        let currentCol: Int
        if let lastNL = before.lastIndex(of: "\n") {
            currentCol = before.distance(from: before.index(after: lastNL), to: before.endIndex)
        } else {
            currentCol = before.count
        }

        if lines < 0 {
            guard let lastNL = before.lastIndex(of: "\n") else {
                if !before.isEmpty {
                    textDocumentProxy.adjustTextPosition(byCharacterOffset: -before.count)
                    return true
                }
                return false
            }
            let prevPrefix = before[..<lastNL]
            let prevLineStart = prevPrefix.lastIndex(of: "\n").map { prevPrefix.index(after: $0) } ?? prevPrefix.startIndex
            let prevLineLen = prevPrefix.distance(from: prevLineStart, to: prevPrefix.endIndex)
            let targetCol = min(currentCol, prevLineLen)
            let delta = -(currentCol + 1 + (prevLineLen - targetCol))
            textDocumentProxy.adjustTextPosition(byCharacterOffset: delta)
            return true
        } else {
            guard let nextNL = after.firstIndex(of: "\n") else {
                if !after.isEmpty {
                    textDocumentProxy.adjustTextPosition(byCharacterOffset: after.count)
                    return true
                }
                return false
            }
            let toEndOfCurLine = after.distance(from: after.startIndex, to: nextNL)
            let rem = after[after.index(after: nextNL)...]
            let nextLineEnd = rem.firstIndex(of: "\n") ?? rem.endIndex
            let nextLineLen = rem.distance(from: rem.startIndex, to: nextLineEnd)
            let targetCol = min(currentCol, nextLineLen)
            let delta = toEndOfCurLine + 1 + targetCol
            textDocumentProxy.adjustTextPosition(byCharacterOffset: delta)
            return true
        }
    }

    private func cancelMarkedCode() {
        guard !markedCode.isEmpty else { return }
        textDocumentProxy.setMarkedText("", selectedRange: NSRange(location: 0, length: 0))
        textDocumentProxy.unmarkText()
        markedCode = ""
    }

    private func scheduleUserDataFlush() {
        saveUserDataTimer?.invalidate()
        saveUserDataTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.bridge.flushUserDataIfNeeded(refreshPublishedLists: false) }
        }
    }

    private func syncContextFromProxyIfIdle() {
        guard bridge.state.preedit.isEmpty else { return }
        let before = textDocumentProxy.documentContextBeforeInput ?? ""
        bridge.setContext(String(before.suffix(32)))
    }

    private func currentIdlePredictions(for st: JuxuActionState) -> [String]? {
        guard st.preedit.isEmpty else { return nil }
        if st.asciiMode {
            return []
        }
        return bridge.predictNextWords(maxCount: 8)
    }

    private func isSensitiveEvalProxy() -> Bool {
        if textDocumentProxy.isSecureTextEntry == true { return true }
        if let ct = textDocumentProxy.textContentType {
            switch ct {
            case .password, .newPassword, .oneTimeCode, .creditCardNumber, .telephoneNumber, .emailAddress, .URL, .username:
                return true
            default:
                break
            }
        }
        switch textDocumentProxy.keyboardType {
        case .numberPad, .phonePad, .decimalPad, .asciiCapableNumberPad, .namePhonePad, .emailAddress, .URL:
            return true
        default:
            return false
        }
    }

    private func isAutoSpacingAllowedForProxy() -> Bool {
        if textDocumentProxy.isSecureTextEntry == true { return false }
        if textDocumentProxy.textContentType == .oneTimeCode { return false }
        switch textDocumentProxy.keyboardType {
        case .URL, .emailAddress, .numberPad, .phonePad, .decimalPad, .asciiCapableNumberPad:
            return false
        default:
            return true
        }
    }

    private func insertTextWithAutoSpace(
        _ text: String,
        splitInternal: Bool,
        overrideBefore: String? = nil
    ) {
        guard !text.isEmpty else { return }
        let options = isAutoSpacingAllowedForProxy()
            ? AutoSpaceOptions(prefs: bridge.prefs)
            : AutoSpaceOptions(enabled: false, spaceDigits: false)
        let formatted = autoSpaceTracker.prepareCommit(
            text,
            hostBefore: textDocumentProxy.documentContextBeforeInput,
            hostBeforeIsAuthoritative: false,
            overrideBefore: overrideBefore,
            splitInternal: splitInternal,
            options: options
        )
        textDocumentProxy.insertText(formatted)
    }

    private func applyEngineState(_ out: JuxuActionState) {
        if !out.commitText.isEmpty {
            let base = markedCode.isEmpty ? nil : compositionBaseContext
            insertTextWithAutoSpace(out.commitText, splitInternal: true, overrideBefore: base)
            markedCode = ""
            scheduleUserDataFlush()
        }
        let inlinePreedit = out.activeInlinePreedit
        if inlinePreedit.isEmpty {
            compositionBaseContext = nil
            cancelMarkedCode()
        } else if inlinePreedit != markedCode {
            if markedCode.isEmpty {
                compositionBaseContext = autoSpaceTracker.effectiveBefore(
                    hostBefore: textDocumentProxy.documentContextBeforeInput,
                    hostBeforeIsAuthoritative: false
                )
            }
            textDocumentProxy.setMarkedText(inlinePreedit, selectedRange: NSRange(location: inlinePreedit.utf16.count, length: 0))
            markedCode = inlinePreedit
        }
        if out.preedit.isEmpty && !expandedGrid.isHidden {
            expandedGrid.isHidden = true
            keyCanvas.isHidden = false
            updateT9SidebarVisibility()
            ribbonView.setExpanded(false)
        }
        syncEngineToUI()
    }

    private func syncEngineToUI() {
        let st = bridge.state
        if st.preedit.isEmpty && activeClipboardSuggestion != nil {
            refreshClipboardSuggestion(allowAsyncRead: false)
        }
        layoutEngine.asciiMode = st.asciiMode
        layoutEngine.schemaId = st.schemaId
        layoutEngine.pinyinT9Layout = bridge.prefs.pinyinT9Layout
        layoutEngine.cangjieKeycapRadicals = bridge.prefs.cangjieKeycapRadicals
        layoutEngine.wubiKeycapRadicals = bridge.prefs.wubiKeycapRadicals
        layoutEngine.swipeUpInputEnabled = bridge.prefs.keySwipeUpInput
        keyCanvas.longPressAlternatesEnabled = bridge.prefs.keyLongPressAlternates
        keyCanvas.backspaceSwipeClearEnabled = bridge.prefs.backspaceSwipeClear
        layoutEngine.isZLookupActive = bridge.prefs.isCangjieFamily
            && bridge.prefs.zOmnipotent
            && st.preedit.hasPrefix("z")
        layoutEngine.isComposing = !st.preedit.isEmpty
        layoutEngine.validNextMask = st.validNextMask
        layoutEngine.nextKeyPriors = st.nextKeyPriors
        layoutEngine.touchOffsets = st.touchOffsets
        layoutEngine.t9ValidNextMask = st.t9ValidNextMask
        layoutEngine.t9NextKeyPriors = st.t9NextKeyPriors
        layoutEngine.t9TouchOffsets = st.t9TouchOffsets
        keyCanvas.validNextMask = st.validNextMask
        keyCanvas.nextKeyPriors = st.nextKeyPriors
        keyCanvas.touchOffsets = st.touchOffsets
        keyCanvas.t9ValidNextMask = st.t9ValidNextMask
        keyCanvas.t9NextKeyPriors = st.t9NextKeyPriors
        keyCanvas.t9TouchOffsets = st.t9TouchOffsets
        t9SidebarView.update(
            syllables: st.t9Syllables,
            composing: !st.preedit.isEmpty,
            dark: isDarkAppearance
        )
        ribbonView.update(
            state: st,
            dark: isDarkAppearance,
            clipboardSuggestion: activeClipboardSuggestion,
            idleWords: currentIdlePredictions(for: st)
        )
        refreshKeyboardLayout()
    }

    private func syncProxyTraits() {
        layoutEngine.showInlineGlobeKey = self.needsInputModeSwitchKey
        layoutEngine.returnRole = Self.mapReturnKeyType(textDocumentProxy.returnKeyType)
        let dark = isDarkAppearance

        glassBackdrop.backgroundColor = .clear
        glassBackdrop.layer.borderWidth = 0

        keyCanvas.applyTheme(dark: dark)
        let st = bridge.state
        ribbonView.update(
            state: st,
            dark: dark,
            clipboardSuggestion: activeClipboardSuggestion,
            idleWords: currentIdlePredictions(for: st)
        )
        refreshKeyboardLayout()
    }

    private func updateT9SidebarVisibility() {
        let shouldShow = (keyCanvas.snapshot?.isT9Layout == true)
            && !layoutEngine.isTrackpadActive
            && expandedGrid.isHidden
            && !keyCanvas.isHidden
        t9SidebarView.isHidden = !shouldShow
        if shouldShow, let snap = keyCanvas.snapshot {
            let f = keyCanvas.convert(snap.t9SidebarFrame, to: inputView)
            if t9SidebarView.frame != f {
                t9SidebarView.frame = f
            }
        }
    }

    private func refreshKeyboardLayout() {
        guard keyCanvas.bounds.width > 10, keyCanvas.bounds.height > 10 else { return }
        let sig = VisualLayoutSignature(
            plane: layoutEngine.plane,
            shiftState: layoutEngine.shiftState,
            asciiMode: layoutEngine.asciiMode,
            schemaId: layoutEngine.schemaId,
            pinyinT9Layout: layoutEngine.pinyinT9Layout,
            cangjieKeycapRadicals: layoutEngine.cangjieKeycapRadicals,
            wubiKeycapRadicals: layoutEngine.wubiKeycapRadicals,
            swipeUpInputEnabled: layoutEngine.swipeUpInputEnabled,
            isZLookupActive: layoutEngine.isZLookupActive,
            showInlineGlobeKey: layoutEngine.showInlineGlobeKey,
            returnRole: layoutEngine.returnRole,
            isTrackpadActive: layoutEngine.isTrackpadActive,
            size: keyCanvas.bounds.size
        )
        if lastVisualKeySignature == sig {
            updateT9SidebarVisibility()
            return
        }
        lastVisualKeySignature = sig

        let snap = layoutEngine.computeLayout(in: keyCanvas.bounds.size)
        keyCanvas.updateSnapshot(snap)
        if snap.isT9Layout {
            bridge.setT9Layout(
                centers16: snap.t9Centers16,
                originX: snap.t9OriginX,
                originY: snap.t9OriginY,
                pitchX: snap.t9PitchX,
                pitchY: snap.t9PitchY
            )
        } else {
            bridge.setKeyLayout(
                centers52: snap.centers52,
                originX: snap.originX,
                originY: snap.originY,
                pitchX: snap.pitchX,
                pitchY: snap.pitchY
            )
        }
        updateT9SidebarVisibility()
    }

    /// Ensure no ancestor container gesture recognizer inside our extension window holds `delaysTouchesBegan`,
    /// so brief light grazing taps (`10-25ms`) reach `keyCanvas.touchesBegan` immediately without lag or drop.
    private func disableTouchDelayOnContainerGestures() {
        var cur: UIView? = inputView ?? view
        while let v = cur {
            if let recognizers = v.gestureRecognizers {
                for gr in recognizers where gr.delaysTouchesBegan {
                    gr.delaysTouchesBegan = false
                }
            }
            cur = v.superview
        }
    }

    private var isDarkAppearance: Bool {
        (textDocumentProxy.keyboardAppearance == .dark)
            || (traitCollection.userInterfaceStyle == .dark)
    }

    private static func mapReturnKeyType(_ t: UIReturnKeyType?) -> IOSReturnKeyRole {
        guard let t else { return .normal }
        switch t {
        case .go: return .go
        case .google, .search, .yahoo: return .search
        case .send: return .send
        case .next: return .next
        case .done: return .done
        case .join: return .join
        case .continue: return .continueRole
        default: return .normal
        }
    }

    private static func nowMs() -> UInt64 {
        UInt64(Date().timeIntervalSince1970 * 1000.0)
    }
}
#endif
