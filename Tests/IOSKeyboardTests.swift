import CoreGraphics
import Foundation

@main
struct IOSKeyboardTests {
    @MainActor
    static func main() {
        // 1. Verify exact iOS 26 440pt Pro Max grid alignment against Apple reference telemetry!
        var layout = IOSKeyboardLayoutEngine()
        let snap = layout.computeLayout(in: CGSize(width: 440, height: 225))
        assert(snap.keys.count == 32, "Expected 26 letters + Shift + Del + 4 bottom keys = 32, got \(snap.keys.count)")

        let qKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "q")) })!
        assert(qKey.primaryLabel == "q", "Expected lowercase 'q' in iOS 26 default mode, got \(qKey.primaryLabel)")
        assert(abs(qKey.frame.width - 37.27) < 0.25, "Expected ~37.27pt key width on 440pt canvas, got \(qKey.frame.width)")

        // Verify column-for-column invariants: `z` == `s`, `Space` == `d..j`, `Return` == `k..p`!
        let sKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "s")) })!
        let dKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "d")) })!
        let jKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "j")) })!
        let kKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "k")) })!
        let zKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "z")) })!
        let spaceKey = snap.keys.first(where: { $0.kind == .space })!
        let returnKey = snap.keys.first(where: { $0.kind == .returnKey })!

        assert(abs(zKey.frame.minX - sKey.frame.minX) < 0.01, "z.minX must equal s.minX")
        assert(abs(spaceKey.frame.minX - dKey.frame.minX) < 0.01, "Space.minX must equal d.minX")
        assert(abs(spaceKey.frame.maxX - jKey.frame.maxX) < 0.01, "Space.maxX must equal j.maxX")
        assert(abs(returnKey.frame.minX - kKey.frame.minX) < 0.01, "Return.minX must equal k.minX")
        assert(spaceKey.cornerSubscript == "五", "Spacebar must show subtle corner '五'")
        assert(returnKey.systemSymbol == "return.left", "Normal return key must use return.left symbol")

        // 2. Test Voronoi gutter hit-testing
        let tKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "t")) })!
        let yKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "y")) })!
        let gutterMid = CGPoint(x: (tKey.frame.maxX + yKey.frame.minX) * 0.5 - 0.5, y: tKey.frame.midY)
        let gutterHit = IOSKeyboardLayoutEngine.hitTest(point: gutterMid, in: snap)
        assert(gutterHit?.kind == .letter(UInt8(ascii: "t")))

        // 3. Test Shift -> Uppercase (`Q`) -> Double-tap CapsLock -> Lowercase (`q`)
        layout.tapShift(at: 10.0)
        assert(layout.shiftState == .upperOnce)
        let upperSnap = layout.computeLayout(in: CGSize(width: 440, height: 225))
        let upperQ = upperSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "q")) })!
        assert(upperQ.primaryLabel == "Q")
        layout.tapShift(at: 10.20)
        assert(layout.shiftState == .capsLocked)
        layout.tapShift(at: 11.0)
        assert(layout.shiftState == .lower)

        // 4. Test Numbers (123) and Symbols (#+=) planes + native stay-vs-return rules
        layout.switchPlane(.numbers)
        let numSnap = layout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(numSnap.keys.contains(where: { $0.primaryLabel == "1" }))
        assert(numSnap.keys.contains(where: { $0.primaryLabel == "，" }))
        assert(numSnap.keys.contains(where: { $0.primaryLabel == "." }))
        assert(!IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("5", on: .numbers))
        assert(!IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("-", on: .numbers))
        assert(!IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("/", on: .numbers))
        assert(!IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("：", on: .numbers))
        assert(!IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol(".", on: .numbers))
        assert(!IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("￥", on: .numbers))
        assert(IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("，", on: .numbers))
        assert(IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("。", on: .numbers))
        assert(IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("（", on: .numbers))
        assert(IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("；", on: .numbers))
        layout.switchPlane(.symbols)
        let symSnap = layout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(symSnap.keys.contains(where: { $0.primaryLabel == "《" }))
        assert(!IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("～", on: .symbols))
        assert(!IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("…", on: .symbols))
        assert(IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("《", on: .symbols))
        assert(IOSKeyboardLayoutEngine.shouldReturnToLettersAfterSymbol("+", on: .symbols))
        layout.switchPlane(.letters)

        // 5. Test Spacebar Trackpad Cursor Accumulator
        var trackpad = SpaceTrackpadAccumulator()
        trackpad.begin(at: 180.0)
        assert(trackpad.update(x: 200.0) == 2)
        assert(trackpad.update(x: 189.0) == -1)
        trackpad.end()

        // 6. Test end-to-end iOS 26 view coordinates -> Rust juxu-core touch slip rescue & drift learning!
        let dictsURL = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
            .appendingPathComponent("Resources/Dicts", isDirectory: true)
        let bridge = JuxuBridge(customDictsDir: dictsURL, inMemoryOnly: true)
        bridge.setKeyLayout(
            centers52: snap.centers52,
            originX: snap.originX,
            originY: snap.originY,
            pitchX: snap.pitchX,
            pitchY: snap.pitchY
        )

        let rKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "r")) })!
        let wKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "w")) })!
        let uKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "u")) })!

        let borderPoint = CGPoint(x: yKey.frame.minX + 1.2, y: tKey.frame.midY + 1.5)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "y"), x: Float(borderPoint.x), y: Float(borderPoint.y), isNormalized: false, timestampMs: 1000)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "r"), x: Float(rKey.frame.midX), y: Float(rKey.frame.midY), isNormalized: false, timestampMs: 1070)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "w"), x: Float(wKey.frame.midX), y: Float(wKey.frame.midY), isNormalized: false, timestampMs: 1140)
        let out = bridge.pushTouch(reportedAscii: UInt8(ascii: "u"), x: Float(uKey.frame.midX), y: Float(uKey.frame.midY), isNormalized: false, timestampMs: 1210)
        assert(out.candidates.first?.text == "我们", "Expected iOS 26 view-space touch slip rescue '我们'")

        // 7. Verify clean typographic mode toggle key (`中/英`), ergonomic downward Y-bias,
        //    and dynamic Wubi valid-next-key hitbox expansion!
        let modeKey = snap.keys.first(where: { $0.kind == .modeToggle })!
        assert(modeKey.primaryLabel == "中" && modeKey.secondaryInlineLabel == "/英" && modeKey.systemSymbol == nil)

        // Ergonomic thumb bias: tapping 2.2pt below `g`'s bottom edge (inside Row 1 -> Row 2 gutter) stays on `g`!
        let gKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "g")) })!
        let lowThumbTap = CGPoint(x: gKey.frame.midX, y: gKey.frame.maxY + 6.5)
        let biasedHit = IOSKeyboardLayoutEngine.hitTest(point: lowThumbTap, in: snap)
        assert(biasedHit?.kind == .letter(UInt8(ascii: "g")), "Expected ergonomic thumb Y-bias to keep low tap on 'g'")

        // Dynamic next-key hitbox expansion: after typing "trw", 'u' is valid in Wubi 86 while 'i' is dead!
        bridge.clearComposition()
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "t"), x: Float(tKey.frame.midX), y: Float(tKey.frame.midY), isNormalized: false, timestampMs: 2000)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "r"), x: Float(rKey.frame.midX), y: Float(rKey.frame.midY), isNormalized: false, timestampMs: 2060)
        let afterTrw = bridge.pushTouch(reportedAscii: UInt8(ascii: "w"), x: Float(wKey.frame.midX), y: Float(wKey.frame.midY), isNormalized: false, timestampMs: 2120)
        let iKey = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "i")) })!
        let uBit = UInt32(1) << UInt32(UInt8(ascii: "u") - UInt8(ascii: "a"))
        let iBit = UInt32(1) << UInt32(UInt8(ascii: "i") - UInt8(ascii: "a"))
        assert((afterTrw.validNextMask & uBit) != 0 && (afterTrw.validNextMask & iBit) == 0, "Expected 'u' valid and 'i' invalid after 'trw'")

        // Tap 2.4pt inside `i`'s left border: dynamic hitbox expansion snaps cleanly to valid 'u' and auto-commits "我们"!
        let edgeInsideI = CGPoint(x: iKey.frame.minX + 2.4, y: uKey.frame.midY)
        let dynHit = IOSKeyboardLayoutEngine.hitTest(point: edgeInsideI, in: snap, validNextMask: afterTrw.validNextMask)
        assert(dynHit?.kind == .letter(UInt8(ascii: "u")), "Expected dynamic hitbox expansion to snap border tap inside 'i' onto valid 'u'")
        let commitOut = bridge.pushTouch(reportedAscii: UInt8(ascii: "u"), x: Float(edgeInsideI.x), y: Float(edgeInsideI.y), isNormalized: false, timestampMs: 2180)
        assert(commitOut.commitText == "我们", "Expected clean 4-code auto-commit '我们' via dynamic next-key expansion, got '\(commitOut.commitText)'")

        // 8. Verify top-edge Spacebar tap during short-code composing (`validNextMask != all`) is NOT stolen by Row 2!
        bridge.clearComposition()
        let kState = bridge.pushTouch(reportedAscii: UInt8(ascii: "k"), x: Float(kKey.frame.midX), y: Float(kKey.frame.midY), isNormalized: false, timestampMs: 3000)
        let topEdgeSpaceTap = CGPoint(x: spaceKey.frame.midX, y: spaceKey.frame.minY - 4.0)
        let spaceHit = IOSKeyboardLayoutEngine.hitTest(point: topEdgeSpaceTap, in: snap, validNextMask: kState.validNextMask)
        assert(spaceHit?.kind == .space, "Expected top-edge Spacebar tap during short-code composing to hit Space, got \(String(describing: spaceHit?.kind))")

        // 9. Verify Sacred Keycap Core: light off-center tap >= 3.0pt inside a non-continuation key ('i' after 'trw')
        //    is NEVER hijacked by adjacent valid key ('u'), while < 3.0pt bevel slip still snaps!
        let interiorInsideI = CGPoint(x: iKey.frame.minX + 3.5, y: uKey.frame.midY)
        let sacredHit = IOSKeyboardLayoutEngine.hitTest(point: interiorInsideI, in: snap, validNextMask: afterTrw.validNextMask)
        assert(sacredHit?.kind == .letter(UInt8(ascii: "i")), "Expected sacred keycap core to preserve tap inside 'i', got \(String(describing: sacredHit?.kind))")

        // 10. Verify portrait 220pt canvas (`273pt - 53pt` ribbon) matches native 45pt keyH + 11pt rowGap + 7.0pt catch margin,
        //     and outer-margin corner tap beside 'z' never mutates to 'a' in Rust `push_touch_letter`!
        let snap220 = layout.computeLayout(in: CGSize(width: 440, height: 220))
        let space220 = snap220.keys.first(where: { $0.kind == .space })!
        let q220 = snap220.keys.first(where: { $0.kind == .letter(UInt8(ascii: "q")) })!
        assert(abs(q220.frame.height - 45.0) < 0.01, "Expected 45pt key height, got \(q220.frame.height)")
        assert(220.0 - space220.frame.maxY >= 6.5, "Expected >= 6.5pt bottom catch margin below Spacebar, got \(220.0 - space220.frame.maxY)")
        let bottomRimSpaceTap = CGPoint(x: space220.frame.midX, y: 219.5)
        let bottomSpaceHit = IOSKeyboardLayoutEngine.hitTest(point: bottomRimSpaceTap, in: snap220, validNextMask: kState.validNextMask)
        assert(bottomSpaceHit?.kind == .space, "Expected bottom-rim tap at y=219.5 to hit Spacebar")

        bridge.clearComposition()
        bridge.setKeyLayout(
            centers52: snap220.centers52,
            originX: snap220.originX,
            originY: snap220.originY,
            pitchX: snap220.pitchX,
            pitchY: snap220.pitchY
        )
        let z220 = snap220.keys.first(where: { $0.kind == .letter(UInt8(ascii: "z")) })!
        let zCorner = CGPoint(x: z220.frame.minX - 9.0, y: z220.frame.minY + 3.0)
        let zHit = IOSKeyboardLayoutEngine.hitTest(point: zCorner, in: snap220)
        assert(zHit?.kind == .letter(UInt8(ascii: "z")), "Expected gutter tap beside 'z' to hit 'z'")
        let zOut = bridge.pushTouch(reportedAscii: UInt8(ascii: "z"), x: Float(zCorner.x), y: Float(zCorner.y), isNormalized: false, timestampMs: 4000)
        assert(zOut.preedit == "z", "Expected Rust view-space touch to preserve UI hit-tested 'z', got '\(zOut.preedit)'")

        // 11. Verify native centered clipboard suggestion parser, smart OTP extraction, and changeCount caching!
        // 11a. Normal multiline text keeps original insertText and collapses displayText to single line
        let multi = IOSClipboardParser.parse(raw: "  第一行\n\n  第二行\t内容  ", changeCount: 1)!
        assert(!multi.isOTP && multi.subtitle == "来自剪贴板")
        assert(multi.insertText == "  第一行\n\n  第二行\t内容  ")
        assert(multi.displayText == "第一行 第二行 内容", "Got '\(multi.displayText)'")
        assert(IOSClipboardParser.parse(raw: "  \n\t ", changeCount: 1) == nil)

        // 11b. Chinese & English SMS/email OTP extraction with noise filtering (years, bank card tail, URLs)
        let bankSMS = IOSClipboardParser.parse(
            raw: "【招商银行】2026年11月19日您尾号8812的卡验证码为394810，5分钟内有效。",
            changeCount: 2
        )!
        assert(bankSMS.isOTP && bankSMS.insertText == "394810" && bankSMS.subtitle == "剪贴板验证码", "Got \(bankSMS)")

        let googleSMS = IOSClipboardParser.parse(
            raw: "G-591024 is your Google verification code.",
            changeCount: 3
        )!
        assert(googleSMS.isOTP && googleSMS.insertText == "591024", "Got \(googleSMS)")

        let appleGrouped = IOSClipboardParser.parse(
            raw: "Your Apple Account verification code is: 904 281",
            changeCount: 4
        )!
        assert(appleGrouped.isOTP && appleGrouped.insertText == "904281", "Got \(appleGrouped)")

        // Non-OTP text with numbers or URLs must NOT trigger OTP extraction
        let meetingNote = IOSClipboardParser.parse(raw: "2026年11月19日开会，房间号1024", changeCount: 5)!
        assert(!meetingNote.isOTP && meetingNote.insertText == "2026年11月19日开会，房间号1024")

        let otpURL = IOSClipboardParser.parse(raw: "https://example.com/otp/123456", changeCount: 6)!
        assert(!otpURL.isOTP && otpURL.insertText == "https://example.com/otp/123456")

        let sentenceWithURL = IOSClipboardParser.parse(raw: "请打开 https://example.com/otp/123456 查看", changeCount: 7)!
        assert(!sentenceWithURL.isOTP && sentenceWithURL.insertText == "请打开 https://example.com/otp/123456 查看")

        // Pure 6-digit code promotes subtitle when focused in `.oneTimeCode` field
        let pureCodeNormal = IOSClipboardParser.parse(raw: "482916", changeCount: 8, prefersOTP: false)!
        assert(!pureCodeNormal.isOTP && pureCodeNormal.insertText == "482916")
        let pureCodeOTP = IOSClipboardParser.parse(raw: "482916", changeCount: 8, prefersOTP: true)!
        assert(pureCodeOTP.isOTP && pureCodeOTP.subtitle == "剪贴板验证码" && pureCodeOTP.insertText == "482916")

        // 11c. IOSClipboardSuggestionStore: readString called ONCE per changeCount, TTL, consume, dismiss
        final class InMemoryKV: IOSClipboardKeyValueStore {
            var dict: [String: Any] = [:]
            func integer(forKey key: String) -> Int { dict[key] as? Int ?? 0 }
            func double(forKey key: String) -> Double { dict[key] as? Double ?? 0 }
            func string(forKey key: String) -> String? { dict[key] as? String }
            func object(forKey key: String) -> Any? { dict[key] }
            func set(_ value: Int, forKey key: String) { dict[key] = value }
            func set(_ value: Double, forKey key: String) { dict[key] = value }
            func set(_ value: Any?, forKey key: String) { dict[key] = value }
            func removeObject(forKey key: String) { dict.removeValue(forKey: key) }
        }

        let kv = InMemoryKV()
        let clipStore = IOSClipboardSuggestionStore(store: kv, ttlSeconds: 180)
        var readCalls = 0
        // No full access -> never reads pasteboard
        assert(clipStore.refresh(hasFullAccess: false, enabled: true, hasStrings: true, changeCount: 10, prefersOTP: false, now: 1000) {
            readCalls += 1; return "hello"
        } == nil)
        assert(readCalls == 0)

        // First observation reads once and caches
        let s1 = clipStore.refresh(hasFullAccess: true, enabled: true, hasStrings: true, changeCount: 10, prefersOTP: false, now: 1000) {
            readCalls += 1; return "482916"
        }
        assert(s1?.insertText == "482916" && readCalls == 1)

        // Second observation with same changeCount uses cache (0 extra pasteboard reads!) and respects prefersOTP dynamically
        let s2 = clipStore.refresh(hasFullAccess: true, enabled: true, hasStrings: true, changeCount: 10, prefersOTP: true, now: 1030) {
            readCalls += 1; return "482916"
        }
        assert(s2?.isOTP == true && readCalls == 1)

        // TTL expiry hides suggestion without re-reading pasteboard
        let sExpired = clipStore.refresh(hasFullAccess: true, enabled: true, hasStrings: true, changeCount: 10, prefersOTP: false, now: 1200) {
            readCalls += 1; return "482916"
        }
        assert(sExpired == nil && readCalls == 1)

        // New changeCount reads once again, then consume() permanently suppresses it
        let s3 = clipStore.refresh(hasFullAccess: true, enabled: true, hasStrings: true, changeCount: 11, prefersOTP: false, now: 1300) {
            readCalls += 1; return "新复制文本"
        }
        assert(s3?.insertText == "新复制文本" && readCalls == 2)
        clipStore.consume(changeCount: 11)
        let sAfterConsume = clipStore.refresh(hasFullAccess: true, enabled: true, hasStrings: true, changeCount: 11, prefersOTP: false, now: 1305) {
            readCalls += 1; return "新复制文本"
        }
        assert(sAfterConsume == nil && readCalls == 2)

        // Preference TOML round-trip
        var prefs = JuxuPreferences()
        assert(prefs.clipboardSuggestion == true)
        assert(prefs.autoSpaceZhEn == true)
        assert(prefs.autoSpaceDigits == true)
        prefs.clipboardSuggestion = false
        prefs.autoSpaceZhEn = false
        prefs.autoSpaceDigits = false
        let parsedPrefs = JuxuPreferences.fromTOML(prefs.toTOML())
        assert(parsedPrefs.clipboardSuggestion == false)
        assert(parsedPrefs.autoSpaceZhEn == false)
        assert(parsedPrefs.autoSpaceDigits == false)

        // iOS-specific auto-space composition flow with `overrideBefore` (simulating `markedCode` isolation + 5th-code push)
        var iosCompTracker = AutoSpaceContextTracker()
        let onOpts = AutoSpaceOptions(enabled: true, spaceDigits: true)
        let baseBeforeMarked = iosCompTracker.effectiveBefore(hostBefore: "hello", hostBeforeIsAuthoritative: false)
        // 5th-code push commits "国" with leading space while `markedCode` becomes "q"
        let pushCommit = iosCompTracker.prepareCommit("国", hostBefore: "hello", hostBeforeIsAuthoritative: false, overrideBefore: baseBeforeMarked, splitInternal: true, options: onOpts)
        assert(pushCommit == " 国")
        let nextBase = iosCompTracker.effectiveBefore(hostBefore: "hello", hostBeforeIsAuthoritative: false)
        assert(nextBase == "hello 国")
        let finalCommit = iosCompTracker.prepareCommit("我", hostBefore: "hello", hostBeforeIsAuthoritative: false, overrideBefore: nextBase, splitInternal: true, options: onOpts)
        assert(finalCommit == "我")
        // 123-plane digit after CJK
        assert(iosCompTracker.prepareCommit("1", hostBefore: "hello 国我", hostBeforeIsAuthoritative: false, splitInternal: false, options: onOpts) == " 1")

        // 12. Guard against transient full-screen bounds (e.g. 440x903 while paste alert interrupts layout):
        //     keyH must stay clamped <= 48pt and all 4 rows must remain inside the 220pt visible tray!
        let transientSnap = layout.computeLayout(in: CGSize(width: 440, height: 903))
        let transientQ = transientSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "q")) })!
        let transientSpace = transientSnap.keys.first(where: { $0.kind == .space })!
        assert(transientQ.frame.height <= 48.0, "Row 0 must never stretch on transient full-screen height, got \(transientQ.frame.height)")
        assert(transientSpace.frame.maxY <= 220.0, "All 4 rows must fit within 220pt visible tray, space.maxY=\(transientSpace.frame.maxY)")

        // 13. Non-blocking 2-step lookup + denied read (`nil`) suppresses repeat paste prompts for same changeCount
        let lookupNeeds = clipStore.lookup(hasFullAccess: true, enabled: true, hasStrings: true, changeCount: 12, prefersOTP: false, now: 1400)
        assert(lookupNeeds == .needsRead(changeCount: 12))
        let denied = clipStore.recordRead(changeCount: 12, rawText: nil, prefersOTP: false, now: 1401)
        assert(denied == nil)
        let afterDenied = clipStore.lookup(hasFullAccess: true, enabled: true, hasStrings: true, changeCount: 12, prefersOTP: false, now: 1405)
        assert(afterDenied == .none, "Denied pasteboard read must not re-prompt for same changeCount")

        // 14. Idle next-word / next-char prediction via WBAUX003 (N-gram + Micro-RWKV-7) FFI
        bridge.setContext("")
        let starters = bridge.predictNextWords(maxCount: 8)
        assert(starters.count == 8 && starters.first == "我")

        bridge.setContext("小红")
        let xhsPreds = bridge.predictNextWords(maxCount: 8)
        assert(xhsPreds.first == "书", "Expected '书' after '小红', got \(xhsPreds)")

        bridge.setContext("情绪")
        let moodPreds = bridge.predictNextWords(maxCount: 8)
        assert(moodPreds.prefix(3).contains("价值"), "Expected '价值' after '情绪', got \(moodPreds)")

        // Chained prediction via acceptPrediction + punctuation reset
        bridge.setContext("大模")
        assert(bridge.predictNextWords(maxCount: 8).first == "型")
        bridge.acceptPrediction("型", timestampMs: 3000)
        let afterModel = bridge.predictNextWords(maxCount: 8)
        assert(afterModel.count == 8 && afterModel.first != "型")

        bridge.setContext("大模型，")
        assert(bridge.predictNextWords(maxCount: 8) == starters, "Punctuation must reset idle predictions to starters")

        // 15. Clipboard visibility gate: show only right after fresh copy or app switch before typing;
        //     hide for the rest of the session once the user types anything.
        var clipGate = IOSClipboardVisibilityGate()
        assert(clipGate.shouldAllowDisplay(for: 20) == true)
        clipGate.markUserTyped()
        assert(clipGate.shouldAllowDisplay(for: 20) == false, "Must stay hidden after typing in same session")
        // Fresh copy (new changeCount) lifts suppression immediately
        assert(clipGate.shouldAllowDisplay(for: 21) == true, "Fresh copy must allow display again")
        clipGate.markUserTyped()
        assert(clipGate.shouldAllowDisplay(for: 21) == false)
        // Switching app / reopening keyboard (`viewWillAppear`) resets typing suppression
        clipGate.resetForKeyboardAppearance()
        assert(clipGate.shouldAllowDisplay(for: 21) == true, "App switch before typing must allow display again")

        // 16. Verify neural `nextKeyPriors` disambiguates BOTH:
        //     (a) Key #1 predictive gutter taps (`validNextMask == 0x03FF_FFFF`), and
        //     (b) Keys #2..#4 valid-vs-valid gutter ties (e.g. `f` vs `g` when both are valid continuations!),
        //     while strictly preserving the `3.0pt x 2.6pt` Sacred Keycap Core!
        let f220 = snap220.keys.first(where: { $0.kind == .letter(UInt8(ascii: "f")) })!
        let g220 = snap220.keys.first(where: { $0.kind == .letter(UInt8(ascii: "g")) })!
        let fgMidGutter = CGPoint(x: (f220.frame.maxX + g220.frame.minX) * 0.5 - 0.8, y: f220.frame.midY)

        // (a) Key #1: without neural priors, point 0.8pt closer to 'f' hits 'f'; with strong 'g' prior, snaps to 'g'!
        let neutralHit1 = IOSKeyboardLayoutEngine.hitTest(point: fgMidGutter, in: snap220)
        assert(neutralHit1?.kind == .letter(UInt8(ascii: "f")))
        var priorsKey1 = Array(repeating: UInt8(100), count: 26)
        priorsKey1[Int(UInt8(ascii: "g") - 97)] = 162
        priorsKey1[Int(UInt8(ascii: "f") - 97)] = 74
        let neuralHit1 = IOSKeyboardLayoutEngine.hitTest(
            point: fgMidGutter,
            in: snap220,
            validNextMask: 0x03FF_FFFF,
            nextKeyPriors: priorsKey1
        )
        assert(neuralHit1?.kind == .letter(UInt8(ascii: "g")), "Expected Key #1 neural prior to pull disputed gutter tap to 'g'")

        // (b) Keys #2..#4 valid-vs-valid gutter tie (both 'f' and 'g' have mask bit = 1, e.g. `im+g 没理` vs `im+f 油井`)
        let bothFGValidMask: UInt32 = (1 << UInt32(UInt8(ascii: "f") - 97)) | (1 << UInt32(UInt8(ascii: "g") - 97))
        let bevelInsideF = CGPoint(x: f220.frame.maxX - 1.8, y: f220.frame.midY)
        let flatMaskHit = IOSKeyboardLayoutEngine.hitTest(point: bevelInsideF, in: snap220, validNextMask: bothFGValidMask)
        assert(flatMaskHit?.kind == .letter(UInt8(ascii: "f")), "Flat binary mask cannot break valid-vs-valid tie toward 'g'")
        var priorsValidTie = Array(repeating: UInt8(0), count: 26)
        priorsValidTie[Int(UInt8(ascii: "g") - 97)] = 168
        priorsValidTie[Int(UInt8(ascii: "f") - 97)] = 58
        let neuralTieHit = IOSKeyboardLayoutEngine.hitTest(
            point: bevelInsideF,
            in: snap220,
            validNextMask: bothFGValidMask,
            nextKeyPriors: priorsValidTie
        )
        assert(neuralTieHit?.kind == .letter(UInt8(ascii: "g")), "Expected neural valid-vs-valid prior to snap 'f' bevel slip onto 'g'")

        // (c) Sacred Keycap Core (`>= 3.0pt` inside 'f') is STILL protected even under maximum 'g' neural prior!
        let coreInsideF = CGPoint(x: f220.frame.maxX - 3.6, y: f220.frame.midY)
        let corePreserved = IOSKeyboardLayoutEngine.hitTest(
            point: coreInsideF,
            in: snap220,
            validNextMask: bothFGValidMask,
            nextKeyPriors: priorsValidTie
        )
        assert(corePreserved?.kind == .letter(UInt8(ascii: "f")), "Sacred keycap core must never be overridden by neural priors")

        // (d) Adaptive personal `touchOffsets` shift gutter ownership toward the user's calibrated key center!
        var personalOffsets = Array(repeating: Float(0), count: 52)
        personalOffsets[Int(UInt8(ascii: "f") - 97) * 2] = 0.16 // User consistently strikes 'f' to the right
        let personalHit = IOSKeyboardLayoutEngine.hitTest(
            point: fgMidGutter,
            in: snap220,
            validNextMask: 0x03FF_FFFF,
            nextKeyPriors: Array(repeating: UInt8(100), count: 26),
            touchOffsets: personalOffsets
        )
        assert(personalHit?.kind == .letter(UInt8(ascii: "f")), "Personal right-shifted TouchProfile for 'f' should capture f/g gutter tap")

        // (e) `compensatedLetterPoint` matches `hitTest` ergonomic Y compensation 1:1
        let row1Comp = IOSKeyboardLayoutEngine.compensatedLetterPoint(CGPoint(x: f220.frame.midX, y: f220.frame.midY), in: snap220)
        assert(abs((f220.frame.midY - row1Comp.y) - IOSKeyboardLayoutEngine.ergonomicThumbYBias) < 0.01)

        // 17. Cangjie radical keycaps option + automatic fallback on ASCII/Shift/disabled
        var cjLayout = IOSKeyboardLayoutEngine()
        cjLayout.schemaId = 35
        cjLayout.cangjieKeycapRadicals = true
        let cjSnap = cjLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(cjSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "q")) })?.primaryLabel == "手")
        assert(cjSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "a")) })?.primaryLabel == "日")
        assert(cjSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "z")) })?.primaryLabel == "重")
        assert(cjSnap.keys.first(where: { $0.kind == .space })?.cornerSubscript == "仓")
        cjLayout.asciiMode = true
        let cjAsciiSnap = cjLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(cjAsciiSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "q")) })?.primaryLabel == "q")
        cjLayout.asciiMode = false
        cjLayout.isZLookupActive = true
        let cjZLookupSnap = cjLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(cjZLookupSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "q")) })?.primaryLabel == "q")
        cjLayout.isZLookupActive = false
        cjLayout.cangjieKeycapRadicals = false
        let cjOffSnap = cjLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(cjOffSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "q")) })?.primaryLabel == "q")
        var cjPrefs = JuxuPreferences()
        assert(cjPrefs.cangjieKeycapRadicals == true)
        cjPrefs.cangjieKeycapRadicals = false
        assert(JuxuPreferences.fromTOML(cjPrefs.toTOML()).cangjieKeycapRadicals == false)

        // 18. Wubi radical keycaps option (86 / 98 / 06 version-specific tables + ASCII/Shift fallback)
        var wbLayout = IOSKeyboardLayoutEngine()
        wbLayout.schemaId = 86
        wbLayout.wubiKeycapRadicals = true
        let wb86Snap = wbLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(wb86Snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "g")) })?.keycapRadicals == "王戋\n五一")
        assert(wb86Snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "m")) })?.keycapRadicals?.contains("几") == true)
        assert(wb86Snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "z")) })?.keycapRadicals == "万能")
        wbLayout.schemaId = 98
        let wb98Snap = wbLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(wb98Snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "w")) })?.keycapRadicals?.contains("几") == true)
        assert(wb98Snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "b")) })?.keycapRadicals?.contains("皮") == true)
        wbLayout.schemaId = 6
        let wb06Snap = wbLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(wb06Snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "y")) })?.keycapRadicals?.contains("六") == true)
        wbLayout.asciiMode = true
        let wbAsciiSnap = wbLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(wbAsciiSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "g")) })?.keycapRadicals == nil)
        var wbPrefs = JuxuPreferences()
        assert(wbPrefs.wubiKeycapRadicals == false)
        wbPrefs.wubiKeycapRadicals = true
        assert(JuxuPreferences.fromTOML(wbPrefs.toTOML()).wubiKeycapRadicals == true)

        // 19. T9 9-key Pinyin layout geometry, ASCII fallback to 26-key, dynamic neural priors & personal T9 touchOffsets
        var t9Layout = IOSKeyboardLayoutEngine()
        t9Layout.schemaId = 20
        t9Layout.pinyinT9Layout = true
        assert(t9Layout.isT9PinyinActive)
        let t9Snap = t9Layout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(t9Snap.isT9Layout)
        assert(t9Snap.t9SidebarFrame.width > 55 && t9Snap.t9SidebarFrame.height > 140)
        assert(t9Snap.keys.filter { if case .t9Key = $0.kind { return true } else { return false } }.count == 9)
        assert(t9Snap.keys.first(where: { $0.kind == .space })?.cornerSubscript == "九")
        assert(t9Snap.keys.contains(where: { $0.kind == .t9Clear }))
        // Verify no T9 key overlaps the left sidebar or escapes 220pt canvas
        for k in t9Snap.keys {
            assert(!k.frame.intersects(t9Snap.t9SidebarFrame), "T9 key \(k.id) overlaps sidebar")
            assert(k.frame.maxY <= 220.0, "T9 key \(k.id) exceeds 220pt canvas height")
        }
        // Dynamic neural prior tie-breaker on the gutter between T9 key '5'(JKL) and '6'(MNO)
        let k5 = t9Snap.keys.first(where: { $0.kind == .t9Key(UInt8(ascii: "5")) })!
        let k6 = t9Snap.keys.first(where: { $0.kind == .t9Key(UInt8(ascii: "6")) })!
        let k56Gutter = CGPoint(x: (k5.frame.maxX + k6.frame.minX) * 0.5, y: k5.frame.midY + IOSKeyboardLayoutEngine.ergonomicThumbYBias)
        var t9Priors = Array(repeating: UInt8(100), count: 8)
        t9Priors[Int(UInt8(ascii: "5") - UInt8(ascii: "2"))] = 78
        t9Priors[Int(UInt8(ascii: "6") - UInt8(ascii: "2"))] = 165
        let t9Hit6 = IOSKeyboardLayoutEngine.hitTest(
            point: k56Gutter,
            in: t9Snap,
            t9ValidNextMask: 0xFF,
            t9NextKeyPriors: t9Priors
        )
        assert(t9Hit6?.kind == .t9Key(UInt8(ascii: "6")), "T9 neural prior should arbitrate 5/6 gutter toward '6'")
        // Personal T9 touchOffset shifts 5/6 gutter toward '5'
        var t9Offsets = Array(repeating: Float(0), count: 16)
        t9Offsets[Int(UInt8(ascii: "5") - UInt8(ascii: "2")) * 2] = 0.16
        let t9PersonalHit5 = IOSKeyboardLayoutEngine.hitTest(
            point: k56Gutter,
            in: t9Snap,
            t9ValidNextMask: 0xFF,
            t9NextKeyPriors: Array(repeating: UInt8(100), count: 8),
            t9TouchOffsets: t9Offsets
        )
        assert(t9PersonalHit5?.kind == .t9Key(UInt8(ascii: "5")), "Personal T9 touchOffset should arbitrate 5/6 gutter toward '5'")
        // Switching to ASCII automatically restores 26-key QWERTY layout
        t9Layout.asciiMode = true
        assert(!t9Layout.isT9PinyinActive)
        let t9AsciiSnap = t9Layout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(!t9AsciiSnap.isT9Layout)
        assert(t9AsciiSnap.keys.filter { if case .letter = $0.kind { return true } else { return false } }.count == 26)
        // Preferences TOML roundtrip
        var t9Prefs = JuxuPreferences()
        assert(t9Prefs.pinyinT9Layout == false)
        t9Prefs.pinyinT9Layout = true
        assert(JuxuPreferences.fromTOML(t9Prefs.toTOML()).pinyinT9Layout == true)

        // Swipe-up secondary input
        assert(IOSSwipeUp.symbol(for: UInt8(ascii: "q"), asciiMode: false) == "1")
        assert(IOSSwipeUp.symbol(for: UInt8(ascii: "p"), asciiMode: true) == "0")
        assert(IOSSwipeUp.symbol(for: UInt8(ascii: "d"), asciiMode: false) == "：")
        assert(IOSSwipeUp.symbol(for: UInt8(ascii: "d"), asciiMode: true) == ":")
        assert(IOSSwipeUp.symbol(for: UInt8(ascii: "l"), asciiMode: true) == "\"")
        for b in Array("abcdefghijklmnopqrstuvwxyz".utf8) {
            assert(IOSSwipeUp.symbol(for: b, asciiMode: false) != nil)
            assert(IOSSwipeUp.symbol(for: b, asciiMode: true) != nil)
        }
        var swipeLayout = IOSKeyboardLayoutEngine()
        swipeLayout.swipeUpInputEnabled = true
        let swipeSnap = swipeLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(swipeSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "w")) })?.swipeUpSymbol == "2")
        assert(swipeSnap.keys.first(where: { $0.kind == .backspace })?.swipeUpSymbol == nil)
        swipeLayout.asciiMode = true
        let swipeAscii = swipeLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(swipeAscii.keys.first(where: { $0.kind == .letter(UInt8(ascii: "b")) })?.swipeUpSymbol == "?")
        let o = CGPoint(x: 100, y: 100)
        assert(!IOSSwipeUp.isActive(from: o, to: CGPoint(x: 101, y: 80), keyHeight: 45, wasActive: false), "Thumb lift roll must stay a tap")
        assert(IOSSwipeUp.isActive(from: o, to: CGPoint(x: 103, y: 72), keyHeight: 45, wasActive: false))
        assert(!IOSSwipeUp.isActive(from: o, to: CGPoint(x: 120, y: 70), keyHeight: 45, wasActive: false), "Diagonal slide is not a swipe-up")
        assert(!IOSSwipeUp.isActive(from: o, to: CGPoint(x: 100, y: 130), keyHeight: 45, wasActive: false))
        assert(IOSSwipeUp.isActive(from: o, to: CGPoint(x: 100, y: 82), keyHeight: 45, wasActive: true), "Hysteresis keeps an armed swipe")
        assert(!IOSSwipeUp.isActive(from: o, to: CGPoint(x: 100, y: 86), keyHeight: 45, wasActive: true), "Sliding back cancels")
        // During active Pinyin/Wubi composition (`isComposing: true`), a 28pt thumb flick must NOT break composition,
        // while a deliberate 38pt vertical swipe still arms cleanly.
        assert(!IOSSwipeUp.isActive(from: o, to: CGPoint(x: 103, y: 72), keyHeight: 45, wasActive: false, isComposing: true))
        assert(IOSSwipeUp.isActive(from: o, to: CGPoint(x: 102, y: 62), keyHeight: 45, wasActive: false, isComposing: true))

        // Long-press alternates
        let uAlt = IOSKeyAlternates.options(for: UInt8(ascii: "u"), asciiMode: false)
        assert(uAlt.items == ["ū", "ú", "ǔ", "ù", "U", "7", "u"])
        assert(uAlt.items[uAlt.defaultIndex] == "7")
        let vAlt = IOSKeyAlternates.options(for: UInt8(ascii: "v"), asciiMode: true)
        assert(vAlt.items.contains("ü") && vAlt.items[vAlt.defaultIndex] == "&")
        let bAlt = IOSKeyAlternates.options(for: UInt8(ascii: "b"), asciiMode: false)
        assert(bAlt.items == ["B", "？", "b"] && bAlt.defaultIndex == 1)
        for key in swipeSnap.keys {
            guard case .letter(let ascii) = key.kind else { continue }
            let o = IOSKeyAlternates.options(for: ascii, asciiMode: false)
            let strip = IOSKeyAlternates.stripFrame(keyFrame: key.frame, count: o.items.count, defaultIndex: o.defaultIndex, containerWidth: 440)
            assert(strip.minX >= 4.0 && strip.maxX <= 436.0, "Alternates strip must stay inside the keyboard")
            assert(IOSKeyAlternates.index(atX: strip.minX - 50, in: strip, count: o.items.count) == 0)
            assert(IOSKeyAlternates.index(atX: strip.maxX + 50, in: strip, count: o.items.count) == o.items.count - 1)
        }
        let narrow = IOSKeyAlternates.stripFrame(keyFrame: CGRect(x: 300, y: 60, width: 30, height: 42), count: 8, defaultIndex: 6, containerWidth: 320)
        assert(narrow.minX >= 4.0 && narrow.maxX <= 316.0)
        // Symbol key long-press alternates
        let zeroAlt = IOSKeyAlternates.symbolOptions(for: "0", asciiMode: false)
        assert(zeroAlt?.items == ["0", "°"] && zeroAlt?.defaultIndex == 0)
        let rmbAlt = IOSKeyAlternates.symbolOptions(for: "￥", asciiMode: false)
        assert(rmbAlt?.items.first == "￥" && (rmbAlt?.items.contains("$") == true) && rmbAlt?.defaultIndex == 0)
        let cnParenAlt = IOSKeyAlternates.symbolOptions(for: "（", asciiMode: false)
        assert(cnParenAlt?.items.first == "（" && (cnParenAlt?.items.contains("「") == true) && cnParenAlt?.defaultIndex == 0)
        assert(IOSKeyAlternates.symbolOptions(for: "1", asciiMode: false) == nil)
        var symPlaneLayout = IOSKeyboardLayoutEngine()
        symPlaneLayout.switchPlane(.numbers)
        for key in symPlaneLayout.computeLayout(in: CGSize(width: 440, height: 220)).keys {
            guard case .symbol(let sym) = key.kind,
                  let o = IOSKeyAlternates.symbolOptions(for: sym, asciiMode: false) else { continue }
            let strip = IOSKeyAlternates.stripFrame(keyFrame: key.frame, count: o.items.count, defaultIndex: o.defaultIndex, containerWidth: 440)
            assert(strip.minX >= 4.0 && strip.maxX <= 436.0, "Symbol alternates strip must stay inside the keyboard")
        }
        var offLayout = IOSKeyboardLayoutEngine()
        offLayout.swipeUpInputEnabled = false
        let offSnap = offLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(offSnap.keys.allSatisfy { $0.swipeUpSymbol == nil }, "Switching swipe-up off must remove letter hints and gesture")
        // Backspace swipe-up clears only the current line, left of the cursor
        assert(IOSLineClear.plan(before: "第一行\n第二行", maxBatch: 128) == .init(deleteCount: 3, reachesLineStart: true))
        assert(IOSLineClear.plan(before: "第一行\n", maxBatch: 128) == .init(deleteCount: 0, reachesLineStart: true), "Cursor at line start must not join lines")
        assert(IOSLineClear.plan(before: "没有换行", maxBatch: 128) == .init(deleteCount: 4, reachesLineStart: false))
        assert(IOSLineClear.plan(before: "a\r\nbc", maxBatch: 128) == .init(deleteCount: 2, reachesLineStart: true))
        assert(IOSLineClear.plan(before: String(repeating: "字", count: 300), maxBatch: 128) == .init(deleteCount: 128, reachesLineStart: false))
        assert(IOSLineClear.plan(before: "x\n" + String(repeating: "字", count: 200), maxBatch: 128).deleteCount == 128)
        let partial = IOSLineClear.Expected(previous: "abcdef", removed: 2)
        assert(partial.isSatisfied(by: "abcd") && partial.isSatisfied(by: "zzabcd"))
        assert(!partial.isSatisfied(by: "abcdef") && !partial.isSatisfied(by: "abcde"), "Stale context must wait")
        assert(!partial.isSatisfied(by: "other"), "Another field must not be deleted")
        let whole = IOSLineClear.Expected(previous: "abc", removed: 3)
        assert(!whole.isSatisfied(by: "abc") && !whole.isSatisfied(by: "ab"))
        assert(whole.isSatisfied(by: "上一行\n"))
        // Backspace swipe-down restores line clears in reverse order
        let docA = UUID(), docB = UUID()
        var undo = IOSLineClearUndo()
        undo.push(text: "第二行", contextBefore: "第一行\n", documentID: docA)
        undo.push(text: "第三行", contextBefore: "第一行\n第二行\n", documentID: docA)
        assert(undo.restorableIndex(before: "第一行\n第二行\n", documentID: docB) == nil, "Must not restore into another field")
        assert(undo.restorableIndex(before: "第一行\n第二行\n", documentID: nil) == nil, "Nil documentID during focus transition must never match")
        assert(undo.restorableIndex(before: "别的内容", documentID: docA) == nil, "Must not restore after the text changed")
        assert(undo.take(before: "第一行\n第二行\n", documentID: docA) == "第三行")
        assert(undo.take(before: "第一行\n", documentID: docA) == "第二行")
        assert(undo.take(before: "第一行\n", documentID: docA) == nil)
        var startUndo = IOSLineClearUndo()
        startUndo.push(text: "整行", contextBefore: "", documentID: docA)
        assert(startUndo.restorableIndex(before: "有字", documentID: docA) == nil, "Document-start record needs an empty context")
        assert(startUndo.take(before: "", documentID: docA) == "整行")
        startUndo.push(text: "", contextBefore: "", documentID: docA)
        assert(startUndo.records.isEmpty)
        for i in 0..<(IOSLineClearUndo.maxRecords + 3) {
            startUndo.push(text: "\(i)", contextBefore: "x", documentID: docA)
        }
        assert(startUndo.records.count == IOSLineClearUndo.maxRecords)
        assert(startUndo.take(before: "x", documentID: docA) == "\(IOSLineClearUndo.maxRecords + 2)")

        // Chinese punctuation pairing
        for (open, close) in [("“", "”"), ("‘", "’"), ("（", "）"), ("《", "》"), ("【", "】"), ("「", "」")] {
            assert(IOSPunctuationPairing.closer(for: open) == close)
            assert(IOSPunctuationPairing.action(for: open, after: nil, openClosers: []) == .insertPair(close: close))
        }
        assert(IOSPunctuationPairing.action(for: "（", after: "，后文", openClosers: []) == .insertPair(close: "）"))
        assert(IOSPunctuationPairing.action(for: "（", after: "\n下一行", openClosers: []) == .insertPair(close: "）"))
        assert(IOSPunctuationPairing.action(for: "“", after: "已有文字", openClosers: []) == .plain, "No auto-close directly before text")
        assert(IOSPunctuationPairing.action(for: "”", after: "”", openClosers: ["”"]) == .skipClose)
        assert(IOSPunctuationPairing.action(for: "”", after: "”", openClosers: []) == .plain, "Only auto-inserted closers are skipped")
        assert(IOSPunctuationPairing.action(for: "）", after: "”", openClosers: ["”"]) == .plain)
        assert(IOSPunctuationPairing.action(for: "，", after: nil, openClosers: []) == .plain)
        assert(IOSPunctuationPairing.shouldDeletePair(before: "你好“", after: "”", openClosers: ["”"]))
        assert(!IOSPunctuationPairing.shouldDeletePair(before: "你好“字", after: "”", openClosers: ["”"]), "Pair with content inside stays")
        assert(!IOSPunctuationPairing.shouldDeletePair(before: "“", after: "”", openClosers: []))
        assert(!IOSPunctuationPairing.shouldDeletePair(before: "（", after: "”", openClosers: ["”"]))
        assert(IOSPunctuationPairing.pruned(["）", "”"], after: "”）") == ["）", "”"])
        assert(IOSPunctuationPairing.pruned(["”"], after: "别的") == [])
        assert(IOSPunctuationPairing.pruned(["”"], after: nil) == [])
        assert(IOSPunctuationPairing.cursorStaysInPairs(["）", "”"], tail: "，后文", after: "”），后文"))
        assert(IOSPunctuationPairing.cursorStaysInPairs(["”"], tail: "很长的后文", after: "”很长"), "A trimmed window still matches")
        assert(!IOSPunctuationPairing.cursorStaysInPairs(["”"], tail: "，后文", after: "”别处的文字"), "Tapping before another closer must forget the pair")
        var featurePrefs = JuxuPreferences()
        assert(!featurePrefs.keySwipeUpInput && !featurePrefs.keyLongPressAlternates && !featurePrefs.backspaceSwipeClear, "Gesture features are opt-in")
        featurePrefs.keySwipeUpInput = true
        featurePrefs.keyLongPressAlternates = true
        featurePrefs.backspaceSwipeClear = true
        let featureRound = JuxuPreferences.fromTOML(featurePrefs.toTOML())
        assert(featureRound.keySwipeUpInput && featureRound.keyLongPressAlternates && featureRound.backspaceSwipeClear)

        // 20. Verify `IOSInlineMarkedTextSession` against BOTH custom `YYTextView` (Bilibili/Weibo)
        //     and standard Apple `UITextView` `UITextInput` semantics!
        //
        //     `YYTextView.m` implements `insertText:` as `[self replaceRange:_selectedTextRange withText:text]`,
        //     completely ignoring `_markedTextRange`, and `unmarkText` sets `_markedTextRange = nil` WITHOUT
        //     firing `textViewDidChange:`.
        @MainActor
        final class YYTextViewDocumentMock: IOSMarkedTextDocumentSink {
            var storage = NSMutableString()
            var selectedRange = NSRange(location: 0, length: 0)
            var markedRange: NSRange?
            var didChangeUnmarkedCount = 0

            init(initialText: String = "", cursor: Int? = nil) {
                storage.setString(initialText)
                let pos = cursor ?? storage.length
                selectedRange = NSRange(location: pos, length: 0)
            }

            var text: String { storage as String }

            var documentContextBeforeInput: String? {
                guard selectedRange.location > 0 else { return nil }
                return storage.substring(with: NSRange(location: 0, length: selectedRange.location))
            }

            var documentContextAfterInput: String? {
                let end = selectedRange.location + selectedRange.length
                guard end < storage.length else { return nil }
                return storage.substring(with: NSRange(location: end, length: storage.length - end))
            }

            var selectedText: String? {
                guard selectedRange.length > 0 else { return nil }
                return storage.substring(with: selectedRange)
            }

            func insertText(_ text: String) {
                guard !text.isEmpty else { return }
                let len = (text as NSString).length
                // Exact `YYTextView.m` behavior: replaces `_selectedTextRange` ONLY; does NOT touch `_markedTextRange`!
                storage.replaceCharacters(in: selectedRange, with: text)
                if let mr = markedRange, selectedRange.location <= mr.location {
                    markedRange = NSRange(location: mr.location + len - selectedRange.length, length: mr.length)
                }
                selectedRange = NSRange(location: selectedRange.location + len, length: 0)
                if markedRange == nil {
                    didChangeUnmarkedCount += 1
                }
            }

            func setMarkedText(_ markedText: String, selectedRange relSel: NSRange) {
                if selectedRange.length > 0 {
                    storage.replaceCharacters(in: selectedRange, with: "")
                    selectedRange = NSRange(location: selectedRange.location, length: 0)
                }
                let nsMarked = markedText as NSString
                if let mr = markedRange {
                    storage.replaceCharacters(in: mr, with: markedText)
                    markedRange = NSRange(location: mr.location, length: nsMarked.length)
                    selectedRange = NSRange(location: mr.location + relSel.location, length: relSel.length)
                } else {
                    let start = selectedRange.location + selectedRange.length
                    markedRange = NSRange(location: start, length: nsMarked.length)
                    storage.replaceCharacters(in: NSRange(location: start, length: 0), with: markedText)
                    selectedRange = NSRange(location: start + relSel.location, length: relSel.length)
                }
                if markedRange?.length == 0 {
                    markedRange = nil
                }
                if markedRange == nil {
                    didChangeUnmarkedCount += 1
                }
            }

            func unmarkText() {
                // Exact `YYTextView.m` behavior: clears `_markedTextRange` without firing `textViewDidChange:`!
                markedRange = nil
            }

            func adjustTextPosition(byCharacterOffset offset: Int) {
                let str = storage as String
                let utf16 = str.utf16
                let clampedLoc = max(0, min(utf16.count, selectedRange.location))
                let curIdx = utf16.index(utf16.startIndex, offsetBy: clampedLoc)
                if let charIdx = curIdx.samePosition(in: str) {
                    let targetCharIdx = str.index(charIdx, offsetBy: offset, limitedBy: offset >= 0 ? str.endIndex : str.startIndex)
                        ?? (offset >= 0 ? str.endIndex : str.startIndex)
                    let newLoc = targetCharIdx.utf16Offset(in: str)
                    selectedRange = NSRange(location: newLoc, length: 0)
                }
            }
        }

        @MainActor
        final class StandardUITextViewMock: IOSMarkedTextDocumentSink {
            var storage = NSMutableString()
            var selectedRange = NSRange(location: 0, length: 0)
            var markedRange: NSRange?
            var simulateReactivatedEmptyMarkedCaretJump = false

            init(initialText: String = "", cursor: Int? = nil) {
                storage.setString(initialText)
                let pos = cursor ?? storage.length
                selectedRange = NSRange(location: pos, length: 0)
            }

            var text: String { storage as String }

            var documentContextBeforeInput: String? {
                guard selectedRange.location > 0 else { return nil }
                return storage.substring(with: NSRange(location: 0, length: selectedRange.location))
            }

            var documentContextAfterInput: String? {
                let end = selectedRange.location + selectedRange.length
                guard end < storage.length else { return nil }
                return storage.substring(with: NSRange(location: end, length: storage.length - end))
            }

            var selectedText: String? {
                guard selectedRange.length > 0 else { return nil }
                return storage.substring(with: selectedRange)
            }

            func insertText(_ text: String) {
                let len = (text as NSString).length
                let target = markedRange ?? selectedRange
                storage.replaceCharacters(in: target, with: text)
                markedRange = nil
                selectedRange = NSRange(location: target.location + len, length: 0)
            }

            func setMarkedText(_ markedText: String, selectedRange relSel: NSRange) {
                let nsMarked = markedText as NSString
                let target = markedRange ?? selectedRange
                storage.replaceCharacters(in: target, with: markedText)
                if nsMarked.length == 0 && simulateReactivatedEmptyMarkedCaretJump {
                    markedRange = nil
                    selectedRange = NSRange(location: 0, length: 0)
                } else {
                    markedRange = NSRange(location: target.location, length: nsMarked.length)
                    selectedRange = NSRange(location: target.location + relSel.location, length: relSel.length)
                }
            }

            func unmarkText() {
                // Standard UIKit commits whatever is currently in `markedRange` in place!
                markedRange = nil
            }

            func adjustTextPosition(byCharacterOffset offset: Int) {
                let str = storage as String
                let utf16 = str.utf16
                let clampedLoc = max(0, min(utf16.count, selectedRange.location))
                let curIdx = utf16.index(utf16.startIndex, offsetBy: clampedLoc)
                if let charIdx = curIdx.samePosition(in: str) {
                    let targetCharIdx = str.index(charIdx, offsetBy: offset, limitedBy: offset >= 0 ? str.endIndex : str.startIndex)
                        ?? (offset >= 0 ? str.endIndex : str.startIndex)
                    let newLoc = targetCharIdx.utf16Offset(in: str)
                    selectedRange = NSRange(location: newLoc, length: 0)
                }
            }
        }

        /// Simulates iOS 26.5 `UIInputViewController` -> `TIKeyboardOutput` XPC coalescing!
        /// Within a single runloop turn, UIKit applies:
        ///   1) `insertionText` (`insertText`)
        ///   2) ONLY the last `setMarkedText` call of the turn
        ///   3) `unmarkIfNecessary` (`unmarkText`) AFTER `setMarkedText`!
        @MainActor
        final class TIKeyboardOutputProxyMock: IOSMarkedTextDocumentSink {
            let host: any IOSMarkedTextDocumentSink
            private var turnInsertion = ""
            private var turnMarked: (String, NSRange)?
            private var turnUnmark = false

            init(host: any IOSMarkedTextDocumentSink) {
                self.host = host
            }

            var documentContextBeforeInput: String? { host.documentContextBeforeInput }
            var documentContextAfterInput: String? { host.documentContextAfterInput }
            var selectedText: String? { host.selectedText }

            func adjustTextPosition(byCharacterOffset offset: Int) {
                host.adjustTextPosition(byCharacterOffset: offset)
            }

            func insertText(_ text: String) {
                turnInsertion += text
            }

            func setMarkedText(_ markedText: String, selectedRange: NSRange) {
                turnMarked = (markedText, selectedRange)
            }

            func unmarkText() {
                turnUnmark = true
            }

            func flushXPCBatch() {
                let ins = turnInsertion
                let mk = turnMarked
                let unm = turnUnmark
                turnInsertion = ""
                turnMarked = nil
                turnUnmark = false
                if !ins.isEmpty {
                    host.insertText(ins)
                }
                if let (text, range) = mk {
                    host.setMarkedText(text, selectedRange: range)
                }
                if unm {
                    host.unmarkText()
                }
            }

            func drainTurn(session: inout IOSInlineMarkedTextSession) {
                flushXPCBatch()
                session.flushTurnBoundary(in: self)
                flushXPCBatch()
            }
        }

        // 20a. Bilibili comment box exact repro #1: `dbfd` -> "破坏", then prediction "计算", then `sm` -> "机"
        bridge.switchSchema(86, persist: false)
        var wb86NormalPrefs = bridge.prefs
        wb86NormalPrefs.spaceless = false
        bridge.applyAndSavePreferences(wb86NormalPrefs, persist: false)
        bridge.clearComposition()

        for rawHost in [YYTextViewDocumentMock() as any IOSMarkedTextDocumentSink, StandardUITextViewMock()] {
            let proxy = TIKeyboardOutputProxyMock(host: rawHost)
            var session = IOSInlineMarkedTextSession()
            bridge.clearComposition()
            for ch in "dbfd".unicodeScalars {
                let st = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 8000)
                session.applyEngineState(commitText: st.commitText, inlinePreedit: st.activeInlinePreedit, options: onOpts, in: proxy)
                proxy.drainTurn(session: &session)
            }
            let commitPohuai = bridge.handleKey(keyCode: 0x31, scalar: 32, hasCmdOrCtrl: false, timestampMs: 8050)
            assert(commitPohuai.commitText == "破坏")
            session.applyEngineState(commitText: commitPohuai.commitText, inlinePreedit: commitPohuai.activeInlinePreedit, options: onOpts, in: proxy)
            proxy.drainTurn(session: &session)
            // Idle prediction tap ("计算")
            session.insertTextWithAutoSpace("计算", splitInternal: true, options: onOpts, in: proxy)
            proxy.drainTurn(session: &session)
            // Follow-up word `sm` -> Space -> "机"
            for ch in "sm".unicodeScalars {
                let st = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 8100)
                session.applyEngineState(commitText: st.commitText, inlinePreedit: st.activeInlinePreedit, options: onOpts, in: proxy)
                proxy.drainTurn(session: &session)
            }
            let commitJi = bridge.handleKey(keyCode: 0x31, scalar: 32, hasCmdOrCtrl: false, timestampMs: 8150)
            assert(commitJi.commitText == "机")
            session.applyEngineState(commitText: commitJi.commitText, inlinePreedit: commitJi.activeInlinePreedit, options: onOpts, in: proxy)
            proxy.drainTurn(session: &session)

            if let yy = rawHost as? YYTextViewDocumentMock {
                assert(yy.text == "破坏计算机", "Expected '破坏计算机' in YYTextView, got '\(yy.text)'")
                assert(yy.markedRange == nil, "Expected nil markedRange after commit in YYTextView")
                assert(yy.selectedRange == NSRange(location: 5, length: 0))
            } else if let std = rawHost as? StandardUITextViewMock {
                assert(std.text == "破坏计算机", "Expected '破坏计算机' in UITextView, got '\(std.text)'")
                assert(std.markedRange == nil)
                assert(std.selectedRange == NSRange(location: 5, length: 0))
            }
        }

        // 20b. Bilibili exact repro #2 + Simulator `TIKeyboardOutput` streaming test:
        //      Spaceless Wubi streaming ("破坏计算机系统罪警告...") must commit strictly left-to-right BEFORE
        //      the active inline preedit tail in BOTH `YYTextView` AND `StandardUITextView` under `TIKeyboardOutput`!
        var wb86StreamPrefs = bridge.prefs
        wb86StreamPrefs.spaceless = true
        bridge.applyAndSavePreferences(wb86StreamPrefs, persist: false)

        for rawHost in [YYTextViewDocumentMock() as any IOSMarkedTextDocumentSink, StandardUITextViewMock()] {
            bridge.clearComposition()
            bridge.setContext("")
            let proxy = TIKeyboardOutputProxyMock(host: rawHost)
            var streamSession = IOSInlineMarkedTextSession()
            let streamCodes = "dbfdytbhsmtxxylhdaqtf" + "txxylhdaqtf"
            for ch in streamCodes.unicodeScalars {
                let st = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 9000)
                streamSession.applyEngineState(
                    commitText: st.commitText,
                    inlinePreedit: st.activeInlinePreedit,
                    options: onOpts,
                    in: proxy
                )
                proxy.drainTurn(session: &streamSession)
            }
            assert(!streamSession.markedCode.isEmpty, "Expected active inline tail in spaceless stream")
            let currentText = (rawHost as? YYTextViewDocumentMock)?.text ?? (rawHost as! StandardUITextViewMock).text
            assert(currentText.hasPrefix("破坏"), "Committed prefix must stay at start, got '\(currentText)'")
            assert(currentText.hasSuffix(streamSession.markedCode), "Active marked tail must stay at end, got '\(currentText)'")
            // Ensure NO raw ASCII codes leaked between committed Chinese words!
            let prefixBeforeMarked = String(currentText.dropLast(streamSession.markedCode.count))
            assert(!prefixBeforeMarked.contains(where: { $0.isASCII && $0.isLetter }), "Leaked raw code in committed prefix: '\(currentText)'")

            let finishStream = bridge.handleKey(keyCode: 0x31, scalar: 32, hasCmdOrCtrl: false, timestampMs: 9100)
            streamSession.applyEngineState(
                commitText: finishStream.commitText,
                inlinePreedit: finishStream.activeInlinePreedit,
                options: onOpts,
                in: proxy
            )
            proxy.drainTurn(session: &streamSession)
            let finalText = (rawHost as? YYTextViewDocumentMock)?.text ?? (rawHost as! StandardUITextViewMock).text
            let finalMarked = (rawHost as? YYTextViewDocumentMock)?.markedRange ?? (rawHost as! StandardUITextViewMock).markedRange
            assert(finalMarked == nil && streamSession.markedCode.isEmpty)
            assert(!finalText.contains(where: { $0.isASCII && $0.isLetter }) && finalText.hasPrefix("破坏计算机"), "Got '\(finalText)'")
        }

        // 20c. Mid-text insertion ("前缀|后缀"), 5th-code push + CJK/EN auto-spacing, AND same-turn
        //      `commitPendingFirst` + `insertPassthrough("，")` (e.g. T9 sidebar punctuation while composing)!
        wb86StreamPrefs.spaceless = false
        bridge.applyAndSavePreferences(wb86StreamPrefs, persist: false)
        bridge.clearComposition()

        for rawHost in [YYTextViewDocumentMock(initialText: "hello后缀", cursor: 5) as any IOSMarkedTextDocumentSink,
                        StandardUITextViewMock(initialText: "hello后缀", cursor: 5)] {
            bridge.clearComposition()
            let proxy = TIKeyboardOutputProxyMock(host: rawHost)
            var midSession = IOSInlineMarkedTextSession()
            midSession.reconcileExternalContext(hostBefore: proxy.documentContextBeforeInput)
            // Type 5 codes `lgdba` ("国" pushed on 5th key 'a' -> "工") after "hello" before "后缀"!
            for ch in "lgdba".unicodeScalars {
                let st = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 9200)
                midSession.applyEngineState(commitText: st.commitText, inlinePreedit: st.activeInlinePreedit, options: onOpts, in: proxy)
                proxy.drainTurn(session: &midSession)
            }
            let midText = (rawHost as? YYTextViewDocumentMock)?.text ?? (rawHost as! StandardUITextViewMock).text
            assert(midText == "hello 国a后缀", "Expected 5th-code push with auto-space in middle of text, got '\(midText)'")
            // Same-turn `commitPendingFirst("工")` + `insertPassthrough("，")` WITHOUT intermediate runloop tick!
            midSession.commitPendingFirst(bridge.state.candidates.first!.text, options: onOpts, in: proxy)
            midSession.insertPassthrough("，", in: proxy)
            bridge.clearComposition()
            proxy.drainTurn(session: &midSession)

            // Simulate intermediate stale XPC `textDidChange` where host still reports `"hello"` before catching up!
            midSession.reconcileExternalContext(hostBefore: "hello")
            assert(midSession.autoSpaceTracker.effectiveBefore(hostBefore: "hello", hostBeforeIsAuthoritative: false) == "hello 国工，")
            let afterPunct = (rawHost as? YYTextViewDocumentMock)?.text ?? (rawHost as! StandardUITextViewMock).text
            let afterMarked = (rawHost as? YYTextViewDocumentMock)?.markedRange ?? (rawHost as! StandardUITextViewMock).markedRange
            let afterSel = (rawHost as? YYTextViewDocumentMock)?.selectedRange ?? (rawHost as! StandardUITextViewMock).selectedRange
            assert(afterPunct == "hello 国工，后缀" && afterMarked == nil, "Got '\(afterPunct)'")
            assert(afterSel == NSRange(location: 9, length: 0))
        }

        // Single-letter preedit Backspace must NEVER commit raw ASCII in either StandardUITextView or YYTextView!
        for rawHost in [YYTextViewDocumentMock() as any IOSMarkedTextDocumentSink, StandardUITextViewMock()] {
            let proxy = TIKeyboardOutputProxyMock(host: rawHost)
            var bsSession = IOSInlineMarkedTextSession()
            let st1 = bridge.handleKey(keyCode: 0, scalar: UnicodeScalar("d").value, hasCmdOrCtrl: false, timestampMs: 9300)
            bsSession.applyEngineState(commitText: st1.commitText, inlinePreedit: st1.activeInlinePreedit, options: onOpts, in: proxy)
            proxy.drainTurn(session: &bsSession)
            let stBs = bridge.handleKey(keyCode: 0x33, scalar: 0, hasCmdOrCtrl: false, timestampMs: 9350)
            bsSession.applyEngineState(commitText: stBs.commitText, inlinePreedit: stBs.activeInlinePreedit, options: onOpts, in: proxy)
            proxy.drainTurn(session: &bsSession)
            let txt = (rawHost as? YYTextViewDocumentMock)?.text ?? (rawHost as! StandardUITextViewMock).text
            let mr = (rawHost as? YYTextViewDocumentMock)?.markedRange ?? (rawHost as! StandardUITextViewMock).markedRange
            assert(txt.isEmpty && mr == nil, "Expected empty text after backspace, got '\(txt)'")
        }

        // 21. Focus-out and refocus while preedit is uncommitted (`recordFocusLoss` + `sweepOrphanedMarkedTextIfNeeded`)
        //     across BOTH `YYTextView` and `StandardUITextView` under `TIKeyboardOutput` coalescing!
        // 21a. Dismiss & refocus same field: orphaned `markedText` ("khk") is wiped cleanly, `markedRange == nil`,
        //      and follow-up Space / typing works with zero residual raw code or reverse-order corruption.
        for rawHost in [YYTextViewDocumentMock() as any IOSMarkedTextDocumentSink, StandardUITextViewMock()] {
            IOSInlineMarkedTextSession.resetSharedOrphanedState()
            bridge.clearComposition()
            let proxy = TIKeyboardOutputProxyMock(host: rawHost)
            var focusSession = IOSInlineMarkedTextSession()
            for ch in "trwukhk".unicodeScalars {
                let st = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 9400)
                focusSession.applyEngineState(commitText: st.commitText, inlinePreedit: st.activeInlinePreedit, options: onOpts, in: proxy)
                proxy.drainTurn(session: &focusSession)
            }
            let preBlurText = (rawHost as? YYTextViewDocumentMock)?.text ?? (rawHost as! StandardUITextViewMock).text
            let preBlurMarked = (rawHost as? YYTextViewDocumentMock)?.markedRange ?? (rawHost as! StandardUITextViewMock).markedRange
            assert(preBlurText == "我们khk" && preBlurMarked == NSRange(location: 2, length: 3))

            // Simulate keyboard disappearing (`KeyboardViewController` #1 deallocated -> fresh `IOSInlineMarkedTextSession` #2 on reopen!)
            focusSession.recordKeyboardDisappear()
            bridge.abandonComposition(timestampMs: 9450)
            if let std = rawHost as? StandardUITextViewMock {
                std.simulateReactivatedEmptyMarkedCaretJump = true
            }
            var reopenedSession = IOSInlineMarkedTextSession()
            assert(reopenedSession.needsAppearanceSweep && bridge.state.preedit.isEmpty)

            // Refocus same field on new VC instance: sweep clears orphaned "khk" AND restores TextKit 2 caret to {2, 0}!
            let outcome = reopenedSession.sweepOrphanedMarkedText(in: proxy)
            assert(outcome.needsAsyncFlush && outcome.expectedBeforeTail == "我们")
            proxy.drainTurn(session: &reopenedSession)
            reopenedSession.restoreCaretAfterOrphanedSweepIfNeeded(expectedBeforeTail: outcome.expectedBeforeTail, in: proxy)

            let afterRefocusText = (rawHost as? YYTextViewDocumentMock)?.text ?? (rawHost as! StandardUITextViewMock).text
            let afterRefocusMarked = (rawHost as? YYTextViewDocumentMock)?.markedRange ?? (rawHost as! StandardUITextViewMock).markedRange
            let afterRefocusSel = (rawHost as? YYTextViewDocumentMock)?.selectedRange ?? (rawHost as! StandardUITextViewMock).selectedRange
            assert(afterRefocusText == "我们" && afterRefocusMarked == nil && afterRefocusSel == NSRange(location: 2, length: 0),
                   "Expected orphaned markedText cleared with cursor at {2,0}, got '\(afterRefocusText)' sel=\(afterRefocusSel) marked=\(String(describing: afterRefocusMarked))")

            // Follow-up Space + new Wubi word `trwu` ("我们") must insert cleanly in forward order!
            reopenedSession.insertPassthrough(" ", in: proxy)
            proxy.drainTurn(session: &reopenedSession)
            for ch in "trwu".unicodeScalars {
                let st = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 9500)
                reopenedSession.applyEngineState(commitText: st.commitText, inlinePreedit: st.activeInlinePreedit, options: onOpts, in: proxy)
                proxy.drainTurn(session: &reopenedSession)
            }
            let afterFollowUp = (rawHost as? YYTextViewDocumentMock)?.text ?? (rawHost as! StandardUITextViewMock).text
            let afterFollowMarked = (rawHost as? YYTextViewDocumentMock)?.markedRange ?? (rawHost as! StandardUITextViewMock).markedRange
            assert(afterFollowUp == "我们 我们" && afterFollowMarked == nil, "Got '\(afterFollowUp)'")
        }

        // 21b. Multi-field switch (`Field 1` with uncommitted "khk" -> `Field 2` with normal text ->
        //      `Field 3` with active non-empty selection -> back to `Field 1`), plus same-turn immediate typing!
        for isYY in [true, false] {
            IOSInlineMarkedTextSession.resetSharedOrphanedState()
            bridge.clearComposition()
            let field1: any IOSMarkedTextDocumentSink = isYY ? YYTextViewDocumentMock(initialText: "你好") : StandardUITextViewMock(initialText: "你好")
            let field2: any IOSMarkedTextDocumentSink = isYY ? YYTextViewDocumentMock(initialText: "第二框") : StandardUITextViewMock(initialText: "第二框")
            let field3YY = YYTextViewDocumentMock(initialText: "选中文字")
            field3YY.selectedRange = NSRange(location: 0, length: 2)
            let field3Std = StandardUITextViewMock(initialText: "选中文字")
            field3Std.selectedRange = NSRange(location: 0, length: 2)
            let field3: any IOSMarkedTextDocumentSink = isYY ? field3YY : field3Std

            let proxy1 = TIKeyboardOutputProxyMock(host: field1)
            let proxy2 = TIKeyboardOutputProxyMock(host: field2)
            let proxy3 = TIKeyboardOutputProxyMock(host: field3)
            var session = IOSInlineMarkedTextSession()
            session.reconcileExternalContext(hostBefore: proxy1.documentContextBeforeInput)

            for ch in "khk".unicodeScalars {
                let st = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 9600)
                session.applyEngineState(commitText: st.commitText, inlinePreedit: st.activeInlinePreedit, options: onOpts, in: proxy1)
                proxy1.drainTurn(session: &session)
            }
            // Switch Field 1 -> Field 2 (`Field 2` gets normal non-nil `documentID`, so `Field 2` is NOT swept!)
            assert(session.recordFocusLoss() == true)
            bridge.abandonComposition(timestampMs: 9620)
            for ch in "trwu".unicodeScalars {
                let st = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 9630)
                session.applyEngineState(commitText: st.commitText, inlinePreedit: st.activeInlinePreedit, options: onOpts, in: proxy2)
                proxy2.drainTurn(session: &session)
            }
            let f2Text = (field2 as? YYTextViewDocumentMock)?.text ?? (field2 as! StandardUITextViewMock).text
            assert(f2Text == "第二框我们", "Typing in second field must stay clean while preserving orphaned sweep flag, got '\(f2Text)'")
            assert(session.hasOrphanedMarkedInSession, "Orphaned sweep flag must survive typing in Field 2 until Field 1 is refocused")

            // If sweep ever runs on Field 3 where "选中" is selected (`selectedText == "选中"`), selection MUST NOT be deleted!
            var selGuardSession = IOSInlineMarkedTextSession()
            _ = selGuardSession.applyEngineState(commitText: "", inlinePreedit: "khk", options: onOpts, in: TIKeyboardOutputProxyMock(host: StandardUITextViewMock()))
            _ = selGuardSession.recordFocusLoss()
            _ = selGuardSession.sweepOrphanedMarkedText(in: proxy3)
            proxy3.drainTurn(session: &selGuardSession)
            let f3Text = (field3 as? YYTextViewDocumentMock)?.text ?? (field3 as! StandardUITextViewMock).text
            let f3Sel = (field3 as? YYTextViewDocumentMock)?.selectedRange ?? (field3 as! StandardUITextViewMock).selectedRange
            assert(f3Text == "选中文字" && f3Sel == NSRange(location: 0, length: 2), "Active text selection must never be erased by sweep")

            // Re-arm orphaned state from Field 1 ("你好" + "khk") and return to Field 1 + immediately type 't' in SAME turn!
            var rearm = IOSInlineMarkedTextSession()
            rearm.reconcileExternalContext(hostBefore: "你好")
            _ = rearm.applyEngineState(commitText: "", inlinePreedit: "khk", options: onOpts, in: TIKeyboardOutputProxyMock(host: StandardUITextViewMock(initialText: "你好")))
            _ = rearm.recordFocusLoss()

            _ = session.sweepOrphanedMarkedText(in: proxy1)
            let stT = bridge.handleKey(keyCode: 0, scalar: UnicodeScalar("t").value, hasCmdOrCtrl: false, timestampMs: 9650)
            session.applyEngineState(commitText: stT.commitText, inlinePreedit: stT.activeInlinePreedit, options: onOpts, in: proxy1)
            proxy1.drainTurn(session: &session)

            let f1MidText = (field1 as? YYTextViewDocumentMock)?.text ?? (field1 as! StandardUITextViewMock).text
            let f1MidMarked = (field1 as? YYTextViewDocumentMock)?.markedRange ?? (field1 as! StandardUITextViewMock).markedRange
            assert(f1MidText == "你好t" && f1MidMarked == NSRange(location: 2, length: 1), "Same-turn refocus + key 't' must clear 'khk' and keep 't' marked, got '\(f1MidText)' marked=\(String(describing: f1MidMarked))")

            for ch in "rwu".unicodeScalars {
                let st = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 9680)
                session.applyEngineState(commitText: st.commitText, inlinePreedit: st.activeInlinePreedit, options: onOpts, in: proxy1)
                proxy1.drainTurn(session: &session)
            }
            let f1FinalText = (field1 as? YYTextViewDocumentMock)?.text ?? (field1 as! StandardUITextViewMock).text
            let f1FinalMarked = (field1 as? YYTextViewDocumentMock)?.markedRange ?? (field1 as! StandardUITextViewMock).markedRange
            assert(f1FinalText == "你好我们" && f1FinalMarked == nil, "Got '\(f1FinalText)'")
        }

        // Whole-keyboard long-press trackpad with delayed alternates promotion
        assert(IOSKeyboardTrackpadHold.supportsTrackpadHold(for: .space, wholeKeyboardEnabled: false))
        assert(!IOSKeyboardTrackpadHold.supportsTrackpadHold(for: .letter(UInt8(ascii: "a")), wholeKeyboardEnabled: false))
        assert(IOSKeyboardTrackpadHold.supportsTrackpadHold(for: .letter(UInt8(ascii: "a")), wholeKeyboardEnabled: true))
        assert(IOSKeyboardTrackpadHold.supportsTrackpadHold(for: .symbol("，"), wholeKeyboardEnabled: true))
        assert(!IOSKeyboardTrackpadHold.supportsTrackpadHold(for: .backspace, wholeKeyboardEnabled: true), "Backspace keeps repeat delete")
        assert(!IOSKeyboardTrackpadHold.supportsTrackpadHold(for: .globe, wholeKeyboardEnabled: true), "Globe keeps system switcher")
        assert(!IOSKeyboardTrackpadHold.supportsTrackpadHold(for: .planeSwitch(.numbers, label: "123"), wholeKeyboardEnabled: true), "Plane switch keeps hold-and-slide")
        assert(IOSKeyboardTrackpadHold.trackpadHoldDelay(wholeKeyboardEnabled: true) < IOSKeyboardTrackpadHold.alternatesDelay(wholeKeyboardEnabled: true), "Trackpad activates before alternates popup")
        assert(IOSKeyboardTrackpadHold.alternatesDelay(wholeKeyboardEnabled: false) == IOSKeyboardTrackpadHold.defaultAlternatesDelay)
        let holdStart = CGPoint(x: 200, y: 180)
        assert(!IOSKeyboardTrackpadHold.shouldCancelAlternatesOnMove(from: holdStart, to: CGPoint(x: 208, y: 184)), "Small jitter keeps alternates timer armed")
        assert(IOSKeyboardTrackpadHold.shouldCancelAlternatesOnMove(from: holdStart, to: CGPoint(x: 215, y: 180)), "Cursor drag cancels pending alternates popup")
        assert(!IOSKeyboardTrackpadHold.shouldCancelAlternatesInTrackpad(from: holdStart, to: CGPoint(x: 208, y: 184), dx: 0, dy: 0), "Stationary trackpad touch keeps alternates timer armed")
        assert(IOSKeyboardTrackpadHold.shouldCancelAlternatesInTrackpad(from: holdStart, to: CGPoint(x: 210, y: 180), dx: 1, dy: 0), "Single horizontal cursor step (9.5pt < 12pt) must cancel pending alternates popup")
        assert(IOSKeyboardTrackpadHold.shouldCancelAlternatesInTrackpad(from: holdStart, to: CGPoint(x: 200, y: 185), dx: 0, dy: 1), "Vertical cursor step must cancel pending alternates popup")
        var holdCursorPrefs = JuxuPreferences()
        assert(!holdCursorPrefs.keyboardHoldCursor, "Whole-keyboard hold cursor is opt-in")
        holdCursorPrefs.keyboardHoldCursor = true
        assert(JuxuPreferences.fromTOML(holdCursorPrefs.toTOML()).keyboardHoldCursor)

        // Bottom-row comma key: tap comma, swipe up period, behind its own toggle
        let commaSize = CGSize(width: 440, height: 220)
        func commaKey(_ snap: IOSKeyboardLayoutEngine.LayoutSnapshot) -> IOSKeySpec? {
            snap.keys.first(where: { $0.id == "B_comma" })
        }
        for (ascii, swipeToggle, tapSymbol, swipeSymbol) in [
            (false, true, "，", "。"), (true, true, ",", "."),
            (false, false, "，", "。"), (true, false, ",", "."),
        ] {
            var engine = IOSKeyboardLayoutEngine()
            engine.spaceCommaKeyEnabled = true
            engine.asciiMode = ascii
            engine.swipeUpInputEnabled = swipeToggle
            let commaSnap = engine.computeLayout(in: commaSize)
            let key = commaKey(commaSnap)!
            assert(key.kind == .symbol(tapSymbol), "Tap must enter \(tapSymbol)")
            assert(key.swipeUpSymbol == swipeSymbol, "Swipe must enter \(swipeSymbol) with letter swipe toggle \(swipeToggle)")
            assert(IOSKeyboardLayoutEngine.hitTest(point: CGPoint(x: key.frame.midX, y: key.frame.midY), in: commaSnap)?.id == "B_comma")
            let letterHints = commaSnap.keys.contains { key in
                if case .letter = key.kind { return key.swipeUpSymbol != nil }
                return false
            }
            assert(letterHints == swipeToggle, "Letter hints must still follow their own toggle")
        }
        var commaOn = IOSKeyboardLayoutEngine()
        commaOn.spaceCommaKeyEnabled = true
        let onSnap = commaOn.computeLayout(in: commaSize)
        let onPlane = onSnap.keys.first(where: { $0.id == "B_plane" })!
        let onComma = commaKey(onSnap)!
        let onSpace = onSnap.keys.first(where: { $0.kind == .space })!
        let onMode = onSnap.keys.first(where: { $0.id == "B_mode" })!
        let onReturn = onSnap.keys.first(where: { $0.kind == .returnKey })!
        let onLetterQ = onSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "q")) })!
        let onLetterP = onSnap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "p")) })!
        let onPitch = CGFloat(onSnap.pitchX)
        let onGap = onPitch - onLetterQ.frame.width
        let onRow = [onPlane, onComma, onSpace, onMode, onReturn]
        assert(onSnap.keys.count == 33 && !onSnap.keys.contains(where: { $0.id == "B_period" }))
        assert(abs(onPlane.frame.minX - onLetterQ.frame.minX) < 0.01 && abs(onReturn.frame.maxX - onLetterP.frame.maxX) < 0.01)
        for (left, right) in zip(onRow, onRow.dropFirst()) {
            assert(abs(right.frame.minX - left.frame.maxX - onGap) < 0.01, "\(left.id) and \(right.id) must be one key gap apart")
        }
        assert(abs(onComma.frame.width - onLetterQ.frame.width) < 0.01, "Comma must be one letter key wide")
        for (key, pitches) in [(onPlane, 1.85), (onSpace, 3.6), (onMode, 1.15), (onReturn, 1.85)] as [(IOSKeySpec, CGFloat)] {
            assert(abs(key.frame.width / onPitch - pitches) < 0.06, "\(key.id) is \(key.frame.width / onPitch) pitches")
        }
        assert(onPlane.usesFunctionFill && onReturn.usesFunctionFill)
        assert(!onComma.usesFunctionFill && !onSpace.usesFunctionFill && !onMode.usesFunctionFill)
        assert(onMode.primaryLabel == "中" && onMode.cornerSubscript == "英")
        var commaEnglish = IOSKeyboardLayoutEngine()
        commaEnglish.spaceCommaKeyEnabled = true
        commaEnglish.asciiMode = true
        let englishMode = commaEnglish.computeLayout(in: commaSize).keys.first(where: { $0.id == "B_mode" })!
        assert(englishMode.primaryLabel == "英" && englishMode.cornerSubscript == "中")
        var commaGlobe = IOSKeyboardLayoutEngine()
        commaGlobe.spaceCommaKeyEnabled = true
        commaGlobe.showInlineGlobeKey = true
        let globeSnap = commaGlobe.computeLayout(in: commaSize)
        assert(globeSnap.keys.first(where: { $0.id == "B_globe" })?.frame == onMode.frame, "Globe must keep working in the 中/英 slot")
        assert(!globeSnap.keys.contains(where: { $0.id == "B_mode" }))
        for key in onRow {
            let center = CGPoint(x: key.frame.midX, y: key.frame.midY)
            assert(IOSKeyboardLayoutEngine.hitTest(point: center, in: onSnap)?.id == key.id, "Center tap must hit \(key.id)")
            let topEdge = CGPoint(x: key.frame.midX, y: key.frame.minY - 2.0)
            assert(IOSKeyboardLayoutEngine.hitTest(point: topEdge, in: onSnap, validNextMask: 1)?.id == key.id, "Top-edge tap must stay on \(key.id)")
        }

        // Toggle off: exactly the original 32-key layout
        var commaOff = IOSKeyboardLayoutEngine()
        commaOff.spaceCommaKeyEnabled = false
        let plainSnap = commaOff.computeLayout(in: commaSize)
        func letter(_ c: Character) -> IOSKeySpec { plainSnap.keys.first(where: { $0.kind == .letter(c.asciiValue!) })! }
        let plainPlane = plainSnap.keys.first(where: { $0.id == "B_plane" })!
        let plainMode = plainSnap.keys.first(where: { $0.id == "B_mode" })!
        let plainSpace = plainSnap.keys.first(where: { $0.kind == .space })!
        let plainReturn = plainSnap.keys.first(where: { $0.kind == .returnKey })!
        let plainLeftW = (letter("s").frame.maxX - letter("q").frame.minX - onGap) * 0.5
        assert(plainSnap.keys.count == 32 && commaKey(plainSnap) == nil)
        assert(plainSnap.keys.allSatisfy { !$0.usesFunctionFill })
        assert(plainMode.primaryLabel == "中" && plainMode.secondaryInlineLabel == "/英" && plainMode.cornerSubscript == nil)
        assert(abs(plainPlane.frame.minX - letter("q").frame.minX) < 0.01 && abs(plainPlane.frame.width - plainLeftW) < 0.01)
        assert(abs(plainMode.frame.maxX - letter("s").frame.maxX) < 0.01 && abs(plainMode.frame.width - plainLeftW) < 0.01)
        assert(abs(plainSpace.frame.minX - letter("d").frame.minX) < 0.01 && abs(plainSpace.frame.maxX - letter("j").frame.maxX) < 0.01)
        assert(abs(plainReturn.frame.minX - letter("k").frame.minX) < 0.01 && abs(plainReturn.frame.maxX - letter("p").frame.maxX) < 0.01)
        for key in plainSnap.keys where !key.id.hasPrefix("B_") {
            assert(onSnap.keys.first(where: { $0.id == key.id })?.frame == key.frame, "\(key.id) must not move")
        }
        let dColumn = CGPoint(x: letter("d").frame.midX, y: plainSpace.frame.midY)
        let kColumn = CGPoint(x: letter("k").frame.midX, y: plainSpace.frame.midY)
        assert(IOSKeyboardLayoutEngine.hitTest(point: dColumn, in: plainSnap)?.kind == .space)
        assert(IOSKeyboardLayoutEngine.hitTest(point: kColumn, in: plainSnap)?.kind == .returnKey)

        // The key stays off other planes and the 9-key layout
        commaOn.switchPlane(.numbers)
        assert(commaKey(commaOn.computeLayout(in: commaSize)) == nil)
        commaOn.switchPlane(.letters)
        commaOn.schemaId = 20
        commaOn.pinyinT9Layout = true
        let t9CommaSnap = commaOn.computeLayout(in: commaSize)
        assert(t9CommaSnap.isT9Layout && commaKey(t9CommaSnap) == nil)

        // 中/英 on the 123 and symbol pages: content switches in place and the mode carries to letters
        func labels(_ snap: IOSKeyboardLayoutEngine.LayoutSnapshot) -> Set<String> {
            Set(snap.keys.compactMap { key in
                if case .symbol(let symbol) = key.kind { return symbol }
                return nil
            })
        }
        for (page, chinese, english, shared) in [
            (IOSKeyPlane.numbers, ["，", "。", "？", "！", "：", "；", "（", "）", "“", "”", "￥"], [",", "?", "!", ":", ";", "(", ")", "\"", "$"], ["1", "0", "-", "/", "@", "."]),
            (IOSKeyPlane.symbols, ["——", "～", "《", "》", "【", "】", "·"], ["_", "~", "<", ">", "€", "£", "•"], ["[", "]", "#", "\\", "|", "…", "‘", "’", "〈", "〉"]),
        ] {
            for commaEnabled in [true, false] {
                var pageEngine = IOSKeyboardLayoutEngine()
                pageEngine.spaceCommaKeyEnabled = commaEnabled
                pageEngine.switchPlane(page)
                let zhLabels = labels(pageEngine.computeLayout(in: commaSize))
                assert(Set(chinese).isSubset(of: zhLabels) && zhLabels.isDisjoint(with: english))
                pageEngine.asciiMode = true
                assert(pageEngine.plane == page, "Mode change must stay on the page")
                let enSnap = pageEngine.computeLayout(in: commaSize)
                let enLabels = labels(enSnap)
                assert(Set(english).isSubset(of: enLabels) && enLabels.isDisjoint(with: chinese))
                assert(Set(shared).isSubset(of: zhLabels) && Set(shared).isSubset(of: enLabels), "Keys without a variant stay")
                let enModeKey = enSnap.keys.first(where: { $0.kind == .modeToggle })!
                assert(enModeKey.primaryLabel == (commaEnabled ? "英" : "EN"))
                pageEngine.asciiMode = false
                assert(pageEngine.plane == page && labels(pageEngine.computeLayout(in: commaSize)) == zhLabels, "Switching back restores the Chinese content")
                pageEngine.asciiMode = true
                pageEngine.switchPlane(.letters)
                let backSnap = pageEngine.computeLayout(in: commaSize)
                assert(backSnap.keys.first(where: { $0.kind == .modeToggle })?.primaryLabel == (commaEnabled ? "英" : "EN"))
                if commaEnabled {
                    assert(commaKey(backSnap)?.kind == .symbol(","), "Letters must use the mode chosen on the page")
                }
            }

            // Toggle on: 123/ABC, 中/英 and Return keep the letter page's frames, with no comma key added
            var onPage = IOSKeyboardLayoutEngine()
            onPage.spaceCommaKeyEnabled = true
            onPage.switchPlane(page)
            let onPageSnap = onPage.computeLayout(in: commaSize)
            func onKey(_ id: String) -> IOSKeySpec { onPageSnap.keys.first(where: { $0.id == id })! }
            assert(commaKey(onPageSnap) == nil)
            assert(onKey("B_mode").frame == onMode.frame && onKey("B_return").frame == onReturn.frame && onKey("B_plane").frame == onPlane.frame)
            assert(onKey("B_plane").primaryLabel == "ABC" && onKey("B_plane").kind == .planeSwitch(.letters, label: "ABC"))
            assert(abs(onKey("B_space").frame.minX - onPlane.frame.maxX - onGap) < 0.01)
            assert(abs(onMode.frame.minX - onKey("B_space").frame.maxX - onGap) < 0.01)
            for id in ["B_plane", "B_space", "B_mode", "B_return"] {
                let key = onKey(id)
                assert(IOSKeyboardLayoutEngine.hitTest(point: CGPoint(x: key.frame.midX, y: key.frame.midY), in: onPageSnap)?.id == id)
            }
            var globePage = IOSKeyboardLayoutEngine()
            globePage.spaceCommaKeyEnabled = true
            globePage.showInlineGlobeKey = true
            globePage.switchPlane(page)
            let globePageSnap = globePage.computeLayout(in: commaSize)
            assert(globePageSnap.keys.first(where: { $0.kind == .globe })?.frame == onMode.frame)
            assert(!globePageSnap.keys.contains(where: { $0.kind == .modeToggle }))

            // Toggle off: the original positions on this page
            var offPage = IOSKeyboardLayoutEngine()
            offPage.spaceCommaKeyEnabled = false
            offPage.switchPlane(page)
            let offPageSnap = offPage.computeLayout(in: commaSize)
            func offKey(_ id: String) -> IOSKeySpec { offPageSnap.keys.first(where: { $0.id == id })! }
            assert(offKey("B_plane").frame == plainPlane.frame && offKey("B_mode").frame == plainMode.frame)
            assert(offKey("B_space").frame == plainSpace.frame && offKey("B_return").frame == plainReturn.frame)
            assert(offPageSnap.keys.allSatisfy { !$0.usesFunctionFill })
        }

        // The setting is opt-in and persists; settings saved before it existed decode to off
        var commaPrefs = JuxuPreferences()
        assert(!commaPrefs.spaceCommaKey)
        commaPrefs.spaceCommaKey = true
        assert(JuxuPreferences.fromTOML(commaPrefs.toTOML()).spaceCommaKey)
        let oldTOML = commaPrefs.toTOML().split(separator: "\n", omittingEmptySubsequences: false)
            .filter { !$0.contains("space_comma_key") }.joined(separator: "\n")
        assert(oldTOML != commaPrefs.toTOML() && !JuxuPreferences.fromTOML(oldTOML).spaceCommaKey)

        print("✓ All iOS 26 native grid, lowercase/shift, trackpad, ergonomic Y-bias, dynamic neural next-key, personal touchOffsets, clipboard, Cangjie, Wubi, T9 Pinyin, YYTextView inline commit & focus-loss recovery tests passed!")
    }
}
