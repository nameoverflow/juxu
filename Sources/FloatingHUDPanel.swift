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

        // Recomputing the shadow is a WindowServer round trip; skip it while the size stays the same.
        let needsShadowRefresh = !isVisible || frame.size != targetFrame.size
        setFrame(targetFrame, display: false, animate: false)
        prepareContent()
        if needsShadowRefresh {
            invalidateShadow()
        }

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

/// System backdrop for the floating input UI: Liquid Glass on macOS 26 and later, the menu
/// material before. Put the panel's own views in `contentContainer`.
@MainActor
public final class HUDBackdropView: NSView {
    public let contentContainer: NSView
    private let backdrop: NSView

    /// True when the backdrop is Liquid Glass, which draws its own edge and highlight.
    public let usesGlass: Bool

    public var cornerRadius: CGFloat {
        didSet { applyCornerRadius() }
    }

    /// Draws a 0.5pt appearance-aware rim on the menu material, which has no edge of its own.
    /// Liquid Glass draws its own edge, so this does nothing there.
    public var showsHairlineBorder = false {
        didSet { applyBorder() }
    }

    public init(
        frame frameRect: NSRect,
        cornerRadius: CGFloat,
        blendingMode: NSVisualEffectView.BlendingMode = .behindWindow
    ) {
        self.cornerRadius = cornerRadius
        let container = NSView(frame: NSRect(origin: .zero, size: frameRect.size))
        container.autoresizingMask = [.width, .height]
        self.contentContainer = container
        if #available(macOS 26.0, *) {
            let glass = NSGlassEffectView(frame: NSRect(origin: .zero, size: frameRect.size))
            glass.contentView = container
            self.backdrop = glass
            self.usesGlass = true
        } else {
            let effect = NSVisualEffectView(frame: NSRect(origin: .zero, size: frameRect.size))
            effect.material = .menu
            effect.blendingMode = blendingMode
            effect.state = .active
            effect.wantsLayer = true
            effect.addSubview(container)
            self.backdrop = effect
            self.usesGlass = false
        }
        super.init(frame: frameRect)
        wantsLayer = true
        backdrop.autoresizingMask = [.width, .height]
        addSubview(backdrop)
        applyCornerRadius()
    }

    override public func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        applyBorder()
    }

    private func applyBorder() {
        guard !usesGlass, let layer = backdrop.layer else { return }
        guard showsHairlineBorder else {
            layer.borderWidth = 0
            return
        }
        let isDark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        layer.borderWidth = 0.5
        layer.borderColor = (isDark
            ? NSColor.white.withAlphaComponent(0.16)
            : NSColor.black.withAlphaComponent(0.11)).cgColor
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    private func applyCornerRadius() {
        if #available(macOS 26.0, *), let glass = backdrop as? NSGlassEffectView {
            glass.cornerRadius = cornerRadius
        } else {
            backdrop.layer?.cornerRadius = cornerRadius
            backdrop.layer?.cornerCurve = .continuous
            backdrop.layer?.masksToBounds = true
        }
    }
}
