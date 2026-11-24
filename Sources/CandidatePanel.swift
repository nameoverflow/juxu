import AppKit
import QuartzCore
import SwiftUI

/// Pure AppKit macOS native candidate view calibrated to Apple InputMethodKit (`16.plist` / `SCIM.app`):
/// - Strictly single-line CoreText/`NSAttributedString` measurement & drawing: NEVER wraps onto multiple lines.
/// - No redundant preedit/schema header box (preedit is already marked inline at the cursor like Apple SCIM).
/// - Clean `30pt` horizontal bar (`24pt` selection rect, `5pt` continuous corner radius, `16pt` PingFang SC Regular,
///   `12pt` guide digits, whisper-light `11.5pt` secondary code annotations without colored pills).
public final class NativeCandidateContentView: NSView {
    public var state: JuxuActionState = .empty {
        didSet {
            hoveredCandidateIndex = nil
            layoutValid = false
            invalidateIntrinsicContentSize()
            needsLayout = true
            needsDisplay = true
        }
    }
    public var maxAllowedWidth: CGFloat = 1200 {
        didSet {
            if maxAllowedWidth != oldValue {
                layoutValid = false
                invalidateIntrinsicContentSize()
                needsLayout = true
                needsDisplay = true
            }
        }
    }
    public var onSelectCandidate: ((Int) -> Void)?
    public var onTurnPage: ((Int32) -> Void)?

    private struct CellLayout {
        let relIndex: Int
        let rect: NSRect
        let numberAttr: NSAttributedString
        let numberOrigin: NSPoint
        let textAttr: NSAttributedString
        let textOrigin: NSPoint
        let noteAttr: NSAttributedString?
        let noteOrigin: NSPoint
    }

    private var cellLayouts: [CellLayout] = []
    private var prevPageRect: NSRect = .zero
    private var nextPageRect: NSRect = .zero
    private var dividerRect: NSRect = .zero
    private var hoveredCandidateIndex: Int?
    private var trackingAreaRef: NSTrackingArea?
    private var layoutValid: Bool = false
    private var cachedFittingSize: NSSize = .zero

    public override var isFlipped: Bool { true }

    public override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
    }

    public required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    public override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let existing = trackingAreaRef {
            removeTrackingArea(existing)
        }
        let area = NSTrackingArea(
            rect: bounds,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(area)
        trackingAreaRef = area
    }

    // MARK: - Exact Single-Line Layout Computation (Zero Line-Wrapping Guarantee)

    public func computeFittingSize() -> NSSize {
        if !layoutValid {
            if state.vertical {
                cachedFittingSize = computeVerticalLayout(apply: true)
            } else {
                cachedFittingSize = computeHorizontalLayout(apply: true)
            }
            layoutValid = true
        }
        return cachedFittingSize
    }

    public override var intrinsicContentSize: NSSize {
        computeFittingSize()
    }

    public override func layout() {
        super.layout()
        _ = computeFittingSize()
    }

    private static let badgeAnnotationMarkers = ["⚡", "⌖", "⟲", "⇄", "⎵"]

    private static func stripBadgeMarkers(_ s: String) -> String {
        var cleaned = s
        for marker in badgeAnnotationMarkers {
            cleaned = cleaned.replacingOccurrences(of: marker, with: "")
        }
        return cleaned.trimmingCharacters(in: .whitespaces)
    }

    private static func cleanAnnotation(for item: JuxuCandidateItem) -> String {
        guard !item.comment.isEmpty else { return "" }
        switch item.badgeKind {
        case .shortcode:
            return stripBadgeMarkers(item.comment)
        case .typoCorrected:
            // Keep only the rescued code (strip confidence percentage & badge symbols -> e.g. "trwu")
            let raw = item.comment.components(separatedBy: " ").first ?? item.comment
            return stripBadgeMarkers(raw)
        case .pinyinLookup, .wildcard:
            return stripBadgeMarkers(item.comment)
        case .userPhrase, .emoji, .none:
            return ""
        }
    }

    private func makeSingleLineTruncated(
        _ text: String,
        font: NSFont,
        color: NSColor,
        maxWidth: CGFloat
    ) -> NSAttributedString {
        let style = NSMutableParagraphStyle()
        style.lineBreakMode = .byTruncatingTail
        style.maximumLineHeight = ceil(font.ascender - font.descender) + 2
        let str = text.replacingOccurrences(of: "\n", with: " ")
        let baseAttrs: [NSAttributedString.Key: Any] = [
            .font: font,
            .foregroundColor: color,
            .paragraphStyle: style,
        ]
        let fullAttr = NSAttributedString(string: str, attributes: baseAttrs)
        let chars = Array(str)
        if fullAttr.size().width <= maxWidth || chars.count <= 1 {
            return fullAttr
        }

        var low = 1
        var high = chars.count - 1
        var bestAttr = NSAttributedString(string: String(chars[0]) + "…", attributes: baseAttrs)
        while low <= high {
            let mid = (low + high) / 2
            let candidate = NSAttributedString(
                string: String(chars[..<mid]) + "…",
                attributes: baseAttrs
            )
            if candidate.size().width <= maxWidth {
                bestAttr = candidate
                low = mid + 1
            } else {
                high = mid - 1
            }
        }
        return bestAttr
    }

    private func computeHorizontalLayout(apply: Bool) -> NSSize {
        // Apple macOS concentric candidate bar metrics:
        // Total height: 30pt, concentric outer inset: 3pt X / 3pt Y (outer radius 8pt, inner selection radius 5pt, pill height 24pt)
        let barHeight: CGFloat = 30.0
        let outerX: CGFloat = 3.0
        let pillY: CGFloat = 3.0
        let pillH: CGFloat = 24.0
        let cellPadLeft: CGFloat = 7.5
        let cellPadRight: CGFloat = 8.0
        let cellGap: CGFloat = 1.5

        let numFont = NSFont.monospacedDigitSystemFont(ofSize: 12.0, weight: .regular)
        let textFont = NSFont.systemFont(ofSize: 16.0, weight: .regular)
        let noteFont = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)

        var layouts: [CellLayout] = []
        var cursorX = outerX

        let hasPager = state.totalPages > 1
        let pagerReserve: CGFloat = hasPager ? 42.0 : 0.0
        let maxSingleTextW = max(96.0, (maxAllowedWidth - pagerReserve - outerX * 2.0) * 0.45)

        if state.candidates.isEmpty {
            let emptyAttr = NSAttributedString(
                string: state.preedit,
                attributes: [
                    .font: textFont,
                    .foregroundColor: NSColor.secondaryLabelColor,
                ]
            )
            let sz = emptyAttr.size()
            let w = max(56.0, ceil(outerX * 2.0 + cellPadLeft + sz.width + cellPadRight))
            if apply {
                cellLayouts = [
                    CellLayout(
                        relIndex: -1,
                        rect: NSRect(x: outerX, y: pillY, width: w - outerX * 2.0, height: pillH),
                        numberAttr: NSAttributedString(string: ""),
                        numberOrigin: .zero,
                        textAttr: emptyAttr,
                        textOrigin: NSPoint(x: outerX + cellPadLeft, y: round((barHeight - sz.height) * 0.5)),
                        noteAttr: nil,
                        noteOrigin: .zero
                    )
                ]
                dividerRect = .zero
                prevPageRect = .zero
                nextPageRect = .zero
            }
            return NSSize(width: w, height: barHeight)
        }

        for (idx, item) in state.candidates.enumerated() {
            let isSelected = (idx == 0)
            let numColor: NSColor = isSelected ? NSColor.white.withAlphaComponent(0.84) : NSColor.tertiaryLabelColor
            let txtColor: NSColor = isSelected ? NSColor.white : NSColor.labelColor
            let noteColor: NSColor = isSelected ? NSColor.white.withAlphaComponent(0.76) : NSColor.secondaryLabelColor

            let numAttr = NSAttributedString(
                string: "\(item.index)",
                attributes: [.font: numFont, .foregroundColor: numColor]
            )
            let txtAttr = makeSingleLineTruncated(item.text, font: textFont, color: txtColor, maxWidth: maxSingleTextW)

            let cleanNote = Self.cleanAnnotation(for: item)
            let noteAttr: NSAttributedString? = cleanNote.isEmpty
                ? nil
                : NSAttributedString(string: cleanNote, attributes: [.font: noteFont, .foregroundColor: noteColor])

            let numSize = numAttr.size()
            let txtSize = txtAttr.size()
            let noteSize = noteAttr?.size() ?? .zero

            let numToTxtGap: CGFloat = 4.0
            let txtToNoteGap: CGFloat = noteAttr == nil ? 0.0 : 4.0

            let contentW = numSize.width + numToTxtGap + txtSize.width + txtToNoteGap + noteSize.width
            let cellW = ceil(cellPadLeft + contentW + cellPadRight)

            // Stop adding cells on the horizontal row if we would overflow the screen width (never wrap!)
            if idx > 0 && (cursorX + cellW + pagerReserve + outerX > maxAllowedWidth) {
                break
            }

            let cellRect = NSRect(x: cursorX, y: pillY, width: cellW, height: pillH)
            let txtY = (barHeight - txtSize.height) * 0.5 - 0.5
            // Align baselines of index digit and annotation with the 16pt PingFang SC candidate text
            let baselineY = txtY + textFont.ascender
            let numY = baselineY - numFont.ascender
            let noteY = baselineY - noteFont.ascender

            let numX = cursorX + cellPadLeft
            let txtX = numX + numSize.width + numToTxtGap
            let noteX = txtX + txtSize.width + txtToNoteGap

            layouts.append(
                CellLayout(
                    relIndex: idx,
                    rect: cellRect,
                    numberAttr: numAttr,
                    numberOrigin: NSPoint(x: numX, y: numY),
                    textAttr: txtAttr,
                    textOrigin: NSPoint(x: txtX, y: txtY),
                    noteAttr: noteAttr,
                    noteOrigin: NSPoint(x: noteX, y: noteY)
                )
            )
            cursorX += cellW + cellGap
        }

        cursorX -= cellGap

        var divR: NSRect = .zero
        var prevR: NSRect = .zero
        var nextR: NSRect = .zero

        if hasPager {
            cursorX += 3.5
            divR = NSRect(x: cursorX, y: 8.0, width: 0.5, height: 14.0)
            cursorX += 3.0
            prevR = NSRect(x: cursorX, y: pillY, width: 15.0, height: pillH)
            cursorX += 15.0
            nextR = NSRect(x: cursorX, y: pillY, width: 15.0, height: pillH)
            cursorX += 15.5
        }

        let totalW = ceil(cursorX + outerX)
        if apply {
            cellLayouts = layouts
            dividerRect = divR
            prevPageRect = prevR
            nextPageRect = nextR
        }
        return NSSize(width: max(64.0, totalW), height: barHeight)
    }

    private func computeVerticalLayout(apply: Bool) -> NSSize {
        let outerX: CGFloat = 3.5
        let outerY: CGFloat = 3.5
        let rowH: CGFloat = 24.0
        let rowGap: CGFloat = 1.0
        let cellPadLeft: CGFloat = 8.0
        let cellPadRight: CGFloat = 10.0

        let numFont = NSFont.monospacedDigitSystemFont(ofSize: 12.0, weight: .regular)
        let textFont = NSFont.systemFont(ofSize: 16.0, weight: .regular)
        let noteFont = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)

        if state.candidates.isEmpty {
            return computeHorizontalLayout(apply: apply)
        }

        var rawItems: [(num: NSAttributedString, txt: NSAttributedString, note: NSAttributedString?, w: CGFloat)] = []
        var maxRowW: CGFloat = 136.0
        let maxTextW = max(120.0, maxAllowedWidth * 0.5)

        for (idx, item) in state.candidates.enumerated() {
            let isSelected = (idx == 0)
            let numColor: NSColor = isSelected ? NSColor.white.withAlphaComponent(0.84) : NSColor.tertiaryLabelColor
            let txtColor: NSColor = isSelected ? NSColor.white : NSColor.labelColor
            let noteColor: NSColor = isSelected ? NSColor.white.withAlphaComponent(0.76) : NSColor.secondaryLabelColor

            let numAttr = NSAttributedString(string: "\(item.index)", attributes: [.font: numFont, .foregroundColor: numColor])
            let txtAttr = makeSingleLineTruncated(item.text, font: textFont, color: txtColor, maxWidth: maxTextW)
            let cleanNote = Self.cleanAnnotation(for: item)
            let noteAttr: NSAttributedString? = cleanNote.isEmpty
                ? nil
                : NSAttributedString(string: cleanNote, attributes: [.font: noteFont, .foregroundColor: noteColor])

            let noteExtra = noteAttr == nil ? 0.0 : (12.0 + noteAttr!.size().width)
            let rowW = ceil(cellPadLeft + 14.0 + 5.0 + txtAttr.size().width + noteExtra + cellPadRight)
            if rowW > maxRowW { maxRowW = rowW }
            rawItems.append((numAttr, txtAttr, noteAttr, rowW))
        }

        let totalW = ceil(maxRowW + outerX * 2.0)
        var layouts: [CellLayout] = []
        var cursorY = outerY

        for (idx, entry) in rawItems.enumerated() {
            let rect = NSRect(x: outerX, y: cursorY, width: maxRowW, height: rowH)
            let txtSize = entry.txt.size()
            let numSize = entry.num.size()
            let txtY = cursorY + (rowH - txtSize.height) * 0.5 - 0.5
            let baselineY = txtY + textFont.ascender
            let numY = baselineY - numFont.ascender

            let numX = outerX + cellPadLeft + (12.0 - numSize.width)
            let txtX = outerX + cellPadLeft + 17.0

            var noteOrigin: NSPoint = .zero
            if let note = entry.note {
                let nSize = note.size()
                noteOrigin = NSPoint(
                    x: rect.maxX - cellPadRight - nSize.width,
                    y: baselineY - noteFont.ascender
                )
            }

            layouts.append(
                CellLayout(
                    relIndex: idx,
                    rect: rect,
                    numberAttr: entry.num,
                    numberOrigin: NSPoint(x: numX, y: numY),
                    textAttr: entry.txt,
                    textOrigin: NSPoint(x: txtX, y: txtY),
                    noteAttr: entry.note,
                    noteOrigin: noteOrigin
                )
            )
            cursorY += rowH + rowGap
        }
        cursorY -= rowGap

        var divR: NSRect = .zero
        var prevR: NSRect = .zero
        var nextR: NSRect = .zero
        if state.totalPages > 1 {
            cursorY += 3.0
            divR = NSRect(x: outerX + 4.0, y: cursorY, width: maxRowW - 8.0, height: 0.5)
            cursorY += 2.5
            prevR = NSRect(x: totalW - outerX - 34.0, y: cursorY, width: 16.0, height: 18.0)
            nextR = NSRect(x: totalW - outerX - 17.0, y: cursorY, width: 16.0, height: 18.0)
            cursorY += 18.0
        }

        let totalH = ceil(cursorY + outerY)
        if apply {
            cellLayouts = layouts
            dividerRect = divR
            prevPageRect = prevR
            nextPageRect = nextR
        }
        return NSSize(width: totalW, height: totalH)
    }

    // MARK: - Crisp Retina Drawing

    public override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)

        for cell in cellLayouts {
            if cell.relIndex == 0 {
                let path = NSBezierPath(roundedRect: cell.rect, xRadius: 5.0, yRadius: 5.0)
                NSColor.controlAccentColor.setFill()
                path.fill()
            } else if cell.relIndex >= 0 && hoveredCandidateIndex == cell.relIndex {
                let path = NSBezierPath(roundedRect: cell.rect, xRadius: 5.0, yRadius: 5.0)
                NSColor.labelColor.withAlphaComponent(0.07).setFill()
                path.fill()
            }

            if !cell.numberAttr.string.isEmpty {
                cell.numberAttr.draw(at: cell.numberOrigin)
            }
            cell.textAttr.draw(at: cell.textOrigin)
            if let note = cell.noteAttr {
                note.draw(at: cell.noteOrigin)
            }
        }

        if dividerRect != .zero {
            NSColor.separatorColor.setFill()
            dividerRect.fill()
        }

        if prevPageRect != .zero {
            drawChevron(up: true, in: prevPageRect, enabled: state.page > 0)
        }
        if nextPageRect != .zero {
            drawChevron(up: false, in: nextPageRect, enabled: state.page + 1 < state.totalPages)
        }

        // Hairline native inner border matching macOS NSMenu / SCIM candidate bar
        let isDark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        let borderColor = isDark
            ? NSColor.white.withAlphaComponent(0.16)
            : NSColor.black.withAlphaComponent(0.11)
        let borderPath = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.25, dy: 0.25), xRadius: 8.0, yRadius: 8.0)
        borderPath.lineWidth = 0.5
        borderColor.setStroke()
        borderPath.stroke()
    }

    private func drawChevron(up: Bool, in rect: NSRect, enabled: Bool) {
        let color = enabled ? NSColor.secondaryLabelColor : NSColor.quaternaryLabelColor
        let path = NSBezierPath()
        let cx = rect.midX
        let cy = rect.midY
        let halfW: CGFloat = 3.4
        let halfH: CGFloat = 1.9
        if up {
            path.move(to: NSPoint(x: cx - halfW, y: cy + halfH))
            path.line(to: NSPoint(x: cx, y: cy - halfH))
            path.line(to: NSPoint(x: cx + halfW, y: cy + halfH))
        } else {
            path.move(to: NSPoint(x: cx - halfW, y: cy - halfH))
            path.line(to: NSPoint(x: cx, y: cy + halfH))
            path.line(to: NSPoint(x: cx + halfW, y: cy - halfH))
        }
        path.lineWidth = 1.25
        path.lineCapStyle = .round
        path.lineJoinStyle = .round
        color.setStroke()
        path.stroke()
    }

    // MARK: - Mouse Interaction

    public override func mouseMoved(with event: NSEvent) {
        let pt = convert(event.locationInWindow, from: nil)
        let hit = cellLayouts.first(where: { $0.relIndex >= 0 && $0.rect.contains(pt) })?.relIndex
        if hit != hoveredCandidateIndex {
            hoveredCandidateIndex = hit
            needsDisplay = true
        }
    }

    public override func mouseExited(with event: NSEvent) {
        if hoveredCandidateIndex != nil {
            hoveredCandidateIndex = nil
            needsDisplay = true
        }
    }

    public override func mouseDown(with event: NSEvent) {
        let pt = convert(event.locationInWindow, from: nil)
        if let cell = cellLayouts.first(where: { $0.relIndex >= 0 && $0.rect.contains(pt) }) {
            onSelectCandidate?(cell.relIndex)
            return
        }
        if prevPageRect.contains(pt), state.page > 0 {
            onTurnPage?(-1)
            return
        }
        if nextPageRect.contains(pt), state.page + 1 < state.totalPages {
            onTurnPage?(1)
            return
        }
    }
}

/// SwiftUI wrapper around `NativeCandidateContentView` + `NSVisualEffectView` solely for Settings / Retina preview rendering.
public struct CandidateBarView: NSViewRepresentable {
    public let state: JuxuActionState
    public var onSelectCandidate: ((Int) -> Void)?
    public var onTurnPage: ((Int32) -> Void)?

    public init(
        state: JuxuActionState,
        onSelectCandidate: ((Int) -> Void)? = nil,
        onTurnPage: ((Int32) -> Void)? = nil
    ) {
        self.state = state
        self.onSelectCandidate = onSelectCandidate
        self.onTurnPage = onTurnPage
    }

    public func makeNSView(context: Context) -> NSVisualEffectView {
        let effect = NSVisualEffectView()
        effect.material = .popover
        effect.blendingMode = .withinWindow
        effect.state = .active
        effect.wantsLayer = true
        effect.layer?.cornerRadius = 8.0
        effect.layer?.cornerCurve = .continuous
        effect.layer?.masksToBounds = true

        let content = NativeCandidateContentView()
        content.translatesAutoresizingMaskIntoConstraints = false
        effect.addSubview(content)
        NSLayoutConstraint.activate([
            content.leadingAnchor.constraint(equalTo: effect.leadingAnchor),
            content.trailingAnchor.constraint(equalTo: effect.trailingAnchor),
            content.topAnchor.constraint(equalTo: effect.topAnchor),
            content.bottomAnchor.constraint(equalTo: effect.bottomAnchor),
        ])
        return effect
    }

    public func updateNSView(_ effect: NSVisualEffectView, context: Context) {
        guard let content = effect.subviews.first as? NativeCandidateContentView else { return }
        if content.state != state {
            content.state = state
        }
        content.onSelectCandidate = onSelectCandidate
        content.onTurnPage = onTurnPage
        content.layout()
    }

    public func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSVisualEffectView, context: Context) -> CGSize? {
        guard let content = nsView.subviews.first as? NativeCandidateContentView else { return nil }
        if content.state != state {
            content.state = state
        }
        return content.computeFittingSize()
    }
}

@MainActor
public final class CandidatePanelController {
    public static let shared = CandidatePanelController()

    private let panel: FloatingHUDPanel
    private let effectView: NSVisualEffectView
    private let candidateView: NativeCandidateContentView

    public var onSelect: ((Int) -> Void)? {
        didSet { candidateView.onSelectCandidate = onSelect }
    }
    public var onPage: ((Int32) -> Void)? {
        didSet { candidateView.onTurnPage = onPage }
    }

    private init() {
        let initialRect = NSRect(x: 0, y: 0, width: 200, height: 32)
        let effect = FloatingHUDPanel.makePopoverEffectView(frame: initialRect, cornerRadius: 8.0)

        let content = NativeCandidateContentView(frame: effect.bounds)
        content.autoresizingMask = [.width, .height]
        effect.addSubview(content)

        let p = FloatingHUDPanel(contentRect: initialRect, hasShadow: true, ignoresMouseEvents: false)
        p.contentView = effect

        self.effectView = effect
        self.candidateView = content
        self.panel = p
    }

    public func update(state: JuxuActionState, caretRect: NSRect, clientWindowLevel: CGWindowLevel? = nil) {
        if state.preedit.isEmpty || (state.candidates.isEmpty && !state.segmentedCode.isEmpty) {
            hide()
            return
        }

        let visible = FloatingHUDPanel.visibleScreenFrame(for: caretRect)
        candidateView.maxAllowedWidth = max(240.0, visible.width - 24.0)
        candidateView.state = state
        let size = candidateView.computeFittingSize()

        let origin = panel.resolvedOrigin(
            for: size,
            caretRect: caretRect,
            visibleFrame: visible,
            verticalGap: 5.0,
            minBottomMargin: 6.0,
            horizontalMargin: 8.0
        )
        panel.updateLevel(for: clientWindowLevel)

        let targetFrame = NSRect(origin: origin, size: size)
        panel.presentWithoutAnimation(frame: targetFrame) {
            candidateView.frame = NSRect(origin: .zero, size: size)
            candidateView.layout()
            candidateView.displayIfNeeded()
        }
    }

    public func hide() {
        panel.hideWithoutAnimation()
    }
}

private final class ModeIndicatorContentView: NSView {
    var label: String = "中" {
        didSet {
            if label != oldValue {
                needsDisplay = true
            }
        }
    }

    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        let font = NSFont(name: "PingFangSC-Medium", size: 14.0)
            ?? NSFont.systemFont(ofSize: 14.0, weight: .medium)
        let attrs: [NSAttributedString.Key: Any] = [
            .font: font,
            .foregroundColor: NSColor.labelColor,
        ]
        let attr = NSAttributedString(string: label, attributes: attrs)
        let textSize = attr.size()
        let origin = NSPoint(
            x: round((bounds.width - textSize.width) * 0.5),
            y: round((bounds.height - textSize.height) * 0.5)
        )
        attr.draw(at: origin)

        let isDark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        let borderColor = isDark
            ? NSColor.white.withAlphaComponent(0.16)
            : NSColor.black.withAlphaComponent(0.11)
        let borderPath = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.25, dy: 0.25), xRadius: 7.0, yRadius: 7.0)
        borderPath.lineWidth = 0.5
        borderColor.setStroke()
        borderPath.stroke()
    }
}

@MainActor
public final class ModeIndicatorPanelController {
    public static let shared = ModeIndicatorPanelController()

    private let panel: FloatingHUDPanel
    private let contentView: ModeIndicatorContentView
    private var hideWorkItem: DispatchWorkItem?
    private let panelSize = NSSize(width: 32, height: 28)

    private init() {
        let frame = NSRect(origin: .zero, size: panelSize)
        let effect = FloatingHUDPanel.makePopoverEffectView(frame: frame, cornerRadius: 7.0)

        let content = ModeIndicatorContentView(frame: frame)
        content.autoresizingMask = [.width, .height]
        effect.addSubview(content)

        let p = FloatingHUDPanel(contentRect: frame, hasShadow: true, ignoresMouseEvents: true)
        p.contentView = effect

        self.contentView = content
        self.panel = p
    }

    public func show(asciiMode: Bool, caretRect: NSRect, clientWindowLevel: CGWindowLevel? = nil) {
        hideWorkItem?.cancel()

        let origin = panel.resolvedOrigin(
            for: panelSize,
            caretRect: caretRect,
            verticalGap: 6.0,
            minBottomMargin: 6.0,
            horizontalMargin: 8.0
        )
        panel.updateLevel(for: clientWindowLevel)

        contentView.label = asciiMode ? "英" : "中"
        let targetFrame = NSRect(origin: origin, size: panelSize)
        panel.presentWithoutAnimation(frame: targetFrame) {
            contentView.displayIfNeeded()
        }

        let item = DispatchWorkItem { [weak self] in
            self?.hide()
        }
        hideWorkItem = item
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.9, execute: item)
    }

    public func hide() {
        hideWorkItem?.cancel()
        hideWorkItem = nil
        panel.hideWithoutAnimation()
    }
}
