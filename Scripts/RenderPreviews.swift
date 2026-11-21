import AppKit
import SwiftUI

final class PreviewKeyWindow: NSWindow {
    override var canBecomeKey: Bool { true }
    override var isKeyWindow: Bool { true }
}

@main
struct RenderPreviews {
    @MainActor
    static func main() throws {
        guard CommandLine.arguments.count >= 2 else {
            fputs("Usage: RenderPreviews <output-dir>\n", stderr)
            exit(1)
        }
        let outDir = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        try FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let dictsURL = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
            .appendingPathComponent("Resources/Dicts", isDirectory: true)
        let bridge = WubiBridge(customDictsDir: dictsURL, inMemoryOnly: true)
        _ = bridge.addUserPhrase("棱五笔", persist: false)

        // Generate real engine states from the live Rust core + 86/98/06 dicts!
        // State A: Touch boundary typo rescue (`yrwu` near T/Y border -> rescues "我们" with confidence!)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "y"), x: 4.54, y: 0.02, isNormalized: true, timestampMs: 100)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "r"), x: 3.01, y: 0.01, isNormalized: true, timestampMs: 160)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "w"), x: 1.00, y: 0.00, isNormalized: true, timestampMs: 220)
        let typoState = bridge.pushTouch(reportedAscii: UInt8(ascii: "u"), x: 6.00, y: 0.00, isNormalized: true, timestampMs: 280)
        _ = bridge.handleKey(keyCode: 0x31, scalar: 32, hasCmdOrCtrl: false, timestampMs: 320)

        // State B: 3-code input with short-code hint (`khk` -> "中 ⚡k" + prefix completions)
        bridge.clearComposition()
        for ch in "khk".unicodeScalars {
            _ = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 400)
        }
        let shortcodeState = bridge.state

        bridge.clearComposition()
        for ch in "zguo".unicodeScalars {
            _ = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 500)
        }
        var pinyinVerticalState = bridge.state
        pinyinVerticalState = WubiActionState(
            consumed: pinyinVerticalState.consumed,
            bell: false,
            asciiMode: false,
            schemaId: 98,
            page: 0,
            totalPages: pinyinVerticalState.totalPages,
            vertical: true,
            schemaName: "98版五笔",
            commitText: "",
            preedit: pinyinVerticalState.preedit,
            notice: "",
            userPhraseCount: 1,
            calibratedKeyCount: 4,
            candidates: Array(pinyinVerticalState.candidates.prefix(5))
        )

        // 1. Render Candidate Bars Showcase (Light & Dark)
        for (modeName, isDark) in [("light", false), ("dark", true)] {
            let showcase = CandidateShowcaseCanvas(
                isDark: isDark,
                typoState: typoState,
                shortcodeState: shortcodeState,
                verticalPinyinState: pinyinVerticalState
            )
            try renderSwiftUIView(
                showcase,
                width: 740,
                height: 410,
                isDark: isDark,
                to: outDir.appendingPathComponent("candidates-\(modeName).png")
            )
        }

        // 2. Prime bridge with active touch typo state so Settings "误触与热区" shows live candidate rescue!
        bridge.clearComposition()
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "y"), x: 4.54, y: 0.02, isNormalized: true, timestampMs: 700)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "r"), x: 3.01, y: 0.01, isNormalized: true, timestampMs: 760)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "w"), x: 1.00, y: 0.00, isNormalized: true, timestampMs: 820)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "u"), x: 6.00, y: 0.00, isNormalized: true, timestampMs: 880)

        try renderSwiftUIView(
            WindowChromeMock(title: "棱五笔 (PrisWubi) 偏好设置", isDark: false) {
                SettingsRootView(bridge: bridge, initialSection: .schema)
            },
            width: 740,
            height: 620,
            isDark: false,
            to: outDir.appendingPathComponent("settings-schema-light.png")
        )

        try renderSwiftUIView(
            WindowChromeMock(title: "棱五笔 (PrisWubi) 偏好设置", isDark: true) {
                SettingsRootView(bridge: bridge, initialSection: .typo)
            },
            width: 740,
            height: 770,
            isDark: true,
            to: outDir.appendingPathComponent("settings-typo-dark.png")
        )

        // 3. Render macOS Voice Settings (Cloud BYOK preset + Local one-click installer previews)
        VoicePreferencesStore.shared.update({
            $0.enabled = true
            $0.providerMode = .cloudBYOK
            $0.cloudPreset = .groq
        }, persist: false)
        try renderSwiftUIView(
            WindowChromeMock(title: "句序 (Juxu) 偏好设置", isDark: false) {
                SettingsRootView(bridge: bridge, initialSection: .voice)
            },
            width: 740,
            height: 760,
            isDark: false,
            to: outDir.appendingPathComponent("settings-voice-cloud.png")
        )
        VoicePreferencesStore.shared.update({
            $0.enabled = true
            $0.providerMode = .local
            $0.localASRPreset = .whisperTurbo
            $0.localLLMPreset = .qwen15B
        }, persist: false)
        try renderSwiftUIView(
            WindowChromeMock(title: "句序 (Juxu) 偏好设置", isDark: false) {
                SettingsRootView(bridge: bridge, initialSection: .voice)
            },
            width: 740,
            height: 760,
            isDark: false,
            to: outDir.appendingPathComponent("settings-voice-local.png")
        )
        VoicePreferencesStore.shared.update({ $0.enabled = false }, persist: false)

        print("Rendered 2x Retina previews in \(outDir.path)")
    }

    @MainActor
    static func renderSwiftUIView<V: View>(
        _ root: V,
        width: CGFloat,
        height: CGFloat,
        isDark: Bool,
        to url: URL
    ) throws {
        let appearance = NSAppearance(named: isDark ? .darkAqua : .aqua)!
        let wrapped = root.environment(\.colorScheme, isDark ? .dark : .light)
        let win = PreviewKeyWindow(
            contentRect: NSRect(x: 0, y: 0, width: width, height: height),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        win.appearance = appearance
        win.isOpaque = false
        win.backgroundColor = .clear

        let hosting = NSHostingView(rootView: wrapped)
        hosting.frame = NSRect(x: 0, y: 0, width: width, height: height)
        hosting.appearance = appearance
        win.contentView = hosting

        hosting.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.12))
        hosting.layoutSubtreeIfNeeded()

        let bounds = hosting.bounds
        let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: Int(bounds.width * 2),
            pixelsHigh: Int(bounds.height * 2),
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        )!
        rep.size = bounds.size
        hosting.cacheDisplay(in: bounds, to: rep)
        let png = rep.representation(using: .png, properties: [:])!
        try png.write(to: url)
    }
}

struct InlineEditorLineMock: View {
    let isDark: Bool
    let committedPrefix: String
    let markedPreedit: String
    let candidateState: WubiActionState

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 0) {
                Text(committedPrefix)
                    .font(.system(size: 15))
                    .foregroundStyle(.primary)
                Text(markedPreedit)
                    .font(.system(size: 15))
                    .underline(true, color: .primary.opacity(0.65))
                    .foregroundStyle(.primary)
                Rectangle()
                    .fill(Color.accentColor)
                    .frame(width: 1.5, height: 17)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .fill(isDark ? Color(white: 0.14) : Color.white)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .strokeBorder(Color.primary.opacity(0.10), lineWidth: 0.6)
            )

            CandidateBarView(state: candidateState)
                .padding(.leading, CGFloat(committedPrefix.count) * 15.0 + 12.0)
                .shadow(color: .black.opacity(isDark ? 0.42 : 0.16), radius: 9, x: 0, y: 3)
        }
    }
}

struct CandidateShowcaseCanvas: View {
    let isDark: Bool
    let typoState: WubiActionState
    let shortcodeState: WubiActionState
    let verticalPinyinState: WubiActionState

    var body: some View {
        ZStack(alignment: .topLeading) {
            LinearGradient(
                colors: isDark
                    ? [Color(red: 0.12, green: 0.13, blue: 0.16), Color(red: 0.08, green: 0.09, blue: 0.11)]
                    : [Color(red: 0.94, green: 0.95, blue: 0.97), Color(red: 0.89, green: 0.91, blue: 0.94)],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )

            VStack(alignment: .leading, spacing: 18) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("1. 原生单行横排 · 触点/邻键纠错轻提示（行内下划线预编辑，零弹出缩放动画、绝不折行）")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.secondary)
                    InlineEditorLineMock(
                        isDark: isDark,
                        committedPrefix: "正在体验苹果原生风格五笔：",
                        markedPreedit: "yrwu",
                        candidateState: typoState
                    )
                }

                HStack(alignment: .top, spacing: 26) {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("2. 全码输入 · 简码轻提示（khk → 中 k）")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(.secondary)
                        InlineEditorLineMock(
                            isDark: isDark,
                            committedPrefix: "中国",
                            markedPreedit: "khk",
                            candidateState: shortcodeState
                        )
                    }

                    VStack(alignment: .leading, spacing: 6) {
                        Text("3. 竖排候选栏 · z 键拼音反查")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(.secondary)
                        CandidateBarView(state: verticalPinyinState)
                            .shadow(color: .black.opacity(isDark ? 0.42 : 0.16), radius: 9, x: 0, y: 3)
                    }
                }
            }
            .padding(24)
        }
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
    }
}

struct WindowChromeMock<Content: View>: View {
    let title: String
    let isDark: Bool
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Circle().fill(Color(red: 1.0, green: 0.37, blue: 0.34)).frame(width: 12, height: 12)
                Circle().fill(Color(red: 1.0, green: 0.74, blue: 0.18)).frame(width: 12, height: 12)
                Circle().fill(Color(red: 0.16, green: 0.78, blue: 0.25)).frame(width: 12, height: 12)
                Spacer()
                Text(title)
                    .font(.system(size: 12.5, weight: .medium))
                    .foregroundStyle(.secondary)
                Spacer()
                Spacer().frame(width: 44)
            }
            .padding(.horizontal, 14)
            .frame(height: 34)
            .background(Color(nsColor: .windowBackgroundColor))

            Divider()
            content()
                .background(Color(nsColor: .windowBackgroundColor))
        }
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Color.primary.opacity(0.15), lineWidth: 0.8)
        )
    }
}
