import AppKit
import QuartzCore

/// Shared borderless, non-activating floating HUD `NSPanel` for candidate bar, mode indicator, and voice HUD.
/// Configured with `.animationBehavior = .none` and `.floatingWindow + 2` so macOS WindowServer never plays
/// pop-up zoom/bounce or fade animations.
@MainActor
public final class FloatingHUDPanel: NSPanel {
    public static var baseWindowLevel: Int {
        Int(CGWindowLevelForKey(.floatingWindow)) + 2
    }

    public init(
        contentRect: NSRect,
        hasShadow: Bool = true,
        ignoresMouseEvents: Bool = false
    ) {
        super.init(
            contentRect: contentRect,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: true
        )
        self.animationBehavior = .none
        self.level = NSWindow.Level(rawValue: Self.baseWindowLevel)
        self.isOpaque = false
        self.backgroundColor = .clear
        self.hasShadow = hasShadow
        self.isMovable = false
        self.ignoresMouseEvents = ignoresMouseEvents
        self.hidesOnDeactivate = false
        self.worksWhenModal = true
        self.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient, .ignoresCycle]
    }

    public static func makePopoverEffectView(
        frame: NSRect,
        cornerRadius: CGFloat
    ) -> NSVisualEffectView {
        let effect = NSVisualEffectView(frame: frame)
        effect.material = .popover
        effect.blendingMode = .behindWindow
        effect.state = .active
        effect.wantsLayer = true
        effect.layer?.cornerRadius = cornerRadius
        effect.layer?.cornerCurve = .continuous
        effect.layer?.masksToBounds = true
        return effect
    }

    public static func visibleScreenFrame(for caretRect: NSRect) -> NSRect {
        let targetScreen = NSScreen.screens.first(where: { $0.frame.intersects(caretRect) })
            ?? NSScreen.main
            ?? NSScreen.screens.first
        return targetScreen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
    }

    public func resolvedOrigin(
        for size: NSSize,
        caretRect: NSRect,
        visibleFrame: NSRect? = nil,
        verticalGap: CGFloat = 5.0,
        minBottomMargin: CGFloat = 6.0,
        horizontalMargin: CGFloat = 8.0
    ) -> NSPoint {
        let visible = visibleFrame ?? Self.visibleScreenFrame(for: caretRect)
        var originX = round(caretRect.minX)
        var originY = round(caretRect.minY - size.height - verticalGap)

        if originY < visible.minY + minBottomMargin {
            originY = round(caretRect.maxY + verticalGap)
        }
        if originX + size.width > visible.maxX - horizontalMargin {
            originX = max(visible.minX + horizontalMargin, round(visible.maxX - size.width - horizontalMargin))
        }
        if originX < visible.minX + horizontalMargin {
            originX = round(visible.minX + horizontalMargin)
        }
        return NSPoint(x: originX, y: originY)
    }

    public func updateLevel(for clientWindowLevel: CGWindowLevel?) {
        let baseLevel = Self.baseWindowLevel
        let targetLevel = clientWindowLevel.map { max(baseLevel, Int($0) + 1) } ?? baseLevel
        if level.rawValue != targetLevel {
            level = NSWindow.Level(rawValue: targetLevel)
        }
    }

    public func presentWithoutAnimation(
        frame targetFrame: NSRect,
        prepareContent: () -> Void = {}
    ) {
        NSAnimationContext.beginGrouping()
        NSAnimationContext.current.duration = 0
        CATransaction.begin()
        CATransaction.setDisableActions(true)

        setFrame(targetFrame, display: false, animate: false)
        prepareContent()
        invalidateShadow()

        if !isVisible {
            orderFrontRegardless()
        }

        CATransaction.commit()
        NSAnimationContext.endGrouping()
    }

    public func hideWithoutAnimation() {
        guard isVisible else { return }
        NSAnimationContext.beginGrouping()
        NSAnimationContext.current.duration = 0
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        orderOut(nil)
        CATransaction.commit()
        NSAnimationContext.endGrouping()
    }
}
