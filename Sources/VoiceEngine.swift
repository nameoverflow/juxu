import AVFoundation
import Foundation
import Speech

/// Which samples of a captured buffer fall inside the time a cue is audible.
enum VoiceCueMute {
    static func sampleRange(
        bufferStart: TimeInterval,
        count: Int,
        sampleRate: Double,
        muteStart: TimeInterval,
        muteEnd: TimeInterval
    ) -> Range<Int> {
        let first = max(0, min(count, Int(((muteStart - bufferStart) * sampleRate).rounded())))
        let last = max(first, min(count, Int(((muteEnd - bufferStart) * sampleRate).rounded())))
        return first..<last
    }
}

public struct VoiceContext: Sendable {
    public let bundleIdentifier: String?
    public let appName: String?
    public let windowTitle: String?
    public let webURL: String?
    public let selectedText: String?
    public let beforeContext: String?
    public let afterContext: String?
    public let surroundingBefore: String?
    public let surroundingAfter: String?
    public let visibleScreenText: String?
    public let personalVocabulary: [String]
    public let captured: VoiceCapturedContext?

    public init(
        bundleIdentifier: String? = nil,
        appName: String? = nil,
        windowTitle: String? = nil,
        webURL: String? = nil,
        selectedText: String? = nil,
        beforeContext: String? = nil,
        afterContext: String? = nil,
        surroundingBefore: String? = nil,
        surroundingAfter: String? = nil,
        visibleScreenText: String? = nil,
        personalVocabulary: [String] = [],
        captured: VoiceCapturedContext? = nil
    ) {
        self.bundleIdentifier = bundleIdentifier
        self.appName = appName
        self.windowTitle = windowTitle
        self.webURL = webURL
        self.selectedText = selectedText
        self.beforeContext = beforeContext
        self.afterContext = afterContext
        self.surroundingBefore = surroundingBefore
        self.surroundingAfter = surroundingAfter
        self.visibleScreenText = visibleScreenText
        self.personalVocabulary = personalVocabulary
        self.captured = captured
    }

    public func merged(with captured: VoiceCapturedContext) -> VoiceContext {
        let app = captured.activeApplication
        let cur = captured.textInsertionPoint.cursorState
        let sur = captured.textInsertionPoint.surroundingContext
        let resolvedBundle = (bundleIdentifier?.isEmpty == false) ? bundleIdentifier : (app.appIdentifier.isEmpty ? nil : app.appIdentifier)
        let resolvedSelected = (selectedText?.isEmpty == false) ? selectedText : (cur.selectedText.isEmpty ? nil : cur.selectedText)
        let resolvedBefore = (beforeContext?.isEmpty == false) ? beforeContext : (cur.textBeforeCursor.isEmpty ? nil : cur.textBeforeCursor)
        return VoiceContext(
            bundleIdentifier: resolvedBundle,
            appName: app.appName.isEmpty ? appName : app.appName,
            windowTitle: app.windowTitle.isEmpty ? windowTitle : app.windowTitle,
            webURL: app.webURL ?? webURL,
            selectedText: resolvedSelected,
            beforeContext: resolvedBefore,
            afterContext: cur.textAfterCursor.isEmpty ? afterContext : cur.textAfterCursor,
            surroundingBefore: sur.textBeforeInputArea.isEmpty ? surroundingBefore : sur.textBeforeInputArea,
            surroundingAfter: sur.textAfterInputArea.isEmpty ? surroundingAfter : sur.textAfterInputArea,
            visibleScreenText: app.visibleScreenContent ?? visibleScreenText,
            personalVocabulary: personalVocabulary,
            captured: captured
        )
    }
}

public enum VoiceEngineError: LocalizedError, Sendable {
    case voiceDisabled
    case microphonePermissionDenied
    case speechPermissionDenied
    case speechRecognizerUnavailable(String)
    case missingAPIKey(String)
    case invalidEndpoint(String)
    case recordingTooShort
    case emptyTranscript
    case noSpeechDetected
    case cliNotFound(String)
    case cliFailed(String)
    case networkError(String)
    case cancelled

    public var errorDescription: String? {
        switch self {
        case .voiceDisabled:
            return "语音输入未开启"
        case .microphonePermissionDenied:
            return "请在「系统设置 → 隐私与安全性 → 麦克风」中允许句序访问麦克风"
        case .speechPermissionDenied:
            return "请在「系统设置 → 隐私与安全性 → 语音识别」中允许句序使用语音识别"
        case .speechRecognizerUnavailable(let detail):
            return "Apple 本地语音识别不可用：\(detail)"
        case .missingAPIKey(let which):
            return "尚未配置 \(which) API Key，请在「句序偏好设置 → 语音输入」中填写"
        case .invalidEndpoint(let url):
            return "接口地址无效：\(url)"
        case .recordingTooShort:
            return "录音过短，已忽略"
        case .emptyTranscript:
            return "未识别到有效语音内容"
        case .noSpeechDetected:
            return "没听清"
        case .cliNotFound(let bin):
            return "未找到本地语音命令：\(bin)"
        case .cliFailed(let msg):
            return "本地语音执行失败：\(msg)"
        case .networkError(let msg):
            return "请求失败：\(msg)"
        case .cancelled:
            return "已取消语音输入"
        }
    }
}

// MARK: - Non-Blocking Subprocess Runner (Concurrent Pipe Drain + Cancellation)

public struct VoiceSubprocessResult: Sendable {
    public let terminationStatus: Int32
    public let terminationReason: Process.TerminationReason
    public let stdout: Data
    public let stderr: Data
}

/// Runs a `Foundation.Process` without blocking cooperative Swift Concurrency threads:
/// - Drains `stdout` and `stderr` pipes concurrently while the child runs (preventing 64KB pipe buffer deadlocks).
/// - Uses `terminationHandler` + `DispatchGroup` with `CheckedContinuation` instead of `waitUntilExit()`.
/// - Wraps execution in `withTaskCancellationHandler` calling `proc.terminate()` immediately on task cancel.
public enum VoiceSubprocessRunner {
    private final class CancellationBox: @unchecked Sendable {
        private let lock = NSLock()
        private var process: Process?
        private var cancelled = false

        var isCancelled: Bool {
            lock.lock()
            defer { lock.unlock() }
            return cancelled
        }

        func attach(_ proc: Process) -> Bool {
            lock.lock()
            if cancelled {
                lock.unlock()
                return false
            }
            self.process = proc
            lock.unlock()
            return true
        }

        func cancel() {
            lock.lock()
            cancelled = true
            let proc = process
            lock.unlock()
            if let proc, proc.isRunning {
                proc.terminate()
            }
        }
    }

    private final class DataBox: @unchecked Sendable {
        var data = Data()
    }

    public static func run(
        executablePath: String,
        arguments: [String],
        environment: [String: String]? = nil
    ) async throws -> VoiceSubprocessResult {
        try Task.checkCancellation()
        let cancelBox = CancellationBox()

        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<VoiceSubprocessResult, Error>) in
                if cancelBox.isCancelled {
                    cont.resume(throwing: CancellationError())
                    return
                }

                let proc = Process()
                proc.executableURL = URL(fileURLWithPath: executablePath)
                proc.arguments = arguments
                if let environment {
                    proc.environment = environment
                }
                proc.standardInput = FileHandle.nullDevice

                let stdoutPipe = Pipe()
                let stderrPipe = Pipe()
                proc.standardOutput = stdoutPipe
                proc.standardError = stderrPipe

                let stdoutRead = stdoutPipe.fileHandleForReading
                let stderrRead = stderrPipe.fileHandleForReading
                let outBox = DataBox()
                let errBox = DataBox()
                let group = DispatchGroup()

                group.enter()
                proc.terminationHandler = { _ in
                    group.leave()
                }

                do {
                    try proc.run()
                } catch {
                    proc.terminationHandler = nil
                    group.leave()
                    cont.resume(throwing: VoiceEngineError.cliFailed(error.localizedDescription))
                    return
                }

                if !cancelBox.attach(proc) {
                    if proc.isRunning {
                        proc.terminate()
                    }
                }

                group.enter()
                DispatchQueue.global(qos: .utility).async {
                    outBox.data = stdoutRead.readDataToEndOfFile()
                    group.leave()
                }

                group.enter()
                DispatchQueue.global(qos: .utility).async {
                    errBox.data = stderrRead.readDataToEndOfFile()
                    group.leave()
                }

                group.notify(queue: .global(qos: .userInitiated)) {
                    if cancelBox.isCancelled || proc.terminationReason == .uncaughtSignal {
                        cont.resume(throwing: CancellationError())
                        return
                    }
                    cont.resume(
                        returning: VoiceSubprocessResult(
                            terminationStatus: proc.terminationStatus,
                            terminationReason: proc.terminationReason,
                            stdout: outBox.data,
                            stderr: errBox.data
                        )
                    )
                }
            }
        } onCancel: {
            cancelBox.cancel()
        }
    }
}

// MARK: - App Tone Classification & Prompt Builder

public enum VoiceAppStyleClassifier {
    public static func styleInstruction(
        toneMode: VoiceToneMode,
        bundleIdentifier: String?
    ) -> String {
        switch toneMode {
        case .casual:
            return "语气自然口语化，像即时聊天一样简洁顺畅，避免生硬书面腔。"
        case .professional:
            return "语气清晰、专业、书面化，适合邮件或正式文档，逻辑分明。"
        case .concise:
            return "尽量精炼，提炼核心信息；若包含多个要点或步骤，自动整理为清晰的条目列表。"
        case .autoByApp:
            guard let bid = bundleIdentifier?.lowercased() else {
                return "语气自然清晰，根据内容长度自动决定单段还是分条。"
            }
            if bid.contains("wechat") || bid.contains("xinwechat") || bid.contains("slack")
                || bid.contains("telegram") || bid.contains("discord") || bid.contains("mobilesms")
                || bid.contains("lark") || bid.contains("feishu") || bid.contains("dingtalk")
            {
                return "当前在即时聊天应用中，语气保持自然口语化、简短直接，不要过度书面化。"
            }
            if bid.contains("vscode") || bid.contains("xcode") || bid.contains("jetbrains")
                || bid.contains("cursor") || bid.contains("terminal") || bid.contains("iterm")
                || bid.contains("ghostty") || bid.contains("warp")
            {
                return "当前在开发工具或终端中，准确保留英文技术术语、标识符、命令和代码格式，不要把英文专有名词强行翻译。"
            }
            if bid.contains("mail") || bid.contains("outlook") || bid.contains("notion")
                || bid.contains("obsidian") || bid.contains("pages") || bid.contains("word")
                || bid.contains("notes")
            {
                return "当前在文档或邮件应用中，语气清晰书面，若口述包含多个要点或步骤自动整理成段落或列表。"
            }
            return "语气自然清晰，忠实保留原意并做好标点与分段。"
        }
    }
}

public enum VoicePromptBuilder {
    public static func asrPrompt(vocabulary: [String]) -> String {
        let deduped = deduplicatedVocabulary(vocabulary, limit: 40)
        guard !deduped.isEmpty else { return "" }
        return deduped.joined(separator: "、")
    }

    public static func polishMessages(
        rawTranscript: String,
        toneMode: VoiceToneMode,
        context: VoiceContext
    ) -> [[String: String]] {
        let style = VoiceAppStyleClassifier.styleInstruction(
            toneMode: toneMode,
            bundleIdentifier: context.bundleIdentifier
        )
        let vocab = deduplicatedVocabulary(context.personalVocabulary, limit: 50)
        let vocabBlock = vocab.isEmpty
            ? ""
            : "\n- 个人常用词表（遇到谐音或近似词时优先拼写为以下词条）：\(vocab.joined(separator: "、"))"

        let selected = context.selectedText?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !selected.isEmpty {
            let system = """
            你是输入法内置的语音编辑助手。用户选中了一段已有文本，并用语音说出了修改指令。
            规则：
            - 根据语音指令改写、润色、翻译或修复选中文本。
            - 直接输出修改后的最终文本，不要加任何解释、前缀、引号或 Markdown 代码围栏。
            - \(style)\(vocabBlock)
            """
            let user = """
            【选中文本】
            \(selected)

            【语音指令】
            \(rawTranscript)
            """
            return [
                ["role": "system", "content": system],
                ["role": "user", "content": user],
            ]
        }

        let beforeBlock: String
        if let before = context.beforeContext?.trimmingCharacters(in: .whitespacesAndNewlines),
           !before.isEmpty {
            beforeBlock = "\n- 光标前文参考（仅用于衔接语义与人称，不要重复输出前文）：\(String(before.suffix(80)))"
        } else {
            beforeBlock = ""
        }

        let system = """
        你是输入法内置的口语整理助手，目标是把用户的语音转写结果整理成可以直接发送或粘贴的干净文本。
        严格遵守以下规则：
        1. 去除口头禅和填充词（如“嗯”“啊”“呃”“那个”“就是说”“um”“uh”“you know”等）。
        2. 识别口误和自我纠正（如“明天……不对，后天下午三点”→“后天下午三点”），只保留用户最终意图。
        3. 补全自然准确的标点符号；中英文混排保持可读性；若口述包含明显的多条要点、步骤或清单，自动整理成清晰列表。
        4. 忠实保留用户原意、语言（中文/英文/中英混说）和事实，绝不凭空扩写或回答用户的问题。
        5. 只输出整理后的最终文本，不要添加任何开场白、解释、标题或引号。
        - \(style)\(vocabBlock)\(beforeBlock)
        """

        return [
            ["role": "system", "content": system],
            ["role": "user", "content": rawTranscript],
        ]
    }

    public static func deduplicatedVocabulary(_ items: [String], limit: Int) -> [String] {
        var seen = Set<String>()
        var result: [String] = []
        for raw in items {
            let t = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !t.isEmpty, seen.insert(t).inserted else { continue }
            result.append(t)
            if result.count >= limit { break }
        }
        return result
    }
}

// MARK: - Audio Conditioning (High-Pass + Soft-Capped Auto Gain for Quiet Mics)

public enum VoiceAudioNormalizer {
    /// Removes sub-80 Hz rumble/DC and gently boosts quiet speech up to +18 dB without clipping or
    /// amplifying room silence. Intended for 16 kHz mono PCM16 after `VoiceSpeechGate` has run.
    public static func conditionForASR(_ samples: [Int16]) -> [Int16] {
        let count = samples.count
        guard count > 0 else { return [] }

        // 1st-order high-pass filter (~80 Hz at 16 kHz) to strip DC offset and low-frequency fan/desk rumble.
        let alpha: Float = 0.969
        var filtered = [Float](repeating: 0, count: count)
        var prevIn: Float = 0
        var prevOut: Float = 0
        var peak: Float = 0
        var sumSq: Float = 0
        let invScale: Float = 1.0 / 32768.0

        for i in 0..<count {
            let x = Float(samples[i]) * invScale
            let y = x - prevIn + alpha * prevOut
            prevIn = x
            prevOut = y
            filtered[i] = y
            let a = abs(y)
            if a > peak { peak = a }
            sumSq += y * y
        }

        // Never boost pure silence / noise floor below -48 dBFS.
        let minSpeechPeak: Float = 0.004
        let targetPeak: Float = 0.70
        let targetRMS: Float = 0.11
        let maxGain: Float = 8.0 // +18 dB cap

        let gain: Float
        if peak >= minSpeechPeak && peak < targetPeak {
            let rms = sqrt(sumSq / Float(count))
            let peakGain = targetPeak / peak
            let rmsGain = rms > 1e-5 ? (targetRMS / rms) * 1.6 : peakGain
            gain = min(maxGain, max(1.0, min(peakGain, rmsGain)))
        } else {
            gain = 1.0
        }

        var out = [Int16](repeating: 0, count: count)
        let knee: Float = 0.88
        let headroom: Float = 1.0 - knee
        for i in 0..<count {
            var v = filtered[i] * gain
            let a = abs(v)
            if a > knee {
                let limited = knee + headroom * tanh((a - knee) / headroom)
                v = v >= 0 ? limited : -limited
            }
            let scaled = max(-32768.0, min(32767.0, (v * 32767.0).rounded()))
            out[i] = Int16(scaled)
        }
        return out
    }
}

// MARK: - WAV Encoder & Multipart Form Builder (16kHz Mono PCM16)

public enum VoiceWAVEncoder {
    public static func encodePCM16Mono(samples: [Int16], sampleRate: Int = 16_000) -> Data {
        let numChannels: UInt16 = 1
        let bitsPerSample: UInt16 = 16
        let byteRate = UInt32(sampleRate) * UInt32(numChannels) * UInt32(bitsPerSample / 8)
        let blockAlign = numChannels * (bitsPerSample / 8)
        let dataSize = UInt32(samples.count * MemoryLayout<Int16>.size)
        let riffChunkSize = 36 + dataSize

        var data = Data(capacity: 44 + Int(dataSize))
        data.append(contentsOf: "RIFF".utf8)
        appendLE(&data, riffChunkSize)
        data.append(contentsOf: "WAVE".utf8)
        data.append(contentsOf: "fmt ".utf8)
        appendLE(&data, UInt32(16))
        appendLE(&data, UInt16(1)) // PCM
        appendLE(&data, numChannels)
        appendLE(&data, UInt32(sampleRate))
        appendLE(&data, byteRate)
        appendLE(&data, blockAlign)
        appendLE(&data, bitsPerSample)
        data.append(contentsOf: "data".utf8)
        appendLE(&data, dataSize)

        samples.withUnsafeBufferPointer { buf in
            if let base = buf.baseAddress {
                let raw = UnsafeRawBufferPointer(start: base, count: Int(dataSize))
                data.append(contentsOf: raw)
            }
        }
        return data
    }

    private static func appendLE<T: FixedWidthInteger>(_ data: inout Data, _ value: T) {
        var le = value.littleEndian
        withUnsafeBytes(of: &le) { data.append(contentsOf: $0) }
    }
}

public enum VoiceMultipartBuilder {
    public static func buildAudioTranscriptionBody(
        boundary: String,
        wavData: Data,
        model: String,
        language: String?,
        prompt: String?
    ) -> Data {
        var body = Data()

        func appendField(name: String, value: String) {
            body.append(Data("--\(boundary)\r\n".utf8))
            body.append(Data("Content-Disposition: form-data; name=\"\(name)\"\r\n\r\n".utf8))
            body.append(Data("\(value)\r\n".utf8))
        }

        appendField(name: "model", value: model)
        if let language, !language.isEmpty, language != "auto" {
            appendField(name: "language", value: language)
        }
        if let prompt, !prompt.isEmpty {
            appendField(name: "prompt", value: prompt)
        }
        appendField(name: "response_format", value: "json")

        body.append(Data("--\(boundary)\r\n".utf8))
        body.append(Data("Content-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\n".utf8))
        body.append(Data("Content-Type: audio/wav\r\n\r\n".utf8))
        body.append(wavData)
        body.append(Data("\r\n--\(boundary)--\r\n".utf8))
        return body
    }
}

// MARK: - On-Demand Audio Recorder & Apple Speech Live Streamer

public final class VoiceAudioRecorder: @unchecked Sendable {
    /// Caps buffered PCM16 audio at 5 minutes (16 kHz mono ≈ 9.6 MB WAV), preventing
    /// unbounded memory growth in hands-free mode and staying well within cloud upload limits.
    public static let maxRecordingSamples = 16_000 * 300

    private let lock = NSLock()
    private let configQueue = DispatchQueue(label: "im.juxu.voice.audio.config")
    private var engine: AVAudioEngine?
    private var configChangeObserver: NSObjectProtocol?
    private var pcm16Samples: [Int16] = []
    private var didReachMaxDuration = false
    private var targetFormat: AVAudioFormat?
    private var converter: AVAudioConverter?
    private var speechRequest: SFSpeechAudioBufferRecognitionRequest?
    private var speechTask: SFSpeechRecognitionTask?
    private var speechRecognizer: SFSpeechRecognizer?
    private var latestAppleTranscript: String = ""
    private var appleFinalContinuation: CheckedContinuation<String, Error>?
    private var appleFinished = false
    private var levelMeter = VoiceLevelMeter()
    private let spectrumAnalyzer = VoiceSpectrumAnalyzer()
    /// Buffers per whole dBFS, for the timing log. Holds no audio.
    private var levelHistogram = [Int](repeating: 0, count: 91)
    private var cueMuteStart: TimeInterval = 0
    private var cueMuteEnd: TimeInterval = 0

    public var onAudioLevel: (@Sendable (Float) -> Void)?
    public var onAudioSpectrum: (@Sendable ([[Float]]) -> Void)?
    public var onPartialTranscript: (@Sendable (String) -> Void)?
    public var onMaxDurationReached: (@Sendable () -> Void)?
    public var onAudioInterrupted: (@Sendable (String) -> Void)?

    public init() {}

    public func start(
        useAppleSpeech: Bool,
        appleLocale: String,
        requireOnDevice: Bool
    ) throws {
        let eng = AVAudioEngine()
        let inputNode = eng.inputNode
        let inputFormat = inputNode.outputFormat(forBus: 0)
        guard inputFormat.sampleRate > 0, inputFormat.channelCount > 0 else {
            throw VoiceEngineError.microphonePermissionDenied
        }

        guard let outFormat = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: 16_000,
            channels: 1,
            interleaved: true
        ) else {
            throw VoiceEngineError.networkError("无法初始化 16kHz PCM 音频格式")
        }

        let conv = AVAudioConverter(from: inputFormat, to: outFormat)
        lock.lock()
        self.targetFormat = outFormat
        self.converter = conv
        pcm16Samples.removeAll(keepingCapacity: true)
        didReachMaxDuration = false
        latestAppleTranscript = ""
        appleFinished = false
        lock.unlock()

        if useAppleSpeech {
            let locale = Locale(identifier: appleLocale.isEmpty ? "zh-CN" : appleLocale)
            guard let recognizer = SFSpeechRecognizer(locale: locale) else {
                throw VoiceEngineError.speechRecognizerUnavailable("不支持语言 \(locale.identifier)")
            }
            guard recognizer.isAvailable else {
                throw VoiceEngineError.speechRecognizerUnavailable("系统语音服务当前不可用")
            }
            let req = SFSpeechAudioBufferRecognitionRequest()
            req.shouldReportPartialResults = true
            req.addsPunctuation = true
            if requireOnDevice {
                if recognizer.supportsOnDeviceRecognition {
                    req.requiresOnDeviceRecognition = true
                } else {
                    throw VoiceEngineError.speechRecognizerUnavailable(
                        "当前系统未就绪 \(locale.identifier) 离线模型（可在系统设置 → 键盘 → 听写中下载，或取消勾选「仅离线」）"
                    )
                }
            }
            lock.lock()
            self.speechRecognizer = recognizer
            self.speechRequest = req
            lock.unlock()
            let task = recognizer.recognitionTask(with: req) { [weak self] result, error in
                guard let self else { return }
                if let result {
                    let text = result.bestTranscription.formattedString
                    self.lock.lock()
                    self.latestAppleTranscript = text
                    let isFinal = result.isFinal
                    let cont = isFinal ? self.appleFinalContinuation : nil
                    if isFinal {
                        self.appleFinished = true
                        self.appleFinalContinuation = nil
                    }
                    self.lock.unlock()
                    self.onPartialTranscript?(text)
                    if let cont {
                        cont.resume(returning: text)
                    }
                } else if let error {
                    self.lock.lock()
                    let fallback = self.latestAppleTranscript
                    let cont = self.appleFinalContinuation
                    self.appleFinished = true
                    self.appleFinalContinuation = nil
                    self.lock.unlock()
                    if let cont {
                        if !fallback.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                            cont.resume(returning: fallback)
                        } else {
                            cont.resume(throwing: error)
                        }
                    }
                }
            }
            lock.lock()
            self.speechTask = task
            lock.unlock()
        }

        installInputTap(on: eng, inputFormat: inputFormat)

        let observer = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange,
            object: eng,
            queue: nil
        ) { [weak self, weak eng] _ in
            guard let self, let eng else { return }
            self.handleConfigurationChange(for: eng)
        }
        lock.lock()
        self.engine = eng
        self.configChangeObserver = observer
        lock.unlock()

        eng.prepare()
        try eng.start()
    }

    private func installInputTap(on eng: AVAudioEngine, inputFormat: AVAudioFormat) {
        eng.inputNode.installTap(onBus: 0, bufferSize: 1024, format: inputFormat) { [weak self] buffer, when in
            guard let self else { return }
            let (req, conv, fmt): (SFSpeechAudioBufferRecognitionRequest?, AVAudioConverter?, AVAudioFormat?) = {
                self.lock.lock()
                defer { self.lock.unlock() }
                return (self.speechRequest, self.converter, self.targetFormat)
            }()
            req?.append(buffer)
            self.processBuffer(buffer, when: when, converter: conv, targetFormat: fmt)
        }
    }

    private func handleConfigurationChange(for eng: AVAudioEngine) {
        configQueue.async { [weak self, weak eng] in
            guard let self, let eng else { return }
            let outFormat: AVAudioFormat? = {
                self.lock.lock()
                defer { self.lock.unlock() }
                guard self.engine === eng else { return nil }
                return self.targetFormat
            }()
            guard let outFormat else { return }

            eng.inputNode.removeTap(onBus: 0)
            eng.stop()
            let newInputFormat = eng.inputNode.outputFormat(forBus: 0)
            guard newInputFormat.sampleRate > 0, newInputFormat.channelCount > 0,
                  let newConv = AVAudioConverter(from: newInputFormat, to: outFormat) else {
                self.onAudioInterrupted?("音频输入设备变化，录音已中断")
                return
            }
            self.lock.lock()
            guard self.engine === eng else {
                self.lock.unlock()
                return
            }
            self.converter = newConv
            self.lock.unlock()

            self.installInputTap(on: eng, inputFormat: newInputFormat)
            eng.prepare()
            do {
                try eng.start()
            } catch {
                self.onAudioInterrupted?("音频设备切换后无法继续录音")
            }
        }
    }

    /// Blanks what the microphone captures while a cue is audible, so it never reaches the recogniser.
    func muteInput(forSeconds seconds: TimeInterval) {
        let now = AVAudioTime.seconds(forHostTime: mach_absolute_time())
        lock.lock()
        cueMuteStart = now
        cueMuteEnd = now + seconds
        lock.unlock()
    }

    private func processBuffer(
        _ buffer: AVAudioPCMBuffer,
        when: AVAudioTime,
        converter: AVAudioConverter?,
        targetFormat: AVAudioFormat?
    ) {
        // HUD meter only: the audio sent to the recogniser is converted below, untouched by this.
        if let channelData = buffer.floatChannelData?[0] {
            let count = Int(buffer.frameLength)
            if count > 0 {
                // CoreAudio tap buffers are often ~100 ms; slicing into ~25 ms windows keeps
                // brief pauses tracking the noise floor while preserving syllable loudness peaks.
                let sampleRate = buffer.format.sampleRate
                let step = max(1, Int((sampleRate * 0.025).rounded()))
                var peakLevel: Float = 0
                var totalSum: Float = 0
                var offset = 0
                while offset < count {
                    let end = min(count, offset + step)
                    let chunk = end - offset
                    var sum: Float = 0
                    for i in offset..<end {
                        let s = channelData[i]
                        sum += s * s
                    }
                    totalSum += sum
                    let rms = sqrt(sum / Float(chunk))
                    let seconds = Float(Double(chunk) / sampleRate)
                    let level = levelMeter.process(rms: rms, seconds: seconds)
                    if level > peakLevel { peakLevel = level }
                    offset = end
                }
                let bufferRMS = sqrt(totalSum / Float(count))
                let bucket = Int((20 * log10(max(bufferRMS, 1e-6))).rounded()) + 90
                lock.lock()
                levelHistogram[max(0, min(90, bucket))] += 1
                lock.unlock()
                onAudioLevel?(peakLevel)
                if let onAudioSpectrum {
                    let frames = spectrumAnalyzer.process(samples: channelData, count: count, sampleRate: sampleRate)
                    if !frames.isEmpty {
                        onAudioSpectrum(frames)
                    }
                }
            }
        }

        // Convert to 16kHz mono Int16 for WAV upload / CLI
        guard let converter, let targetFormat else { return }
        let ratio = targetFormat.sampleRate / buffer.format.sampleRate
        let outFrames = AVAudioFrameCount(ceil(Double(buffer.frameLength) * ratio)) + 16
        guard let outBuffer = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: outFrames) else {
            return
        }

        var error: NSError?
        var consumed = false
        converter.convert(to: outBuffer, error: &error) { _, outStatus in
            if consumed {
                outStatus.pointee = .noDataNow
                return nil
            }
            consumed = true
            outStatus.pointee = .haveData
            return buffer
        }

        if error == nil, outBuffer.frameLength > 0, let int16Ptr = outBuffer.int16ChannelData?[0] {
            let count = Int(outBuffer.frameLength)
            var hitLimitNow = false
            lock.lock()
            let remaining = Self.maxRecordingSamples - pcm16Samples.count
            if remaining > 0 {
                let appendCount = min(count, remaining)
                let chunk = UnsafeBufferPointer(start: int16Ptr, count: appendCount)
                let firstNew = pcm16Samples.count
                pcm16Samples.append(contentsOf: chunk)
                if when.isHostTimeValid, cueMuteEnd > cueMuteStart {
                    let muted = VoiceCueMute.sampleRange(
                        bufferStart: AVAudioTime.seconds(forHostTime: when.hostTime),
                        count: appendCount,
                        sampleRate: targetFormat.sampleRate,
                        muteStart: cueMuteStart,
                        muteEnd: cueMuteEnd
                    )
                    for i in muted { pcm16Samples[firstNew + i] = 0 }
                }
                if pcm16Samples.count >= Self.maxRecordingSamples && !didReachMaxDuration {
                    didReachMaxDuration = true
                    hitLimitNow = true
                }
            } else if !didReachMaxDuration {
                didReachMaxDuration = true
                hitLimitNow = true
            }
            lock.unlock()
            if hitLimitNow {
                onMaxDurationReached?()
            }
        }
    }

    public func stopAndCollectSamples() -> [Int16] {
        let (eng, observer, req): (AVAudioEngine?, NSObjectProtocol?, SFSpeechAudioBufferRecognitionRequest?) = {
            lock.lock()
            defer { lock.unlock() }
            let e = engine
            let o = configChangeObserver
            let r = speechRequest
            engine = nil
            configChangeObserver = nil
            return (e, o, r)
        }()
        if let observer {
            NotificationCenter.default.removeObserver(observer)
        }
        eng?.inputNode.removeTap(onBus: 0)
        eng?.stop()
        req?.endAudio()

        lock.lock()
        converter = nil
        targetFormat = nil
        let out = pcm16Samples
        pcm16Samples.removeAll(keepingCapacity: false)
        lock.unlock()
        return out
    }

    /// Copies at most the newest `maxCount` samples while recording continues.
    /// Input loudness percentiles in dBFS for the timing log.
    public func levelSummary() -> String {
        lock.lock()
        let histogram = levelHistogram
        lock.unlock()
        let total = histogram.reduce(0, +)
        guard total > 0 else { return "" }
        let percentile = { (p: Int) -> Int in
            var seen = 0
            for (i, n) in histogram.enumerated() {
                seen += n
                if seen * 100 >= total * p { return i - 90 }
            }
            return 0
        }
        let top = (histogram.lastIndex { $0 > 0 } ?? 0) - 90
        return "dbfs_p10=\(percentile(10)) dbfs_p50=\(percentile(50)) dbfs_p90=\(percentile(90)) dbfs_p99=\(percentile(99)) dbfs_max=\(top) buffers=\(total)"
    }

    public func snapshotRecentSamples(maxCount: Int) -> (samples: [Int16], total: Int) {
        lock.lock()
        defer { lock.unlock() }
        let total = pcm16Samples.count
        return (Array(pcm16Samples.suffix(max(0, maxCount))), total)
    }

    public func finishAppleSpeech(timeoutSeconds: Double = 3.5) async throws -> String {
        let alreadyResult: String? = {
            lock.lock()
            defer { lock.unlock() }
            return appleFinished ? latestAppleTranscript : nil
        }()
        if let alreadyResult {
            teardownAppleSpeech()
            return alreadyResult
        }

        final class TimeoutBox: @unchecked Sendable {
            var item: DispatchWorkItem?
            func cancel() { item?.cancel(); item = nil }
        }
        let timeoutBox = TimeoutBox()
        defer { timeoutBox.cancel() }

        let text: String = try await withCheckedThrowingContinuation { cont in
            lock.lock()
            if appleFinished {
                let t = latestAppleTranscript
                lock.unlock()
                cont.resume(returning: t)
                return
            }
            appleFinalContinuation = cont
            lock.unlock()

            let item = DispatchWorkItem { [weak self] in
                guard let self else { return }
                self.lock.lock()
                let pending = self.appleFinalContinuation
                let fallback = self.latestAppleTranscript
                self.appleFinalContinuation = nil
                self.appleFinished = true
                self.lock.unlock()
                pending?.resume(returning: fallback)
            }
            timeoutBox.item = item
            DispatchQueue.global().asyncAfter(deadline: .now() + timeoutSeconds, execute: item)
        }
        teardownAppleSpeech()
        return text
    }

    public func cancel() {
        let (eng, observer): (AVAudioEngine?, NSObjectProtocol?) = {
            lock.lock()
            defer { lock.unlock() }
            let e = engine
            let o = configChangeObserver
            engine = nil
            configChangeObserver = nil
            return (e, o)
        }()
        if let observer {
            NotificationCenter.default.removeObserver(observer)
        }
        eng?.inputNode.removeTap(onBus: 0)
        eng?.stop()
        teardownAppleSpeech()
        lock.lock()
        converter = nil
        targetFormat = nil
        pcm16Samples.removeAll(keepingCapacity: false)
        latestAppleTranscript = ""
        let pending = appleFinalContinuation
        appleFinalContinuation = nil
        appleFinished = true
        lock.unlock()
        pending?.resume(throwing: VoiceEngineError.cancelled)
    }

    private func teardownAppleSpeech() {
        let (req, task): (SFSpeechAudioBufferRecognitionRequest?, SFSpeechRecognitionTask?) = {
            lock.lock()
            defer { lock.unlock() }
            let r = speechRequest
            let t = speechTask
            speechRequest = nil
            speechTask = nil
            speechRecognizer = nil
            return (r, t)
        }()
        req?.endAudio()
        task?.cancel()
    }
}

// MARK: - Cloud BYOK & Local HTTP/CLI Pipeline Runner

public final class VoicePipelineClient: Sendable {
    private let session: URLSession

    public init() {
        let cfg = URLSessionConfiguration.ephemeral
        cfg.timeoutIntervalForRequest = 30
        cfg.timeoutIntervalForResource = 60
        self.session = URLSession(configuration: cfg)
    }

    public func invalidate() {
        session.invalidateAndCancel()
    }

    public func transcribeOpenAICompatible(
        baseURL: String,
        apiKey: String?,
        model: String,
        language: String?,
        prompt: String?,
        wavData: Data
    ) async throws -> String {
        let trimmedBase = baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard let url = URL(string: "\(trimmedBase)/audio/transcriptions") else {
            throw VoiceEngineError.invalidEndpoint(baseURL)
        }

        let boundary = "JuxuVoice-\(UUID().uuidString)"
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        if let apiKey, !apiKey.isEmpty {
            req.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = VoiceMultipartBuilder.buildAudioTranscriptionBody(
            boundary: boundary,
            wavData: wavData,
            model: model,
            language: language,
            prompt: prompt
        )

        let (data, response) = try await session.data(for: req)
        try validateHTTP(response: response, data: data)

        if let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let text = json["text"] as? String {
            return text.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        if let plain = String(data: data, encoding: .utf8) {
            return plain.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        throw VoiceEngineError.emptyTranscript
    }

    public func polishOpenAICompatible(
        baseURL: String,
        apiKey: String?,
        model: String,
        rawTranscript: String,
        toneMode: VoiceToneMode,
        context: VoiceContext
    ) async throws -> String {
        let trimmedBase = baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard let url = URL(string: "\(trimmedBase)/chat/completions") else {
            throw VoiceEngineError.invalidEndpoint(baseURL)
        }

        let messages = VoicePromptBuilder.polishMessages(
            rawTranscript: rawTranscript,
            toneMode: toneMode,
            context: context
        )
        let payload: [String: Any] = [
            "model": model,
            "temperature": 0.1,
            "messages": messages,
        ]

        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let apiKey, !apiKey.isEmpty {
            req.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = try JSONSerialization.data(withJSONObject: payload)

        let (data, response) = try await session.data(for: req)
        try validateHTTP(response: response, data: data)

        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let choices = json["choices"] as? [[String: Any]],
              let first = choices.first,
              let message = first["message"] as? [String: Any],
              let content = message["content"] as? String else {
            throw VoiceEngineError.networkError("整理接口返回格式无法解析")
        }
        let cleaned = content.trimmingCharacters(in: .whitespacesAndNewlines)
        return cleaned.isEmpty ? rawTranscript : cleaned
    }

    /// Runs `whisper-cli` (`whisper.cpp`) or `mlx_whisper` without blocking cooperative threads or 64KB pipe deadlocks.
    public func transcribeLocalCLI(
        binaryPath: String,
        modelPath: String,
        language: String,
        wavData: Data
    ) async throws -> String {
        let trimmedBin = binaryPath.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedBin.isEmpty, FileManager.default.isExecutableFile(atPath: trimmedBin) else {
            throw VoiceEngineError.cliNotFound(trimmedBin.isEmpty ? "whisper-cli / mlx_whisper" : trimmedBin)
        }

        let tempDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("juxu-voice-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let wavURL = tempDir.appendingPathComponent("input.wav")
        try wavData.write(to: wavURL)

        let binName = URL(fileURLWithPath: trimmedBin).lastPathComponent.lowercased()
        let trimmedModel = modelPath.trimmingCharacters(in: .whitespacesAndNewlines)
        let args: [String]
        if binName.contains("mlx_whisper") || binName.contains("mlx-whisper") {
            var mlxArgs = [
                wavURL.path,
                "--output-format", "txt",
                "--output-dir", tempDir.path,
            ]
            if !trimmedModel.isEmpty {
                mlxArgs.append(contentsOf: ["--model", trimmedModel])
            }
            if !language.isEmpty && language != "auto" {
                mlxArgs.append(contentsOf: ["--language", language])
            }
            args = mlxArgs
        } else {
            // `whisper.cpp` (`whisper-cli` / `whisper-cpp`)
            var cppArgs = ["-f", wavURL.path, "--no-timestamps", "-nt"]
            if !trimmedModel.isEmpty {
                cppArgs.append(contentsOf: ["-m", trimmedModel])
            }
            if !language.isEmpty && language != "auto" {
                cppArgs.append(contentsOf: ["-l", language])
            }
            args = cppArgs
        }

        let result = try await VoiceSubprocessRunner.run(
            executablePath: trimmedBin,
            arguments: args
        )

        if result.terminationStatus != 0 {
            let errMsg = String(data: result.stderr, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? "退出码 \(result.terminationStatus)"
            throw VoiceEngineError.cliFailed(String(errMsg.prefix(160)))
        }

        let txtFile = tempDir.appendingPathComponent("input.txt")
        if FileManager.default.fileExists(atPath: txtFile.path),
           let fileText = try? String(contentsOf: txtFile, encoding: .utf8) {
            return fileText.trimmingCharacters(in: .whitespacesAndNewlines)
        }

        return String(data: result.stdout, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    }

    public func polishLocalLlamaCLI(
        binaryPath: String,
        modelPath: String,
        rawTranscript: String,
        toneMode: VoiceToneMode,
        context: VoiceContext
    ) async throws -> String {
        guard FileManager.default.isExecutableFile(atPath: binaryPath) else {
            throw VoiceEngineError.cliNotFound(binaryPath)
        }
        guard FileManager.default.fileExists(atPath: modelPath) else {
            throw VoiceEngineError.cliFailed("未找到本地整理模型文件，请在设置中点击「一键下载并安装」")
        }

        let messages = VoicePromptBuilder.polishMessages(
            rawTranscript: rawTranscript,
            toneMode: toneMode,
            context: context
        )
        let systemPrompt = messages.first?["content"] ?? ""
        let userPrompt = messages.last?["content"] ?? rawTranscript
        let chatML = "<|im_start|>system\n\(systemPrompt)<|im_end|>\n<|im_start|>user\n\(userPrompt)<|im_end|>\n<|im_start|>assistant\n"

        let result = try await VoiceSubprocessRunner.run(
            executablePath: binaryPath,
            arguments: [
                "-m", modelPath,
                "-ngl", "99",
                "-c", "2048",
                "-n", "512",
                "--temp", "0.1",
                "--no-display-prompt",
                "-no-cnv",
                "-p", chatML,
            ]
        )

        if result.terminationStatus != 0 {
            let errMsg = String(data: result.stderr, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? "退出码 \(result.terminationStatus)"
            throw VoiceEngineError.cliFailed(String(errMsg.suffix(160)))
        }

        var outText = String(data: result.stdout, encoding: .utf8) ?? ""
        for marker in ["<|im_end|>", "[end of text]", "<|endoftext|>"] {
            outText = outText.replacingOccurrences(of: marker, with: "")
        }
        let cleaned = outText.trimmingCharacters(in: .whitespacesAndNewlines)
        return cleaned.isEmpty ? rawTranscript : cleaned
    }

    private func validateHTTP(response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else {
            throw VoiceEngineError.networkError("无效的 HTTP 响应")
        }
        guard (200..<300).contains(http.statusCode) else {
            var detail = "HTTP \(http.statusCode)"
            if let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let err = json["error"] as? [String: Any],
               let msg = err["message"] as? String {
                detail += "：\(msg)"
            } else if let raw = String(data: data, encoding: .utf8), !raw.isEmpty {
                detail += "：\(String(raw.prefix(140)))"
            }
            throw VoiceEngineError.networkError(detail)
        }
    }
}
