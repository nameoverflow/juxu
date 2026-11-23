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
        assert(!IOSSwipeUp.isActive(from: o, to: CGPoint(x: 101, y: 90), keyHeight: 45, wasActive: false), "Thumb roll must stay a tap")
        assert(IOSSwipeUp.isActive(from: o, to: CGPoint(x: 103, y: 80), keyHeight: 45, wasActive: false))
        assert(!IOSSwipeUp.isActive(from: o, to: CGPoint(x: 140, y: 75), keyHeight: 45, wasActive: false), "Diagonal slide is not a swipe-up")
        assert(!IOSSwipeUp.isActive(from: o, to: CGPoint(x: 100, y: 120), keyHeight: 45, wasActive: false))
        assert(IOSSwipeUp.isActive(from: o, to: CGPoint(x: 100, y: 88), keyHeight: 45, wasActive: true), "Hysteresis keeps an armed swipe")
        assert(!IOSSwipeUp.isActive(from: o, to: CGPoint(x: 100, y: 92), keyHeight: 45, wasActive: true), "Sliding back cancels")

        var offLayout = IOSKeyboardLayoutEngine()
        offLayout.swipeUpInputEnabled = false
        let offSnap = offLayout.computeLayout(in: CGSize(width: 440, height: 220))
        assert(offSnap.keys.allSatisfy { $0.swipeUpSymbol == nil }, "Switching swipe-up off must remove hints and gesture")
        var featurePrefs = JuxuPreferences()
        assert(!featurePrefs.keySwipeUpInput, "Swipe-up input is opt-in")
        featurePrefs.keySwipeUpInput = true
        let featureRound = JuxuPreferences.fromTOML(featurePrefs.toTOML())
        assert(featureRound.keySwipeUpInput)

        print("✓ All iOS 26 native grid, lowercase/shift, trackpad, ergonomic Y-bias, dynamic neural next-key, personal touchOffsets, clipboard, Cangjie, Wubi & T9 Pinyin tests passed!")
    }
}
