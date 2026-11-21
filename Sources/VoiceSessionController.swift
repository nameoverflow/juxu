import AppKit
import AVFoundation
import Foundation
import InputMethodKit
import Speech

/// Strictly lazy macOS voice dictation & AI polish coordinator.
/// When `VoicePreferencesStore.shared.prefs.enabled == false`, `_shared` remains `nil`
/// and zero audio, speech, networking, or HUD resources are allocated.
@MainActor
public final class VoiceSessionController {
    private static var _shared: VoiceSessionController?

    public static var isLoaded: Bool { _shared != nil }

    public static var sharedIfLoaded: VoiceSessionController? { _shared }

    public static var shared: VoiceSessionController {
        if let existing = _shared {
            return existing
        }
        let created = VoiceSessionController()
        _shared = created
        return created
    }

    public static func unload() {
        _shared?.teardown()
        _shared = nil
        VoiceHUDPanelController.unload()
    }

    private enum Phase {
        case idle
        case recording
        case processing
    }

    private var phase: Phase = .idle
    private var holdTimer: DispatchWorkItem?
    private var pendingHoldKeyCode: UInt16?
    private var levelTimer: Timer?
    private var recordingStartTime: Date?
    private var latestLevel: Float = 0
    private var latestPartial: String = ""
    private var activeRecorder: VoiceAudioRecorder?
    private var pipelineClient: VoicePipelineClient?
    private var activeTask: Task<Void, Never>?
    private var activeCaretRect: NSRect = .zero
    private var activeWindowLevel: CGWindowLevel?
    private var activeCommitHandler: ((String, Bool) -> Void)?
    private var activeContext: VoiceContext = VoiceContext()

    private init() {}

    public var isActive: Bool {
        phase != .idle
    }

    /// Handles modifier/key events from `JuxuInputController` ONLY when voice is enabled.
    /// Returns `true` if the event was consumed by voice dictation.
    public func handleEvent(
        _ event: NSEvent,
        caretRectProvider: @escaping () -> NSRect,
        windowLevel: CGWindowLevel?,
        contextProvider: @escaping () -> VoiceContext,
        onCommit: @escaping (_ text: String, _ replaceSelection: Bool) -> Void
    ) -> Bool {
        let prefs = VoicePreferencesStore.shared.prefs
        guard prefs.enabled else {
            Self.unload()
            return false
        }

        // Esc cancels recording or in-flight transcription/polishing immediately
        if event.type == .keyDown && event.keyCode == 0x35 && phase != .idle {
            cancelCurrentSession(showNotice: true)
            return true
        }

        // Toggle mode: Option + Space
        if prefs.triggerKey == .optionSpace {
            if event.type == .keyDown {
                let mods = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
                if event.keyCode == 0x31 && mods == .option {
                    if phase == .idle {
                        startRecording(
                            caretRect: caretRectProvider(),
                            windowLevel: windowLevel,
                            context: contextProvider(),
                            onCommit: onCommit
                        )
                    } else if phase == .recording {
                        finishRecordingAndProcess()
                    }
                    return true
                }
                if phase == .recording && (event.keyCode == 0x24 || event.keyCode == 0x4C) {
                    finishRecordingAndProcess()
                    return true
                }
            }
            return false
        }

        // Hold-to-talk modifier mode (Right Option / Right Command / Fn)
        guard let targetKeyCode = prefs.triggerKey.modifierKeyCode else { return false }

        if event.type == .keyDown {
            // Any regular key pressed while waiting for hold threshold cancels voice trigger so shortcuts work normally
            if pendingHoldKeyCode != nil {
                cancelPendingHold()
            }
            return phase == .recording
        }

        guard event.type == .flagsChanged else { return false }

        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let targetFlag: NSEvent.ModifierFlags
        switch prefs.triggerKey {
        case .rightOption: targetFlag = .option
        case .rightCommand: targetFlag = .command
        case .fnKey: targetFlag = .function
        case .optionSpace: return false
        }

        if event.keyCode == targetKeyCode {
            let isDown = flags.contains(targetFlag) && flags.subtracting([targetFlag, .capsLock]).isEmpty
            if isDown {
                if phase == .idle && pendingHoldKeyCode == nil {
                    pendingHoldKeyCode = targetKeyCode
                    let item = DispatchWorkItem { [weak self] in
                        guard let self, self.pendingHoldKeyCode == targetKeyCode, self.phase == .idle else { return }
                        self.pendingHoldKeyCode = nil
                        self.startRecording(
                            caretRect: caretRectProvider(),
                            windowLevel: windowLevel,
                            context: contextProvider(),
                            onCommit: onCommit
                        )
                    }
                    holdTimer = item
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.22, execute: item)
                }
                return false
            } else {
                cancelPendingHold()
                if phase == .recording {
                    finishRecordingAndProcess()
                    return true
                }
                return false
            }
        } else if pendingHoldKeyCode != nil {
            // Another modifier was pressed concurrently -> abort hold trigger
            cancelPendingHold()
        }

        return false
    }

    public func handleDeactivate() {
        cancelPendingHold()
        if phase == .recording {
            cancelCurrentSession(showNotice: false)
        }
    }

    /// Runs a quick non-recording connectivity / config check for Settings UI.
    public func testCurrentConfiguration() async -> (ok: Bool, message: String) {
        let prefs = VoicePreferencesStore.shared.prefs
        guard prefs.enabled else {
            return (false, "请先开启语音输入总开关")
        }
        switch prefs.providerMode {
        case .cloudBYOK:
            let asrKey = VoiceKeychainStore.read(.cloudASRAPIKey)
            if asrKey.isEmpty {
                return (false, "未配置云端 ASR API Key")
            }
            if prefs.outputMode == .smartPolish && prefs.cloudPolishEnabled {
                let llmKey = prefs.cloudReuseASRKeyForLLM ? asrKey : VoiceKeychainStore.read(.cloudLLMAPIKey)
                if llmKey.isEmpty {
                    return (false, "已开启云端整理，但未配置 LLM API Key（或勾选「复用 ASR Key」）")
                }
                do {
                    let client = ensurePipelineClient()
                    let polished = try await client.polishOpenAICompatible(
                        baseURL: prefs.cloudLLMBaseURL,
                        apiKey: llmKey,
                        model: prefs.cloudLLMModel,
                        rawTranscript: "嗯，那个，测试一下语音整理功能，明天，不对，后天下午三点开会。",
                        toneMode: prefs.toneMode,
                        context: VoiceContext(personalVocabulary: prefs.parsedCustomVocabulary)
                    )
                    return (true, "云端整理连通正常：\(polished)")
                } catch {
                    return (false, error.localizedDescription)
                }
            }
            return (true, "云端 ASR 配置就绪（模型：\(prefs.cloudASRModel)）")

        case .local:
            switch prefs.localASRKind {
            case .appleSpeech:
                let locale = Locale(identifier: prefs.localAppleLocale.isEmpty ? "zh-CN" : prefs.localAppleLocale)
                guard let rec = SFSpeechRecognizer(locale: locale) else {
                    return (false, "系统不支持语言 \(locale.identifier)")
                }
                if prefs.localAppleRequireOnDevice && !rec.supportsOnDeviceRecognition {
                    return (false, "未检测到 \(locale.identifier) 离线语音模型，请在系统设置 → 键盘 → 听写中下载，或关闭「仅离线」")
                }
                return (true, "Apple 语音识别就绪（\(locale.identifier)\(rec.supportsOnDeviceRecognition ? " · 支持离线" : " · 在线辅助")）")
            case .cli:
                let bin = prefs.localCLIBinaryPath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    ? (VoiceLocalCLIDiscovery.detectInstalledBinary() ?? "")
                    : prefs.localCLIBinaryPath.trimmingCharacters(in: .whitespacesAndNewlines)
                if bin.isEmpty || !FileManager.default.isExecutableFile(atPath: bin) {
                    return (false, "未找到可执行命令：\(bin.isEmpty ? "whisper-cli / whisper" : bin)")
                }
                return (true, "已检测到本地 CLI：\(bin)")
            case .http:
                if URL(string: prefs.localHTTPASRBaseURL) == nil {
                    return (false, "本地 ASR 地址无效")
                }
                if prefs.outputMode == .smartPolish && prefs.localPolishKind == .http {
                    do {
                        let client = ensurePipelineClient()
                        let polished = try await client.polishOpenAICompatible(
                            baseURL: prefs.localHTTPLLMBaseURL,
                            apiKey: nil,
                            model: prefs.localHTTPLLMModel,
                            rawTranscript: "嗯，那个，测试一下本地整理功能。",
                            toneMode: prefs.toneMode,
                            context: VoiceContext()
                        )
                        return (true, "本地 LLM 整理连通正常：\(polished)")
                    } catch {
                        return (false, "本地 LLM 整理不可达：\(error.localizedDescription)")
                    }
                }
                return (true, "本地 HTTP ASR 地址已配置：\(prefs.localHTTPASRBaseURL)")
            }
        }
    }

    // MARK: - Private Lifecycle

    private func cancelPendingHold() {
        holdTimer?.cancel()
        holdTimer = nil
        pendingHoldKeyCode = nil
    }

    private func ensurePipelineClient() -> VoicePipelineClient {
        if let existing = pipelineClient {
            return existing
        }
        let created = VoicePipelineClient()
        pipelineClient = created
        return created
    }

    private func startRecording(
        caretRect: NSRect,
        windowLevel: CGWindowLevel?,
        context: VoiceContext,
        onCommit: @escaping (_ text: String, _ replaceSelection: Bool) -> Void
    ) {
        let prefs = VoicePreferencesStore.shared.prefs
        let useAppleSpeech = (prefs.providerMode == .local && prefs.localASRKind == .appleSpeech)

        activeCaretRect = caretRect
        activeWindowLevel = windowLevel
        activeContext = context
        activeCommitHandler = onCommit
        latestLevel = 0
        latestPartial = ""

        Task { @MainActor [weak self] in
            guard let self else { return }
            let micGranted = await Self.requestMicrophoneAccess()
            guard micGranted else {
                self.showNotice(VoiceEngineError.microphonePermissionDenied.localizedDescription, isError: true)
                return
            }
            if useAppleSpeech {
                let speechGranted = await Self.requestSpeechRecognitionAccess()
                guard speechGranted else {
                    self.showNotice(VoiceEngineError.speechPermissionDenied.localizedDescription, isError: true)
                    return
                }
            }

            let recorder = VoiceAudioRecorder()
            recorder.onAudioLevel = { [weak self] level in
                Task { @MainActor [weak self] in
                    self?.latestLevel = level
                }
            }
            recorder.onPartialTranscript = { [weak self] partial in
                Task { @MainActor [weak self] in
                    self?.latestPartial = partial
                }
            }

            do {
                try recorder.start(
                    useAppleSpeech: useAppleSpeech,
                    appleLocale: prefs.localAppleLocale,
                    requireOnDevice: prefs.localAppleRequireOnDevice
                )
                self.activeRecorder = recorder
                self.phase = .recording
                self.recordingStartTime = Date()
                self.startHUDTimer()
                self.refreshRecordingHUD()
            } catch {
                recorder.cancel()
                self.showNotice(error.localizedDescription, isError: true)
            }
        }
    }

    private func finishRecordingAndProcess() {
        guard phase == .recording, let recorder = activeRecorder else { return }
        stopHUDTimer()
        let samples = recorder.stopAndCollectSamples()
        let elapsed = Date().timeIntervalSince(recordingStartTime ?? Date())
        phase = .processing

        let prefs = VoicePreferencesStore.shared.prefs
        let modeLabel = prefs.providerMode.displayName
        let useAppleSpeech = (prefs.providerMode == .local && prefs.localASRKind == .appleSpeech)
        let context = activeContext
        let commitHandler = activeCommitHandler
        let replaceSelection = prefs.speakToEditSelection
            && !(context.selectedText?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)

        if elapsed < 0.25 && samples.count < 3200 && latestPartial.isEmpty {
            recorder.cancel()
            activeRecorder = nil
            phase = .idle
            showNotice(VoiceEngineError.recordingTooShort.localizedDescription, isError: false, duration: 1.1)
            return
        }

        VoiceHUDPanelController.shared.show(
            state: .transcribing(modeLabel: modeLabel),
            caretRect: activeCaretRect,
            clientWindowLevel: activeWindowLevel
        )

        let client = ensurePipelineClient()
        let asrKey = VoiceKeychainStore.read(.cloudASRAPIKey)
        let llmKey = prefs.cloudReuseASRKeyForLLM ? asrKey : VoiceKeychainStore.read(.cloudLLMAPIKey)

        activeTask?.cancel()
        activeTask = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                self.activeRecorder = nil
                self.activeTask = nil
                self.phase = .idle
            }

            do {
                let rawTranscript: String
                if useAppleSpeech {
                    rawTranscript = try await recorder.finishAppleSpeech()
                } else {
                    let wavData = VoiceWAVEncoder.encodePCM16Mono(samples: samples, sampleRate: 16_000)
                    rawTranscript = try await self.runNonAppleASR(
                        prefs: prefs,
                        client: client,
                        asrKey: asrKey,
                        wavData: wavData,
                        vocabulary: context.personalVocabulary
                    )
                }

                try Task.checkCancellation()
                let cleanedRaw = rawTranscript.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !cleanedRaw.isEmpty else {
                    throw VoiceEngineError.emptyTranscript
                }

                let finalText: String
                if prefs.outputMode == .rawDictation {
                    finalText = cleanedRaw
                } else {
                    VoiceHUDPanelController.shared.show(
                        state: .polishing(preview: String(cleanedRaw.prefix(24))),
                        caretRect: self.activeCaretRect,
                        clientWindowLevel: self.activeWindowLevel
                    )
                    finalText = try await self.runPolish(
                        prefs: prefs,
                        client: client,
                        llmKey: llmKey,
                        rawTranscript: cleanedRaw,
                        context: context
                    )
                }

                try Task.checkCancellation()
                VoiceHUDPanelController.shared.hide()
                commitHandler?(finalText, replaceSelection)
            } catch is CancellationError {
                VoiceHUDPanelController.shared.hide()
            } catch VoiceEngineError.cancelled {
                VoiceHUDPanelController.shared.hide()
            } catch {
                self.showNotice(error.localizedDescription, isError: true, duration: 3.2)
            }
        }
    }

    private func runNonAppleASR(
        prefs: VoicePreferences,
        client: VoicePipelineClient,
        asrKey: String,
        wavData: Data,
        vocabulary: [String]
    ) async throws -> String {
        let prompt = VoicePromptBuilder.asrPrompt(vocabulary: vocabulary)
        switch prefs.providerMode {
        case .cloudBYOK:
            guard !asrKey.isEmpty else {
                throw VoiceEngineError.missingAPIKey("云端 ASR")
            }
            return try await client.transcribeOpenAICompatible(
                baseURL: prefs.cloudASRBaseURL,
                apiKey: asrKey,
                model: prefs.cloudASRModel,
                language: prefs.language,
                prompt: prompt,
                wavData: wavData
            )
        case .local:
            switch prefs.localASRKind {
            case .appleSpeech:
                return ""
            case .cli:
                return try await client.transcribeLocalCLI(
                    binaryPath: prefs.localCLIBinaryPath,
                    modelPath: prefs.localCLIModelPath,
                    language: prefs.language,
                    wavData: wavData
                )
            case .http:
                return try await client.transcribeOpenAICompatible(
                    baseURL: prefs.localHTTPASRBaseURL,
                    apiKey: nil,
                    model: prefs.localHTTPASRModel,
                    language: prefs.language,
                    prompt: prompt,
                    wavData: wavData
                )
            }
        }
    }

    private func runPolish(
        prefs: VoicePreferences,
        client: VoicePipelineClient,
        llmKey: String,
        rawTranscript: String,
        context: VoiceContext
    ) async throws -> String {
        switch prefs.providerMode {
        case .cloudBYOK:
            guard prefs.cloudPolishEnabled else {
                return rawTranscript
            }
            guard !llmKey.isEmpty else {
                // Fall back to lightweight rule cleanup rather than losing the user's speech
                return VoiceLocalRulePolisher.polish(rawTranscript)
            }
            return try await client.polishOpenAICompatible(
                baseURL: prefs.cloudLLMBaseURL,
                apiKey: llmKey,
                model: prefs.cloudLLMModel,
                rawTranscript: rawTranscript,
                toneMode: prefs.toneMode,
                context: context
            )
        case .local:
            switch prefs.localPolishKind {
            case .none:
                return rawTranscript
            case .ruleBased:
                return VoiceLocalRulePolisher.polish(rawTranscript)
            case .http:
                return try await client.polishOpenAICompatible(
                    baseURL: prefs.localHTTPLLMBaseURL,
                    apiKey: nil,
                    model: prefs.localHTTPLLMModel,
                    rawTranscript: rawTranscript,
                    toneMode: prefs.toneMode,
                    context: context
                )
            }
        }
    }

    private func cancelCurrentSession(showNotice: Bool) {
        cancelPendingHold()
        stopHUDTimer()
        activeTask?.cancel()
        activeTask = nil
        activeRecorder?.cancel()
        activeRecorder = nil
        phase = .idle
        if showNotice {
            self.showNotice("已取消语音输入", isError: false, duration: 1.0)
        } else {
            VoiceHUDPanelController.shared.hide()
        }
    }

    private func teardown() {
        cancelPendingHold()
        stopHUDTimer()
        activeTask?.cancel()
        activeTask = nil
        activeRecorder?.cancel()
        activeRecorder = nil
        pipelineClient?.invalidate()
        pipelineClient = nil
        phase = .idle
    }

    private func startHUDTimer() {
        stopHUDTimer()
        let timer = Timer(timeInterval: 0.08, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.refreshRecordingHUD()
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        levelTimer = timer
    }

    private func stopHUDTimer() {
        levelTimer?.invalidate()
        levelTimer = nil
    }

    private func refreshRecordingHUD() {
        guard phase == .recording else { return }
        let elapsed = Date().timeIntervalSince(recordingStartTime ?? Date())
        let modeLabel = VoicePreferencesStore.shared.prefs.providerMode.displayName
        VoiceHUDPanelController.shared.show(
            state: .recording(
                level: latestLevel,
                elapsed: elapsed,
                partial: latestPartial,
                modeLabel: modeLabel
            ),
            caretRect: activeCaretRect,
            clientWindowLevel: activeWindowLevel
        )
    }

    private func showNotice(_ message: String, isError: Bool, duration: TimeInterval = 2.4) {
        VoiceHUDPanelController.shared.show(
            state: .notice(text: message, isError: isError),
            caretRect: activeCaretRect,
            clientWindowLevel: activeWindowLevel,
            autoHideAfter: duration
        )
    }

    private static func requestMicrophoneAccess() async -> Bool {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            return true
        case .notDetermined:
            return await withCheckedContinuation { cont in
                AVCaptureDevice.requestAccess(for: .audio) { granted in
                    cont.resume(returning: granted)
                }
            }
        case .denied, .restricted:
            return false
        @unknown default:
            return false
        }
    }

    private static func requestSpeechRecognitionAccess() async -> Bool {
        switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized:
            return true
        case .notDetermined:
            return await withCheckedContinuation { cont in
                SFSpeechRecognizer.requestAuthorization { status in
                    cont.resume(returning: status == .authorized)
                }
            }
        case .denied, .restricted:
            return false
        @unknown default:
            return false
        }
    }
}
