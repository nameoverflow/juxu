import Foundation
import Security

public enum VoiceProviderMode: String, Codable, CaseIterable, Identifiable, Sendable {
    case cloudBYOK = "cloud_byok"
    case local = "local"

    public var id: String { rawValue }

    public var displayName: String {
        switch self {
        case .cloudBYOK: return "云端 BYOK"
        case .local: return "本地模型"
        }
    }

    public var summary: String {
        switch self {
        case .cloudBYOK: return "自备 API Key（OpenAI / Groq / DeepSeek / 通义 / Gemini 等兼容接口）"
        case .local: return "完全本机运行（Apple 端侧语音 / 本地 Whisper CLI / Ollama 等）"
        }
    }
}

public enum VoiceTriggerKey: String, Codable, CaseIterable, Identifiable, Sendable {
    case rightOption = "right_option"
    case rightCommand = "right_command"
    case fnKey = "fn_key"
    case optionSpace = "option_space"

    public var id: String { rawValue }

    public var displayName: String {
        switch self {
        case .rightOption: return "按住右 Option 键"
        case .rightCommand: return "按住右 Command 键"
        case .fnKey: return "按住 Fn / 地球键"
        case .optionSpace: return "点按 Option + Space 启停"
        }
    }

    public var isHoldToTalk: Bool {
        self != .optionSpace
    }

    public var modifierKeyCode: UInt16? {
        switch self {
        case .rightOption: return 0x3D
        case .rightCommand: return 0x36
        case .fnKey: return 0x3F
        case .optionSpace: return nil
        }
    }
}

public enum VoiceOutputMode: String, Codable, CaseIterable, Identifiable, Sendable {
    case smartPolish = "smart_polish"
    case rawDictation = "raw_dictation"

    public var id: String { rawValue }

    public var displayName: String {
        switch self {
        case .smartPolish: return "智能整理（推荐）"
        case .rawDictation: return "原始听写（原话直出）"
        }
    }
}

public enum VoiceToneMode: String, Codable, CaseIterable, Identifiable, Sendable {
    case autoByApp = "auto_by_app"
    case casual = "casual"
    case professional = "professional"
    case concise = "concise"

    public var id: String { rawValue }

    public var displayName: String {
        switch self {
        case .autoByApp: return "按当前应用自动适配"
        case .casual: return "自然口语（聊天）"
        case .professional: return "清晰书面（邮件/文档）"
        case .concise: return "精炼要点（列表/结论）"
        }
    }
}

public enum VoiceLocalASRKind: String, Codable, CaseIterable, Identifiable, Sendable {
    case appleSpeech = "apple_speech"
    case cli = "cli"
    case http = "http"

    public var id: String { rawValue }

    public var displayName: String {
        switch self {
        case .appleSpeech: return "Apple 端侧语音识别（零配置）"
        case .cli: return "本地 Whisper CLI（按需拉起）"
        case .http: return "本地 HTTP 接口（whisper-server / MLX）"
        }
    }
}

public enum VoiceLocalPolishKind: String, Codable, CaseIterable, Identifiable, Sendable {
    case ruleBased = "rule_based"
    case http = "http"
    case none = "none"

    public var id: String { rawValue }

    public var displayName: String {
        switch self {
        case .ruleBased: return "轻量本地规则整理（零内存）"
        case .http: return "本地 LLM 服务（Ollama / LM Studio / MLX）"
        case .none: return "仅原始转写，不整理"
        }
    }
}

public struct VoicePreferences: Codable, Equatable, Sendable {
    /// Master switch: defaults to false so zero voice resources are loaded unless explicitly enabled.
    public var enabled: Bool = false
    public var providerMode: VoiceProviderMode = .cloudBYOK
    public var triggerKey: VoiceTriggerKey = .rightOption
    public var outputMode: VoiceOutputMode = .smartPolish
    public var toneMode: VoiceToneMode = .autoByApp
    public var language: String = "zh"
    public var speakToEditSelection: Bool = true
    public var syncJuxuLexicon: Bool = true
    public var customVocabulary: String = ""

    // Cloud BYOK (OpenAI-compatible ASR + Chat Completions)
    public var cloudASRBaseURL: String = "https://api.groq.com/openai/v1"
    public var cloudASRModel: String = "whisper-large-v3-turbo"
    public var cloudPolishEnabled: Bool = true
    public var cloudLLMBaseURL: String = "https://api.openai.com/v1"
    public var cloudLLMModel: String = "gpt-4o-mini"
    public var cloudReuseASRKeyForLLM: Bool = false

    // Local models (on-demand)
    public var localASRKind: VoiceLocalASRKind = .appleSpeech
    public var localAppleLocale: String = "zh-CN"
    public var localAppleRequireOnDevice: Bool = true
    public var localCLIBinaryPath: String = ""
    public var localCLIModelPath: String = ""
    public var localHTTPASRBaseURL: String = "http://127.0.0.1:8080/v1"
    public var localHTTPASRModel: String = "whisper-1"

    public var localPolishKind: VoiceLocalPolishKind = .ruleBased
    public var localHTTPLLMBaseURL: String = "http://127.0.0.1:11434/v1"
    public var localHTTPLLMModel: String = "qwen3:4b"

    public init() {}

    public var parsedCustomVocabulary: [String] {
        let separators = CharacterSet(charactersIn: ",，\n;；")
        return customVocabulary
            .components(separatedBy: separators)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
    }
}

public enum VoiceKeychainAccount: String, Sendable {
    case cloudASRAPIKey = "cloud-asr-api-key"
    case cloudLLMAPIKey = "cloud-llm-api-key"
}

public enum VoiceKeychainStore {
    private static let service = "com.hcyue.juxu.voice"

    public static func read(_ account: VoiceKeychainAccount) -> String {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account.rawValue,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess,
              let data = result as? Data,
              let value = String(data: data, encoding: .utf8) else {
            return ""
        }
        return value
    }

    @discardableResult
    public static func write(_ value: String, for account: VoiceKeychainAccount) -> Bool {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            return delete(account)
        }
        let data = Data(trimmed.utf8)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account.rawValue,
        ]
        let update: [String: Any] = [kSecValueData as String: data]
        let status = SecItemUpdate(query as CFDictionary, update as CFDictionary)
        if status == errSecItemNotFound {
            var addQuery = query
            addQuery[kSecValueData as String] = data
            addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
            return SecItemAdd(addQuery as CFDictionary, nil) == errSecSuccess
        }
        return status == errSecSuccess
    }

    @discardableResult
    public static func delete(_ account: VoiceKeychainAccount) -> Bool {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account.rawValue,
        ]
        let status = SecItemDelete(query as CFDictionary)
        return status == errSecSuccess || status == errSecItemNotFound
    }
}

@MainActor
public final class VoicePreferencesStore: ObservableObject {
    public static let shared = VoicePreferencesStore()

    @Published public private(set) var prefs: VoicePreferences
    public let fileURL: URL
    private let inMemoryOnly: Bool

    public init(fileURL: URL? = nil, inMemoryOnly: Bool = false) {
        self.inMemoryOnly = inMemoryOnly
        if let fileURL {
            self.fileURL = fileURL
        } else {
            let home = URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true)
            self.fileURL = home
                .appendingPathComponent("Library/Application Support/Juxu", isDirectory: true)
                .appendingPathComponent("voice.json")
        }
        if !inMemoryOnly,
           FileManager.default.fileExists(atPath: self.fileURL.path),
           let data = try? Data(contentsOf: self.fileURL),
           let decoded = try? JSONDecoder().decode(VoicePreferences.self, from: data) {
            self.prefs = decoded
        } else {
            self.prefs = VoicePreferences()
        }
    }

    public func update(_ newPrefs: VoicePreferences, persist: Bool = true) {
        let wasEnabled = prefs.enabled
        self.prefs = newPrefs
        if persist && !inMemoryOnly {
            save()
        }
        if wasEnabled && !newPrefs.enabled {
            VoiceSessionController.unload()
        }
    }

    public func update(_ mutate: (inout VoicePreferences) -> Void, persist: Bool = true) {
        var copy = prefs
        mutate(&copy)
        update(copy, persist: persist)
    }

    private func save() {
        let dir = fileURL.deletingLastPathComponent()
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        if let data = try? encoder.encode(prefs) {
            try? data.write(to: fileURL, options: .atomic)
        }
    }
}
