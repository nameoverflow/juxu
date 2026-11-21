import AppKit
import SwiftUI

@main
struct RenderIOSPreviews {
    @MainActor
    static func main() throws {
        guard CommandLine.arguments.count >= 2 else {
            fputs("Usage: RenderIOSPreviews <output-dir>\n", stderr)
            exit(1)
        }
        let outDir = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        try FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let dictsURL = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
            .appendingPathComponent("Resources/Dicts", isDirectory: true)
        let bridge = JuxuBridge(customDictsDir: dictsURL, inMemoryOnly: true)

        // Exact 440pt iPhone Pro Max iOS 26 keyboard dimensions (`440 x 225` key area + `52pt` candidate bar + `69pt` bottom safe bar)
        let kbEngine = IOSKeyboardLayoutEngine()
        let snap = kbEngine.computeLayout(in: CGSize(width: 440, height: 225))
        bridge.setKeyLayout(
            centers52: snap.centers52,
            originX: snap.originX,
            originY: snap.originY,
            pitchX: snap.pitchX,
            pitchY: snap.pitchY
        )

        let tSpec = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "t")) })!
        let ySpec = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "y")) })!
        let rSpec = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "r")) })!
        let wSpec = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "w")) })!
        let uSpec = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "u")) })!
        let hSpec = snap.keys.first(where: { $0.kind == .letter(UInt8(ascii: "h")) })!

        // 1. Render Pressing `h` Callout State (1:1 comparison against user's IMG_2628.PNG!)
        bridge.clearComposition()
        try renderCanvas(
            IOS26KeyboardSheetPreview(
                isDark: false,
                snapshot: snap,
                state: bridge.state,
                pressedKey: hSpec,
                expandedCandidates: nil
            ),
            width: 440,
            height: 430,
            isDark: false,
            to: outDir.appendingPathComponent("ios26-bubble-h-light.png")
        )

        // 2. Render Composing + Bayesian Touch-Border Rescue (`yrwu` -> "我们") + Row-0 Floating Squircle Bubble on `u`
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "y"), x: Float(ySpec.frame.minX + 1.2), y: Float(tSpec.frame.midY + 1.0), isNormalized: false, timestampMs: 1000)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "r"), x: Float(rSpec.frame.midX), y: Float(rSpec.frame.midY), isNormalized: false, timestampMs: 1060)
        _ = bridge.pushTouch(reportedAscii: UInt8(ascii: "w"), x: Float(wSpec.frame.midX), y: Float(wSpec.frame.midY), isNormalized: false, timestampMs: 1120)
        let typoState = bridge.pushTouch(reportedAscii: UInt8(ascii: "u"), x: Float(uSpec.frame.midX), y: Float(uSpec.frame.midY), isNormalized: false, timestampMs: 1180)

        try renderCanvas(
            IOS26KeyboardSheetPreview(
                isDark: false,
                snapshot: snap,
                state: typoState,
                pressedKey: uSpec,
                expandedCandidates: nil
            ),
            width: 440,
            height: 430,
            isDark: false,
            to: outDir.appendingPathComponent("ios26-bubble-u-composing.png")
        )

        // 3. Render Dark Mode iOS 26 Liquid Glass Keyboard with Floating Squircle Bubble on `h`!
        bridge.clearComposition()
        for ch in "kh".unicodeScalars {
            _ = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 2000)
        }
        try renderCanvas(
            IOS26KeyboardSheetPreview(
                isDark: true,
                snapshot: snap,
                state: bridge.state,
                pressedKey: hSpec,
                expandedCandidates: nil
            ),
            width: 440,
            height: 430,
            isDark: true,
            to: outDir.appendingPathComponent("ios26-bubble-dark.png")
        )

        // 4. Render Expanded 4-Column Candidate Grid (`zguo` Pinyin Reverse Lookup) in Light iOS 26 Glass
        bridge.clearComposition()
        for ch in "zguo".unicodeScalars {
            _ = bridge.handleKey(keyCode: 0, scalar: ch.value, hasCmdOrCtrl: false, timestampMs: 3000)
        }
        let expanded = bridge.allCandidates(maxCount: 16)
        try renderCanvas(
            IOS26KeyboardSheetPreview(
                isDark: false,
                snapshot: snap,
                state: bridge.state,
                pressedKey: nil,
                expandedCandidates: expanded
            ),
            width: 440,
            height: 430,
            isDark: false,
            to: outDir.appendingPathComponent("ios26-keyboard-expanded-light.png")
        )

        print("Rendered iOS 26 native previews in \(outDir.path)")
    }

    @MainActor
    static func renderCanvas<V: View>(
        _ content: V,
        width: CGFloat,
        height: CGFloat,
        isDark: Bool,
        to url: URL
    ) throws {
        let appearance = NSAppearance(named: isDark ? .darkAqua : .aqua)!
        let root = content.environment(\.colorScheme, isDark ? .dark : .light)

        let win = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: width, height: height),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        win.appearance = appearance
        win.isOpaque = false
        win.backgroundColor = .clear

        let hosting = NSHostingView(rootView: root)
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

/// Renders the exact bottom half of an iOS 26 iPhone screen (Input composer + 26pt rounded Liquid Glass keyboard tray + system Globe/Mic bar).
struct IOS26KeyboardSheetPreview: View {
    let isDark: Bool
    let snapshot: IOSKeyboardLayoutEngine.LayoutSnapshot
    let state: JuxuActionState
    let pressedKey: IOSKeySpec?
    let expandedCandidates: [JuxuCandidateItem]?

    private static let starterWords = ["我", "海南", "不是", "怎么", "你", "在", "这", "不"]

    var body: some View {
        let appSurface = isDark
            ? Color(red: 0.08, green: 0.08, blue: 0.09)
            : Color(red: 0.965, green: 0.961, blue: 0.949) // Exact warm off-white from IMG_2627!
        let trayBg = isDark
            ? Color(red: 0.14, green: 0.14, blue: 0.16)
            : Color(red: 230.0 / 255.0, green: 231.0 / 255.0, blue: 234.0 / 255.0)
        let composerBg = isDark ? Color(white: 0.15) : Color.white

        VStack(spacing: 0) {
            Spacer(minLength: 10)

            // Input Composer Box above keyboard
            HStack(alignment: .center, spacing: 10) {
                Image(systemName: "plus")
                    .font(.system(size: 19, weight: .regular))
                    .foregroundStyle(.secondary)

                if state.preedit.isEmpty {
                    Spacer()
                } else {
                    HStack(spacing: 1) {
                        Text(state.preedit)
                            .font(.system(size: 17, weight: .regular))
                            .underline(true, color: .primary.opacity(0.45))
                        Rectangle()
                            .fill(Color.blue)
                            .frame(width: 2, height: 20)
                    }
                    Spacer()
                }

                ZStack {
                    Circle()
                        .fill(state.preedit.isEmpty ? Color.primary.opacity(0.14) : Color.blue)
                        .frame(width: 34, height: 34)
                    Image(systemName: "arrow.up")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(.white)
                }
            }
            .padding(.horizontal, 16)
            .frame(height: 54)
            .background(composerBg, in: RoundedRectangle(cornerRadius: 24, style: .continuous))
            .padding(.horizontal, 10)
            .padding(.bottom, 10)

            // iOS 26 Rounded Liquid Glass Keyboard Container (`346pt` = 52pt Candidate + 225pt Keys + 69pt System Bar)
            ZStack(alignment: .topLeading) {
                VStack(spacing: 0) {
                    // 52pt Airy Unboxed Candidate Bar
                    HStack(spacing: 0) {
                        ScrollView(.horizontal, showsIndicators: false) {
                            HStack(spacing: 23) {
                                if state.preedit.isEmpty {
                                    ForEach(Self.starterWords, id: \.self) { w in
                                        Text(w)
                                            .font(.system(size: 18.5, weight: .regular))
                                            .foregroundStyle(isDark ? Color.white : Color.black)
                                    }
                                } else {
                                    ForEach(Array(state.candidates.enumerated()), id: \.element.id) { idx, item in
                                        HStack(alignment: .firstTextBaseline, spacing: 4) {
                                            Text(item.text)
                                                .font(.system(size: 18.5, weight: idx == 0 ? .medium : .regular))
                                                .foregroundStyle(isDark ? Color.white : Color.black)
                                            if !item.comment.isEmpty && (item.badgeKind == .typoCorrected || item.badgeKind == .shortcode || item.badgeKind == .pinyinLookup) {
                                                let shortHint = item.badgeKind == .typoCorrected
                                                    ? (item.comment.components(separatedBy: " ").first ?? item.comment)
                                                    : item.comment
                                                Text(shortHint)
                                                    .font(.system(size: 11, design: .monospaced))
                                                    .foregroundStyle(.secondary)
                                            }
                                        }
                                    }
                                }
                            }
                            .padding(.horizontal, 15)
                        }

                        Rectangle()
                            .fill(Color.primary.opacity(0.14))
                            .frame(width: 0.7, height: 21)

                        Image(systemName: expandedCandidates != nil ? "chevron.up" : "chevron.down")
                            .font(.system(size: 15, weight: .medium))
                            .foregroundStyle(isDark ? Color.white : Color.black)
                            .frame(width: 44)
                    }
                    .frame(height: 52.5)

                    // 225pt 4-Row Key Area (or Expanded 4-Column Grid)
                    if let expanded = expandedCandidates {
                        IOS26ExpandedGridPreview(candidates: expanded, isDark: isDark)
                            .frame(width: 440, height: 225)
                    } else {
                        IOS26KeyCanvasPreview(snapshot: snapshot, isDark: isDark, pressedKey: pressedKey)
                            .frame(width: 440, height: 225)
                    }

                    // 69pt System Bottom Safe-Area Bar (`🌐` left, `🎤` right) matching Face ID iOS 26!
                    HStack {
                        Image(systemName: "globe")
                            .font(.system(size: 25, weight: .light))
                            .foregroundStyle(isDark ? Color.white.opacity(0.88) : Color.black.opacity(0.85))
                            .padding(.leading, 29)
                        Spacer()
                        Image(systemName: "mic")
                            .font(.system(size: 24, weight: .light))
                            .foregroundStyle(isDark ? Color.white.opacity(0.88) : Color.black.opacity(0.85))
                            .padding(.trailing, 31)
                    }
                    .frame(height: 68.5)
                }
                .background(
                    TopRoundedShape(radius: 26)
                        .fill(trayBg)
                )
                .overlay(
                    TopRoundedShape(radius: 25.5)
                        .stroke(isDark ? Color.white.opacity(0.16) : Color.white.opacity(0.92), lineWidth: 0.9)
                        .padding(0.7)
                )
                .overlay(
                    TopRoundedShape(radius: 26)
                        .stroke(isDark ? Color.black.opacity(0.55) : Color(white: 0.81), lineWidth: 0.6)
                )

                // Render standalone iOS 26 floating squircle bubble ABOVE the tray & candidate rim!
                if expandedCandidates == nil, let pk = pressedKey {
                    let bw: CGFloat = max(61.33, pk.frame.width * 1.67)
                    let bh: CGFloat = max(65.0, pk.frame.height * 1.44)
                    let clampedMidX = min(max(4.0 + bw * 0.5, pk.frame.midX), snapshot.bounds.width - 4.0 - bw * 0.5)
                    let bubbleMidY = 52.5 + pk.frame.minY - 3.67 - bh * 0.5
                    let fgColor = isDark ? Color.white : Color.black

                    RoundedRectangle(cornerRadius: 13.5, style: .continuous)
                        .fill(isDark ? Color(red: 0.34, green: 0.34, blue: 0.37) : Color.white)
                        .shadow(color: .black.opacity(isDark ? 0.38 : 0.13), radius: 15, x: 0, y: 4.0)
                        .overlay(
                            RoundedRectangle(cornerRadius: 13.5, style: .continuous)
                                .strokeBorder(isDark ? Color.white.opacity(0.14) : Color.black.opacity(0.04), lineWidth: 0.4)
                        )
                        .overlay(
                            Text(pk.primaryLabel)
                                .font(.system(size: 36.5, weight: .regular))
                                .foregroundStyle(fgColor)
                                .offset(y: -1.0)
                        )
                        .frame(width: bw, height: bh)
                        .position(x: clampedMidX, y: bubbleMidY)
                }
            }
        }
        .background(appSurface)
    }
}

struct IOS26KeyCanvasPreview: View {
    let snapshot: IOSKeyboardLayoutEngine.LayoutSnapshot
    let isDark: Bool
    let pressedKey: IOSKeySpec?

    var body: some View {
        let keyFill = isDark ? Color(red: 0.28, green: 0.28, blue: 0.30) : Color.white
        let pressedFill = isDark
            ? Color(red: 0.20, green: 0.20, blue: 0.22)
            : Color(red: 219.0 / 255.0, green: 220.0 / 255.0, blue: 224.0 / 255.0)
        let fgColor = isDark ? Color.white : Color.black
        let subColor = isDark ? Color.white.opacity(0.40) : Color(red: 0.64, green: 0.65, blue: 0.67)

        ZStack(alignment: .topLeading) {
            ForEach(snapshot.keys) { spec in
                let isPressed = (spec.id == pressedKey?.id)
                let fill = spec.isBlueAccent ? Color.blue : (isPressed ? pressedFill : keyFill)

                RoundedRectangle(cornerRadius: 8.5, style: .continuous)
                    .fill(fill)
                    .shadow(color: .black.opacity(isPressed ? 0.0 : (isDark ? 0.24 : 0.035)), radius: 0.5, x: 0, y: 0.5)
                    .frame(width: spec.frame.width, height: spec.frame.height)
                    .overlay {
                        ZStack {
                            if let sym = spec.systemSymbol {
                                Image(systemName: sym)
                                    .environment(\.colorScheme, .light)
                                    .font(.system(size: 18.5, weight: .regular))
                                    .foregroundStyle(spec.isBlueAccent ? .white : fgColor)
                            } else if !spec.primaryLabel.isEmpty {
                                Text(spec.primaryLabel)
                                    .font(.system(
                                        size: isLetter(spec.kind) ? 23.5 : 16.5,
                                        weight: spec.isBlueAccent ? .semibold : .regular
                                    ))
                                    .foregroundStyle(spec.isBlueAccent ? .white : fgColor)
                                    .offset(y: isLetter(spec.kind) ? -1.2 : 0)
                            }

                            if let sub = spec.cornerSubscript {
                                VStack {
                                    Spacer()
                                    HStack {
                                        Spacer()
                                        Text(sub)
                                            .font(.system(size: 11.5, weight: .regular))
                                            .foregroundStyle(subColor)
                                            .padding(.trailing, 7.5)
                                            .padding(.bottom, 4.5)
                                    }
                                }
                            }
                        }
                    }
                    .position(x: spec.frame.midX, y: spec.frame.midY)
            }
        }
    }

    private func isLetter(_ k: IOSKeyKind) -> Bool {
        if case .letter = k { return true }
        return false
    }
}

struct IOS26ExpandedGridPreview: View {
    let candidates: [JuxuCandidateItem]
    let isDark: Bool

    var body: some View {
        let cellBg = isDark ? Color(red: 0.28, green: 0.28, blue: 0.30) : Color.white
        HStack(spacing: 8) {
            VStack(spacing: 8) {
                ForEach(0..<4, id: \.self) { row in
                    HStack(spacing: 7) {
                        ForEach(0..<3, id: \.self) { col in
                            let idx = row * 3 + col
                            if idx < candidates.count {
                                let item = candidates[idx]
                                VStack(spacing: 1.5) {
                                    Text(item.text)
                                        .font(.system(size: 17.5, weight: .regular))
                                    if !item.comment.isEmpty {
                                        Text(item.comment)
                                            .font(.system(size: 10, design: .monospaced))
                                            .foregroundStyle(.secondary)
                                            .lineLimit(1)
                                    }
                                }
                                .frame(maxWidth: .infinity, maxHeight: .infinity)
                                .background(cellBg, in: RoundedRectangle(cornerRadius: 8.5, style: .continuous))
                                .shadow(color: .black.opacity(0.05), radius: 1, x: 0, y: 0.7)
                            }
                        }
                    }
                }
            }

            VStack(spacing: 8) {
                railKey(icon: "chevron.up", text: nil, bg: cellBg)
                railKey(icon: "delete.backward", text: nil, bg: cellBg)
                railKey(icon: nil, text: "重输", bg: cellBg)
            }
            .frame(width: 50)
        }
        .padding(.horizontal, 7)
        .padding(.top, 5)
        .padding(.bottom, 8)
    }

    private func railKey(icon: String?, text: String?, bg: Color) -> some View {
        ZStack {
            RoundedRectangle(cornerRadius: 8.5, style: .continuous)
                .fill(bg)
                .shadow(color: .black.opacity(0.05), radius: 1, x: 0, y: 0.7)
            if let icon {
                Image(systemName: icon)
                    .font(.system(size: 16.5, weight: .regular))
            } else if let text {
                Text(text)
                    .font(.system(size: 14, weight: .regular))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct TopRoundedShape: Shape {
    let radius: CGFloat

    func path(in rect: CGRect) -> Path {
        var p = Path()
        let r = min(radius, rect.width * 0.5)
        p.move(to: CGPoint(x: 0, y: rect.maxY))
        p.addLine(to: CGPoint(x: 0, y: r))
        p.addQuadCurve(to: CGPoint(x: r, y: 0), control: CGPoint(x: 0, y: 0))
        p.addLine(to: CGPoint(x: rect.maxX - r, y: 0))
        p.addQuadCurve(to: CGPoint(x: rect.maxX, y: r), control: CGPoint(x: rect.maxX, y: 0))
        p.addLine(to: CGPoint(x: rect.maxX, y: rect.maxY))
        p.closeSubpath()
        return p
    }
}

struct IOS26NeckedCalloutShape: Shape {
    let baseWidth: CGFloat

    func path(in rect: CGRect) -> Path {
        let w = rect.width
        let h = rect.height
        let baseLeft = (w - baseWidth) * 0.5
        let baseRight = baseLeft + baseWidth
        let neckY: CGFloat = 54
        let rTop: CGFloat = 12.0
        let rBot: CGFloat = 8.5

        var p = Path()
        p.move(to: CGPoint(x: rTop + 2, y: 2))
        p.addLine(to: CGPoint(x: w - rTop - 2, y: 2))
        p.addQuadCurve(to: CGPoint(x: w - 2, y: rTop + 2), control: CGPoint(x: w - 2, y: 2))
        p.addLine(to: CGPoint(x: w - 2, y: neckY - 12))
        p.addCurve(
            to: CGPoint(x: baseRight, y: neckY + 4),
            control1: CGPoint(x: w - 2, y: neckY - 3),
            control2: CGPoint(x: baseRight, y: neckY - 2)
        )
        p.addLine(to: CGPoint(x: baseRight, y: h - rBot))
        p.addQuadCurve(to: CGPoint(x: baseRight - rBot, y: h), control: CGPoint(x: baseRight, y: h))
        p.addLine(to: CGPoint(x: baseLeft + rBot, y: h))
        p.addQuadCurve(to: CGPoint(x: baseLeft, y: h - rBot), control: CGPoint(x: baseLeft, y: h))
        p.addLine(to: CGPoint(x: baseLeft, y: neckY + 4))
        p.addCurve(
            to: CGPoint(x: 2, y: neckY - 12),
            control1: CGPoint(x: baseLeft, y: neckY - 2),
            control2: CGPoint(x: 2, y: neckY - 3)
        )
        p.addLine(to: CGPoint(x: 2, y: rTop + 2))
        p.addQuadCurve(to: CGPoint(x: rTop + 2, y: 2), control: CGPoint(x: 2, y: 2))
        p.closeSubpath()
        return p
    }
}
