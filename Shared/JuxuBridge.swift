import Foundation
#if os(iOS)
import Security

// The Personal Team profile grants shared keychain groups for this team's apps.
private enum IOSSharedSettings {
    static let preferencesAccount = "input-settings"
    static let touchProfileAccount = "touch-profile"
    static let evalLogAccount = "eval-log-ring"
    static let userDataAccount = "user-data-zlib"

    static func parseUserDataSeq(_ tsv: String) -> UInt64 {
        for line in tsv.prefix(512).split(separator: "\n", omittingEmptySubsequences: true) {
            if line.hasPrefix("SEQ\t") {
                return UInt64(line.dropFirst(4).trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0
            }
        }
        return 0
    }

    private static func query(account: String, groupKey: String = "JuxuSettingsGroup", service: String = "Juxu") -> [String: Any]? {
        guard let group = Bundle.main.object(forInfoDictionaryKey: groupKey) as? String else { return nil }
        return [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrAccessGroup as String: group,
        ]
    }

    static func read(account: String) -> Data? {
        for (groupKey, service) in [
            ("JuxuSettingsGroup", "Juxu"),
            ("LegacyJuxuSettingsGroup", "Juxu"),
            ("PrisWubiSettingsGroup", "PrisWubi"),
        ] {
            if var q = query(account: account, groupKey: groupKey, service: service) {
                q[kSecReturnData as String] = true
                var result: CFTypeRef?
                if SecItemCopyMatching(q as CFDictionary, &result) == errSecSuccess, let data = result as? Data {
                    if groupKey != "JuxuSettingsGroup" {
                        write(data, account: account)
                    }
                    return data
                }
            }
        }
        return nil
    }

    @discardableResult
    static func write(_ data: Data, account: String) -> Bool {
        guard var query = query(account: account) else { return false }
        let update = [kSecValueData as String: data]
        let status = SecItemUpdate(query as CFDictionary, update as CFDictionary)
        if status == errSecItemNotFound {
            query[kSecValueData as String] = data
            query[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
            return SecItemAdd(query as CFDictionary, nil) == errSecSuccess
        }
        return status == errSecSuccess
    }
}
#endif

/// Serial background persistence worker keeping all file writes, `zlib` compression, and
/// iOS Keychain (`SecItemCopyMatching` / `SecItemUpdate`) IPC off `@MainActor`.
private final class JuxuPersistenceWorker: @unchecked Sendable {
    private let queue = DispatchQueue(label: "im.juxu.persistence", qos: .utility)
    private let evalLogURL: URL
    private let userDataURL: URL
    private let inMemoryOnly: Bool

    #if os(iOS)
    private var iosEvalRingLoaded = false
    private var iosEvalRingLines: [String] = []
    private var iosEvalRingDirty = false
    private var pendingRingFlush: DispatchWorkItem?
    private var pendingUserDataFlush: DispatchWorkItem?
    private var pendingUserDataTSV: String?
    #endif

    init(evalLogURL: URL, userDataURL: URL, inMemoryOnly: Bool) {
        self.evalLogURL = evalLogURL
        self.userDataURL = userDataURL
        self.inMemoryOnly = inMemoryOnly
    }

    func syncBarrier<T>(_ body: () -> T) -> T {
        queue.sync(execute: body)
    }

    func enqueueEvalLogBatch(_ batch: String) {
        guard !inMemoryOnly, !batch.isEmpty else { return }
        queue.async { [self] in
            appendEvalLogOnQueue(batch)
        }
    }

    /// Synchronously drains all previously enqueued `enqueueUserDataSave` and `enqueueEvalLogBatch`
    /// blocks on the serial queue (plus any debounced iOS Keychain ring/user-data flushes).
    func flushPendingSync() {
        guard !inMemoryOnly else { return }
        queue.sync {
            #if os(iOS)
            pendingRingFlush?.cancel()
            pendingRingFlush = nil
            flushIOSEvalRingOnQueue()
            pendingUserDataFlush?.cancel()
            pendingUserDataFlush = nil
            flushIOSUserDataOnQueue()
            #endif
        }
    }

    func enqueueFlushPending() {
        #if os(iOS)
        guard !inMemoryOnly else { return }
        queue.async { [self] in
            pendingRingFlush?.cancel()
            pendingRingFlush = nil
            flushIOSEvalRingOnQueue()
            pendingUserDataFlush?.cancel()
            pendingUserDataFlush = nil
            flushIOSUserDataOnQueue()
        }
        #endif
    }

    func saveUserDataFileSync(_ tsv: String) -> Date? {
        guard !inMemoryOnly else { return nil }
        return syncBarrier {
            writeUserDataOnQueue(tsv)
        }
    }

    func enqueueUserDataSave(
        tsv: String,
        publishSharedIOS: Bool = true,
        onSavedMtime: @escaping @Sendable (Date?) -> Void
    ) {
        guard !inMemoryOnly else { return }
        queue.async { [self] in
            let mtime = writeUserDataOnQueue(tsv)
            #if os(iOS)
            if publishSharedIOS {
                pendingUserDataTSV = tsv
                if pendingUserDataFlush == nil {
                    let work = DispatchWorkItem { [weak self] in
                        guard let self else { return }
                        self.pendingUserDataFlush = nil
                        self.flushIOSUserDataOnQueue()
                    }
                    pendingUserDataFlush = work
                    queue.asyncAfter(deadline: .now() + 1.5, execute: work)
                }
            }
            #endif
            onSavedMtime(mtime)
        }
    }

    private func writeUserDataOnQueue(_ tsv: String) -> Date? {
        let dir = userDataURL.deletingLastPathComponent()
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? Data(tsv.utf8).write(to: userDataURL, options: .atomic)
        return (try? FileManager.default.attributesOfItem(atPath: userDataURL.path)[.modificationDate]) as? Date
    }

    #if os(iOS)
    private func flushIOSUserDataOnQueue() {
        guard let tsv = pendingUserDataTSV else { return }
        pendingUserDataTSV = nil
        Self.writeSharedUserDataOnQueue(tsv)
    }

    private static func writeSharedUserDataOnQueue(_ tsv: String) {
        let newSeq = IOSSharedSettings.parseUserDataSeq(tsv)
        guard newSeq > 0 else { return }
        if let existing = decodeSharedUserDataOnQueue(), existing.seq > newSeq {
            return
        }
        if let compressed = try? (Data(tsv.utf8) as NSData).compressed(using: .zlib) as Data {
            _ = IOSSharedSettings.write(compressed, account: IOSSharedSettings.userDataAccount)
        }
    }

    private static func decodeSharedUserDataOnQueue() -> (seq: UInt64, tsv: String)? {
        guard let compressed = IOSSharedSettings.read(account: IOSSharedSettings.userDataAccount) else {
            return nil
        }
        let rawData: Data
        if let inflated = try? (compressed as NSData).decompressed(using: .zlib) as Data {
            rawData = inflated
        } else {
            rawData = compressed
        }
        guard let tsv = String(data: rawData, encoding: .utf8), !tsv.isEmpty else { return nil }
        return (IOSSharedSettings.parseUserDataSeq(tsv), tsv)
    }

    func readSharedUserDataSync() -> (seq: UInt64, tsv: String)? {
        guard !inMemoryOnly else { return nil }
        return queue.sync {
            Self.decodeSharedUserDataOnQueue()
        }
    }

    func publishIOSUserDataSync(_ tsv: String) {
        guard !inMemoryOnly, !tsv.isEmpty else { return }
        queue.sync {
            pendingUserDataFlush?.cancel()
            pendingUserDataFlush = nil
            pendingUserDataTSV = nil
            Self.writeSharedUserDataOnQueue(tsv)
        }
    }

    func publishIOSUserDataAsync(_ tsv: String) {
        guard !inMemoryOnly, !tsv.isEmpty else { return }
        queue.async {
            Self.writeSharedUserDataOnQueue(tsv)
        }
    }

    func enqueueIOSTouchProfilePublish(
        profile: [TouchKeyCalibrationItem],
        lastKnown: [TouchKeyCalibrationItem]?,
        isReset: Bool,
        onResult: @escaping @Sendable (_ adoptedExternal: [TouchKeyCalibrationItem]?, _ published: [TouchKeyCalibrationItem]?) -> Void
    ) {
        guard !inMemoryOnly else { return }
        queue.async {
            if !isReset,
               let data = IOSSharedSettings.read(account: IOSSharedSettings.touchProfileAccount),
               let current = try? JSONDecoder().decode([TouchKeyCalibrationItem].self, from: data),
               current.map(\.key) == "abcdefghijklmnopqrstuvwxyz".map(String.init),
               current != lastKnown {
                onResult(current, nil)
                return
            }
            if let data = try? JSONEncoder().encode(profile),
               IOSSharedSettings.write(data, account: IOSSharedSettings.touchProfileAccount) {
                onResult(nil, profile)
            }
        }
    }

    func checkIOSAppearanceUpdates(
        knownSharedSeq: UInt64,
        onResult: @escaping @Sendable (Data?, [TouchKeyCalibrationItem]?, Date?, (UInt64, String)?) -> Void
    ) {
        guard !inMemoryOnly else { return }
        queue.async { [self] in
            let prefsData = IOSSharedSettings.read(account: IOSSharedSettings.preferencesAccount)
            var sharedTouch: [TouchKeyCalibrationItem]? = nil
            if let tData = IOSSharedSettings.read(account: IOSSharedSettings.touchProfileAccount),
               let items = try? JSONDecoder().decode([TouchKeyCalibrationItem].self, from: tData),
               items.map(\.key) == "abcdefghijklmnopqrstuvwxyz".map(String.init) {
                sharedTouch = items
            }
            let mtime: Date?
            if FileManager.default.fileExists(atPath: userDataURL.path) {
                mtime = (try? FileManager.default.attributesOfItem(atPath: userDataURL.path)[.modificationDate]) as? Date
            } else {
                mtime = nil
            }
            var newerSharedUser: (UInt64, String)? = nil
            if let shared = Self.decodeSharedUserDataOnQueue(), shared.seq > knownSharedSeq {
                newerSharedUser = (shared.seq, shared.tsv)
            }
            onResult(prefsData, sharedTouch, mtime, newerSharedUser)
        }
    }
    #endif

    func readEvalLogTextSync() -> String {
        guard !inMemoryOnly else { return "" }
        return queue.sync {
            #if os(iOS)
            pendingRingFlush?.cancel()
            pendingRingFlush = nil
            flushIOSEvalRingOnQueue()
            if iosEvalRingLoaded && !iosEvalRingLines.isEmpty {
                return iosEvalRingLines.joined(separator: "\n") + "\n"
            }
            let ring = Self.decodeSharedEvalRingOnQueue()
            if !ring.isEmpty {
                return ring.joined(separator: "\n") + "\n"
            }
            #endif
            return (try? String(contentsOf: evalLogURL, encoding: .utf8)) ?? ""
        }
    }

    func clearEvalLogSync() {
        guard !inMemoryOnly else { return }
        queue.sync {
            #if os(iOS)
            pendingRingFlush?.cancel()
            pendingRingFlush = nil
            iosEvalRingLines.removeAll()
            iosEvalRingLoaded = true
            iosEvalRingDirty = false
            if let emptyCompressed = try? (Data() as NSData).compressed(using: .zlib) as Data {
                _ = IOSSharedSettings.write(emptyCompressed, account: IOSSharedSettings.evalLogAccount)
            }
            #endif
            try? FileManager.default.removeItem(at: evalLogURL)
            let prevURL = evalLogURL.deletingLastPathComponent().appendingPathComponent("eval_log.prev.jsonl")
            try? FileManager.default.removeItem(at: prevURL)
        }
    }

    private func appendEvalLogOnQueue(_ batch: String) {
        let payload = Data((batch + "\n").utf8)
        let fm = FileManager.default
        let dir = evalLogURL.deletingLastPathComponent()
        try? fm.createDirectory(at: dir, withIntermediateDirectories: true)
        if let attrs = try? fm.attributesOfItem(atPath: evalLogURL.path),
           let size = attrs[.size] as? UInt64,
           size > 8 * 1024 * 1024 {
            let prevURL = dir.appendingPathComponent("eval_log.prev.jsonl")
            try? fm.removeItem(at: prevURL)
            try? fm.moveItem(at: evalLogURL, to: prevURL)
        }
        if !fm.fileExists(atPath: evalLogURL.path) {
            fm.createFile(
                atPath: evalLogURL.path,
                contents: payload,
                attributes: [.posixPermissions: NSNumber(value: Int16(0o600))]
            )
        } else if let fh = try? FileHandle(forWritingTo: evalLogURL) {
            defer { try? fh.close() }
            _ = try? fh.seekToEnd()
            try? fh.write(contentsOf: payload)
        }

        #if os(iOS)
        if !iosEvalRingLoaded {
            iosEvalRingLines = Self.decodeSharedEvalRingOnQueue()
            iosEvalRingLoaded = true
        }
        let added = batch.split(separator: "\n", omittingEmptySubsequences: true).map(String.init)
        if !added.isEmpty {
            iosEvalRingLines.append(contentsOf: added)
            if iosEvalRingLines.count > 400 {
                iosEvalRingLines.removeFirst(iosEvalRingLines.count - 400)
            }
            iosEvalRingDirty = true
            if pendingRingFlush == nil {
                let work = DispatchWorkItem { [weak self] in
                    guard let self else { return }
                    self.pendingRingFlush = nil
                    self.flushIOSEvalRingOnQueue()
                }
                pendingRingFlush = work
                queue.asyncAfter(deadline: .now() + 1.2, execute: work)
            }
        }
        #endif
    }

    #if os(iOS)
    private func flushIOSEvalRingOnQueue() {
        guard iosEvalRingDirty else { return }
        iosEvalRingDirty = false
        let joined = iosEvalRingLines.joined(separator: "\n")
        if let compressed = try? (Data(joined.utf8) as NSData).compressed(using: .zlib) as Data {
            _ = IOSSharedSettings.write(compressed, account: IOSSharedSettings.evalLogAccount)
        }
    }

    private static func decodeSharedEvalRingOnQueue() -> [String] {
        guard let compressed = IOSSharedSettings.read(account: IOSSharedSettings.evalLogAccount) else {
            return []
        }
        let rawData: Data
        if let inflated = try? (compressed as NSData).decompressed(using: .zlib) as Data {
            rawData = inflated
        } else {
            rawData = compressed
        }
        guard let text = String(data: rawData, encoding: .utf8) else { return [] }
        return text.split(separator: "\n", omittingEmptySubsequences: true).map(String.init)
    }
    #endif
}

/// Owns the raw `JuxuEngineHandle *` lifecycle without requiring `nonisolated(unsafe)` on `@MainActor` `JuxuBridge`.
private final class EngineHandleBox {
    let ptr: OpaquePointer?

    init(_ ptr: OpaquePointer?) {
        self.ptr = ptr
    }

    deinit {
        if let ptr {
            juxu_engine_free(ptr)
        }
    }
}

@MainActor
public final class JuxuBridge: ObservableObject {
    public static let shared = JuxuBridge()

    private let handleBox: EngineHandleBox
    private var handle: OpaquePointer? { handleBox.ptr }

    private let decoder = JSONDecoder()
    private let encoder = JSONEncoder()
    private var resolvedDictsDir: URL?
    private var emojiTableLoaded = false
    private var loadedDictSchemas: Set<UInt32> = []
    private var pinyinDictLoaded: Bool = false
    private var auxDictLoaded: Bool = false
    private var lastLoadedUserDataMtime: Date?
    private var userDataSaveGeneration: UInt64 = 0
    #if os(iOS)
    private let inMemoryOnly: Bool
    private var lastSharedTouchProfile: [TouchKeyCalibrationItem]?
    private var lastSharedPrefsData: Data?
    private var lastSharedUserDataSeq: UInt64 = 0
    private var lastAppearanceCheckUptime: TimeInterval = 0
    #endif

    @Published public private(set) var state: JuxuActionState = .empty
    @Published public var prefs: JuxuPreferences = JuxuPreferences()
    @Published public private(set) var userPhrases: [UserPhraseRecord] = []
    @Published public private(set) var touchCalibrations: [TouchKeyCalibrationItem] = []
    @Published public private(set) var evalLogSummary: EvalLogSummary = .empty
    /// When true (e.g. password / OTP / numeric field), queued eval events are drained and discarded.
    public var suppressEvalLogging: Bool = false

    public let configURL: URL
    public let userDataURL: URL
    public let evalLogURL: URL
    public let hasSharedContainerAccess: Bool
    private let bridgeInMemoryOnly: Bool
    private let persistence: JuxuPersistenceWorker

    public init(customDictsDir: URL? = nil, inMemoryOnly: Bool = false) {
        self.bridgeInMemoryOnly = inMemoryOnly
        #if os(iOS)
        self.inMemoryOnly = inMemoryOnly
        #endif
        #if os(macOS)
        let home = URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true)
        let dotfile = home.appendingPathComponent(".juxu.toml")
        let legacyDotfile = home.appendingPathComponent(".priswubi.toml")
        let appSupport = home
            .appendingPathComponent("Library/Application Support/Juxu", isDirectory: true)
        let legacyAppSupport = home
            .appendingPathComponent("Library/Application Support/PrisWubi", isDirectory: true)
        if !inMemoryOnly {
            let fm = FileManager.default
            if !fm.fileExists(atPath: appSupport.path), fm.fileExists(atPath: legacyAppSupport.path) {
                try? fm.createDirectory(at: appSupport, withIntermediateDirectories: true)
                for file in ["config.toml", "user_data.tsv"] {
                    let src = legacyAppSupport.appendingPathComponent(file)
                    let dst = appSupport.appendingPathComponent(file)
                    if fm.fileExists(atPath: src.path), !fm.fileExists(atPath: dst.path) {
                        try? fm.copyItem(at: src, to: dst)
                    }
                }
            } else {
                try? fm.createDirectory(at: appSupport, withIntermediateDirectories: true)
            }
        }
        if FileManager.default.fileExists(atPath: dotfile.path) {
            self.configURL = dotfile
        } else if FileManager.default.fileExists(atPath: legacyDotfile.path) {
            self.configURL = legacyDotfile
        } else {
            self.configURL = appSupport.appendingPathComponent("config.toml")
        }
        self.userDataURL = appSupport.appendingPathComponent("user_data.tsv")
        self.evalLogURL = appSupport.appendingPathComponent("eval_log.jsonl")
        self.hasSharedContainerAccess = true
        #else
        if let groupURL = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: JuxuPreferences.appGroupIdentifier
        ) {
            self.configURL = groupURL.appendingPathComponent("config.toml")
            self.userDataURL = groupURL.appendingPathComponent("user_data.tsv")
            self.evalLogURL = groupURL.appendingPathComponent("eval_log.jsonl")
            self.hasSharedContainerAccess = true
        } else {
            let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first
                ?? URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            self.configURL = docs.appendingPathComponent("config.toml")
            self.userDataURL = docs.appendingPathComponent("user_data.tsv")
            self.evalLogURL = docs.appendingPathComponent("eval_log.jsonl")
            self.hasSharedContainerAccess = false
        }
        #endif

        self.persistence = JuxuPersistenceWorker(
            evalLogURL: self.evalLogURL,
            userDataURL: self.userDataURL,
            inMemoryOnly: inMemoryOnly
        )
        self.handleBox = EngineHandleBox(juxu_engine_new())
        loadDictionaries(overrideDir: customDictsDir)
        if !inMemoryOnly {
            loadConfigAndUser()
        } else {
            ensureDictionaryLoaded(for: prefs.schemaId)
            refreshState()
            refreshTouchProfile()
        }
    }

    public func loadDictionaries(overrideDir: URL? = nil) {
        guard let h = handle else { return }
        let candidateDirs: [URL] = [
            overrideDir,
            // On iOS App Extensions (`JuxuApp.app/PlugIns/JuxuKeyboard.appex`), prefer the
            // containing app's single `JuxuApp.app/Dicts` copy first so stale extension copies
            // can never shadow freshly bundled dictionaries.
            Bundle.main.bundleURL
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("Dicts", isDirectory: true),
            Bundle.main.resourceURL?.appendingPathComponent("Dicts", isDirectory: true),
            Bundle.main.resourceURL,
            URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
                .appendingPathComponent("Resources/Dicts", isDirectory: true),
        ].compactMap { $0 }

        for dir in candidateDirs {
            let p86 = dir.appendingPathComponent("wubi86.wbdict").path
            if FileManager.default.fileExists(atPath: p86) {
                resolvedDictsDir = dir
                if !auxDictLoaded {
                    let pAux = dir.appendingPathComponent("wubi_aux.wbaux").path
                    if pAux.withCString({ juxu_engine_load_aux_mmap(h, $0) }) {
                        auxDictLoaded = true
                    }
                }
                let pEn = dir.appendingPathComponent("english.endict").path
                if FileManager.default.fileExists(atPath: pEn) {
                    _ = pEn.withCString { juxu_engine_load_english_dict_mmap(h, $0) }
                }
                #if os(macOS)
                for sid: UInt32 in [86, 98, 6, 10, 33, 35, 36] {
                    ensureDictionaryLoaded(for: sid)
                }
                ensureDictionaryLoaded(for: 20)
                #endif
                break
            }
        }
    }

    public func ensureDictionaryLoaded(for schemaId: UInt32) {
        guard let h = handle else { return }
        if resolvedDictsDir == nil {
            loadDictionaries()
        }
        guard let dir = resolvedDictsDir else { return }
        if !auxDictLoaded {
            let pAux = dir.appendingPathComponent("wubi_aux.wbaux").path
            if pAux.withCString({ juxu_engine_load_aux_mmap(h, $0) }) {
                auxDictLoaded = true
            }
        }
        if schemaId == 20 || schemaId == 21 {
            #if os(iOS)
            if !loadedDictSchemas.isEmpty {
                juxu_engine_retain_only_schema_dict(h, schemaId)
                loadedDictSchemas.removeAll()
            }
            #endif
            if !pinyinDictLoaded || !juxu_engine_has_active_dict(h) {
                let pPy = dir.appendingPathComponent("pinyin.pydict").path
                if pPy.withCString({ juxu_engine_load_pinyin_dict_mmap(h, $0) }) {
                    pinyinDictLoaded = true
                }
            }
            return
        }
        #if os(iOS)
        // Release the 67 MB Pinyin dictionary and other unused dicts BEFORE mapping the new `.wbdict`
        // so iOS keyboard extension never peaks above its memory ceiling during a schema switch.
        if pinyinDictLoaded || loadedDictSchemas.contains(where: { $0 != schemaId }) {
            juxu_engine_retain_only_schema_dict(h, schemaId)
            pinyinDictLoaded = false
            loadedDictSchemas = loadedDictSchemas.contains(schemaId) ? [schemaId] : []
        }
        #endif
        if !loadedDictSchemas.contains(schemaId) || (juxu_engine_get_schema(h) == schemaId && !juxu_engine_has_active_dict(h)) {
            let fileName: String
            switch schemaId {
            case 98: fileName = "wubi98.wbdict"
            case 6: fileName = "wubi06.wbdict"
            case 10: fileName = "xiaohe.wbdict"
            case 33: fileName = "cangjie3.wbdict"
            case 35: fileName = "cangjie5.wbdict"
            case 36: fileName = "cangjie6.wbdict"
            default: fileName = "wubi86.wbdict"
            }
            let path = dir.appendingPathComponent(fileName).path
            if path.withCString({ juxu_engine_load_dict_mmap(h, schemaId, $0) }) {
                loadedDictSchemas.insert(schemaId)
            }
        }
    }

    public func loadConfigAndUser(refreshPublishedLists: Bool = true) {
        guard let h = handle else { return }
        if !bridgeInMemoryOnly {
            if FileManager.default.fileExists(atPath: configURL.path) {
                _ = configURL.path.withCString { juxu_engine_load_config(h, $0) }
            } else {
                _ = configURL.path.withCString { juxu_engine_save_config(h, $0) }
            }
        }
        #if os(iOS)
        if !inMemoryOnly {
            let data = IOSSharedSettings.read(account: IOSSharedSettings.preferencesAccount)
            lastSharedPrefsData = data
            if let data, let shared = String(data: data, encoding: .utf8) {
                shared.withCString { juxu_engine_set_config_toml(h, $0) }
            }
            loadSharedTouchProfile()
            lastAppearanceCheckUptime = ProcessInfo.processInfo.systemUptime
        }
        #endif
        let configJSON = takeString(juxu_engine_get_config_json(h))
        if let jsonData = configJSON.data(using: .utf8),
           let loaded = try? decoder.decode(JuxuPreferences.self, from: jsonData) {
            self.prefs = loaded
        }
        ensureDictionaryLoaded(for: self.prefs.schemaId)
        ensureEmojiTableLoaded()
        if !bridgeInMemoryOnly, FileManager.default.fileExists(atPath: userDataURL.path) {
            let mtime = (try? FileManager.default.attributesOfItem(atPath: userDataURL.path)[.modificationDate]) as? Date
            if lastLoadedUserDataMtime == nil || mtime == nil || mtime != lastLoadedUserDataMtime {
                if userDataURL.path.withCString({ juxu_engine_load_user_data(h, $0) }) {
                    lastLoadedUserDataMtime = mtime
                }
            }
        }
        #if os(iOS)
        if !inMemoryOnly && !hasSharedContainerAccess {
            let localSeq = juxu_engine_user_data_seq(h)
            if let shared = persistence.readSharedUserDataSync() {
                if shared.seq > localSeq {
                    if shared.tsv.withCString({ juxu_engine_import_user_data_tsv(h, $0) }) {
                        lastSharedUserDataSeq = shared.seq
                        let exportedTSV = takeString(juxu_engine_export_user_data_tsv(h))
                        lastLoadedUserDataMtime = persistence.saveUserDataFileSync(exportedTSV)
                    }
                } else {
                    lastSharedUserDataSeq = max(localSeq, shared.seq)
                    if localSeq > shared.seq && localSeq > 1 {
                        let tsv = takeString(juxu_engine_export_user_data_tsv(h))
                        persistence.publishIOSUserDataAsync(tsv)
                    }
                }
            } else if localSeq > 1 {
                lastSharedUserDataSeq = localSeq
                let tsv = takeString(juxu_engine_export_user_data_tsv(h))
                persistence.publishIOSUserDataAsync(tsv)
            }
        }
        #endif
        refreshState()
        if refreshPublishedLists {
            refreshUserPhrases()
            refreshTouchProfile()
        }
    }

    /// The emoji table is a separate read-only file, mapped only once the feature is switched on.
    private func ensureEmojiTableLoaded() {
        guard prefs.emojiCandidates, !emojiTableLoaded, let h = handle, let dir = resolvedDictsDir else { return }
        let path = dir.appendingPathComponent("emoji.tsv").path
        emojiTableLoaded = path.withCString { juxu_engine_load_emoji_table_mmap(h, $0) }
    }

    public func applyAndSavePreferences(_ newPrefs: JuxuPreferences, persist: Bool = true) {
        guard let h = handle else { return }
        self.prefs = newPrefs
        ensureDictionaryLoaded(for: newPrefs.schemaId)
        ensureEmojiTableLoaded()
        if let jsonData = try? encoder.encode(newPrefs),
           let json = String(data: jsonData, encoding: .utf8) {
            json.withCString { juxu_engine_set_config_json(h, $0) }
        }
        if persist && !bridgeInMemoryOnly {
            _ = configURL.path.withCString { juxu_engine_save_config(h, $0) }
            #if os(iOS)
            let toml = takeString(juxu_engine_get_config_toml(h))
            let raw = Data(toml.utf8)
            lastSharedPrefsData = raw
            IOSSharedSettings.write(raw, account: IOSSharedSettings.preferencesAccount)
            #endif
        }
        refreshState()
        refreshUserPhrases()
    }

    public func switchSchema(_ schemaId: UInt32, persist: Bool = true) {
        var updated = prefs
        updated.schemaId = schemaId
        applyAndSavePreferences(updated, persist: persist)
    }

    public func setKeyLayout(
        centers52: [Float],
        originX: Float,
        originY: Float,
        pitchX: Float,
        pitchY: Float
    ) {
        guard let h = handle, centers52.count == 52 else { return }
        centers52.withUnsafeBufferPointer { buf in
            juxu_engine_set_key_layout(h, buf.baseAddress, originX, originY, pitchX, pitchY)
        }
    }

    public func setT9Layout(
        centers16: [Float],
        originX: Float,
        originY: Float,
        pitchX: Float,
        pitchY: Float
    ) {
        guard let h = handle, centers16.count == 16 else { return }
        centers16.withUnsafeBufferPointer { buf in
            juxu_engine_set_t9_layout(h, buf.baseAddress, originX, originY, pitchX, pitchY)
        }
    }

    public func flushUserDataSync(refreshPublishedLists: Bool = false) {
        flushUserDataIfNeeded(refreshPublishedLists: refreshPublishedLists)
    }

    public func flushUserDataIfNeeded(refreshPublishedLists: Bool = true) {
        guard let h = handle else {
            persistence.flushPendingSync()
            return
        }
        drainEvalLogIfNeeded()
        if juxu_engine_is_user_dirty(h) {
            userDataSaveGeneration &+= 1
            let tsv = takeString(juxu_engine_export_user_data_tsv(h))
            #if os(iOS)
            publishTouchProfile()
            let shouldPublishIOS = !inMemoryOnly && !hasSharedContainerAccess
            #endif
            lastLoadedUserDataMtime = persistence.saveUserDataFileSync(tsv)
            #if os(iOS)
            if shouldPublishIOS && !tsv.isEmpty {
                lastSharedUserDataSeq = juxu_engine_user_data_seq(h)
                persistence.publishIOSUserDataSync(tsv)
            }
            #endif
            if refreshPublishedLists {
                refreshUserPhrases()
                refreshTouchProfile()
            }
        }
        persistence.flushPendingSync()
    }

    /// Non-blocking user data + touch-profile persistence for live typing commits:
    /// snapshots TSV in memory on `@MainActor` (`~30 µs`) and performs atomic disk + Keychain writes off-thread.
    public func flushUserDataAsync() {
        guard let h = handle, !bridgeInMemoryOnly, juxu_engine_is_user_dirty(h) else { return }
        userDataSaveGeneration &+= 1
        let gen = userDataSaveGeneration
        let tsv = takeString(juxu_engine_export_user_data_tsv(h))
        #if os(iOS)
        let profile = readTouchProfile()
        if profile.count == 26,
           profile != lastSharedTouchProfile,
           profile.contains(where: { $0.samples > 0 }) {
            let known = lastSharedTouchProfile
            persistence.enqueueIOSTouchProfilePublish(
                profile: profile,
                lastKnown: known,
                isReset: false
            ) { [weak self] adopted, published in
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    if let adopted {
                        self.adoptSharedTouchProfile(adopted)
                    } else if let published {
                        self.lastSharedTouchProfile = published
                    }
                }
            }
        }
        #endif
        persistence.enqueueUserDataSave(tsv: tsv) { [weak self] mtime in
            Task { @MainActor [weak self] in
                guard let self, self.userDataSaveGeneration == gen else { return }
                self.lastLoadedUserDataMtime = mtime
                #if os(iOS)
                if let h = self.handle {
                    self.lastSharedUserDataSeq = max(self.lastSharedUserDataSeq, juxu_engine_user_data_seq(h))
                }
                #endif
            }
        }
    }

    /// Enqueue immediate flush of any debounced eval-log ring writes (e.g. on `viewWillDisappear`).
    public func flushPendingPersistence() {
        persistence.enqueueFlushPending()
    }

    #if os(iOS)
    /// Non-blocking check on keyboard `viewWillAppear`: avoids stalling initial keyboard presentation
    /// on synchronous Keychain (`securityd`) IPC when nothing changed in the host app.
    public func reloadConfigAndUserOnAppear(onApplied: (@MainActor @Sendable () -> Void)? = nil) {
        guard !inMemoryOnly else { return }
        let now = ProcessInfo.processInfo.systemUptime
        guard now - lastAppearanceCheckUptime >= 0.8 else { return }
        lastAppearanceCheckUptime = now
        let knownSeq = max(lastSharedUserDataSeq, handle.map { juxu_engine_user_data_seq($0) } ?? 0)
        persistence.checkIOSAppearanceUpdates(knownSharedSeq: knownSeq) { [weak self] prefsData, sharedTouch, mtime, sharedUser in
            Task { @MainActor [weak self] in
                guard let self, let h = self.handle else { return }
                var changed = false
                if prefsData != self.lastSharedPrefsData {
                    self.lastSharedPrefsData = prefsData
                    if let prefsData, let shared = String(data: prefsData, encoding: .utf8) {
                        shared.withCString { juxu_engine_set_config_toml(h, $0) }
                        let configJSON = self.takeString(juxu_engine_get_config_json(h))
                        if let jsonData = configJSON.data(using: .utf8),
                           let loaded = try? self.decoder.decode(JuxuPreferences.self, from: jsonData) {
                            self.prefs = loaded
                        }
                        self.ensureDictionaryLoaded(for: self.prefs.schemaId)
                        self.ensureEmojiTableLoaded()
                        changed = true
                    }
                }
                if let sharedTouch, sharedTouch != self.lastSharedTouchProfile {
                    self.adoptSharedTouchProfile(sharedTouch)
                    changed = true
                }
                if let (sSeq, sTSV) = sharedUser,
                   !juxu_engine_is_user_dirty(h),
                   sSeq > juxu_engine_user_data_seq(h) {
                    if sTSV.withCString({ juxu_engine_import_user_data_tsv(h, $0) }) {
                        self.lastSharedUserDataSeq = sSeq
                        self.userDataSaveGeneration &+= 1
                        let gen = self.userDataSaveGeneration
                        let exportedTSV = self.takeString(juxu_engine_export_user_data_tsv(h))
                        self.persistence.enqueueUserDataSave(tsv: exportedTSV, publishSharedIOS: false) { [weak self] savedMtime in
                            Task { @MainActor [weak self] in
                                guard let self, self.userDataSaveGeneration == gen else { return }
                                self.lastLoadedUserDataMtime = savedMtime
                            }
                        }
                        self.refreshUserPhrases()
                        changed = true
                    }
                } else if let mtime,
                   !juxu_engine_is_user_dirty(h),
                   self.lastLoadedUserDataMtime != mtime {
                    if self.userDataURL.path.withCString({ juxu_engine_load_user_data(h, $0) }) {
                        self.lastLoadedUserDataMtime = mtime
                        self.lastSharedUserDataSeq = max(self.lastSharedUserDataSeq, juxu_engine_user_data_seq(h))
                        changed = true
                    }
                }
                if changed {
                    self.refreshState()
                    onApplied?()
                }
            }
        }
    }
    #endif

    public func handleKey(
        keyCode: UInt16,
        scalar: UInt32,
        hasCmdOrCtrl: Bool,
        timestampMs: UInt64
    ) -> JuxuActionState {
        guard let h = handle else { return .empty }
        if !juxu_engine_has_active_dict(h) {
            ensureDictionaryLoaded(for: juxu_engine_get_schema(h))
        }
        let json = takeString(juxu_engine_handle_key_json(h, keyCode, scalar, hasCmdOrCtrl, timestampMs))
        let decoded = decodeState(json)
        self.state = decoded
        drainEvalLogIfNeeded()
        return decoded
    }

    public func pushTouch(
        reportedAscii: UInt8,
        x: Float,
        y: Float,
        isNormalized: Bool,
        timestampMs: UInt64
    ) -> JuxuActionState {
        guard let h = handle else { return .empty }
        if !juxu_engine_has_active_dict(h) {
            ensureDictionaryLoaded(for: juxu_engine_get_schema(h))
        }
        let json = takeString(juxu_engine_push_touch_json(h, reportedAscii, x, y, isNormalized, timestampMs))
        let decoded = decodeState(json)
        self.state = decoded
        drainEvalLogIfNeeded()
        return decoded
    }

    public func pushT9Touch(
        reportedDigit: UInt8,
        x: Float,
        y: Float,
        isNormalized: Bool,
        timestampMs: UInt64
    ) -> JuxuActionState {
        guard let h = handle else { return .empty }
        if !juxu_engine_has_active_dict(h) {
            ensureDictionaryLoaded(for: juxu_engine_get_schema(h))
        }
        let json = takeString(juxu_engine_push_t9_touch_json(h, reportedDigit, x, y, isNormalized, timestampMs))
        let decoded = decodeState(json)
        self.state = decoded
        drainEvalLogIfNeeded()
        return decoded
    }

    public func lockT9Syllable(_ syllable: String) -> JuxuActionState {
        guard let h = handle else { return .empty }
        let json = syllable.withCString { cStr in
            takeString(juxu_engine_lock_t9_syllable_json(h, cStr))
        }
        let decoded = decodeState(json)
        self.state = decoded
        return decoded
    }

    public func selectCandidate(pageRelIndex: Int, timestampMs: UInt64) -> JuxuActionState {
        guard let h = handle else { return .empty }
        let json = takeString(juxu_engine_select_candidate_json(h, UInt32(max(0, pageRelIndex)), timestampMs))
        let decoded = decodeState(json)
        self.state = decoded
        drainEvalLogIfNeeded()
        return decoded
    }

    public func selectAbsCandidate(absIndex: Int, timestampMs: UInt64) -> JuxuActionState {
        guard let h = handle else { return .empty }
        let json = takeString(juxu_engine_select_abs_candidate_json(h, UInt32(max(0, absIndex)), timestampMs))
        let decoded = decodeState(json)
        self.state = decoded
        drainEvalLogIfNeeded()
        return decoded
    }

    public func commitPending(timestampMs: UInt64) -> JuxuActionState {
        guard let h = handle else { return .empty }
        if !juxu_engine_has_active_dict(h) {
            ensureDictionaryLoaded(for: juxu_engine_get_schema(h))
        }
        let json = takeString(juxu_engine_commit_pending_json(h, timestampMs))
        let decoded = decodeState(json)
        self.state = decoded
        drainEvalLogIfNeeded()
        return decoded
    }

    public func resetQuoteState() {
        guard let h = handle else { return }
        juxu_engine_reset_quote_state(h)
    }

    public func allCandidates(maxCount: Int = 60) -> [JuxuCandidateItem] {
        guard let h = handle else { return [] }
        let json = takeString(juxu_engine_all_candidates_json(h, UInt32(max(1, maxCount))))
        if let data = json.data(using: .utf8),
           let list = try? decoder.decode([JuxuCandidateItem].self, from: data) {
            return list
        }
        return []
    }

    public func turnPage(delta: Int32) -> JuxuActionState {
        guard let h = handle else { return .empty }
        let json = takeString(juxu_engine_page_turn_json(h, delta))
        let decoded = decodeState(json)
        self.state = decoded
        return decoded
    }

    public func toggleAsciiMode() -> JuxuActionState {
        guard let h = handle else { return .empty }
        let json = takeString(juxu_engine_toggle_ascii_json(h))
        let decoded = decodeState(json)
        self.state = decoded
        return decoded
    }

    public func clearComposition() {
        guard let h = handle else { return }
        juxu_engine_clear(h)
        refreshState()
    }

    public func abandonComposition(timestampMs: UInt64) {
        guard let h = handle else { return }
        juxu_engine_abandon(h, timestampMs)
        drainEvalLogIfNeeded()
        refreshState()
    }

    public func notifyPostCommitBackspace(timestampMs: UInt64) {
        guard let h = handle else { return }
        juxu_engine_notify_post_commit_backspace(h, timestampMs)
        drainEvalLogIfNeeded()
    }

    public func setContext(_ text: String) {
        guard let h = handle else { return }
        text.withCString { juxu_engine_set_context(h, $0) }
    }

    public func acceptPrediction(_ text: String, timestampMs: UInt64) {
        guard let h = handle else { return }
        text.withCString { juxu_engine_accept_prediction(h, $0, timestampMs) }
    }

    public func predictNextWords(maxCount: Int = 8) -> [String] {
        guard let h = handle else { return [] }
        let json = takeString(juxu_engine_predict_next_json(h, UInt32(max(1, maxCount))))
        if let data = json.data(using: .utf8),
           let list = try? decoder.decode([String].self, from: data) {
            return list
        }
        return []
    }

    @discardableResult
    public func addUserPhrase(_ text: String, persist: Bool = true) -> String {
        guard let h = handle else { return "" }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.count >= 2 else { return "" }
        #if os(iOS)
        if persist && !inMemoryOnly && !hasSharedContainerAccess {
            if let shared = persistence.readSharedUserDataSync(), shared.seq > juxu_engine_user_data_seq(h) {
                _ = shared.tsv.withCString { juxu_engine_import_user_data_tsv(h, $0) }
            }
        }
        #endif
        let code = trimmed.withCString { takeString(juxu_engine_add_user_phrase(h, $0)) }
        if !code.isEmpty && persist {
            flushUserDataIfNeeded()
        }
        refreshState()
        refreshUserPhrases()
        return code
    }

    public func removeUserPhrase(_ text: String, persist: Bool = true) {
        guard let h = handle else { return }
        #if os(iOS)
        if persist && !inMemoryOnly && !hasSharedContainerAccess {
            if let shared = persistence.readSharedUserDataSync(), shared.seq > juxu_engine_user_data_seq(h) {
                _ = shared.tsv.withCString { juxu_engine_import_user_data_tsv(h, $0) }
            }
        }
        #endif
        _ = text.withCString { juxu_engine_remove_user_phrase(h, $0) }
        if persist {
            flushUserDataIfNeeded()
        }
        refreshState()
        refreshUserPhrases()
    }

    public func resetTouchCalibration(persist: Bool = true) {
        guard let h = handle else { return }
        juxu_engine_reset_touch_profile(h)
        if persist {
            #if os(iOS)
            publishTouchProfile(isReset: true)
            #endif
            flushUserDataIfNeeded()
        }
        refreshState()
        refreshTouchProfile()
    }

    public func applyTouchCalibrations(_ profile: [TouchKeyCalibrationItem]) {
        guard let h = handle, profile.count == 26 else { return }
        let offsets = profile.flatMap { [$0.dx, $0.dy] }
        let counts = profile.map(\.samples)
        offsets.withUnsafeBufferPointer { xy in
            counts.withUnsafeBufferPointer { samples in
                juxu_engine_set_touch_profile(h, xy.baseAddress, samples.baseAddress)
            }
        }
        refreshState()
        refreshTouchProfile()
    }

    public func refreshUserPhrases() {
        guard let h = handle else { return }
        let json = takeString(juxu_engine_list_user_phrases_json(h))
        if let data = json.data(using: .utf8),
           let list = try? decoder.decode([UserPhraseRecord].self, from: data) {
            self.userPhrases = list
        }
    }

    public func refreshTouchProfile() {
        touchCalibrations = readTouchProfile()
    }

    private func readTouchProfile() -> [TouchKeyCalibrationItem] {
        guard let h = handle else { return [] }
        let json = takeString(juxu_engine_touch_profile_json(h))
        return (try? decoder.decode([TouchKeyCalibrationItem].self, from: Data(json.utf8))) ?? []
    }

    #if os(iOS)
    public func reloadTouchCalibration() {
        loadSharedTouchProfile()
        refreshTouchProfile()
    }

    private func readSharedTouchProfile() -> [TouchKeyCalibrationItem]? {
        guard let data = IOSSharedSettings.read(account: IOSSharedSettings.touchProfileAccount),
              let items = try? decoder.decode([TouchKeyCalibrationItem].self, from: data),
              items.map(\.key) == "abcdefghijklmnopqrstuvwxyz".map(String.init) else { return nil }
        return items
    }

    private func loadSharedTouchProfile() {
        guard !inMemoryOnly else { return }
        if let profile = readSharedTouchProfile() {
            adoptSharedTouchProfile(profile)
        } else {
            // Migrate calibration from the keyboard's existing local user store.
            publishTouchProfile()
        }
    }

    private func adoptSharedTouchProfile(_ profile: [TouchKeyCalibrationItem]) {
        lastSharedTouchProfile = profile
        guard let h = handle, profile.count == 26, profile != readTouchProfile() else { return }
        let offsets = profile.flatMap { [$0.dx, $0.dy] }
        let counts = profile.map(\.samples)
        offsets.withUnsafeBufferPointer { xy in
            counts.withUnsafeBufferPointer { samples in
                juxu_engine_set_touch_profile(h, xy.baseAddress, samples.baseAddress)
            }
        }
    }

    private func publishTouchProfile(isReset: Bool = false) {
        guard !inMemoryOnly else { return }
        let profile = readTouchProfile()
        guard profile.count == 26,
              isReset || (profile != lastSharedTouchProfile && profile.contains { $0.samples > 0 })
        else { return }

        // A running keyboard must not overwrite a reset made in the host app.
        if !isReset, let current = readSharedTouchProfile(), current != lastSharedTouchProfile {
            adoptSharedTouchProfile(current)
            return
        }
        if let data = try? encoder.encode(profile),
           IOSSharedSettings.write(data, account: IOSSharedSettings.touchProfileAccount) {
            lastSharedTouchProfile = profile
        }
    }
    #endif

    public func refreshState() {
        guard let h = handle else { return }
        let json = takeString(juxu_engine_state_json(h))
        self.state = decodeState(json)
    }

    private func decodeState(_ json: String) -> JuxuActionState {
        guard let data = json.data(using: .utf8),
              let s = try? decoder.decode(JuxuActionState.self, from: data) else {
            return .empty
        }
        return s.inheritingMissingTouchOffsets(from: self.state)
    }

    private func takeString(_ ptr: UnsafeMutablePointer<CChar>?) -> String {
        guard let p = ptr else { return "" }
        defer { juxu_string_free(p) }
        return String(cString: p)
    }

    // MARK: - Local Input Eval Log (macOS JSONL + iOS Shared Keychain Ring)

    private func drainEvalLogIfNeeded() {
        guard let h = handle else { return }
        let batch = takeString(juxu_engine_drain_eval_log_jsonl(h))
        guard !bridgeInMemoryOnly, !suppressEvalLogging, !batch.isEmpty else { return }
        persistence.enqueueEvalLogBatch(batch)
    }

    public func readEvalLogText() -> String {
        persistence.readEvalLogTextSync()
    }

    public func reloadEvalLogSummary() {
        let text = readEvalLogText()
        var totalEvents = 0
        var commits = 0
        var top1Hits = 0
        var manualSelects = 0
        var lmOverruled = 0
        var typoTriggered = 0
        var backspaceAfterCommit = 0
        var abandons = 0

        for line in text.split(separator: "\n", omittingEmptySubsequences: true) {
            let s = String(line)
            guard s.hasPrefix("{") else { continue }
            totalEvents += 1
            if s.contains("\"event\":\"commit\"") || s.contains("\"event\":\"partial_commit\"") {
                commits += 1
                if s.contains("\"rank\":0,") {
                    top1Hits += 1
                } else {
                    manualSelects += 1
                }
                if s.contains("\"lm_overruled\":true") {
                    lmOverruled += 1
                }
                if !s.contains("\"typo\":\"none\"") && s.contains("\"typo\":\"") {
                    typoTriggered += 1
                }
            } else if s.contains("\"event\":\"backspace_after_commit\"") {
                backspaceAfterCommit += 1
            } else if s.contains("\"event\":\"abandon\"") {
                abandons += 1
            }
        }
        self.evalLogSummary = EvalLogSummary(
            totalEvents: totalEvents,
            commits: commits,
            top1Hits: top1Hits,
            manualSelects: manualSelects,
            lmOverruled: lmOverruled,
            typoTriggered: typoTriggered,
            backspaceAfterCommit: backspaceAfterCommit,
            abandons: abandons
        )
    }

    public func exportEvalLogFileURL() -> URL? {
        #if os(macOS)
        persistence.flushPendingSync()
        return FileManager.default.fileExists(atPath: evalLogURL.path) ? evalLogURL : nil
        #else
        let text = readEvalLogText()
        guard !text.isEmpty else { return nil }
        let out = FileManager.default.temporaryDirectory.appendingPathComponent("juxu_ios_eval_log.jsonl")
        try? Data(text.utf8).write(to: out, options: .atomic)
        return out
        #endif
    }

    public func clearEvalLog() {
        persistence.clearEvalLogSync()
        self.evalLogSummary = .empty
    }
}
