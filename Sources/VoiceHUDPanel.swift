import AppKit
import QuartzCore

public enum VoiceHUDState: Equatable {
    case hidden
    case recording(level: Float, elapsed: TimeInterval, partial: String, modeLabel: String)
    case transcribing(modeLabel: String)
    case polishing(preview: String)
    case notice(text: String, isError: Bool)
}

private final class VoiceHUDContentView: NSView {
    var hudState: VoiceHUDState = .hidden {
        didSet {
            if hudState != oldValue {
                needsDisplay = true
            }
        }
    }

    override var isFlipped: Bool { true }

    func computeFittingSize() -> NSSize {
        let titleFont = NSFont.systemFont(ofSize: 12.5, weight: .medium)
        let subFont = NSFont.systemFont(ofSize: 11.5, weight: .regular)

        switch hudState {
        case .hidden:
            return NSSize(width: 140, height: 32)
        case .recording(_, let elapsed, let partial, let modeLabel):
            let timeStr = String(format: "%.1fs", elapsed)
            let label = partial.isEmpty ? "聆听中 · \(modeLabel) · \(timeStr)" : partial
            let textW = min(280, ceil((label as NSString).size(withAttributes: [.font: titleFont]).width))
            return NSSize(width: max(160, 56 + textW + 16), height: 34)
        case .transcribing(let modeLabel):
            let label = "转写中 · \(modeLabel)…"
            let textW = ceil((label as NSString).size(withAttributes: [.font: titleFont]).width)
            return NSSize(width: max(150, 34 + textW + 16), height: 34)
        case .polishing(let preview):
            let label = preview.isEmpty ? "智能整理中…" : "整理中 · \(preview)"
            let textW = min(280, ceil((label as NSString).size(withAttributes: [.font: subFont]).width))
            return NSSize(width: max(150, 34 + textW + 16), height: 34)
        case .notice(let text, _):
            let textW = min(320, ceil((text as NSString).size(withAttributes: [.font: titleFont]).width))
            return NSSize(width: max(130, 32 + textW + 16), height: 34)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let isDark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        let borderColor = isDark
            ? NSColor.white.withAlphaComponent(0.16)
            : NSColor.black.withAlphaComponent(0.11)
        let borderPath = NSBezierPath(
            roundedRect: bounds.insetBy(dx: 0.25, dy: 0.25),
            xRadius: 10.0,
            yRadius: 10.0
        )
        borderPath.lineWidth = 0.5
        borderColor.setStroke()
        borderPath.stroke()

        let titleFont = NSFont.systemFont(ofSize: 12.5, weight: .medium)
        let style = NSMutableParagraphStyle()
        style.lineBreakMode = .byTruncatingHead

        switch hudState {
        case .hidden:
            return

        case .recording(let level, let elapsed, let partial, let modeLabel):
            // Pulsing red dot
            let dotRect = NSRect(x: 11, y: 13, width: 8, height: 8)
            NSColor.systemRed.setFill()
            NSBezierPath(ovalIn: dotRect).fill()

            // 5 mini waveform bars driven by live RMS level
            let weights: [Float] = [0.55, 0.85, 1.0, 0.78, 0.48]
            for (i, w) in weights.enumerated() {
                let barH = CGFloat(4.0 + min(1.0, level * w) * 14.0)
                let barRect = NSRect(
                    x: 24 + CGFloat(i) * 4.5,
                    y: round((bounds.height - barH) * 0.5),
                    width: 2.5,
                    height: barH
                )
                NSColor.controlAccentColor.withAlphaComponent(0.9).setFill()
                NSBezierPath(roundedRect: barRect, xRadius: 1.25, yRadius: 1.25).fill()
            }

            let text = partial.isEmpty
                ? String(format: "聆听中 · %@ · %.1fs", modeLabel, elapsed)
                : partial
            let attrs: [NSAttributedString.Key: Any] = [
                .font: titleFont,
                .foregroundColor: NSColor.labelColor,
                .paragraphStyle: style,
            ]
            let textRect = NSRect(x: 52, y: 9, width: bounds.width - 64, height: 18)
            (text as NSString).draw(in: textRect, withAttributes: attrs)

        case .transcribing(let modeLabel):
            drawIndicatorDot(color: .systemOrange)
            let attrs: [NSAttributedString.Key: Any] = [
                .font: titleFont,
                .foregroundColor: NSColor.labelColor,
                .paragraphStyle: style,
            ]
            let textRect = NSRect(x: 28, y: 9, width: bounds.width - 40, height: 18)
            ("转写中 · \(modeLabel)…" as NSString).draw(in: textRect, withAttributes: attrs)

        case .polishing(let preview):
            drawIndicatorDot(color: .controlAccentColor)
            let attrs: [NSAttributedString.Key: Any] = [
                .font: titleFont,
                .foregroundColor: NSColor.labelColor,
                .paragraphStyle: style,
            ]
            let label = preview.isEmpty ? "智能整理中…" : "整理中 · \(preview)"
            let textRect = NSRect(x: 28, y: 9, width: bounds.width - 40, height: 18)
            (label as NSString).draw(in: textRect, withAttributes: attrs)

        case .notice(let text, let isError):
            drawIndicatorDot(color: isError ? .systemRed : .systemGreen)
            let attrs: [NSAttributedString.Key: Any] = [
                .font: titleFont,
                .foregroundColor: isError ? NSColor.systemRed : NSColor.labelColor,
                .paragraphStyle: style,
            ]
            let textRect = NSRect(x: 28, y: 9, width: bounds.width - 40, height: 18)
            (text as NSString).draw(in: textRect, withAttributes: attrs)
        }
    }

    private func drawIndicatorDot(color: NSColor) {
        let dotRect = NSRect(x: 12, y: 13, width: 8, height: 8)
        color.setFill()
        NSBezierPath(ovalIn: dotRect).fill()
    }
}

/// Strictly on-demand floating HUD panel for macOS voice dictation.
/// Never allocated until voice input is enabled and triggered; released on `unload()`.
@MainActor
public final class VoiceHUDPanelController {
    private static var _shared: VoiceHUDPanelController?

    public static var isLoaded: Bool { _shared != nil }

    public static var shared: VoiceHUDPanelController {
        if let existing = _shared {
            return existing
        }
        let created = VoiceHUDPanelController()
        _shared = created
        return created
    }

    public static func unload() {
        _shared?.hide()
        _shared?.panel.close()
        _shared = nil
    }

    private let panel: NSPanel
    private let contentView: VoiceHUDContentView
    private var hideWorkItem: DispatchWorkItem?

    private init() {
        let initialFrame = NSRect(x: 0, y: 0, width: 180, height: 34)
        let effect = NSVisualEffectView(frame: initialFrame)
        effect.material = .popover
        effect.blendingMode = .behindWindow
        effect.state = .active
        effect.wantsLayer = true
        effect.layer?.cornerRadius = 10.0
        effect.layer?.cornerCurve = .continuous
        effect.layer?.masksToBounds = true

        let content = VoiceHUDContentView(frame: initialFrame)
        content.autoresizingMask = [.width, .height]
        effect.addSubview(content)

        let p = NSPanel(
            contentRect: initialFrame,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: true
        )
        p.animationBehavior = .none
        p.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.floatingWindow)) + 2)
        p.isOpaque = false
        p.backgroundColor = .clear
        p.hasShadow = true
        p.isMovable = false
        p.ignoresMouseEvents = true
        p.hidesOnDeactivate = false
        p.worksWhenModal = true
        p.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient, .ignoresCycle]
        p.contentView = effect

        self.contentView = content
        self.panel = p
    }

    public func show(
        state: VoiceHUDState,
        caretRect: NSRect,
        clientWindowLevel: CGWindowLevel? = nil,
        autoHideAfter: TimeInterval? = nil
    ) {
        hideWorkItem?.cancel()
        hideWorkItem = nil

        if state == .hidden {
            hide()
            return
        }

        contentView.hudState = state
        let size = contentView.computeFittingSize()

        let targetScreen = NSScreen.screens.first(where: { $0.frame.intersects(caretRect) })
            ?? NSScreen.main
            ?? NSScreen.screens.first
        let visible = targetScreen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)

        var originX = round(caretRect.minX)
        var originY = round(caretRect.minY - size.height - 8.0)

        if originY < visible.minY + 8.0 {
            originY = round(caretRect.maxY + 8.0)
        }
        if originX + size.width > visible.maxX - 10.0 {
            originX = max(visible.minX + 10.0, round(visible.maxX - size.width - 10.0))
        }
        if originX < visible.minX + 10.0 {
            originX = round(visible.minX + 10.0)
        }

        let baseLevel = Int(CGWindowLevelForKey(.floatingWindow)) + 2
        let targetLevel = clientWindowLevel.map { max(baseLevel, Int($0) + 1) } ?? baseLevel
        if panel.level.rawValue != targetLevel {
            panel.level = NSWindow.Level(rawValue: targetLevel)
        }

        let targetFrame = NSRect(origin: NSPoint(x: originX, y: originY), size: size)

        NSAnimationContext.beginGrouping()
        NSAnimationContext.current.duration = 0
        CATransaction.begin()
        CATransaction.setDisableActions(true)

        panel.setFrame(targetFrame, display: false, animate: false)
        contentView.displayIfNeeded()
        panel.invalidateShadow()

        if !panel.isVisible {
            panel.orderFrontRegardless()
        }

        CATransaction.commit()
        NSAnimationContext.endGrouping()

        if let autoHideAfter {
            let item = DispatchWorkItem { [weak self] in
                self?.hide()
            }
            hideWorkItem = item
            DispatchQueue.main.asyncAfter(deadline: .now() + autoHideAfter, execute: item)
        }
    }

    public func hide() {
        hideWorkItem?.cancel()
        hideWorkItem = nil
        guard panel.isVisible else { return }
        NSAnimationContext.beginGrouping()
        NSAnimationContext.current.duration = 0
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        panel.orderOut(nil)
        CATransaction.commit()
        NSAnimationContext.endGrouping()
    }
}
