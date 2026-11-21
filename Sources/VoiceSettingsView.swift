import AppKit
import SwiftUI

public struct VoiceSettingsView: View {
    @ObservedObject public var bridge: WubiBridge
    @ObservedObject public var voiceStore: VoicePreferencesStore

    @State private var asrKeyDraft: String = ""
    @State private var llmKeyDraft: String = ""
    @State private var keysLoaded: Bool = false
    @State private var keyStatusBanner: String = ""
    @State private var testStatus: String = ""
    @State private var testOK: Bool = true
    @State private var isTesting: Bool = false
    @State private var detectedCLI: String? = nil

    public init(bridge: WubiBridge, voiceStore: VoicePreferencesStore) {
        self.bridge = bridge
        self.voiceStore = voiceStore
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 3) {
                Text("语音识别与智能整理")
                    .font(.system(size: 17, weight: .semibold))
                Text("支持云端 BYOK 与本机模型双方案，按需加载；未开启时不占内存、不申请麦克风权限")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }

            // Master Switch Card — always visible, zero side effects when off
            card {
                VStack(alignment: .leading, spacing: 10) {
                    HStack(alignment: .center, spacing: 12) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("开启句序语音输入入口")
                                .font(.system(size: 13.5, weight: .semibold))
                            Text(
                                voiceStore.prefs.enabled
                                    ? "已激活：\(voiceStore.prefs.triggerKey.displayName)即可说话，松手自动转写并上屏（Esc 取消）"
                                    : "默认关闭。关闭状态下完全不加载音频引擎、语音识别器、网络会话与悬浮窗"
                            )
                            .font(.system(size: 11.5))
                            .foregroundStyle(.secondary)
                        }
                        Spacer()
                        Toggle("", isOn: voiceBinding(\.enabled))
                            .labelsHidden()
                            .toggleStyle(.switch)
                            .controlSize(.small)
                    }

                    if !voiceStore.prefs.enabled {
                        Divider()
                        HStack(spacing: 8) {
                            Image(systemName: "bolt.shield")
                                .font(.system(size: 12))
                                .foregroundStyle(.secondary)
                            Text("按需加载说明：开启后才提供云端 BYOK / 本地模型配置与快捷键监听；随时关闭即立刻释放全部资源。")
                                .font(.system(size: 11.5))
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }

            // Everything below is ONLY rendered & loaded when voice entry is enabled!
            if voiceStore.prefs.enabled {
                providerCard
                triggerAndBehaviorCard
                if voiceStore.prefs.providerMode == .cloudBYOK {
                    cloudBYOKCard
                } else {
                    localModelCard
                }
                vocabularyAndTestCard
            }
        }
        .onAppear {
            if voiceStore.prefs.enabled {
                loadSecretsIfNeeded()
            }
        }
        .onChange(of: voiceStore.prefs.enabled) { _, newValue in
            if newValue {
                loadSecretsIfNeeded()
            } else {
                asrKeyDraft = ""
                llmKeyDraft = ""
                keysLoaded = false
                keyStatusBanner = ""
                testStatus = ""
                detectedCLI = nil
            }
        }
    }

    // MARK: - Provider Mode Selector

    private var providerCard: some View {
        card {
            VStack(alignment: .leading, spacing: 10) {
                Text("识别与整理方案")
                    .font(.system(size: 13, weight: .medium))
                HStack(spacing: 10) {
                    providerOptionCard(.cloudBYOK)
                    providerOptionCard(.local)
                }
            }
        }
    }

    private func providerOptionCard(_ mode: VoiceProviderMode) -> some View {
        let selected = voiceStore.prefs.providerMode == mode
        return Button {
            voiceStore.update { $0.providerMode = mode }
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(mode.displayName)
                        .font(.system(size: 13, weight: .semibold))
                    Spacer()
                    Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                        .foregroundStyle(selected ? Color.accentColor : Color.secondary.opacity(0.5))
                }
                Text(mode.summary)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(selected ? Color.accentColor.opacity(0.11) : Color.primary.opacity(0.03))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(
                        selected ? Color.accentColor : Color.primary.opacity(0.10),
                        lineWidth: selected ? 1.2 : 0.6
                    )
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Trigger & Behavior

    private var triggerAndBehaviorCard: some View {
        card {
            VStack(spacing: 11) {
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("触发快捷键")
                            .font(.system(size: 13, weight: .medium))
                        Text("在句序激活状态下生效；长按模式需按住超过 0.22 秒以避免误触组合键")
                            .font(.system(size: 11.5))
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Picker("", selection: voiceBinding(\.triggerKey)) {
                        ForEach(VoiceTriggerKey.allCases) { key in
                            Text(key.displayName).tag(key)
                        }
                    }
                    .labelsHidden()
                    .frame(width: 210)
                }

                Divider()

                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("输出整理模式")
                            .font(.system(size: 13, weight: .medium))
                        Text("智能整理会自动去口水词、修正口误并规范标点排版")
                            .font(.system(size: 11.5))
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Picker("", selection: voiceBinding(\.outputMode)) {
                        ForEach(VoiceOutputMode.allCases) { mode in
                            Text(mode.displayName).tag(mode)
                        }
                    }
                    .labelsHidden()
                    .frame(width: 210)
                }

                if voiceStore.prefs.outputMode == .smartPolish {
                    Divider()
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("语气与风格")
                                .font(.system(size: 13, weight: .medium))
                            Text("自动模式会根据当前宿主应用（聊天 / IDE / 邮件文档）调整语气")
                                .font(.system(size: 11.5))
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        Picker("", selection: voiceBinding(\.toneMode)) {
                            ForEach(VoiceToneMode.allCases) { tone in
                                Text(tone.displayName).tag(tone)
                            }
                        }
                        .labelsHidden()
                        .frame(width: 210)
                    }
                }

                Divider()

                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("选中文本语音改写（Speak to Edit）")
                            .font(.system(size: 13, weight: .medium))
                        Text("先选中一段文字再触发语音说出修改指令（如“改正式一点 / 翻成英文”），直接替换选区")
                            .font(.system(size: 11.5))
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Toggle("", isOn: voiceBinding(\.speakToEditSelection))
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .controlSize(.small)
                }
            }
        }
    }

    // MARK: - Cloud BYOK Card

    private var cloudBYOKCard: some View {
        card {
            VStack(alignment: .leading, spacing: 11) {
                HStack {
                    Text("云端 BYOK 接口配置（OpenAI 兼容协议）")
                        .font(.system(size: 13, weight: .semibold))
                    Spacer()
                    Text("API Key 仅存于 macOS 钥匙串")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }

                HStack(spacing: 6) {
                    Text("快速填充：")
                        .font(.system(size: 11.5))
                        .foregroundStyle(.secondary)
                    presetButton("Groq Whisper") {
                        voiceStore.update {
                            $0.cloudASRBaseURL = "https://api.groq.com/openai/v1"
                            $0.cloudASRModel = "whisper-large-v3-turbo"
                        }
                    }
                    presetButton("OpenAI 全套") {
                        voiceStore.update {
                            $0.cloudASRBaseURL = "https://api.openai.com/v1"
                            $0.cloudASRModel = "gpt-4o-mini-transcribe"
                            $0.cloudLLMBaseURL = "https://api.openai.com/v1"
                            $0.cloudLLMModel = "gpt-4o-mini"
                            $0.cloudReuseASRKeyForLLM = true
                        }
                    }
                    presetButton("DeepSeek 整理") {
                        voiceStore.update {
                            $0.cloudLLMBaseURL = "https://api.deepseek.com/v1"
                            $0.cloudLLMModel = "deepseek-chat"
                            $0.cloudReuseASRKeyForLLM = false
                        }
                    }
                    presetButton("通义千问整理") {
                        voiceStore.update {
                            $0.cloudLLMBaseURL = "https://dashscope.aliyuncs.com/compatible-mode/v1"
                            $0.cloudLLMModel = "qwen-turbo"
                            $0.cloudReuseASRKeyForLLM = false
                        }
                    }
                }

                Divider()

                fieldRow(title: "ASR Base URL") {
                    TextField("https://api.groq.com/openai/v1", text: voiceBinding(\.cloudASRBaseURL))
                        .textFieldStyle(.roundedBorder)
                }
                fieldRow(title: "ASR 模型") {
                    TextField("whisper-large-v3-turbo", text: voiceBinding(\.cloudASRModel))
                        .textFieldStyle(.roundedBorder)
                }
                fieldRow(title: "ASR API Key") {
                    HStack(spacing: 6) {
                        SecureField("sk-... / gsk_...", text: $asrKeyDraft)
                            .textFieldStyle(.roundedBorder)
                        Button("保存") {
                            let ok = VoiceKeychainStore.write(asrKeyDraft, for: .cloudASRAPIKey)
                            keyStatusBanner = ok ? "✓ 已将 ASR Key 保存至钥匙串" : "保存 ASR Key 失败"
                        }
                        .controlSize(.small)
                    }
                }

                Divider()

                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("启用云端 LLM 口语整理")
                            .font(.system(size: 13, weight: .medium))
                        Text("关闭后仅使用云端 ASR，不做二次整理")
                            .font(.system(size: 11.5))
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Toggle("", isOn: voiceBinding(\.cloudPolishEnabled))
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .controlSize(.small)
                }

                if voiceStore.prefs.cloudPolishEnabled {
                    fieldRow(title: "LLM Base URL") {
                        TextField("https://api.openai.com/v1", text: voiceBinding(\.cloudLLMBaseURL))
                            .textFieldStyle(.roundedBorder)
                    }
                    fieldRow(title: "LLM 模型") {
                        TextField("gpt-4o-mini", text: voiceBinding(\.cloudLLMModel))
                            .textFieldStyle(.roundedBorder)
                    }
                    HStack {
                        Text("LLM 复用 ASR API Key")
                            .font(.system(size: 12.5))
                        Spacer()
                        Toggle("", isOn: voiceBinding(\.cloudReuseASRKeyForLLM))
                            .labelsHidden()
                            .toggleStyle(.switch)
                            .controlSize(.small)
                    }
                    if !voiceStore.prefs.cloudReuseASRKeyForLLM {
                        fieldRow(title: "LLM API Key") {
                            HStack(spacing: 6) {
                                SecureField("sk-...", text: $llmKeyDraft)
                                    .textFieldStyle(.roundedBorder)
                                Button("保存") {
                                    let ok = VoiceKeychainStore.write(llmKeyDraft, for: .cloudLLMAPIKey)
                                    keyStatusBanner = ok ? "✓ 已将 LLM Key 保存至钥匙串" : "保存 LLM Key 失败"
                                }
                                .controlSize(.small)
                            }
                        }
                    }
                }

                if !keyStatusBanner.isEmpty {
                    Text(keyStatusBanner)
                        .font(.system(size: 11.5))
                        .foregroundStyle(Color.accentColor)
                }
            }
        }
    }

    // MARK: - Local Model Card

    private var localModelCard: some View {
        card {
            VStack(alignment: .leading, spacing: 11) {
                Text("本地模型配置（按需调用，不常驻后台）")
                    .font(.system(size: 13, weight: .semibold))

                HStack {
                    Text("本地语音识别引擎")
                        .font(.system(size: 12.5, weight: .medium))
                    Spacer()
                    Picker("", selection: voiceBinding(\.localASRKind)) {
                        ForEach(VoiceLocalASRKind.allCases) { kind in
                            Text(kind.displayName).tag(kind)
                        }
                    }
                    .labelsHidden()
                    .frame(width: 260)
                }

                switch voiceStore.prefs.localASRKind {
                case .appleSpeech:
                    fieldRow(title: "识别语言") {
                        TextField("zh-CN", text: voiceBinding(\.localAppleLocale))
                            .textFieldStyle(.roundedBorder)
                    }
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("强制纯离线识别（不上传音频）")
                                .font(.system(size: 12.5))
                            Text("需系统已下载对应语言听写模型；若提示不可用可关闭此项")
                                .font(.system(size: 11))
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        Toggle("", isOn: voiceBinding(\.localAppleRequireOnDevice))
                            .labelsHidden()
                            .toggleStyle(.switch)
                            .controlSize(.small)
                    }
                case .cli:
                    fieldRow(title: "可执行文件路径") {
                        HStack(spacing: 6) {
                            TextField("留空自动探测 whisper-cli / whisper", text: voiceBinding(\.localCLIBinaryPath))
                                .textFieldStyle(.roundedBorder)
                            Button("探测") {
                                detectedCLI = VoiceLocalCLIDiscovery.detectInstalledBinary()
                                if let found = detectedCLI {
                                    voiceStore.update { $0.localCLIBinaryPath = found }
                                }
                            }
                            .controlSize(.small)
                        }
                    }
                    fieldRow(title: "模型路径/名称") {
                        TextField("whisper-cli 填 ggml 模型路径；whisper 填 turbo/medium", text: voiceBinding(\.localCLIModelPath))
                            .textFieldStyle(.roundedBorder)
                    }
                    if let detectedCLI {
                        Text("已检测到：\(detectedCLI)")
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                    }
                case .http:
                    fieldRow(title: "本地 ASR 地址") {
                        TextField("http://127.0.0.1:8080/v1", text: voiceBinding(\.localHTTPASRBaseURL))
                            .textFieldStyle(.roundedBorder)
                    }
                    fieldRow(title: "本地 ASR 模型") {
                        TextField("whisper-1", text: voiceBinding(\.localHTTPASRModel))
                            .textFieldStyle(.roundedBorder)
                    }
                }

                Divider()

                HStack {
                    Text("本地口语整理方式")
                        .font(.system(size: 12.5, weight: .medium))
                    Spacer()
                    Picker("", selection: voiceBinding(\.localPolishKind)) {
                        ForEach(VoiceLocalPolishKind.allCases) { kind in
                            Text(kind.displayName).tag(kind)
                        }
                    }
                    .labelsHidden()
                    .frame(width: 260)
                }

                if voiceStore.prefs.localPolishKind == .http {
                    fieldRow(title: "本地 LLM 地址") {
                        TextField("http://127.0.0.1:11434/v1", text: voiceBinding(\.localHTTPLLMBaseURL))
                            .textFieldStyle(.roundedBorder)
                    }
                    fieldRow(title: "本地 LLM 模型") {
                        TextField("qwen3:4b", text: voiceBinding(\.localHTTPLLMModel))
                            .textFieldStyle(.roundedBorder)
                    }
                }
            }
        }
    }

    // MARK: - Vocabulary & Connectivity Test

    private var vocabularyAndTestCard: some View {
        card {
            VStack(alignment: .leading, spacing: 11) {
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("联动句序个人词库（\(bridge.userPhrases.count) 条）")
                            .font(.system(size: 13, weight: .medium))
                        Text("自动把句序里常用的自造词作为热词注入语音识别与整理")
                            .font(.system(size: 11.5))
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Toggle("", isOn: voiceBinding(\.syncJuxuLexicon))
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .controlSize(.small)
                }

                Divider()

                VStack(alignment: .leading, spacing: 4) {
                    Text("自定义语音热词（逗号或换行分隔）")
                        .font(.system(size: 12.5, weight: .medium))
                    TextField("例如：句序, Juxu, RWKV-7, Typeless, SwiftUI", text: voiceBinding(\.customVocabulary))
                        .textFieldStyle(.roundedBorder)
                }

                Divider()

                HStack {
                    Button(isTesting ? "检测中…" : "检测当前语音配置") {
                        guard !isTesting else { return }
                        isTesting = true
                        testStatus = ""
                        Task { @MainActor in
                            let res = await VoiceSessionController.shared.testCurrentConfiguration()
                            testOK = res.ok
                            testStatus = res.message
                            isTesting = false
                        }
                    }
                    .controlSize(.small)
                    .disabled(isTesting)

                    Spacer()

                    Text("配置文件：\(voiceStore.fileURL.lastPathComponent)")
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                }

                if !testStatus.isEmpty {
                    Text(testStatus)
                        .font(.system(size: 11.5))
                        .foregroundStyle(testOK ? Color.green : Color.red)
                        .textSelection(.enabled)
                }
            }
        }
    }

    // MARK: - Helpers

    private func loadSecretsIfNeeded() {
        guard !keysLoaded else { return }
        keysLoaded = true
        asrKeyDraft = VoiceKeychainStore.read(.cloudASRAPIKey)
        llmKeyDraft = VoiceKeychainStore.read(.cloudLLMAPIKey)
    }

    private func presetButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(title, action: action)
            .controlSize(.mini)
    }

    private func fieldRow<Content: View>(title: String, @ViewBuilder content: () -> Content) -> some View {
        HStack(alignment: .center, spacing: 10) {
            Text(title)
                .font(.system(size: 12.5))
                .foregroundStyle(.secondary)
                .frame(width: 96, alignment: .leading)
            content()
        }
    }

    private func card<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        content()
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(Color(nsColor: .controlBackgroundColor))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(Color.primary.opacity(0.08), lineWidth: 0.6)
            )
    }

    private func voiceBinding<T>(_ keyPath: WritableKeyPath<VoicePreferences, T>) -> Binding<T> {
        Binding(
            get: { voiceStore.prefs[keyPath: keyPath] },
            set: { newValue in
                voiceStore.update { $0[keyPath: keyPath] = newValue }
            }
        )
    }
}
