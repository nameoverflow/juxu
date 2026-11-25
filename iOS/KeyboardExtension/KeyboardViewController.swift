#if os(iOS)
import UIKit

@MainActor
private final class ProxyDocumentSink: IOSMarkedTextDocumentSink {
    let proxy: any UITextDocumentProxy
    init(proxy: any UITextDocumentProxy) { self.proxy = proxy }
    var documentContextBeforeInput: String? { proxy.documentContextBeforeInput }
    var documentContextAfterInput: String? { proxy.documentContextAfterInput }
    var selectedText: String? { proxy.selectedText }
    func setMarkedText(_ markedText: String, selectedRange: NSRange) {
        proxy.setMarkedText(markedText, selectedRange: selectedRange)
    }
    func unmarkText() {
        proxy.unmarkText()
    }
    func insertText(_ text: String) {
        proxy.insertText(text)
    }
    func adjustTextPosition(byCharacterOffset offset: Int) {
        proxy.adjustTextPosition(byCharacterOffset: offset)
    }
}

@objc(KeyboardViewController)
public final class KeyboardViewController: UIInputViewController {
    /// Share the single process-wide `JuxuBridge` (`JuxuEngine` + `UserStore` + `NeuralContextTracker`
    /// + `JuxuPersistenceWorker`) across `KeyboardViewController` lifecycles so UIKit's overlapping
    /// teardown/recreation of `UIInputViewController` never duplicates heap or hits the ~48 MB Jetsam ceiling.
    private var bridge: JuxuBridge { JuxuBridge.shared }
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
    nonisolated(unsafe) private var saveUserDataTimer: Timer?
    nonisolated(unsafe) private var backspaceTimer: Timer?
    private var backspaceHoldStart: TimeInterval = 0
    private var lastVisualKeySignature: VisualLayoutSignature?
    private var planeBeforePress: IOSKeyPlane = .letters
    private var inlineSession = IOSInlineMarkedTextSession()
    private var pairingTracker = PunctuationPairingTracker()
    private var turnFlushScheduled = false
    private var composingDocumentID: UUID?
    private var lastKnownDocumentID: UUID?
    private var lastKeyboardEditUptime: TimeInterval = 0
    nonisolated(unsafe) private var nilDocSweepWorkItem: DispatchWorkItem?
    nonisolated(unsafe) private var appearanceSweepWorkItem: DispatchWorkItem?
    private var isKeyboardPresentingOrVisible = false
    private var proxySink: ProxyDocumentSink { ProxyDocumentSink(proxy: textDocumentProxy) }
    private let clipboardStore = IOSClipboardSuggestionStore()
    private var clipboardGate = IOSClipboardVisibilityGate()
    private let clipboardReadQueue = DispatchQueue(
        label: "im.juxu.clipboard",
        qos: .userInitiated,
        autoreleaseFrequency: .workItem
    )
    private var activeClipboardSuggestion: IOSClipboardSuggestion?
    private var clipboardProbeInFlight = false
    private var clipboardProbeNeedsRerun = false
    private var lastClipboardPrefersOTP = false
    private var expandedIdleWords: [String]?
    private var hasAppeared = false

    private var lineClearUndo = IOSLineClearUndo()
    /// Text removed so far by the backspace press in progress.
    private var backspaceSessionDeleted = ""
    /// A tap's deletion waits for release while a swipe-down undo is possible.
    private var backspaceTapDeferred = false
    /// Bumped to abandon an in-flight line clear as soon as anything else happens.
    private var clearLineGeneration: UInt64 = 0

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
        let spaceCommaKeyEnabled: Bool
        let isZLookupActive: Bool
        let showInlineGlobeKey: Bool
        let returnRole: IOSReturnKeyRole
        let isTrackpadActive: Bool
        let size: CGSize
    }

    deinit {
        saveUserDataTimer?.invalidate()
        backspaceTimer?.invalidate()
        nilDocSweepWorkItem?.cancel()
        appearanceSweepWorkItem?.cancel()
        NotificationCenter.default.removeObserver(self)
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

    public override func didReceiveMemoryWarning() {
        super.didReceiveMemoryWarning()
        lineClearUndo.removeAll()
        if expandedGrid.isHidden {
            expandedGrid.clearAndTrim(keep: 0)
        } else {
            expandedGrid.trimCellPool(keep: 48)
        }
        keyCanvas.clearCaches()
        bridge.handleMemoryWarning()
    }

    /// `UITextDocumentProxy.documentIdentifier` is bridged as non-optional `UUID`, yet UIKit's
    /// `_UITextDocumentInterface` returns Objective-C `nil` during focus transitions or when
    /// refocusing a responder that still carries an orphaned `markedTextRange`.
    private var currentDocumentIdentifier: UUID? {
        (textDocumentProxy as? NSObject)?.value(forKey: "documentIdentifier") as? UUID
    }

    private func noteKeyboardProxyMutation() {
        lastKeyboardEditUptime = ProcessInfo.processInfo.systemUptime
    }

    private func setExpandedGridVisible(_ visible: Bool) {
        if !visible {
            expandedIdleWords = nil
            expandedGrid.clearAndTrim(keep: 32)
        }
        expandedGrid.isHidden = !visible
        keyCanvas.isHidden = visible
        ribbonView.setExpanded(visible)
        updateT9SidebarVisibility()
        if visible {
            reloadExpandedGrid()
        }
    }

    private func abandonCompositionForExternalFocusOrEdit(clearHostMarked: Bool, isDisappearing: Bool = false) {
        nilDocSweepWorkItem?.cancel()
        nilDocSweepWorkItem = nil
        if isDisappearing {
            appearanceSweepWorkItem?.cancel()
            appearanceSweepWorkItem = nil
        }
        if clearHostMarked {
            if !inlineSession.markedCode.isEmpty {
                noteKeyboardProxyMutation()
            }
            cancelMarkedCode()
            inlineSession.reset()
        } else if isDisappearing {
            inlineSession.recordKeyboardDisappear()
        } else {
            _ = inlineSession.recordFocusLoss()
        }
        composingDocumentID = nil
        pairingTracker.reset()
        bridge.resetQuoteState()
        if !bridge.state.preedit.isEmpty {
            bridge.abandonComposition(timestampMs: Self.nowMs())
        }
        if !expandedGrid.isHidden {
            setExpandedGridVisible(false)
        }
        calloutView.hideImmediately()
        keyCanvas.coveredCalloutKeyId = nil
        alternatesView.hide()
        stopBackspaceRepeat()
    }

    private func sweepOrphanedMarkedTextNow() {
        guard inlineSession.hasOrphanedMarkedInSession else { return }
        noteKeyboardProxyMutation()
        pairingTracker.reset()
        bridge.resetQuoteState()
        let outcome = inlineSession.sweepOrphanedMarkedText(in: proxySink)
        scheduleTurnBoundaryFlushIfNeeded(outcome.needsAsyncFlush)
        let expectedTail = outcome.expectedBeforeTail
        guard !expectedTail.isEmpty else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.04) { [weak self] in
            guard let self, self.isKeyboardPresentingOrVisible else { return }
            self.noteKeyboardProxyMutation()
            self.inlineSession.restoreCaretAfterOrphanedSweepIfNeeded(
                expectedBeforeTail: expectedTail,
                in: self.proxySink
            )
            self.syncContextFromProxyIfIdle()
            self.syncEngineToUI()
        }
    }

    private func scheduleAppearanceOrphanedSweepIfNeeded() {
        appearanceSweepWorkItem?.cancel()
        appearanceSweepWorkItem = nil
        guard inlineSession.needsAppearanceSweep, isKeyboardPresentingOrVisible else { return }
        let work = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.appearanceSweepWorkItem = nil
                guard self.isKeyboardPresentingOrVisible,
                      self.inlineSession.needsAppearanceSweep,
                      self.bridge.state.preedit.isEmpty,
                      self.inlineSession.markedCode.isEmpty else { return }
                self.sweepOrphanedMarkedTextNow()
            }
        }
        appearanceSweepWorkItem = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.06, execute: work)
    }

    private func scheduleNilDocOrphanedSweepIfNeeded() {
        nilDocSweepWorkItem?.cancel()
        nilDocSweepWorkItem = nil
        guard inlineSession.hasOrphanedMarkedInSession,
              !inlineSession.needsAppearanceSweep,
              isKeyboardPresentingOrVisible,
              currentDocumentIdentifier == nil else {
            return
        }
        let work = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.nilDocSweepWorkItem = nil
                guard self.isKeyboardPresentingOrVisible,
                      self.currentDocumentIdentifier == nil,
                      self.bridge.state.preedit.isEmpty,
                      self.inlineSession.markedCode.isEmpty else { return }
                self.sweepOrphanedMarkedTextNow()
            }
        }
        nilDocSweepWorkItem = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08, execute: work)
    }

    public override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        isKeyboardPresentingOrVisible = true
        feedback.prepare()
        bridge.suppressEvalLogging = isSensitiveEvalProxy()
        if !bridge.state.preedit.isEmpty && inlineSession.markedCode.isEmpty {
            bridge.abandonComposition(timestampMs: Self.nowMs())
        }
        bridge.reloadConfigAndUserOnAppear { [weak self] in
            self?.syncEngineToUI()
        }
        pairingTracker.reset()
        bridge.resetQuoteState()
        lastKnownDocumentID = currentDocumentIdentifier
        lastClipboardPrefersOTP = (textDocumentProxy.textContentType == .oneTimeCode)
        inlineSession.resetForAppearance()
        scheduleAppearanceOrphanedSweepIfNeeded()
        clipboardGate.resetForKeyboardAppearance()
        activeClipboardSuggestion = nil
        applyDesiredTrayDimensions()
        syncContextFromProxyIfIdle()
        syncEngineToUI()
    }

    public override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        hasAppeared = true
        isKeyboardPresentingOrVisible = true
        applyDesiredTrayDimensions()
        refreshKeyboardLayout()
        disableTouchDelayOnContainerGestures()
        if inlineSession.needsAppearanceSweep && appearanceSweepWorkItem == nil {
            sweepOrphanedMarkedTextNow()
        } else {
            scheduleNilDocOrphanedSweepIfNeeded()
        }
        // Perform pasteboard `changeCount` / `hasStrings` / `string` XPC reads strictly off `@MainActor`
        // AFTER initial 4-row layout is on screen so Universal Clipboard or system paste alerts never stall UIKit.
        DispatchQueue.main.async { [weak self] in
            guard let self, self.hasAppeared else { return }
            self.refreshClipboardSuggestionAsync()
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
        isKeyboardPresentingOrVisible = false
        keyCanvas.resetAllTouches()
        saveUserDataTimer?.invalidate()
        abandonCompositionForExternalFocusOrEdit(clearHostMarked: false, isDisappearing: true)
        bridge.flushUserDataAsync()
        bridge.flushPendingPersistence()
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

    public override func textWillChange(_ textInput: UITextInput?) {
        super.textWillChange(textInput)
        let elapsed = ProcessInfo.processInfo.systemUptime - lastKeyboardEditUptime
        if elapsed > 0.08 {
            pairingTracker.reset()
            bridge.resetQuoteState()
        }
    }

    public override func selectionDidChange(_ textInput: UITextInput?) {
        super.selectionDidChange(textInput)
        let isComposing = !bridge.state.preedit.isEmpty || !inlineSession.markedCode.isEmpty
        let elapsed = ProcessInfo.processInfo.systemUptime - lastKeyboardEditUptime
        guard isComposing else {
            pairingTracker.pruneOnCursorChange(after: textDocumentProxy.documentContextAfterInput)
            if elapsed > 0.08 {
                pairingTracker.reset()
                bridge.resetQuoteState()
            }
            inlineSession.reconcileExternalContext(hostBefore: textDocumentProxy.documentContextBeforeInput)
            syncContextFromProxyIfIdle()
            syncEngineToUI()
            return
        }
        guard elapsed > 0.08 else { return }
        abandonCompositionForExternalFocusOrEdit(clearHostMarked: true)
        syncContextFromProxyIfIdle()
        syncEngineToUI()
    }

    public override func textDidChange(_ textInput: UITextInput?) {
        super.textDidChange(textInput)
        let docID = currentDocumentIdentifier
        let isComposing = !bridge.state.preedit.isEmpty || !inlineSession.markedCode.isEmpty

        let docChanged = (docID != lastKnownDocumentID)
        if docChanged {
            lastKnownDocumentID = docID
            pairingTracker.reset()
            bridge.resetQuoteState()
            if docID != nil {
                clipboardGate.resetForKeyboardAppearance()
            }
        }

        if isComposing {
            if docID == nil {
                abandonCompositionForExternalFocusOrEdit(clearHostMarked: false)
                syncEngineToUI()
            } else if let compDoc = composingDocumentID, docID != compDoc {
                abandonCompositionForExternalFocusOrEdit(clearHostMarked: false)
                syncEngineToUI()
            } else {
                if composingDocumentID == nil {
                    composingDocumentID = docID
                }
                let elapsed = ProcessInfo.processInfo.systemUptime - lastKeyboardEditUptime
                if elapsed > 0.12, let base = inlineSession.compositionBaseContext {
                    let currentBefore = textDocumentProxy.documentContextBeforeInput ?? ""
                    if (!base.isEmpty && currentBefore.isEmpty)
                        || (!currentBefore.isEmpty && !base.hasSuffix(currentBefore) && !currentBefore.hasSuffix(base)) {
                        abandonCompositionForExternalFocusOrEdit(clearHostMarked: true)
                        syncEngineToUI()
                    }
                }
            }
        }

        if docID != nil {
            nilDocSweepWorkItem?.cancel()
            nilDocSweepWorkItem = nil
        } else {
            scheduleNilDocOrphanedSweepIfNeeded()
        }

        bridge.suppressEvalLogging = isSensitiveEvalProxy()
        inlineSession.reconcileExternalContext(hostBefore: textDocumentProxy.documentContextBeforeInput)
        syncContextFromProxyIfIdle()
        let prefersOTP = (textDocumentProxy.textContentType == .oneTimeCode)
        let otpChanged = (prefersOTP != lastClipboardPrefersOTP)
        lastClipboardPrefersOTP = prefersOTP
        if hasAppeared && (docChanged || otpChanged) {
            refreshClipboardSuggestionAsync()
        }
        syncProxyTraits()
    }

    @objc private func handlePasteboardChanged() {
        guard hasAppeared else { return }
        refreshClipboardSuggestionAsync()
    }

    private func markUserTypedForClipboard() {
        clipboardGate.markUserTyped()
        activeClipboardSuggestion = nil
    }

    /// Probes `UIPasteboard.general` (`changeCount`, `hasStrings`, `string`) exclusively on
    /// `clipboardReadQueue` so `pasted` / Universal Clipboard XPC calls can never block `@MainActor`.
    private func refreshClipboardSuggestionAsync() {
        guard hasAppeared, isKeyboardPresentingOrVisible, bridge.prefs.clipboardSuggestion, self.hasFullAccess else {
            if activeClipboardSuggestion != nil {
                activeClipboardSuggestion = nil
                syncEngineToUI()
            }
            return
        }
        if clipboardProbeInFlight {
            clipboardProbeNeedsRerun = true
            return
        }
        clipboardProbeInFlight = true
        clipboardProbeNeedsRerun = false

        let gateSnapshot = clipboardGate
        let seenCC = clipboardStore.seenChangeCount
        let handledCC = clipboardStore.handledChangeCount

        clipboardReadQueue.async { [weak self] in
            let pb = UIPasteboard.general
            let cc = pb.changeCount
            var gateCopy = gateSnapshot
            let allowed = gateCopy.shouldAllowDisplay(for: cc) && (handledCC != cc)
            let isUnseen = (seenCC != cc)
            let hasStrings: Bool
            let fetchedText: String?
            if !allowed {
                hasStrings = false
                fetchedText = nil
            } else if !isUnseen {
                // Already cached in `IOSClipboardSuggestionStore`: avoid `pb.hasStrings` / `pb.string` XPC!
                hasStrings = true
                fetchedText = nil
            } else {
                let hs = pb.hasStrings
                hasStrings = hs
                fetchedText = hs ? pb.string : nil
            }

            Task { @MainActor [weak self] in
                guard let self else { return }
                self.clipboardProbeInFlight = false
                let shouldRerun = self.clipboardProbeNeedsRerun
                self.clipboardProbeNeedsRerun = false

                guard self.hasAppeared,
                      self.isKeyboardPresentingOrVisible,
                      self.bridge.prefs.clipboardSuggestion,
                      self.hasFullAccess else {
                    if self.activeClipboardSuggestion != nil {
                        self.activeClipboardSuggestion = nil
                        self.syncEngineToUI()
                    }
                    return
                }

                let now = Date().timeIntervalSince1970
                let currentOTP = (self.textDocumentProxy.textContentType == .oneTimeCode)
                let canShow = self.clipboardGate.shouldAllowDisplay(for: cc)
                let resolved: IOSClipboardSuggestion?
                if !canShow {
                    resolved = nil
                } else if isUnseen && allowed {
                    resolved = self.clipboardStore.recordRead(
                        changeCount: cc,
                        rawText: hasStrings ? fetchedText : nil,
                        prefersOTP: currentOTP,
                        now: now
                    )
                } else {
                    switch self.clipboardStore.lookup(
                        hasFullAccess: true,
                        enabled: true,
                        hasStrings: hasStrings,
                        changeCount: cc,
                        prefersOTP: currentOTP,
                        now: now
                    ) {
                    case .cached(let s):
                        resolved = s
                    case .none, .needsRead:
                        resolved = nil
                    }
                }

                if self.activeClipboardSuggestion != resolved {
                    self.activeClipboardSuggestion = resolved
                    self.syncEngineToUI()
                }
                if shouldRerun {
                    self.refreshClipboardSuggestionAsync()
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
                        documentID: self.currentDocumentIdentifier
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
            guard let self else { return }
            let options: (items: [String], defaultIndex: Int)
            switch spec.kind {
            case .letter(let ascii):
                options = IOSKeyAlternates.options(for: ascii, asciiMode: self.layoutEngine.asciiMode)
            case .symbol(let sym):
                guard let symOptions = IOSKeyAlternates.symbolOptions(for: sym, asciiMode: self.layoutEngine.asciiMode) else { return }
                options = symOptions
            default:
                return
            }
            self.calloutView.hideImmediately()
            self.keyCanvas.coveredCalloutKeyId = nil
            self.feedback.fireSoftStep(playClick: false)
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
            if IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol(text, on: self.layoutEngine.plane) {
                self.layoutEngine.switchPlane(.letters)
            }
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
                self.noteKeyboardProxyMutation()
                self.textDocumentProxy.adjustTextPosition(byCharacterOffset: dxSteps)
                moved = true
            }
            if dySteps != 0 {
                if self.moveCursorVertically(lines: dySteps) {
                    moved = true
                }
            }
            if moved {
                self.pairingTracker.reset()
                self.bridge.resetQuoteState()
                self.inlineSession.reset()
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
            self.setExpandedGridVisible(self.expandedGrid.isHidden)
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
            self.setExpandedGridVisible(false)
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
            guard let self else { return }
            self.feedback.fireSoftStep(playClick: true)
            self.setExpandedGridVisible(false)
        }

        expandedGrid.onBackspace = { [weak self] in
            guard let self else { return }
            let wasComposing = !self.bridge.state.preedit.isEmpty
            self.feedback.fireKeyDown()
            self.performBackspace()
            if !self.expandedGrid.isHidden {
                if wasComposing && self.bridge.state.preedit.isEmpty {
                    self.setExpandedGridVisible(false)
                } else {
                    self.reloadExpandedGrid()
                }
            }
        }

        expandedGrid.onClear = { [weak self] in
            guard let self else { return }
            self.feedback.fireSoftStep(playClick: true)
            self.cancelMarkedCode()
            self.bridge.abandonComposition(timestampMs: Self.nowMs())
            self.setExpandedGridVisible(false)
            self.syncEngineToUI()
        }

        expandedGrid.onOpenSettings = { [weak self] in
            guard let self else { return }
            self.feedback.fireSoftStep(playClick: true)
            self.setExpandedGridVisible(false)
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
            expandedGrid.reload(candidates: bridge.allCandidates(maxCount: 120), dark: isDarkAppearance)
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
                syncContextFromProxyIfIdle()
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
            syncEngineToUI()

        case .globe:
            commitPendingFirstIfComposing()
            advanceToNextInputMode()

        case .modeToggle:
            // With the comma-key row, stay on the current page: 123 and symbol keys switch content in place.
            if !bridge.prefs.spaceCommaKey, layoutEngine.plane != .letters {
                layoutEngine.switchPlane(.letters)
            }
            let out = bridge.toggleAsciiMode()
            applyEngineState(out)

        case .space:
            markUserTypedForClipboard()
            if bridge.state.preedit.isEmpty {
                noteKeyboardProxyMutation()
                let needsFlush = inlineSession.insertPassthrough(" ", in: proxySink)
                scheduleTurnBoundaryFlushIfNeeded(needsFlush)
                syncContextFromProxyIfIdle()
                syncEngineToUI()
            } else {
                let out = bridge.handleKey(keyCode: 0x31, scalar: 32, hasCmdOrCtrl: false, timestampMs: Self.nowMs())
                applyEngineState(out)
            }

        case .returnKey:
            markUserTypedForClipboard()
            if bridge.state.preedit.isEmpty {
                noteKeyboardProxyMutation()
                let needsFlush = inlineSession.insertPassthrough("\n", in: proxySink)
                scheduleTurnBoundaryFlushIfNeeded(needsFlush)
                syncContextFromProxyIfIdle()
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
        noteKeyboardProxyMutation()
        commitPendingFirstIfComposing()
        var pairCloser: String?
        if isPunctuationPairingActive {
            let after = textDocumentProxy.documentContextAfterInput
            switch pairingTracker.handleSymbol(sym, after: after) {
            case .skipClose:
                textDocumentProxy.adjustTextPosition(byCharacterOffset: 1)
                inlineSession.autoSpaceTracker.invalidate()
                bridge.setContext("")
                return
            case .insertPair(let close):
                pairCloser = close
            case .plain:
                break
            }
        } else {
            pairingTracker.reset()
        }
        if AutoSpaceFormatter.isAutoSpaceRightTrigger(sym, options: AutoSpaceOptions(prefs: bridge.prefs)) {
            insertTextWithAutoSpace(sym, splitInternal: false)
        } else {
            let needsFlush = inlineSession.insertPassthrough(sym, in: proxySink)
            scheduleTurnBoundaryFlushIfNeeded(needsFlush)
        }
        if let pairCloser {
            textDocumentProxy.insertText(pairCloser)
            textDocumentProxy.adjustTextPosition(byCharacterOffset: -1)
            inlineSession.autoSpaceTracker.invalidate()
            bridge.setContext("")
        } else {
            syncContextFromProxyIfIdle()
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
                  documentID: currentDocumentIdentifier
              ) else { return }
        noteKeyboardProxyMutation()
        clearLineGeneration &+= 1
        pairingTracker.reset()
        bridge.resetQuoteState()
        inlineSession.autoSpaceTracker.invalidate()
        textDocumentProxy.insertText(text)
        bridge.setContext(String(text.suffix(16)))
        syncEngineToUI()
    }

    /// Backspace swipe-up: drops an active composition, otherwise deletes leftward to the start of the line.
    /// The proxy only exposes a window of context, so long lines are deleted in verified batches.
    private func clearCurrentLineBeforeCursor() {
        markUserTypedForClipboard()
        if !bridge.state.preedit.isEmpty {
            noteKeyboardProxyMutation()
            cancelMarkedCode()
            bridge.clearComposition()
            syncEngineToUI()
            return
        }
        noteKeyboardProxyMutation()
        pairingTracker.reset()
        bridge.resetQuoteState()
        inlineSession.autoSpaceTracker.invalidate()
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
            documentID: currentDocumentIdentifier
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
        noteKeyboardProxyMutation()
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
        noteKeyboardProxyMutation()
        composingDocumentID = nil
        let out = bridge.commitPending(timestampMs: Self.nowMs())
        let needsFlush = inlineSession.commitPendingFirst(
            out.commitText,
            options: currentAutoSpaceOptions(),
            in: proxySink
        )
        scheduleTurnBoundaryFlushIfNeeded(needsFlush)
        if !out.commitText.isEmpty {
            bridge.flushUserDataAsync()
        }
        if !expandedGrid.isHidden {
            setExpandedGridVisible(false)
        }
    }

    private func performBackspace(acceleratedChunk: Bool = false) {
        backspaceTapDeferred = false
        markUserTypedForClipboard()
        noteKeyboardProxyMutation()
        if !bridge.state.preedit.isEmpty {
            let out = bridge.handleKey(keyCode: 0x33, scalar: 0, hasCmdOrCtrl: false, timestampMs: Self.nowMs())
            applyEngineState(out)
            return
        }
        if isPunctuationPairingActive, !pairingTracker.openClosers.isEmpty {
            let hostBefore = textDocumentProxy.documentContextBeforeInput
            let hostAfter = textDocumentProxy.documentContextAfterInput
            let closer = pairingTracker.openClosers.last ?? ""
            if pairingTracker.consumeBackspacePairIfNeeded(before: hostBefore, after: hostAfter) {
                noteBackspaceDeleted(String((hostBefore ?? "").suffix(1)) + closer)
                textDocumentProxy.adjustTextPosition(byCharacterOffset: 1)
                textDocumentProxy.deleteBackward()
                textDocumentProxy.deleteBackward()
                inlineSession.autoSpaceTracker.invalidate()
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
            inlineSession.autoSpaceTracker.willDeleteBackward(
                hostBefore: before,
                hostBeforeIsAuthoritative: false
            )
            noteBackspaceDeleted(String(before.suffix(1)))
            textDocumentProxy.deleteBackward()
            let afterDel = before.isEmpty ? "" : String(before.dropLast())
            bridge.setContext(String(afterDel.suffix(32)))
            syncEngineToUI()
            return
        }
        inlineSession.autoSpaceTracker.invalidate()
        let nDel = IOSBackspaceChunkDelete.acceleratedDeleteCount(before: before)
        noteBackspaceDeleted(String(before.suffix(nDel)))
        for _ in 0..<nDel {
            textDocumentProxy.deleteBackward()
        }
        let afterDel = String(before.dropLast(min(nDel, before.count)))
        bridge.setContext(String(afterDel.suffix(32)))
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
        let before = textDocumentProxy.documentContextBeforeInput ?? ""
        let after = textDocumentProxy.documentContextAfterInput ?? ""
        guard let delta = IOSTextCursorNavigation.verticalMoveOffset(before: before, after: after, lines: lines) else {
            return false
        }
        noteKeyboardProxyMutation()
        textDocumentProxy.adjustTextPosition(byCharacterOffset: delta)
        return true
    }

    private func cancelMarkedCode() {
        inlineSession.cancelMarkedCode(in: proxySink)
    }

    private func scheduleUserDataFlush() {
        saveUserDataTimer?.invalidate()
        saveUserDataTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.bridge.flushUserDataAsync() }
        }
    }

    private func syncContextFromProxyIfIdle() {
        guard bridge.state.preedit.isEmpty else { return }
        let before = inlineSession.autoSpaceTracker.effectiveBefore(
            hostBefore: textDocumentProxy.documentContextBeforeInput,
            hostBeforeIsAuthoritative: false
        )
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

    private func currentAutoSpaceOptions() -> AutoSpaceOptions {
        isAutoSpacingAllowedForProxy()
            ? AutoSpaceOptions(prefs: bridge.prefs)
            : AutoSpaceOptions(enabled: false, spaceDigits: false)
    }

    private func scheduleTurnBoundaryFlushIfNeeded(_ needed: Bool) {
        guard needed, !turnFlushScheduled else { return }
        turnFlushScheduled = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.turnFlushScheduled = false
            self.noteKeyboardProxyMutation()
            self.inlineSession.flushTurnBoundary(in: self.proxySink)
        }
    }

    private func insertTextWithAutoSpace(
        _ text: String,
        splitInternal: Bool,
        overrideBefore: String? = nil
    ) {
        noteKeyboardProxyMutation()
        let needsFlush = inlineSession.insertTextWithAutoSpace(
            text,
            splitInternal: splitInternal,
            overrideBefore: overrideBefore,
            options: currentAutoSpaceOptions(),
            in: proxySink
        )
        scheduleTurnBoundaryFlushIfNeeded(needsFlush)
    }

    private func applyEngineState(_ out: JuxuActionState) {
        noteKeyboardProxyMutation()
        if out.preedit.isEmpty {
            composingDocumentID = nil
        } else if let docID = currentDocumentIdentifier {
            composingDocumentID = docID
        }
        let res = inlineSession.applyEngineState(
            commitText: out.commitText,
            inlinePreedit: out.activeInlinePreedit,
            options: currentAutoSpaceOptions(),
            in: proxySink
        )
        scheduleTurnBoundaryFlushIfNeeded(res.needsAsyncFlush)
        if res.didCommit {
            scheduleUserDataFlush()
        }
        if out.preedit.isEmpty && !expandedGrid.isHidden {
            setExpandedGridVisible(false)
        }
        syncEngineToUI()
    }

    private func syncEngineToUI() {
        let st = bridge.state
        if st.preedit.isEmpty, activeClipboardSuggestion != nil {
            if clipboardGate.isSuppressedByTyping || !bridge.prefs.clipboardSuggestion || !self.hasFullAccess {
                activeClipboardSuggestion = nil
            }
        }
        layoutEngine.asciiMode = st.asciiMode
        layoutEngine.schemaId = st.schemaId
        layoutEngine.pinyinT9Layout = bridge.prefs.pinyinT9Layout
        layoutEngine.cangjieKeycapRadicals = bridge.prefs.cangjieKeycapRadicals
        layoutEngine.wubiKeycapRadicals = bridge.prefs.wubiKeycapRadicals
        layoutEngine.swipeUpInputEnabled = bridge.prefs.keySwipeUpInput
        layoutEngine.spaceCommaKeyEnabled = bridge.prefs.spaceCommaKey
        keyCanvas.longPressAlternatesEnabled = bridge.prefs.keyLongPressAlternates
        keyCanvas.asciiMode = st.asciiMode
        keyCanvas.backspaceSwipeClearEnabled = bridge.prefs.backspaceSwipeClear
        keyCanvas.keyboardHoldCursorEnabled = bridge.prefs.keyboardHoldCursor
        layoutEngine.isZLookupActive = bridge.prefs.isCangjieFamily
            && bridge.prefs.zOmnipotent
            && st.preedit.hasPrefix("z")
        layoutEngine.isComposing = !st.preedit.isEmpty
        keyCanvas.isComposing = !st.preedit.isEmpty
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
        if !expandedGrid.isHidden {
            reloadExpandedGrid()
        }
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
            spaceCommaKeyEnabled: layoutEngine.spaceCommaKeyEnabled,
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
