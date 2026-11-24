import UIKit
import XCTest

private final class SyntheticFingerTouch: UITouch {
    var mockPoint: CGPoint = .zero
    var mockTimestamp: TimeInterval = 0
    override func location(in view: UIView?) -> CGPoint { mockPoint }
    override var timestamp: TimeInterval { mockTimestamp }
}

final class JuxuSimulatorUITests: XCTestCase {
    @MainActor private static var midPressShot: XCUIScreenshot?

    @MainActor
    func testMultiTouchRolloverAndLiftFlickHysteresis() {
        let canvas = IOSKeyCanvasView()
        let snapshot = IOSKeyboardLayoutEngine().computeLayout(in: CGSize(width: 440, height: 219))
        canvas.updateSnapshot(snapshot)

        var committed = ""
        canvas.onKeyTouchUp = { spec, _ in
            committed += spec.primaryLabel
        }

        // 1. Overlapping two-thumb rollover (32 consecutive overlapping taps where finger N+1
        //    presses down BEFORE finger N lifts, and finger N drifts into the candidate ribbon on lift!)
        let sequence = String(repeating: "rtdgthtc", count: 4)
        var clock: TimeInterval = 100.0
        var prevTouch: SyntheticFingerTouch?

        for ch in sequence {
            let key = snapshot.keys.first { $0.primaryLabel == String(ch) }!
            let nextTouch = SyntheticFingerTouch()
            nextTouch.mockPoint = CGPoint(x: key.frame.midX, y: key.frame.midY)
            nextTouch.mockTimestamp = clock
            canvas.touchesBegan([nextTouch], with: nil)

            if let prev = prevTouch {
                // Previous thumb flicks 18pt upward into candidate bar while lifting 22ms later
                clock += 0.022
                prev.mockPoint = CGPoint(x: prev.mockPoint.x + 8.0, y: -18.0)
                prev.mockTimestamp = clock
                canvas.touchesMoved([prev], with: nil)
                canvas.touchesEnded([prev], with: nil)
            }
            prevTouch = nextTouch
            clock += 0.035
        }
        if let last = prevTouch {
            clock += 0.030
            last.mockPoint = CGPoint(x: last.mockPoint.x - 7.0, y: -16.0)
            last.mockTimestamp = clock
            canvas.touchesMoved([last], with: nil)
            canvas.touchesEnded([last], with: nil)
        }
        XCTAssertEqual(committed, sequence, "Zero dropped or reordered keys under overlapping rollover + ribbon lift")

        // 2. Rapid sub-90ms lift-roll across neighbor border (`t` -> rolled 8pt right onto `y` edge on release)
        committed = ""
        let tKey = snapshot.keys.first { $0.primaryLabel == "t" }!
        let yKey = snapshot.keys.first { $0.primaryLabel == "y" }!
        let flick = SyntheticFingerTouch()
        flick.mockPoint = CGPoint(x: tKey.frame.midX, y: tKey.frame.midY)
        flick.mockTimestamp = 200.0
        canvas.touchesBegan([flick], with: nil)

        // 45ms later, thumb rolls 8pt into neighbor `y` while lifting: sticky press-lock keeps `t`!
        flick.mockPoint = CGPoint(x: tKey.frame.maxX + 8.0, y: tKey.frame.midY + 4.0)
        flick.mockTimestamp = 200.045
        canvas.touchesMoved([flick], with: nil)
        canvas.touchesEnded([flick], with: nil)
        XCTAssertEqual(committed, "t", "Sub-90ms thumb lift-roll must stay locked on initial key 't'")

        // 3. Deliberate hold-and-slide (>= 90ms) onto neighbor `y` switches cleanly to `y`!
        committed = ""
        flick.mockPoint = CGPoint(x: tKey.frame.midX, y: tKey.frame.midY)
        flick.mockTimestamp = 205.0
        canvas.touchesBegan([flick], with: nil)
        flick.mockPoint = CGPoint(x: yKey.frame.midX, y: yKey.frame.midY)
        flick.mockTimestamp = 205.16
        canvas.touchesMoved([flick], with: nil)
        canvas.touchesEnded([flick], with: nil)
        XCTAssertEqual(committed, "y", "Deliberate hold-and-slide must switch to neighbor key 'y'")

        // 4. Stationary edge-gate cancellation (`drift <= 14pt`, `dt <= 0.22s`) rescues tap without dropping key,
        //    whereas scroll/dismiss swipe cancellation (`drift > 14pt`) emits zero stray letters!
        committed = ""
        flick.mockPoint = CGPoint(x: tKey.frame.midX, y: tKey.frame.midY)
        flick.mockTimestamp = 210.0
        canvas.touchesBegan([flick], with: nil)
        flick.mockTimestamp = 210.06
        canvas.touchesCancelled([flick], with: nil)
        XCTAssertEqual(committed, "t", "Brief stationary edge-gated cancellation must rescue tap")

        committed = ""
        flick.mockPoint = CGPoint(x: tKey.frame.midX, y: tKey.frame.midY)
        flick.mockTimestamp = 211.0
        canvas.touchesBegan([flick], with: nil)
        flick.mockPoint = CGPoint(x: tKey.frame.midX, y: tKey.frame.midY + 28.0)
        flick.mockTimestamp = 211.09
        canvas.touchesMoved([flick], with: nil)
        canvas.touchesCancelled([flick], with: nil)
        XCTAssertEqual(committed, "", "Interactive scroll/dismiss swipe cancellation must not emit stray letters")

        // 5. Overlapping Backspace rollover ends delete repeat without entering a stray letter
        var releasedKeys: [IOSKeyKind] = []
        canvas.onKeyTouchUp = { spec, _ in
            releasedKeys.append(spec.kind)
            if case .letter = spec.kind {
                committed += spec.primaryLabel
            }
        }
        let backspaceKey = snapshot.keys.first { $0.kind == .backspace }!
        committed = ""
        releasedKeys = []
        let deleteTouch = SyntheticFingerTouch()
        deleteTouch.mockPoint = CGPoint(x: backspaceKey.frame.midX, y: backspaceKey.frame.midY)
        deleteTouch.mockTimestamp = 212.0
        canvas.touchesBegan([deleteTouch], with: nil)
        deleteTouch.mockPoint = CGPoint(x: tKey.frame.midX, y: tKey.frame.midY)
        deleteTouch.mockTimestamp = 212.1
        canvas.touchesMoved([deleteTouch], with: nil)
        flick.mockPoint = CGPoint(x: tKey.frame.midX, y: tKey.frame.midY)
        flick.mockTimestamp = 212.2
        canvas.touchesBegan([flick], with: nil)
        flick.mockTimestamp = 212.25
        canvas.touchesEnded([deleteTouch, flick], with: nil)
        XCTAssertEqual(releasedKeys, [.backspace, .letter(UInt8(ascii: "t"))])
        XCTAssertEqual(committed, "t")

        // 6. Letter lift-roll onto Backspace (`m` -> `⌫` after 130ms) or Spacebar (`v` -> `Space` after 130ms)
        //    must NEVER hijack or silently drop the pressed letter!
        committed = ""
        releasedKeys = []
        let mKey = snapshot.keys.first { $0.primaryLabel == "m" }!
        let vKey = snapshot.keys.first { $0.primaryLabel == "v" }!
        let spaceKey = snapshot.keys.first { $0.kind == .space }!

        let mTouch = SyntheticFingerTouch()
        mTouch.mockPoint = CGPoint(x: mKey.frame.midX, y: mKey.frame.midY)
        mTouch.mockTimestamp = 220.0
        canvas.touchesBegan([mTouch], with: nil)
        mTouch.mockPoint = CGPoint(x: backspaceKey.frame.midX, y: backspaceKey.frame.midY)
        mTouch.mockTimestamp = 220.13
        canvas.touchesMoved([mTouch], with: nil)
        mTouch.mockTimestamp = 220.14
        canvas.touchesEnded([mTouch], with: nil)
        XCTAssertEqual(releasedKeys, [.letter(UInt8(ascii: "m"))], "Lift-roll from 'm' onto Backspace must commit 'm', never silently drop")

        releasedKeys = []
        let vTouch = SyntheticFingerTouch()
        vTouch.mockPoint = CGPoint(x: vKey.frame.midX, y: vKey.frame.midY)
        vTouch.mockTimestamp = 221.0
        canvas.touchesBegan([vTouch], with: nil)
        vTouch.mockPoint = CGPoint(x: spaceKey.frame.midX, y: spaceKey.frame.midY)
        vTouch.mockTimestamp = 221.13
        canvas.touchesMoved([vTouch], with: nil)
        vTouch.mockTimestamp = 221.14
        canvas.touchesEnded([vTouch], with: nil)
        XCTAssertEqual(releasedKeys, [.letter(UInt8(ascii: "v"))], "Lift-roll from 'v' onto Spacebar must commit 'v'")

        // 7. Fast horizontal sweep across Spacebar cancelled by bottom system gesture gate (`dx = 36pt, dy = 3pt`)
        //    rescues Space cleanly!
        releasedKeys = []
        let spTouch = SyntheticFingerTouch()
        spTouch.mockPoint = CGPoint(x: spaceKey.frame.midX - 20.0, y: spaceKey.frame.midY + 4.0)
        spTouch.mockTimestamp = 222.0
        canvas.touchesBegan([spTouch], with: nil)
        spTouch.mockPoint = CGPoint(x: spaceKey.frame.midX + 16.0, y: spaceKey.frame.midY + 7.0)
        spTouch.mockTimestamp = 222.09
        canvas.touchesMoved([spTouch], with: nil)
        spTouch.mockTimestamp = 222.10
        canvas.touchesCancelled([spTouch], with: nil)
        XCTAssertEqual(releasedKeys, [.space], "Horizontal Spacebar sweep cancelled by bottom gesture gate must rescue Space")
    }

    @MainActor
    private func openLiveKeyboard() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["--autofocus"]
        app.launch()
        if app.otherElements["JuxuKeyCanvas"].waitForExistence(timeout: 3) {
            return app
        }

        // 1. Add 句序 in iOS 26.5 Settings -> 通用 -> 键盘 -> 键盘 -> 添加新键盘
        let settings = XCUIApplication(bundleIdentifier: "com.apple.Preferences")
        settings.launch()

        let general = settings.cells.containing(.staticText, identifier: "通用").firstMatch
        if general.waitForExistence(timeout: 3) {
            general.tap()
        }
        let kb = settings.cells.containing(.staticText, identifier: "键盘").firstMatch
        if kb.waitForExistence(timeout: 3) {
            kb.tap()
        }
        // Inside General -> Keyboard, tap Keyboards list cell ("键盘")
        let kbsCell = settings.tables.cells.containing(.staticText, identifier: "键盘").firstMatch
        if kbsCell.waitForExistence(timeout: 3) {
            kbsCell.tap()
        }

        let alreadyAdded = settings.cells.containing(NSPredicate(format: "label CONTAINS '句序'")).firstMatch
        if !alreadyAdded.waitForExistence(timeout: 1.5) {
            let addBtn = settings.buttons["AddNewKeyboard"].firstMatch
            let addCell = settings.cells["AddNewKeyboard"].firstMatch
            if addBtn.waitForExistence(timeout: 3) {
                addBtn.tap()
            } else if addCell.exists {
                addCell.tap()
            } else {
                let fallback = settings.staticTexts.containing(NSPredicate(format: "label CONTAINS '添加新键盘'")).firstMatch
                if fallback.exists { fallback.tap() }
            }

            Thread.sleep(forTimeInterval: 0.8)
            attachHierarchy(of: settings, name: "settings-add-keyboard-tree")

            let prisRow = settings.staticTexts.containing(NSPredicate(format: "label CONTAINS '句序'")).firstMatch
            if prisRow.waitForExistence(timeout: 3) {
                prisRow.tap()
                Thread.sleep(forTimeInterval: 0.8)
            }
            let doneBtn = settings.navigationBars.buttons["完成"].firstMatch
            if doneBtn.exists {
                doneBtn.tap()
                Thread.sleep(forTimeInterval: 0.6)
            }
        }
        attachShot(name: "01-settings-keyboards-added")

        // 2. Launch JuxuApp and focus the sandbox text field
        app.launch()
        Thread.sleep(forTimeInterval: 1.2)

        let inputUI = XCUIApplication(bundleIdentifier: "com.apple.InputUI")
        let globeCoord = app.coordinate(withNormalizedOffset: CGVector(dx: 0.095, dy: 0.955))

        // Long-press Globe to open the Input Mode picker and select 句序 directly
        globeCoord.press(forDuration: 0.75)
        Thread.sleep(forTimeInterval: 0.6)
        attachHierarchy(of: inputUI, name: "inputui-globe-menu-tree")

        let pickerInInputUI = inputUI.staticTexts.containing(NSPredicate(format: "label CONTAINS '句序'")).firstMatch
        let pickerInApp = app.staticTexts.containing(NSPredicate(format: "label CONTAINS '句序'")).firstMatch
        if pickerInInputUI.waitForExistence(timeout: 1.5) {
            pickerInInputUI.tap()
        } else if pickerInApp.exists && pickerInApp.frame.minY > 500 {
            pickerInApp.tap()
        } else {
            // Dismiss menu if any and cycle Globe until 句序 is active
            for _ in 0..<6 {
                globeCoord.tap()
                Thread.sleep(forTimeInterval: 0.7)
                if inputUI.otherElements["JuxuRibbon"].exists || app.otherElements["JuxuRibbon"].exists {
                    break
                }
            }
        }

        Thread.sleep(forTimeInterval: 1.0)
        attachHierarchy(of: inputUI, name: "inputui-active-keyboard-tree")
        return app
    }

    @MainActor
    func testSpacelessTouchCalibrationAppearsInHost() throws {
        let app = XCUIApplication()
        app.launch()
        app.tabBars.buttons["输入设置"].tap()
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "86 版")).firstMatch.tap()
        let streaming = app.switches["流式无空格连打（实验）"]
        if streaming.value as? String == "0" { streaming.tap() }
        XCTAssertEqual(streaming.value as? String, "1")

        app.tabBars.buttons["误触与热区"].tap()
        for title in ["统计 + 触控高斯误触纠正", "个人拇指热区 EMA 自适应"] {
            let toggle = app.switches[title]
            if toggle.value as? String == "0" { toggle.tap() }
            XCTAssertEqual(toggle.value as? String, "1")
        }
        app.buttons["重置"].tap()

        let typingApp = openLiveKeyboard()
        let canvas = typingApp.otherElements["JuxuKeyCanvas"]
        XCTAssertTrue(canvas.waitForExistence(timeout: 5))
        let origin = canvas.coordinate(withNormalizedOffset: .zero)
        let layout = IOSKeyboardLayoutEngine().computeLayout(in: canvas.frame.size)
        func letter(_ byte: UInt8) throws -> CGRect {
            try XCTUnwrap(layout.keys.first { $0.kind == .letter(byte) }).frame
        }
        let t = try letter(UInt8(ascii: "t"))
        let y = try letter(UInt8(ascii: "y"))
        let touchX = t.midX + (y.midX - t.midX) * 0.60
        origin.withOffset(CGVector(dx: touchX, dy: t.midY)).tap()
        for byte in "rwu".utf8 {
            let key = try letter(byte)
            origin.withOffset(CGVector(dx: key.midX, dy: key.midY)).tap()
        }
        XCTAssertEqual(typingApp.textFields.firstMatch.value as? String, "yrwu")
        let corrected = typingApp.staticTexts["我们 trwu"].firstMatch
        XCTAssertTrue(corrected.waitForExistence(timeout: 3))
        attachShot(name: "spaceless-neighbor-key-correction")
        corrected.tap()
        XCTAssertEqual(typingApp.textFields.firstMatch.value as? String, "我们")

        Thread.sleep(forTimeInterval: 1.7) // Allow the keyboard's deferred user-data flush.
        typingApp.launchArguments = []
        typingApp.launch()
        typingApp.tabBars.buttons["误触与热区"].tap()
        let learnedT = typingApp.descendants(matching: .any)["TouchCalibration-t"].firstMatch
        XCTAssertTrue(learnedT.waitForExistence(timeout: 5))
        let sampled = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "value == %@", "1 次采样"),
            object: learnedT
        )
        XCTAssertEqual(XCTWaiter.wait(for: [sampled], timeout: 5), .completed)
        XCTAssertEqual(typingApp.descendants(matching: .any)["TouchCalibration-y"].firstMatch.value as? String, "尚未采样")
        attachShot(name: "spaceless-learned-touch-calibration")
        typingApp.buttons["重置"].tap()
        XCTAssertEqual(learnedT.value as? String, "尚未采样")
    }

    @MainActor
    private func openSpaceless98Keyboard() -> XCUIApplication {
        let app = XCUIApplication()
        app.launch()
        app.tabBars.buttons["输入设置"].tap()
        let schema98 = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "98 版")).firstMatch
        XCTAssertTrue(schema98.waitForExistence(timeout: 5))
        schema98.tap()
        for label in ["流式无空格连打（实验）", "出简不出全"] {
            let toggle = app.switches[label]
            XCTAssertTrue(toggle.waitForExistence(timeout: 5))
            if toggle.value as? String == "0" {
                toggle.tap()
            }
            XCTAssertEqual(toggle.value as? String, "1")
        }
        attachShot(name: "spaceless-98-settings")
        return openLiveKeyboard()
    }

    @MainActor
    private func resetSpacelessTestPreferences() {
        let app = XCUIApplication()
        app.launch()
        app.tabBars.buttons["输入设置"].tap()
        for label in ["流式无空格连打（实验）", "出简不出全"] where app.switches[label].value as? String == "1" {
            app.switches[label].tap()
        }
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "86 版")).firstMatch.tap()
    }

    @MainActor
    func testSpaceless98Typing() throws {
        let typingApp = openSpaceless98Keyboard()
        defer { resetSpacelessTestPreferences() }
        let field = typingApp.textFields.firstMatch
        let canvas = typingApp.otherElements["JuxuKeyCanvas"]
        XCTAssertTrue(canvas.waitForExistence(timeout: 5))
        let origin = canvas.coordinate(withNormalizedOffset: .zero)
        let layout = IOSKeyboardLayoutEngine().computeLayout(in: canvas.frame.size)
        func tap(_ kind: IOSKeyKind) throws {
            let key = try XCTUnwrap(layout.keys.first { $0.kind == kind })
            origin.withOffset(CGVector(dx: key.frame.midX, dy: key.frame.midY)).tap()
        }
        let target = "今天天气真好我们一起去吃饭吧"
        for letter in "wygdgdrtfhvbtrwuggfhfcuktqnkc".utf8 {
            try tap(.letter(letter))
        }
        let fieldVal = (field.value as? String) ?? ""
        XCTAssertTrue(
            fieldVal == "今天天气真好我们一起fcu·ktqn·kc" || fieldVal == "今天天气真好我们一起fcu·kt·qnkc",
            "Expected committed prefix + active code tail in text field, got: \(fieldVal)"
        )
        attachShot(name: "spaceless-98-composing")
        typingApp.staticTexts["去吃饭吧"].tap()
        XCTAssertEqual(field.value as? String, target)
        attachShot(name: "spaceless-98-confirmed")
        for letter in "hello".utf8 {
            try tap(.letter(letter))
        }
        try tap(.returnKey)
        XCTAssertEqual(field.value as? String, target + " hello")
        attachShot(name: "spaceless-98-return-english")
    }

    @MainActor
    func testExpandedSentenceCandidatesWrapAndScroll() throws {
        let app = openSpaceless98Keyboard()
        defer { resetSpacelessTestPreferences() }
        let canvas = app.otherElements["JuxuKeyCanvas"]
        let expand = app.buttons["JuxuCandidateToggle"]
        let candidates = app.scrollViews["JuxuExpandedCandidates"]
        let first = app.buttons["JuxuExpandedCandidate0"]
        func typeCodes(_ codes: String) throws {
            XCTAssertTrue(canvas.waitForExistence(timeout: 5))
            let origin = canvas.coordinate(withNormalizedOffset: .zero)
            let layout = IOSKeyboardLayoutEngine().computeLayout(in: canvas.frame.size)
            for letter in codes.utf8 {
                let key = try XCTUnwrap(layout.keys.first { $0.kind == .letter(letter) })
                origin.withOffset(CGVector(dx: key.frame.midX, dy: key.frame.midY)).tap()
            }
        }

        try typeCodes("rstaqkdrjfwh")
        let keyboardWidth = canvas.frame.width
        attachShot(name: "code-strip-short")
        expand.tap()
        XCTAssertTrue(first.waitForExistence(timeout: 5))
        XCTAssertEqual(first.label, "打长句的时候")
        XCTAssertGreaterThan(first.frame.width, keyboardWidth * 0.75)
        attachShot(name: "expanded-full-candidates")
        app.buttons["重输"].tap()

        try typeCodes("tvqqiepbdggtebjfddwpukwdedgxbtudcakftdgdcgwgdc")
        let inlineVal = (app.textFields.firstMatch.value as? String) ?? ""
        XCTAssertTrue(inlineVal.contains("·"), "Expected segmented inline code in text field, got: \(inlineVal)")
        XCTAssertFalse(app.staticTexts["JuxuSegmentedCode"].exists)
        attachShot(name: "code-strip-long")
        expand.tap()
        XCTAssertTrue(first.waitForExistence(timeout: 5))
        XCTAssertEqual(first.label, "常用字在五笔里其实只有三级简码或者三码全码")
        XCTAssertGreaterThan(first.frame.height, 45)
        attachShot(name: "expanded-wrapped-candidates")

        let last = app.buttons["JuxuExpandedCandidate7"]
        for _ in 0..<3 where !last.isHittable {
            candidates.swipeUp()
        }
        XCTAssertTrue(last.isHittable)
        let selected = last.label
        last.tap()
        XCTAssertEqual(app.textFields.firstMatch.value as? String, "很多" + selected)
    }

    @MainActor
    func testFourthCodeInLetterGutter() throws {
        let app = openLiveKeyboard()
        let canvas = app.otherElements["JuxuKeyCanvas"]
        XCTAssertTrue(canvas.waitForExistence(timeout: 5))
        let origin = canvas.coordinate(withNormalizedOffset: .zero)
        let layout = IOSKeyboardLayoutEngine().computeLayout(in: canvas.frame.size)
        func frame(_ letter: Character) throws -> CGRect {
            try XCTUnwrap(layout.keys.first { $0.primaryLabel == String(letter) }).frame
        }
        func tap(_ x: CGFloat, _ y: CGFloat) {
            origin.withOffset(CGVector(dx: x, dy: y)).tap()
        }

        for letter in "skc" {
            let key = try frame(letter)
            tap(key.midX, key.midY)
        }
        let field = app.textFields.firstMatch
        XCTAssertEqual(field.value as? String, "skc")

        let e = try frame("e")
        let r = try frame("r")
        tap((e.maxX + r.minX) / 2, e.midY)
        XCTAssertEqual(field.value as? String, "可能")
    }

    @MainActor
    func testEnableKeyboardAndLiveWubiTyping() throws {
        let app = openLiveKeyboard()
        attachShot(name: "02-live-priswubi-idle")

        // Measure exact keyboard position on 440x956pt iPhone 17 Pro Max (`heightConstraint = 263pt`):
        //   Row 0 (q..p): midY ≈ 688pt -> dy = 0.720
        //   Row 1 (a..l): midY ≈ 744pt -> dy = 0.778
        //   Row 2 (z..m): midY ≈ 800pt -> dy = 0.837
        //   Row 3 (space): midY ≈ 856pt -> dy = 0.895

        // 3. Hold the first-row `q` key (`dx = 0.598, dy = 0.778`) to capture real live iOS 26 callout bubble!
        Self.midPressShot = nil
        let timer = Timer.scheduledTimer(withTimeInterval: 0.35, repeats: false) { _ in
            MainActor.assumeIsolated {
                Self.midPressShot = XCUIScreen.main.screenshot()
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.05, dy: 0.720)).press(forDuration: 0.75)
        Thread.sleep(forTimeInterval: 0.25)
        if let midShot = Self.midPressShot {
            let att = XCTAttachment(screenshot: midShot)
            att.name = "03-live-priswubi-callout-q"
            att.lifetime = .keepAlways
            add(att)
        } else {
            attachShot(name: "03-live-priswubi-callout-q-fallback")
        }
        // Clear the single `q` composition via backspace (`dx = 0.935, dy = 0.837`)
        tapNorm(app, dx: 0.935, dy: 0.837)
        Thread.sleep(forTimeInterval: 0.25)

        // 3a. Hold Row-0 interior `r` key (`dx = 0.352, dy = 0.720`) to verify symmetric Row-0 docked popout!
        Self.midPressShot = nil
        let rTimer = Timer.scheduledTimer(withTimeInterval: 0.35, repeats: false) { _ in
            MainActor.assumeIsolated {
                Self.midPressShot = XCUIScreen.main.screenshot()
            }
        }
        RunLoop.main.add(rTimer, forMode: .common)
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.352, dy: 0.720)).press(forDuration: 0.75)
        Thread.sleep(forTimeInterval: 0.25)
        if let midShot = Self.midPressShot {
            let att = XCTAttachment(screenshot: midShot)
            att.name = "03a-live-priswubi-callout-r"
            att.lifetime = .keepAlways
            add(att)
        }
        tapNorm(app, dx: 0.935, dy: 0.837)
        Thread.sleep(forTimeInterval: 0.25)

        // 3b. Hold the second-row `h` key (`dx = 0.598, dy = 0.778`) to verify standalone floating squircle parity!
        Self.midPressShot = nil
        let hTimer = Timer.scheduledTimer(withTimeInterval: 0.35, repeats: false) { _ in
            MainActor.assumeIsolated {
                Self.midPressShot = XCUIScreen.main.screenshot()
            }
        }
        RunLoop.main.add(hTimer, forMode: .common)
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.598, dy: 0.778)).press(forDuration: 0.75)
        Thread.sleep(forTimeInterval: 0.25)
        if let midShot = Self.midPressShot {
            let att = XCTAttachment(screenshot: midShot)
            att.name = "03b-live-priswubi-callout-h"
            att.lifetime = .keepAlways
            add(att)
        }
        tapNorm(app, dx: 0.935, dy: 0.837)
        Thread.sleep(forTimeInterval: 0.25)

        // 4. Tap Wubi 4-code `t-r-w-u` ("我们" auto-commits on 4th keystroke!)
        tapNorm(app, dx: 0.450, dy: 0.720) // t
        Thread.sleep(forTimeInterval: 0.18)
        tapNorm(app, dx: 0.352, dy: 0.720) // r
        Thread.sleep(forTimeInterval: 0.18)
        tapNorm(app, dx: 0.155, dy: 0.720) // w
        Thread.sleep(forTimeInterval: 0.35)
        attachShot(name: "04-live-priswubi-composing-trw")

        tapNorm(app, dx: 0.648, dy: 0.720) // u -> 4-code unique auto-commits "我们"!
        Thread.sleep(forTimeInterval: 0.45)

        // 5. Test boundary-slip typo rescue: tap near the t/y border inside `y` (`dx = 0.515, dy = 0.720`) + `r-w-u`!
        // Pure `yrwu` is an empty Wubi 86 code, so our 2D Gaussian + N-gram decoder rescues `trwu` ("我们") with confidence badge!
        tapNorm(app, dx: 0.515, dy: 0.720) // slip onto left side of `y` (near `t`)
        Thread.sleep(forTimeInterval: 0.18)
        tapNorm(app, dx: 0.352, dy: 0.720) // r
        Thread.sleep(forTimeInterval: 0.18)
        tapNorm(app, dx: 0.155, dy: 0.720) // w
        Thread.sleep(forTimeInterval: 0.18)
        tapNorm(app, dx: 0.648, dy: 0.720) // u -> rescued candidate stays on ribbon (no blind 4-code auto-commit!)
        Thread.sleep(forTimeInterval: 0.45)
        attachShot(name: "05-live-priswubi-typo-rescue-yrwu")

        // Commit rescued candidate via Spacebar (`dx = 0.500, dy = 0.895`)
        tapNorm(app, dx: 0.500, dy: 0.895)
        Thread.sleep(forTimeInterval: 0.35)

        // A fifth key commits the previous word and begins the next composition.
        // This is the exact boundary that left a literal 't' in the phone recording.
        let rows = ["qwertyuiop", "asdfghjkl", "zxcvbnm"]
        for ch in "rtdgthtcs" {
            for (row, letters) in rows.enumerated() {
                guard let index = Array(letters).firstIndex(of: ch) else { continue }
                let x = [0.05, 0.10, 0.20][row] + Double(index) * 0.10
                tapNorm(app, dx: x, dy: [0.720, 0.778, 0.837][row])
            }
        }
        let field = app.textFields.firstMatch
        XCTAssertEqual(field.value as? String, "我们我们手感怎么s")
        attachShot(name: "continuous-input")
        tapNorm(app, dx: 0.935, dy: 0.837) // discard pending s
        XCTAssertEqual(field.value as? String, "我们我们手感怎么")

        // 6. Tap `k-h-k` (`中 ⚡k`) so Simulator ends with live Wubi candidates & short-code hint on screen
        tapNorm(app, dx: 0.795, dy: 0.778) // k
        Thread.sleep(forTimeInterval: 0.18)
        tapNorm(app, dx: 0.598, dy: 0.778) // h
        Thread.sleep(forTimeInterval: 0.18)
        tapNorm(app, dx: 0.795, dy: 0.778) // k
        Thread.sleep(forTimeInterval: 0.45)
        attachShot(name: "06-live-priswubi-khk")

        let tf = app.textFields.firstMatch
        let val = (tf.value as? String) ?? ""
        XCTAssertEqual(val, "我们我们手感怎么khk")

        // Change an option in the host, then open the keyboard in a fresh session.
        app.launchArguments = []
        app.launch()
        app.buttons["输入设置"].tap()
        let autoCommit = app.switches["四码唯一自动上屏"]
        XCTAssertEqual(autoCommit.value as? String, "1")
        autoCommit.tap()
        attachShot(name: "host-settings")
        defer {
            app.launchArguments = []
            app.launch()
            app.buttons["输入设置"].tap()
            if autoCommit.value as? String == "0" { autoCommit.tap() }
        }
        app.launchArguments = ["--autofocus"]
        app.launch()
        XCTAssertTrue(app.textFields.firstMatch.waitForExistence(timeout: 5))
        Thread.sleep(forTimeInterval: 1)
        for x in [0.450, 0.352, 0.155, 0.648] { tapNorm(app, dx: x, dy: 0.720) }
        XCTAssertNotEqual(app.textFields.firstMatch.value as? String, "我们")
        tapNorm(app, dx: 0.500, dy: 0.895)
        XCTAssertEqual(app.textFields.firstMatch.value as? String, "我们")

        let space = app.coordinate(withNormalizedOffset: CGVector(dx: 0.50, dy: 0.895))
        let left = app.coordinate(withNormalizedOffset: CGVector(dx: 0.478, dy: 0.895))
        space.press(forDuration: 0.6, thenDragTo: left)
        tapNorm(app, dx: 0.06, dy: 0.895) // 123
        tapNorm(app, dx: 0.21, dy: 0.837) // full stop
        XCTAssertEqual(app.textFields.firstMatch.value as? String, "我。们")
        tapNorm(app, dx: 0.450, dy: 0.720) // back on letters
        XCTAssertEqual(app.textFields.firstMatch.value as? String, "我。t们")
        tapNorm(app, dx: 0.935, dy: 0.837) // erase marked t
        let numbers = app.coordinate(withNormalizedOffset: CGVector(dx: 0.06, dy: 0.895))
        let five = app.coordinate(withNormalizedOffset: CGVector(dx: 0.45, dy: 0.720))
        numbers.press(forDuration: 0.4, thenDragTo: five)
        XCTAssertEqual(app.textFields.firstMatch.value as? String, "我。5们")
        five.tap()
        XCTAssertEqual(app.textFields.firstMatch.value as? String, "我。5t们")
        tapNorm(app, dx: 0.935, dy: 0.837) // erase marked t
        tapNorm(app, dx: 0.06, dy: 0.895) // 123
        tapNorm(app, dx: 0.15, dy: 0.720) // '2' (stays on 123)
        tapNorm(app, dx: 0.05, dy: 0.778) // '-' (stays on 123)
        tapNorm(app, dx: 0.25, dy: 0.720) // '3' (stays on 123)
        tapNorm(app, dx: 0.78, dy: 0.837) // '.' (stays on 123)
        tapNorm(app, dx: 0.35, dy: 0.720) // '4' (stays on 123)
        XCTAssertEqual(app.textFields.firstMatch.value as? String, "我。52-3.4们")

    }

    @MainActor
    func testClipboardSuggestionAndOTPExtraction() throws {
        // 1. Ensure Full Access is enabled for 句序 in iOS Settings -> 通用 -> 键盘 -> 键盘 -> 句序
        let settings = XCUIApplication(bundleIdentifier: "com.apple.Preferences")
        settings.terminate()
        settings.launch()

        let general = settings.cells.containing(.staticText, identifier: "通用").firstMatch
        if general.waitForExistence(timeout: 3) { general.tap() }
        let kb = settings.cells.containing(.staticText, identifier: "键盘").firstMatch
        if kb.waitForExistence(timeout: 3) { kb.tap() }
        let kbsCell = settings.tables.cells.containing(.staticText, identifier: "键盘").firstMatch
        if kbsCell.waitForExistence(timeout: 3) { kbsCell.tap() }

        let prisRow = settings.cells.containing(NSPredicate(format: "label CONTAINS '句序'")).firstMatch
        if prisRow.waitForExistence(timeout: 3) {
            prisRow.tap()
            let fullAccessSwitch = settings.switches.containing(NSPredicate(format: "label CONTAINS '允许完全访问'")).firstMatch
            if fullAccessSwitch.waitForExistence(timeout: 3), (fullAccessSwitch.value as? String) == "0" {
                let innerKnob = fullAccessSwitch.switches.firstMatch
                if innerKnob.exists {
                    innerKnob.tap()
                } else {
                    fullAccessSwitch.coordinate(withNormalizedOffset: CGVector(dx: 0.92, dy: 0.5)).tap()
                }
                let allowBtn = settings.alerts.buttons["允许"].firstMatch
                if allowBtn.waitForExistence(timeout: 2.5) {
                    allowBtn.tap()
                } else {
                    let sbAllow = XCUIApplication(bundleIdentifier: "com.apple.springboard").alerts.buttons["允许"].firstMatch
                    if sbAllow.waitForExistence(timeout: 1.5) {
                        sbAllow.tap()
                    }
                }
            }
        }

        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        func acceptPastePromptIfShown(captureName: String? = nil) {
            let allowPaste = springboard.buttons["允许粘贴"].firstMatch
            if allowPaste.waitForExistence(timeout: 2.0) {
                if let captureName {
                    attachShot(name: captureName)
                }
                allowPaste.tap()
                Thread.sleep(forTimeInterval: 0.4)
            }
        }

        defer {
            UIPasteboard.general.items = []
        }

        // 2. Copy a normal URL -> launch app -> centered clipboard pill appears -> tap inserts & dismisses
        UIPasteboard.general.string = "https://still.fail/docs/formordo"
        let app = openLiveKeyboard()
        acceptPastePromptIfShown(captureName: "clip-00-during-paste-prompt")
        Thread.sleep(forTimeInterval: 0.5)

        let inputUI = XCUIApplication(bundleIdentifier: "com.apple.InputUI")
        let pillInApp = app.buttons["JuxuClipboardPill"]
        let pillInInput = inputUI.buttons["JuxuClipboardPill"]
        let pill = pillInApp.waitForExistence(timeout: 2.5) ? pillInApp : pillInInput
        XCTAssertTrue(pill.waitForExistence(timeout: 2.5), "Expected centered clipboard pill to appear in idle candidate bar")
        attachShot(name: "clip-01-normal-light")

        pill.tap()
        Thread.sleep(forTimeInterval: 0.4)
        XCTAssertEqual(app.textFields.firstMatch.value as? String, "https://still.fail/docs/formordo")
        XCTAssertFalse(pill.exists, "Clipboard pill should disappear immediately after pasting")

        // 3. Copy an SMS with OTP -> keyboard extracts 6-digit code -> tap inserts pure code
        UIPasteboard.general.string = "【招商银行】2026年11月19日您尾号8812的卡验证码为394810，5分钟内有效。"
        app.launchArguments = ["--autofocus"]
        app.launch()
        acceptPastePromptIfShown()
        Thread.sleep(forTimeInterval: 0.5)

        let otpPill = pillInApp.waitForExistence(timeout: 2.5) ? pillInApp : pillInInput
        XCTAssertTrue(otpPill.waitForExistence(timeout: 2.5), "Expected OTP clipboard pill to appear")
        attachShot(name: "clip-02-otp-light")

        otpPill.tap()
        Thread.sleep(forTimeInterval: 0.4)
        XCTAssertEqual(app.textFields.firstMatch.value as? String, "394810")

        // 4. Copy another string and tap the subtle dismiss 'x' button -> hides without inserting
        UIPasteboard.general.string = "临时忽略测试文本"
        app.launchArguments = ["--autofocus"]
        app.launch()
        acceptPastePromptIfShown()
        Thread.sleep(forTimeInterval: 0.5)

        let dismissInApp = app.buttons["JuxuClipboardDismiss"]
        let dismissInInput = inputUI.buttons["JuxuClipboardDismiss"]
        let dismissBtn = dismissInApp.waitForExistence(timeout: 2.5) ? dismissInApp : dismissInInput
        XCTAssertTrue(dismissBtn.waitForExistence(timeout: 2.5), "Expected clipboard dismiss button")
        dismissBtn.tap()
        Thread.sleep(forTimeInterval: 0.3)
        XCTAssertFalse(pillInApp.exists || pillInInput.exists, "Dismiss button must hide clipboard pill")
        XCTAssertEqual((app.textFields.firstMatch.value as? String) ?? "", "点此弹出键盘试打五笔、长按空格滑动移光标…")
    }

    @MainActor
    func testChevronHitAreaAndZPinyinExpandedGrid() throws {
        let app = openLiveKeyboard()
        let canvas = app.otherElements["JuxuKeyCanvas"]
        XCTAssertTrue(canvas.waitForExistence(timeout: 5))
        let origin = canvas.coordinate(withNormalizedOffset: .zero)
        let layout = IOSKeyboardLayoutEngine().computeLayout(in: canvas.frame.size)
        for letter in "zshi".utf8 {
            let key = try XCTUnwrap(layout.keys.first { $0.kind == .letter(letter) })
            origin.withOffset(CGVector(dx: key.frame.midX, dy: key.frame.midY)).tap()
        }

        let expand = app.buttons["JuxuCandidateToggle"]
        XCTAssertTrue(expand.waitForExistence(timeout: 5))
        XCTAssertGreaterThanOrEqual(expand.frame.height, 50.0, "Chevron button should span full candidate bar height")
        attachShot(name: "z-pinyin-ribbon")

        // Tap chevron button
        expand.tap()

        let first = app.buttons["JuxuExpandedCandidate0"]
        XCTAssertTrue(first.waitForExistence(timeout: 5), "Tapping upper chevron hit zone must open expanded grid")
        let beyond36 = app.buttons["JuxuExpandedCandidate50"]
        XCTAssertTrue(beyond36.exists, "Expanded grid must list all z-pinyin candidates beyond the old 36 cap")
        attachShot(name: "z-pinyin-expanded")
    }

    @MainActor
    func testFocusOutAndRefocusClearsUncommittedPreedit() throws {
        let app = openLiveKeyboard()
        app.launchArguments = ["--autofocus", "--dual-sandbox"]
        app.launch()
        let canvas = app.otherElements["JuxuKeyCanvas"]
        XCTAssertTrue(canvas.waitForExistence(timeout: 5))
        let origin = canvas.coordinate(withNormalizedOffset: .zero)
        let layout = IOSKeyboardLayoutEngine().computeLayout(in: canvas.frame.size)
        func tapKey(_ ch: Character) throws {
            let key = try XCTUnwrap(layout.keys.first { $0.primaryLabel == String(ch) })
            origin.withOffset(CGVector(dx: key.frame.midX, dy: key.frame.midY)).tap()
        }
        func tapKind(_ kind: IOSKeyKind) throws {
            let key = try XCTUnwrap(layout.keys.first { $0.kind == kind })
            origin.withOffset(CGVector(dx: key.frame.midX, dy: key.frame.midY)).tap()
        }

        let f1 = app.textFields["JuxuSandboxPrimary"]
        let f2 = app.textFields["JuxuSandboxSecondary"]
        let tv = app.textViews["JuxuSandboxEditor"]
        XCTAssertTrue(f1.waitForExistence(timeout: 3))
        XCTAssertTrue(f2.waitForExistence(timeout: 3))
        XCTAssertTrue(tv.waitForExistence(timeout: 3))

        // 1. UITextField -> UITextField (type in f2!) -> UITextField:
        //    Uncommitted `khk` in f1 must NOT leak into f2, and refocusing f1 clears orphaned `khk`.
        for ch in "wqvb" { try tapKey(ch) } // "你好"
        for ch in "khk" { try tapKey(ch) }
        XCTAssertEqual(f1.value as? String, "你好khk")

        f2.tap()
        Thread.sleep(forTimeInterval: 0.25)
        for ch in "trwu" { try tapKey(ch) } // "我们" in f2 without leaking "中" from f1!
        XCTAssertEqual(f2.value as? String, "我们")

        f1.tap()
        Thread.sleep(forTimeInterval: 0.35)
        XCTAssertEqual(f1.value as? String, "你好")
        for ch in "trwu" { try tapKey(ch) }
        XCTAssertEqual(f1.value as? String, "你好我们")

        // 2. UITextView -> UITextField -> UITextView + Spacebar:
        if app.buttons["清空"].exists { app.buttons["清空"].tap() }
        tv.tap()
        Thread.sleep(forTimeInterval: 0.25)
        for ch in "trwukhk" { try tapKey(ch) }
        XCTAssertEqual(tv.value as? String, "我们khk")

        f2.tap()
        Thread.sleep(forTimeInterval: 0.25)
        tv.tap()
        Thread.sleep(forTimeInterval: 0.35)
        XCTAssertEqual(tv.value as? String, "我们")
        try tapKind(.space)
        XCTAssertEqual(tv.value as? String, "我们 ")

        // 3. UITextView keyboard dismiss (`resignFirstResponder`) -> refocus -> Backspace & typing:
        if app.buttons["清空"].exists { app.buttons["清空"].tap() }
        tv.tap()
        Thread.sleep(forTimeInterval: 0.25)
        for ch in "trwukhk" { try tapKey(ch) }
        XCTAssertEqual(tv.value as? String, "我们khk")

        app.buttons["JuxuSandboxDone"].tap()
        Thread.sleep(forTimeInterval: 0.6)
        tv.tap()
        XCTAssertTrue(canvas.waitForExistence(timeout: 5))
        Thread.sleep(forTimeInterval: 0.35)
        XCTAssertEqual(tv.value as? String, "我们")
        try tapKind(.backspace)
        XCTAssertEqual(tv.value as? String, "我")
        for ch in "trwu" { try tapKey(ch) }
        XCTAssertEqual(tv.value as? String, "我我们")

        // 4. WKWebView `<textarea>` field-switch AND keyboard-dismiss refocus:
        let webBox = app.webViews.textViews.firstMatch
        if webBox.waitForExistence(timeout: 3) {
            webBox.tap()
            XCTAssertTrue(canvas.waitForExistence(timeout: 5))
            Thread.sleep(forTimeInterval: 0.3)
            for ch in "trwukhk" { try tapKey(ch) }
            XCTAssertEqual(webBox.value as? String, "我们khk")

            f2.tap()
            Thread.sleep(forTimeInterval: 0.25)
            webBox.tap()
            Thread.sleep(forTimeInterval: 0.4)
            XCTAssertEqual(webBox.value as? String, "我们")

            for ch in "khk" { try tapKey(ch) }
            XCTAssertEqual(webBox.value as? String, "我们khk")
            app.buttons["JuxuSandboxDone"].tap()
            Thread.sleep(forTimeInterval: 0.6)
            webBox.tap()
            XCTAssertTrue(canvas.waitForExistence(timeout: 5))
            Thread.sleep(forTimeInterval: 0.4)
            XCTAssertEqual(webBox.value as? String, "我们")
            for ch in "trwu" { try tapKey(ch) }
            XCTAssertEqual(webBox.value as? String, "我们我们")
        }
    }

    @MainActor
    private func tapNorm(_ app: XCUIApplication, dx: CGFloat, dy: CGFloat) {
        app.coordinate(withNormalizedOffset: CGVector(dx: dx, dy: dy)).tap()
    }

    @MainActor
    private func attachShot(name: String) {
        let shot = XCUIScreen.main.screenshot()
        let att = XCTAttachment(screenshot: shot)
        att.name = name
        att.lifetime = .keepAlways
        add(att)
    }

    @MainActor
    private func attachHierarchy(of app: XCUIApplication, name: String) {
        let att = XCTAttachment(string: app.debugDescription)
        att.name = name
        att.lifetime = .keepAlways
        add(att)
    }
}
