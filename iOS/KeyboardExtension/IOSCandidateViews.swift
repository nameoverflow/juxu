#if os(iOS)
import UIKit

/// Strips duplicated typo/spatial badge glyphs (`⌖`, `⟲`, `⇄`, `⎵`) from candidate comment strings.
private func cleanIOSCandidateAnnotation(for item: JuxuCandidateItem) -> String {
    guard !item.comment.isEmpty else { return "" }
    let raw: String
    switch item.badgeKind {
    case .typoCorrected:
        raw = item.comment.components(separatedBy: " ").first ?? item.comment
    case .pinyinLookup, .shortcode, .wildcard:
        raw = item.comment
    default:
        return ""
    }
    var cleaned = raw
    for sym in ["⌖", "⟲", "⇄", "⎵"] {
        cleaned = cleaned.replacingOccurrences(of: sym, with: "")
    }
    return cleaned.trimmingCharacters(in: .whitespaces)
}

/// Direct `UIControl` + `UILabel` candidate slot (zero `UIButton.Configuration` deferred layout pass).
@MainActor
private final class IOSRibbonSlotControl: UIControl {
    static let horizontalPadding: CGFloat = 12.0

    var slotIndex: Int = 0
    var starterWord: String?
    private(set) var slotWidth: CGFloat = 0
    private let label = UILabel()

    private struct TitleCacheKey: Equatable {
        let primary: String
        let secondary: String
        let fontSize: CGFloat
        let isEmphasized: Bool
        let dark: Bool
    }
    private var cachedKey: TitleCacheKey?

    private static let idleHitBacking = UIColor(white: 0.5, alpha: 0.01)

    override var isHighlighted: Bool {
        didSet {
            backgroundColor = isHighlighted
                ? UIColor.label.withAlphaComponent(0.10)
                : Self.idleHitBacking
        }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = false
        // Non-zero alpha backing prevents iOS cross-process CALayerHost hit-testing from skipping transparent pixels.
        backgroundColor = Self.idleHitBacking
        layer.cornerRadius = 9.0
        layer.cornerCurve = .continuous

        label.numberOfLines = 1
        label.textAlignment = .center
        label.lineBreakMode = .byClipping
        label.isUserInteractionEnabled = false
        addSubview(label)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func setSlotContent(
        primary: String,
        secondary: String = "",
        fontSize: CGFloat,
        isEmphasized: Bool,
        primaryColor: UIColor,
        subtleColor: UIColor,
        dark: Bool
    ) {
        let key = TitleCacheKey(
            primary: primary,
            secondary: secondary,
            fontSize: fontSize,
            isEmphasized: isEmphasized,
            dark: dark
        )
        if cachedKey == key {
            return
        }
        cachedKey = key
        let attr = NSMutableAttributedString(
            string: primary,
            attributes: [
                .font: UIFont.systemFont(ofSize: fontSize, weight: isEmphasized ? .medium : .regular),
                .foregroundColor: primaryColor,
            ]
        )
        if !secondary.isEmpty {
            attr.append(
                NSAttributedString(
                    string: " \(secondary)",
                    attributes: [
                        .font: UIFont.monospacedSystemFont(ofSize: 11.0, weight: .regular),
                        .foregroundColor: subtleColor,
                    ]
                )
            )
        }
        label.attributedText = attr
        slotWidth = ceil(attr.size().width) + Self.horizontalPadding * 2.0
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        label.frame = bounds
    }
}

/// Native iOS 26 centered QuickType / AutoFill style clipboard suggestion pill (pure `UIControl`, zero `UIButton.Configuration`).
@MainActor
private final class IOSClipboardPillControl: UIControl {
    private let iconView = UIImageView()
    private let subtitleLabel = UILabel()
    private let valueLabel = UILabel()

    private var isDarkTheme = false
    private var isCompactLandscape = false
    private var currentSuggestion: IOSClipboardSuggestion?

    private(set) var intrinsicContentWidth: CGFloat = 140

    override var isHighlighted: Bool {
        didSet {
            updateSurfaceColor()
        }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = false
        layer.cornerRadius = 11.0
        layer.cornerCurve = .continuous
        isAccessibilityElement = true
        accessibilityTraits = .button
        accessibilityIdentifier = "JuxuClipboardPill"

        iconView.contentMode = .scaleAspectFit
        iconView.isUserInteractionEnabled = false

        subtitleLabel.numberOfLines = 1
        subtitleLabel.textAlignment = .left
        subtitleLabel.lineBreakMode = .byClipping
        subtitleLabel.isUserInteractionEnabled = false

        valueLabel.numberOfLines = 1
        valueLabel.textAlignment = .center
        valueLabel.lineBreakMode = .byTruncatingTail
        valueLabel.isUserInteractionEnabled = false

        addSubview(iconView)
        addSubview(subtitleLabel)
        addSubview(valueLabel)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func configure(suggestion: IOSClipboardSuggestion, dark: Bool, compactLandscape: Bool) {
        currentSuggestion = suggestion
        isDarkTheme = dark
        isCompactLandscape = compactLandscape

        let textCol: UIColor = dark ? .white : .black
        let subtleCol: UIColor = dark ? UIColor(white: 0.70, alpha: 1.0) : UIColor(white: 0.42, alpha: 1.0)
        let otpTint: UIColor = dark
            ? UIColor(red: 0.40, green: 0.70, blue: 1.0, alpha: 1.0)
            : UIColor.systemBlue

        let symbolName = suggestion.isOTP ? "key.fill" : "doc.on.clipboard"
        let symSize: CGFloat = compactLandscape ? 12.0 : 9.5
        let symCfg = UIImage.SymbolConfiguration(pointSize: symSize, weight: .medium)
        iconView.image = UIImage(systemName: symbolName, withConfiguration: symCfg)?.withRenderingMode(.alwaysTemplate)
        iconView.tintColor = suggestion.isOTP ? otpTint : subtleCol

        subtitleLabel.isHidden = compactLandscape
        subtitleLabel.font = .systemFont(ofSize: 10.5, weight: .regular)
        subtitleLabel.textColor = subtleCol
        subtitleLabel.text = suggestion.subtitle

        if suggestion.isOTP {
            valueLabel.font = .monospacedDigitSystemFont(ofSize: compactLandscape ? 15.5 : 16.5, weight: .semibold)
        } else {
            valueLabel.font = .systemFont(ofSize: compactLandscape ? 14.5 : 15.0, weight: .medium)
        }
        valueLabel.textColor = textCol
        valueLabel.text = suggestion.displayText

        let iconW: CGFloat = compactLandscape ? 14.0 : 11.5
        let valueW = ceil((suggestion.displayText as NSString).size(withAttributes: [.font: valueLabel.font!]).width)
        if compactLandscape {
            intrinsicContentWidth = iconW + 5.0 + valueW + 24.0
        } else {
            let subW = ceil((suggestion.subtitle as NSString).size(withAttributes: [.font: subtitleLabel.font!]).width)
            let topW = iconW + 3.5 + subW
            intrinsicContentWidth = max(topW, valueW) + 28.0
        }

        accessibilityLabel = "\(suggestion.subtitle)：\(suggestion.displayText)"
        updateSurfaceColor()
        setNeedsLayout()
    }

    private func updateSurfaceColor() {
        if isHighlighted {
            backgroundColor = isDarkTheme
                ? UIColor(white: 1.0, alpha: 0.22)
                : UIColor(white: 0.0, alpha: 0.11)
        } else {
            backgroundColor = isDarkTheme
                ? UIColor(white: 1.0, alpha: 0.11)
                : UIColor(white: 0.0, alpha: 0.048)
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let w = bounds.width
        let h = bounds.height
        let padX: CGFloat = 12.0

        if isCompactLandscape {
            let iconSide: CGFloat = 14.0
            let gap: CGFloat = 5.0
            let maxTextW = max(20.0, w - padX * 2.0 - iconSide - gap)
            let textW = min(maxTextW, ceil(valueLabel.sizeThatFits(CGSize(width: maxTextW, height: h)).width))
            let totalW = iconSide + gap + textW
            let startX = floor((w - totalW) * 0.5)
            iconView.frame = CGRect(x: startX, y: floor((h - iconSide) * 0.5), width: iconSide, height: iconSide)
            valueLabel.frame = CGRect(x: startX + iconSide + gap, y: 0, width: textW, height: h)
        } else {
            let iconSide: CGFloat = 11.0
            let gap: CGFloat = 3.5
            let subSize = subtitleLabel.sizeThatFits(CGSize(width: max(20.0, w - padX * 2.0), height: 14.0))
            let subW = min(max(20.0, w - padX * 2.0 - iconSide - gap), ceil(subSize.width))
            let topTotalW = iconSide + gap + subW
            let topStartX = floor((w - topTotalW) * 0.5)
            let topY: CGFloat = 3.5
            iconView.frame = CGRect(x: topStartX, y: topY + 1.0, width: iconSide, height: iconSide)
            subtitleLabel.frame = CGRect(x: topStartX + iconSide + gap, y: topY, width: subW, height: 13.0)

            let valY: CGFloat = 16.5
            valueLabel.frame = CGRect(x: padX, y: valY, width: max(20.0, w - padX * 2.0), height: max(18.0, h - valY - 2.5))
        }
    }
}

/// Cross-process hit-test safe chevron toggle button with single-row visual pill & generous touch bounds.
@MainActor
private final class IOSRibbonChevronButton: UIControl {
    private static let idleHitBacking = UIColor(white: 0.5, alpha: 0.01)
    private let highlightPill = UIView()
    private let iconView = UIImageView()

    var bandTopInset: CGFloat = 4.0 {
        didSet {
            if bandTopInset != oldValue {
                setNeedsLayout()
            }
        }
    }
    var bandBottomInset: CGFloat = 7.0 {
        didSet {
            if bandBottomInset != oldValue {
                setNeedsLayout()
            }
        }
    }

    override var isHighlighted: Bool {
        didSet {
            highlightPill.backgroundColor = isHighlighted
                ? UIColor.label.withAlphaComponent(0.10)
                : .clear
        }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = false
        backgroundColor = Self.idleHitBacking
        isAccessibilityElement = true
        accessibilityTraits = .button

        highlightPill.isUserInteractionEnabled = false
        highlightPill.backgroundColor = .clear
        highlightPill.layer.cornerRadius = 9.0
        highlightPill.layer.cornerCurve = .continuous
        addSubview(highlightPill)

        iconView.contentMode = .center
        iconView.isUserInteractionEnabled = false
        addSubview(iconView)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func setChevronImage(_ image: UIImage?) {
        iconView.image = image?.withRenderingMode(.alwaysTemplate)
    }

    override func tintColorDidChange() {
        super.tintColorDidChange()
        iconView.tintColor = tintColor
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let bandHeight = max(20.0, bounds.height - bandTopInset - bandBottomInset)
        let centerY = bandTopInset + bandHeight * 0.5
        let centerX = bounds.width * 0.5
        let iconSide: CGFloat = 18.0
        iconView.frame = CGRect(
            x: round(centerX - iconSide * 0.5),
            y: round(centerY - iconSide * 0.5),
            width: iconSide,
            height: iconSide
        )
        let pillW = min(36.0, bounds.width - 8.0)
        let pillH = min(34.0, bandHeight - 4.0)
        highlightPill.frame = CGRect(
            x: round(centerX - pillW * 0.5),
            y: round(centerY - pillH * 0.5),
            width: pillW,
            height: pillH
        )
    }

    override func point(inside point: CGPoint, with event: UIEvent?) -> Bool {
        bounds.inset(by: UIEdgeInsets(top: -4, left: -12, bottom: -6, right: -4)).contains(point)
    }
}

/// iOS 26 Native Airy Unboxed Candidate Bar (`53pt` portrait / `40pt` landscape):
/// - Pure `UIView` horizontal track (`clipContainer` + `contentTrack` + horizontal `UIPanGestureRecognizer`)
///   with ZERO `UIScrollView` and ZERO `UIButton.Configuration`, eliminating `_UIScrollPocket` /
///   `UIScrollEdgeEffect` variable blur and scroll-view touch delay.
/// - Passes touches in the bottom `8pt` empty strip directly through to Row 0 (`q..p`).
final class IOSCandidateRibbonView: UIView, UIGestureRecognizerDelegate {
    var onSelectCandidate: ((Int) -> Void)?
    var onSelectStarterWord: ((String) -> Void)?
    var onSelectClipboard: ((IOSClipboardSuggestion) -> Void)?
    var onDismissClipboard: ((IOSClipboardSuggestion) -> Void)?
    var onTapChevron: (() -> Void)?

    private static let visualHorizontalInset: CGFloat = 18.0
    private static let chevronWidth: CGFloat = 48.0

    private let clipContainer = UIView()
    private let contentTrack = UIView()
    private let edgeFadeMask = CAGradientLayer()
    private let chevronDivider = UIView()
    private let chevronBtn = IOSRibbonChevronButton(frame: .zero)
    private let clipboardPill = IOSClipboardPillControl()
    private let clipboardDismissBtn = UIButton(type: .system)
    private var slotControls: [IOSRibbonSlotControl] = []
    private var slotDividers: [UIView] = []

    private var clipTopConstraint: NSLayoutConstraint!
    private var clipBottomConstraint: NSLayoutConstraint!
    private var clipTrailingToChevron: NSLayoutConstraint!
    private var clipTrailingToFull: NSLayoutConstraint!

    private var contentWidth: CGFloat = 0
    private var scrollOffsetX: CGFloat = 0
    private var panStartOffsetX: CGFloat = 0
    private var activeClipboardSuggestion: IOSClipboardSuggestion?
    private var lastClipboardCompactLayout: Bool?

    static let starterWords = ["我", "我们", "不是", "怎么", "你", "在", "这", "可以"]

    override init(frame: CGRect) {
        super.init(frame: frame)
        setupViews()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func isInsideChevronHitZone(_ point: CGPoint) -> Bool {
        guard !chevronBtn.isHidden else { return false }
        let expanded = CGRect(
            x: chevronBtn.frame.minX - 10.0,
            y: -4.0,
            width: bounds.width - (chevronBtn.frame.minX - 10.0) + 4.0,
            height: bounds.height + 10.0
        )
        return expanded.contains(point)
    }

    /// Pass touches in the bottom `8pt` strip (below candidate glyphs) and in the lower half
    /// of unoccupied trailing candidate-bar space straight through to Row 0 (`q..p`),
    /// while preserving full hit area on `chevronBtn`, centered clipboard pill, and eliminating inter-slot dead gaps.
    override func point(inside point: CGPoint, with event: UIEvent?) -> Bool {
        if isInsideChevronHitZone(point) {
            return true
        }
        guard bounds.contains(point) else { return false }
        if point.y > bounds.height - 8.0 {
            return false
        }
        if !clipboardPill.isHidden {
            if !clipboardDismissBtn.isHidden && clipboardDismissBtn.frame.insetBy(dx: -6.0, dy: -4.0).contains(point) {
                return true
            }
            if clipboardPill.frame.insetBy(dx: -10.0, dy: -4.0).contains(point) {
                return true
            }
            // Empty flanks below mid-height pass straight through to Row 0 (`q..p`).
            return point.y < bounds.height * 0.45
        }
        let availW = clipContainer.bounds.width
        let hasScrollableOverflow = contentWidth > availW + 2.0
        if !hasScrollableOverflow {
            let maxVisibleMaxX = slotControls
                .lazy
                .filter { !$0.isHidden }
                .map { $0.frame.maxX - self.scrollOffsetX }
                .max() ?? 0
            if point.x > maxVisibleMaxX + 10.0 && point.y >= bounds.height * 0.45 {
                return false
            }
        }
        return true
    }

    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
        guard !isHidden, alpha >= 0.01, isUserInteractionEnabled else { return nil }
        if isInsideChevronHitZone(point) {
            return chevronBtn
        }
        guard let hit = super.hitTest(point, with: event) else { return nil }
        if !clipboardPill.isHidden {
            if !clipboardDismissBtn.isHidden && clipboardDismissBtn.frame.insetBy(dx: -6.0, dy: -4.0).contains(point) {
                return clipboardDismissBtn
            }
            if clipboardPill.frame.insetBy(dx: -10.0, dy: -4.0).contains(point) {
                return clipboardPill
            }
        }
        if hit === clipContainer || hit === contentTrack || (hit === self && point.y >= 4.0) {
            let trackPt = convert(point, to: contentTrack)
            var nearestSlot: IOSRibbonSlotControl?
            var bestDx = CGFloat.greatestFiniteMagnitude
            for slot in slotControls where !slot.isHidden {
                let dx = max(slot.frame.minX - trackPt.x, 0, trackPt.x - slot.frame.maxX)
                if dx <= 10.0 && dx < bestDx {
                    bestDx = dx
                    nearestSlot = slot
                }
            }
            if let nearestSlot {
                return nearestSlot
            }
        }
        return hit
    }

    private func setupViews() {
        clipContainer.translatesAutoresizingMaskIntoConstraints = false
        clipContainer.clipsToBounds = true
        clipContainer.backgroundColor = .clear

        contentTrack.backgroundColor = .clear
        clipContainer.addSubview(contentTrack)

        edgeFadeMask.colors = [
            UIColor.black.cgColor,
            UIColor.black.cgColor,
            UIColor.clear.cgColor,
        ]
        edgeFadeMask.locations = [0.0, 0.91, 1.0]
        edgeFadeMask.startPoint = CGPoint(x: 0.0, y: 0.5)
        edgeFadeMask.endPoint = CGPoint(x: 1.0, y: 0.5)

        let pan = UIPanGestureRecognizer(target: self, action: #selector(handleHorizontalPan(_:)))
        pan.delegate = self
        pan.cancelsTouchesInView = true
        clipContainer.addGestureRecognizer(pan)

        chevronDivider.isUserInteractionEnabled = false

        chevronBtn.translatesAutoresizingMaskIntoConstraints = false
        chevronBtn.accessibilityIdentifier = "JuxuCandidateToggle"
        chevronBtn.accessibilityLabel = "展开候选"
        let cfg = UIImage.SymbolConfiguration(pointSize: 15.0, weight: .semibold)
        chevronBtn.setChevronImage(UIImage(systemName: "chevron.down", withConfiguration: cfg))
        chevronBtn.addTarget(self, action: #selector(handleChevron), for: .touchUpInside)

        clipboardPill.isHidden = true
        clipboardPill.addTarget(self, action: #selector(handleClipboardTap), for: .touchUpInside)

        let dismissCfg = UIImage.SymbolConfiguration(pointSize: 13.0, weight: .medium)
        clipboardDismissBtn.setImage(UIImage(systemName: "xmark", withConfiguration: dismissCfg), for: .normal)
        clipboardDismissBtn.accessibilityIdentifier = "JuxuClipboardDismiss"
        clipboardDismissBtn.accessibilityLabel = "关闭剪贴板提示"
        clipboardDismissBtn.isHidden = true
        clipboardDismissBtn.addTarget(self, action: #selector(handleClipboardDismiss), for: .touchUpInside)

        addSubview(clipContainer)
        addSubview(chevronDivider)
        addSubview(chevronBtn)
        addSubview(clipboardPill)
        addSubview(clipboardDismissBtn)

        clipTrailingToChevron = clipContainer.trailingAnchor.constraint(equalTo: chevronBtn.leadingAnchor)
        clipTrailingToFull = clipContainer.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6)
        clipTopConstraint = clipContainer.topAnchor.constraint(equalTo: topAnchor, constant: 4.0)
        clipBottomConstraint = clipContainer.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -7.0)

        NSLayoutConstraint.activate([
            clipContainer.leadingAnchor.constraint(equalTo: leadingAnchor),
            clipTrailingToFull,
            clipTopConstraint,
            clipBottomConstraint,

            chevronBtn.trailingAnchor.constraint(equalTo: trailingAnchor),
            chevronBtn.topAnchor.constraint(equalTo: topAnchor),
            chevronBtn.bottomAnchor.constraint(equalTo: bottomAnchor),
            chevronBtn.widthAnchor.constraint(equalToConstant: Self.chevronWidth),
        ])

        ensureSlotCapacity(10)
    }

    override func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
        guard let pan = gestureRecognizer as? UIPanGestureRecognizer else { return true }
        guard contentWidth > clipContainer.bounds.width + 2.0 else { return false }
        let vel = pan.velocity(in: clipContainer)
        return abs(vel.x) > abs(vel.y) * 1.2
    }

    private var isCompactBar: Bool {
        bounds.height > 0 && bounds.height < 46.0
    }

    private var bandTopInset: CGFloat {
        isCompactBar ? 2.0 : 4.0
    }

    private var bandBottomInset: CGFloat {
        isCompactBar ? 5.0 : 7.0
    }

    @objc private func handleHorizontalPan(_ pan: UIPanGestureRecognizer) {
        let maxOffset = max(0, contentWidth - clipContainer.bounds.width)
        guard maxOffset > 0 else { return }

        switch pan.state {
        case .began:
            contentTrack.layer.removeAllAnimations()
            panStartOffsetX = scrollOffsetX
        case .changed:
            let dx = pan.translation(in: clipContainer).x
            let raw = panStartOffsetX - dx
            if raw < 0 {
                scrollOffsetX = raw * 0.25
            } else if raw > maxOffset {
                scrollOffsetX = maxOffset + (raw - maxOffset) * 0.25
            } else {
                scrollOffsetX = raw
            }
            applyScrollOffset()
        case .ended, .cancelled:
            let vx = pan.velocity(in: clipContainer).x
            let projected = min(max(0, scrollOffsetX - vx * 0.16), maxOffset)
            UIView.animate(
                withDuration: 0.24,
                delay: 0,
                options: [.curveEaseOut, .allowUserInteraction, .beginFromCurrentState],
                animations: {
                    self.scrollOffsetX = projected
                    self.applyScrollOffset()
                }
            )
        default:
            break
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let topInset = bandTopInset
        let bottomInset = bandBottomInset
        if clipTopConstraint.constant != topInset {
            clipTopConstraint.constant = topInset
        }
        if clipBottomConstraint.constant != -bottomInset {
            clipBottomConstraint.constant = -bottomInset
        }
        chevronBtn.bandTopInset = topInset
        chevronBtn.bandBottomInset = bottomInset

        let scale = max(1.0, traitCollection.displayScale)
        let hairline = 1.0 / scale
        let barH = max(0, bounds.height - topInset - bottomInset)
        let slotH = min(36.0, barH)
        let slotY = floor((barH - slotH) * 0.5)

        if !chevronBtn.isHidden {
            let dividerH = min(20.0, max(14.0, barH - 20.0))
            let dividerY = topInset + (barH - dividerH) * 0.5
            chevronDivider.frame = CGRect(
                x: bounds.width - Self.chevronWidth,
                y: round(dividerY * scale) / scale,
                width: hairline,
                height: round(dividerH * scale) / scale
            )
        }

        // Layout centered native clipboard pill when visible (keeping bottom >= 8pt clear for Row 0 q..p)
        if !clipboardPill.isHidden, let suggestion = activeClipboardSuggestion {
            let compact = bounds.height < 46.0
            if lastClipboardCompactLayout != compact {
                lastClipboardCompactLayout = compact
                clipboardPill.configure(suggestion: suggestion, dark: lastDark ?? false, compactLandscape: compact)
            }
            let totalW = bounds.width
            let dismissW: CGFloat = 38.0
            let pillH: CGFloat = compact ? min(30.0, max(26.0, bounds.height - 8.0)) : 38.0
            let usableBandH = max(pillH, bounds.height - 8.0)
            let pillY = compact
                ? floor((usableBandH - pillH) * 0.5)
                : min(bounds.height - 8.5 - pillH, floor((bounds.height - pillH) * 0.5) - 1.0)
            let maxPillW = max(120.0, min(totalW - (dismissW + 12.0) * 2.0, 292.0))
            let desiredW = min(maxPillW, max(126.0, clipboardPill.intrinsicContentWidth))
            let pillX = floor((totalW - desiredW) * 0.5)
            clipboardPill.frame = CGRect(x: pillX, y: pillY, width: desiredW, height: pillH)
            clipboardDismissBtn.frame = CGRect(
                x: totalW - dismissW - 4.0,
                y: pillY,
                width: dismissW,
                height: pillH
            )
        }

        // Keep first candidate text comfortably clear of the 26pt top-left continuous corner.
        let firstSlotX = Self.visualHorizontalInset - IOSRibbonSlotControl.horizontalPadding
        let visibleSlots = slotControls.filter { !$0.isHidden }
        let totalSlotWidths = visibleSlots.reduce(CGFloat(0)) { $0 + $1.slotWidth }
        let availW = clipContainer.bounds.width
        let spacing: CGFloat
        if lastPreeditEmpty == true, visibleSlots.count > 1, availW > 200 {
            let naturalSpan = firstSlotX + totalSlotWidths + CGFloat(visibleSlots.count - 1) * 6.0 + 10.0
            if naturalSpan <= availW {
                spacing = min(14.0, floor((availW - firstSlotX - 10.0 - totalSlotWidths) / CGFloat(visibleSlots.count - 1)))
            } else {
                spacing = 4.0
            }
        } else {
            spacing = 6.0
        }

        let dividerH = min(18.0, max(12.0, slotH - 16.0))
        let dividerY = slotY + (slotH - dividerH) * 0.5
        var x = firstSlotX
        for (idx, slot) in visibleSlots.enumerated() {
            slot.frame = CGRect(x: x, y: slotY, width: slot.slotWidth, height: slotH)
            if idx + 1 < visibleSlots.count, idx < slotDividers.count {
                let div = slotDividers[idx]
                div.isHidden = false
                div.frame = CGRect(
                    x: round((slot.frame.maxX + spacing * 0.5) * scale) / scale,
                    y: round(dividerY * scale) / scale,
                    width: hairline,
                    height: round(dividerH * scale) / scale
                )
            }
            x = slot.frame.maxX + spacing
        }
        for idx in max(0, visibleSlots.count - 1)..<slotDividers.count {
            slotDividers[idx].isHidden = true
        }
        contentWidth = max(0, x - spacing + 10.0)

        let maxOffset = max(0, contentWidth - availW)
        if scrollOffsetX > maxOffset {
            scrollOffsetX = maxOffset
        }
        applyScrollOffset()
    }

    private func applyScrollOffset() {
        let barH = clipContainer.bounds.height
        let availW = clipContainer.bounds.width
        contentTrack.frame = CGRect(
            x: -scrollOffsetX,
            y: 0,
            width: max(availW, contentWidth),
            height: barH
        )
        // Only attach an alpha-fade mask when horizontal content actually overflows:
        // avoids unnecessary Core Animation offscreen group rendering on normal/idle frames!
        let hasLeftOverflow = scrollOffsetX > 0.5
        let hasRightOverflow = (contentWidth - scrollOffsetX) > availW + 2.0
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        if (hasLeftOverflow || hasRightOverflow), availW > 0 {
            let fadeFraction = Double(min(18.0 / availW, 0.25))
            let solid = UIColor.black.cgColor
            let clear = UIColor.clear.cgColor
            edgeFadeMask.frame = clipContainer.bounds
            edgeFadeMask.colors = [
                hasLeftOverflow ? clear : solid,
                solid,
                solid,
                hasRightOverflow ? clear : solid,
            ]
            edgeFadeMask.locations = [
                0.0,
                NSNumber(value: hasLeftOverflow ? fadeFraction : 0.0),
                NSNumber(value: hasRightOverflow ? 1.0 - fadeFraction : 1.0),
                1.0,
            ]
            if clipContainer.layer.mask !== edgeFadeMask {
                clipContainer.layer.mask = edgeFadeMask
            }
        } else if clipContainer.layer.mask != nil {
            clipContainer.layer.mask = nil
        }
        CATransaction.commit()
    }

    private func ensureSlotCapacity(_ count: Int) {
        while slotControls.count < count {
            let ctrl = IOSRibbonSlotControl(frame: .zero)
            ctrl.addTarget(self, action: #selector(handleSlotTap(_:)), for: .touchUpInside)
            ctrl.isHidden = true
            contentTrack.addSubview(ctrl)
            slotControls.append(ctrl)
        }
        let neededDividers = max(0, count - 1)
        while slotDividers.count < neededDividers {
            let div = UIView()
            div.isUserInteractionEnabled = false
            div.isHidden = true
            contentTrack.addSubview(div)
            slotDividers.append(div)
        }
    }

    @objc private func handleSlotTap(_ sender: IOSRibbonSlotControl) {
        if let starter = sender.starterWord {
            onSelectStarterWord?(starter)
        } else {
            onSelectCandidate?(sender.slotIndex)
        }
    }

    func setExpanded(_ expanded: Bool) {
        let cfg = UIImage.SymbolConfiguration(pointSize: 15.0, weight: .semibold)
        chevronBtn.setChevronImage(
            UIImage(systemName: expanded ? "chevron.up" : "chevron.down", withConfiguration: cfg)
        )
        chevronBtn.accessibilityLabel = expanded ? "收起候选" : "展开候选"
    }

    private var lastCandidates: [JuxuCandidateItem]?
    private var lastIdleWords: [String]?
    private var lastPreeditEmpty: Bool?
    private var lastDark: Bool?

    func update(
        state: JuxuActionState,
        dark: Bool,
        clipboardSuggestion: IOSClipboardSuggestion? = nil,
        idleWords: [String]? = nil
    ) {
        let isEmpty = state.preedit.isEmpty
        let effectiveClipboard = isEmpty ? clipboardSuggestion : nil
        let resolvedIdleWords = idleWords ?? Self.starterWords
        if lastPreeditEmpty == isEmpty,
           lastDark == dark,
           activeClipboardSuggestion == effectiveClipboard,
           (isEmpty ? lastIdleWords == resolvedIdleWords : lastCandidates == state.candidates) {
            return
        }
        lastPreeditEmpty = isEmpty
        lastDark = dark
        lastCandidates = state.candidates
        lastIdleWords = isEmpty ? resolvedIdleWords : nil
        activeClipboardSuggestion = effectiveClipboard

        let primaryCol: UIColor = dark ? .white : UIColor(white: 0.08, alpha: 1.0)
        let secondaryCol: UIColor = dark ? UIColor(white: 0.90, alpha: 1.0) : UIColor(white: 0.16, alpha: 1.0)
        let subtleCol: UIColor = dark ? UIColor(white: 0.62, alpha: 1.0) : UIColor(white: 0.46, alpha: 1.0)
        let chevronCol: UIColor = dark ? UIColor(white: 0.84, alpha: 1.0) : UIColor(white: 0.26, alpha: 1.0)
        let slotDividerCol: UIColor = dark ? UIColor(white: 1.0, alpha: 0.16) : UIColor(white: 0.0, alpha: 0.14)
        let chevronDividerCol: UIColor = dark ? UIColor(white: 1.0, alpha: 0.22) : UIColor(white: 0.0, alpha: 0.18)
        let candidateFontSize: CGFloat = isCompactBar ? 17.5 : 19.5
        let idleFontSize: CGFloat = isCompactBar ? 17.0 : 19.0

        let hideChevron = (effectiveClipboard != nil)
        chevronBtn.tintColor = chevronCol
        chevronBtn.isHidden = hideChevron
        chevronDivider.backgroundColor = chevronDividerCol
        chevronDivider.isHidden = hideChevron
        clipTrailingToChevron.isActive = !hideChevron
        clipTrailingToFull.isActive = hideChevron

        UIView.performWithoutAnimation {
            if let clip = effectiveClipboard {
                clipContainer.isHidden = true
                clipboardPill.isHidden = false
                clipboardDismissBtn.isHidden = false
                clipboardDismissBtn.tintColor = subtleCol
                let compact = bounds.height > 10 && bounds.height < 46.0
                lastClipboardCompactLayout = compact
                clipboardPill.configure(suggestion: clip, dark: dark, compactLandscape: compact)
            } else {
                clipContainer.isHidden = false
                clipboardPill.isHidden = true
                clipboardDismissBtn.isHidden = true
                lastClipboardCompactLayout = nil

                if isEmpty {
                    let words = resolvedIdleWords
                    ensureSlotCapacity(words.count)
                    for (i, ctrl) in slotControls.enumerated() {
                        if i < words.count {
                            ctrl.slotIndex = i
                            ctrl.starterWord = words[i]
                            ctrl.setSlotContent(
                                primary: words[i],
                                fontSize: idleFontSize,
                                isEmphasized: false,
                                primaryColor: secondaryCol,
                                subtleColor: subtleCol,
                                dark: dark
                            )
                            ctrl.isHidden = false
                        } else {
                            ctrl.isHidden = true
                        }
                    }
                } else {
                    let items = state.candidates
                    ensureSlotCapacity(items.count)
                    for (i, ctrl) in slotControls.enumerated() {
                        if i < items.count {
                            let item = items[i]
                            ctrl.slotIndex = i
                            ctrl.starterWord = nil
                            let secondary = cleanIOSCandidateAnnotation(for: item)
                            ctrl.setSlotContent(
                                primary: item.text,
                                secondary: secondary,
                                fontSize: candidateFontSize,
                                isEmphasized: i == 0,
                                primaryColor: i == 0 ? primaryCol : secondaryCol,
                                subtleColor: subtleCol,
                                dark: dark
                            )
                            ctrl.isHidden = false
                        } else {
                            ctrl.isHidden = true
                        }
                    }
                }
                for div in slotDividers {
                    div.backgroundColor = slotDividerCol
                }
            }
            scrollOffsetX = 0
            setNeedsLayout()
            layoutIfNeeded()
        }
    }

    @objc private func handleChevron() {
        onTapChevron?()
    }

    @objc private func handleClipboardTap() {
        guard let suggestion = activeClipboardSuggestion else { return }
        onSelectClipboard?(suggestion)
    }

    @objc private func handleClipboardDismiss() {
        guard let suggestion = activeClipboardSuggestion else { return }
        onDismissClipboard?(suggestion)
    }
}

/// ScrollView for expanded candidates: zero touch-down delay on buttons while still allowing drag-to-scroll.
@MainActor
private final class IOSExpandedScrollView: UIScrollView {
    override init(frame: CGRect) {
        super.init(frame: frame)
        delaysContentTouches = false
        canCancelContentTouches = true
        panGestureRecognizer.delaysTouchesBegan = false
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func touchesShouldCancel(in view: UIView) -> Bool {
        true
    }
}

/// Reusable tactile button for expanded candidate cells & right rail with instant keycap-style press highlight
/// and manual `CGRect` subview layout (zero Auto Layout constraint churn inside `UIScrollView`).
@MainActor
private final class IOSExpandedCellButton: UIButton {
    var candidateIndex: Int = 0

    var normalSurfaceColor: UIColor = .white {
        didSet { updateSurface() }
    }
    var pressedSurfaceColor: UIColor = UIColor(red: 214.0 / 255.0, green: 216.0 / 255.0, blue: 222.0 / 255.0, alpha: 1.0) {
        didSet { updateSurface() }
    }

    override var isHighlighted: Bool {
        didSet { updateSurface() }
    }

    private let candidateLabel = UILabel()
    private let hintLabel = UILabel()
    private var isCandidateCell = false
    private var hasHint = false
    private var measuredTextHeight: CGFloat = 22.0
    private var measuredHintHeight: CGFloat = 12.0
    private var currentTextInset: CGFloat = 10.0

    func setupAsCandidateCell() {
        guard !isCandidateCell else { return }
        isCandidateCell = true
        translatesAutoresizingMaskIntoConstraints = true
        layer.cornerRadius = IOSKeyboardLayoutEngine.keyCornerRadius
        layer.cornerCurve = .continuous
        isAccessibilityElement = true

        candidateLabel.translatesAutoresizingMaskIntoConstraints = true
        candidateLabel.lineBreakMode = .byCharWrapping
        candidateLabel.numberOfLines = 0
        candidateLabel.isUserInteractionEnabled = false
        candidateLabel.isAccessibilityElement = false
        addSubview(candidateLabel)

        hintLabel.translatesAutoresizingMaskIntoConstraints = true
        hintLabel.font = .systemFont(ofSize: 10.0, weight: .regular)
        hintLabel.lineBreakMode = .byTruncatingMiddle
        hintLabel.adjustsFontSizeToFitWidth = true
        hintLabel.minimumScaleFactor = 0.72
        hintLabel.numberOfLines = 1
        hintLabel.isUserInteractionEnabled = false
        hintLabel.isAccessibilityElement = false
        hintLabel.isHidden = true
        addSubview(hintLabel)
    }

    func configureCandidate(
        text: String,
        hint: String?,
        index: Int,
        font: UIFont,
        dark: Bool
    ) {
        candidateIndex = index
        normalSurfaceColor = dark ? UIColor(white: 1.0, alpha: 0.175) : .white
        pressedSurfaceColor = dark
            ? UIColor(white: 1.0, alpha: 0.30)
            : UIColor(red: 214.0 / 255.0, green: 216.0 / 255.0, blue: 222.0 / 255.0, alpha: 1.0)
        accessibilityLabel = text
        accessibilityIdentifier = "JuxuExpandedCandidate\(index)"

        candidateLabel.text = text
        candidateLabel.font = font
        candidateLabel.textColor = dark ? .white : .black

        if let hint, !hint.isEmpty {
            hasHint = true
            hintLabel.text = hint
            hintLabel.textColor = dark ? UIColor(white: 0.72, alpha: 1.0) : UIColor(white: 0.42, alpha: 1.0)
            hintLabel.isHidden = false
            measuredHintHeight = ceil(hintLabel.font.lineHeight)
        } else {
            hasHint = false
            hintLabel.text = nil
            hintLabel.isHidden = true
            measuredHintHeight = 0
        }
    }

    func measureHeight(forWidth width: CGFloat, fullWidth: Bool, textInset: CGFloat) -> CGFloat {
        currentTextInset = textInset
        let alignment: NSTextAlignment = fullWidth ? .natural : .center
        candidateLabel.textAlignment = alignment
        hintLabel.textAlignment = alignment

        let availableWidth = max(1.0, width - 2.0 * textInset)
        let textHeight = ceil(candidateLabel.sizeThatFits(CGSize(width: availableWidth, height: .greatestFiniteMagnitude)).height)
        measuredTextHeight = textHeight

        if hasHint {
            let spacing: CGFloat = 1.0
            let totalContentH = textHeight + spacing + measuredHintHeight
            return max(48.0, totalContentH + 12.0)
        } else {
            return max(45.0, textHeight + 16.0)
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        guard isCandidateCell else { return }
        let w = bounds.width
        let h = bounds.height
        let textW = max(1.0, w - 2.0 * currentTextInset)
        if hasHint {
            let spacing: CGFloat = 1.0
            let totalH = measuredTextHeight + spacing + measuredHintHeight
            let textY = floor((h - totalH) * 0.5)
            candidateLabel.frame = CGRect(
                x: currentTextInset,
                y: textY,
                width: textW,
                height: measuredTextHeight
            )
            hintLabel.frame = CGRect(
                x: 4.0,
                y: textY + measuredTextHeight + spacing,
                width: max(1.0, w - 8.0),
                height: measuredHintHeight
            )
        } else {
            candidateLabel.frame = CGRect(
                x: currentTextInset,
                y: floor((h - measuredTextHeight) * 0.5),
                width: textW,
                height: measuredTextHeight
            )
        }
    }

    private func updateSurface() {
        backgroundColor = isHighlighted ? pressedSurfaceColor : normalSurfaceColor
    }
}

/// Pooled manual-frame expanded candidate grid:
/// - Compact choices share a 4-column row; longer candidates take full width and wrap.
/// - Reuses `cellPool` (`translatesAutoresizingMaskIntoConstraints = true`) with deterministic `CGRect` layout,
///   doing zero Auto Layout constraint churn and zero view allocation on scroll or repeated reloads.
final class IOSExpandedCandidateView: UIView {
    var onSelectAbsCandidate: ((Int) -> Void)?
    var onCollapse: (() -> Void)?
    var onBackspace: (() -> Void)?
    var onClear: (() -> Void)?
    var onOpenSettings: (() -> Void)?

    private let scrollView = IOSExpandedScrollView()
    private let railStack = UIStackView()
    private let candidateFont = UIFont.systemFont(ofSize: 17.5, weight: .regular)
    private let firstCandidateFont = UIFont.systemFont(ofSize: 17.5, weight: .medium)
    private let columns = 4
    private let columnSpacing: CGFloat = 6
    private let rowSpacing: CGFloat = 8
    private let textInset: CGFloat = 10

    private var cellPool: [IOSExpandedCellButton] = []
    private var candidates: [JuxuCandidateItem] = []
    private var dark = false
    private var lastLaidOutWidth: CGFloat = 0
    private var needsRelayout = false

    override init(frame: CGRect) {
        super.init(frame: frame)
        // Non-zero alpha backing prevents iOS cross-process CALayerHost hit-testing from skipping gutters/margins.
        backgroundColor = UIColor.systemBackground.withAlphaComponent(0.02)
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        scrollView.accessibilityIdentifier = "JuxuExpandedCandidates"
        scrollView.contentInsetAdjustmentBehavior = .never
        #if compiler(>=6.2)
        if #available(iOS 26.0, *) {
            scrollView.topEdgeEffect.isHidden = true
            scrollView.bottomEdgeEffect.isHidden = true
            scrollView.leftEdgeEffect.isHidden = true
            scrollView.rightEdgeEffect.isHidden = true
        }
        #endif

        railStack.translatesAutoresizingMaskIntoConstraints = false
        railStack.axis = .vertical
        railStack.distribution = .fillEqually
        railStack.spacing = 8

        addSubview(scrollView)
        addSubview(railStack)

        NSLayoutConstraint.activate([
            scrollView.topAnchor.constraint(equalTo: topAnchor, constant: 5),
            scrollView.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 7),
            scrollView.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8),
            scrollView.trailingAnchor.constraint(equalTo: railStack.leadingAnchor, constant: -8),

            railStack.topAnchor.constraint(equalTo: topAnchor, constant: 5),
            railStack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -7),
            railStack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8),
            railStack.widthAnchor.constraint(equalToConstant: 50),
        ])

        let collapseBtn = makeRailButton(title: nil, symbol: "chevron.up") { [weak self] in self?.onCollapse?() }
        let delBtn = makeRailButton(title: nil, symbol: "delete.backward") { [weak self] in self?.onBackspace?() }
        let clrBtn = makeRailButton(title: "重输", symbol: nil) { [weak self] in self?.onClear?() }
        let settingsBtn = makeRailButton(title: nil, symbol: "gearshape") { [weak self] in self?.onOpenSettings?() }
        settingsBtn.accessibilityIdentifier = "JuxuOpenSettings"
        settingsBtn.accessibilityLabel = "打开句序设置"
        railStack.addArrangedSubview(collapseBtn)
        railStack.addArrangedSubview(delBtn)
        railStack.addArrangedSubview(clrBtn)
        railStack.addArrangedSubview(settingsBtn)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func reload(candidates: [JuxuCandidateItem], dark: Bool) {
        self.candidates = candidates
        self.dark = dark
        let cellBg = dark ? UIColor(white: 1.0, alpha: 0.175) : UIColor.white
        let pressedBg = dark
            ? UIColor(white: 1.0, alpha: 0.30)
            : UIColor(red: 214.0 / 255.0, green: 216.0 / 255.0, blue: 222.0 / 255.0, alpha: 1.0)
        let textCol = dark ? UIColor.white : UIColor.black

        for case let btn as IOSExpandedCellButton in railStack.arrangedSubviews {
            btn.normalSurfaceColor = cellBg
            btn.pressedSurfaceColor = pressedBg
            btn.tintColor = textCol
            btn.setTitleColor(textCol, for: .normal)
            btn.layer.shadowOpacity = 0
        }

        ensureCellPoolCapacity(candidates.count)
        UIView.performWithoutAnimation {
            for (index, item) in candidates.enumerated() {
                let cell = cellPool[index]
                let hint = Self.displayHint(for: item)
                cell.configureCandidate(
                    text: item.text,
                    hint: hint,
                    index: index,
                    font: index == 0 ? firstCandidateFont : candidateFont,
                    dark: dark
                )
                cell.isHidden = false
            }
            if candidates.count < cellPool.count {
                for index in candidates.count..<cellPool.count {
                    cellPool[index].isHidden = true
                }
            }
        }

        needsRelayout = true
        scrollView.setContentOffset(.zero, animated: false)
        setNeedsLayout()
    }

    private func ensureCellPoolCapacity(_ count: Int) {
        while cellPool.count < count {
            let cell = IOSExpandedCellButton(type: .custom)
            cell.setupAsCandidateCell()
            cell.isHidden = true
            cell.addTarget(self, action: #selector(handleCandidateCellTap(_:)), for: .touchUpInside)
            scrollView.addSubview(cell)
            cellPool.append(cell)
        }
    }

    @objc private func handleCandidateCellTap(_ sender: IOSExpandedCellButton) {
        onSelectAbsCandidate?(sender.candidateIndex)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let width = scrollView.bounds.width
        guard width > columnSpacing * 3 else { return }
        guard needsRelayout || abs(width - lastLaidOutWidth) > 0.5 else { return }
        lastLaidOutWidth = width
        needsRelayout = false

        let cellWidth = (width - CGFloat(columns - 1) * columnSpacing) / CGFloat(columns)
        var y: CGFloat = 0
        var compactIndices: [Int] = []
        compactIndices.reserveCapacity(columns)
        var compactRowHeight: CGFloat = 0

        func flushCompactRow() {
            guard !compactIndices.isEmpty else { return }
            if y > 0 {
                y += rowSpacing
            }
            for (col, idx) in compactIndices.enumerated() {
                let x = CGFloat(col) * (cellWidth + columnSpacing)
                let cell = cellPool[idx]
                cell.frame = CGRect(x: x, y: y, width: cellWidth, height: compactRowHeight)
                cell.setNeedsLayout()
                cell.layoutIfNeeded()
            }
            y += compactRowHeight
            compactIndices.removeAll(keepingCapacity: true)
            compactRowHeight = 0
        }

        UIView.performWithoutAnimation {
            for (index, item) in candidates.enumerated() {
                let cell = cellPool[index]
                let font = index == 0 ? firstCandidateFont : candidateFont
                let textWidth = (item.text as NSString).size(withAttributes: [.font: font]).width
                let fullWidth = textWidth > cellWidth - 2.0 * textInset || item.text.contains("\n")
                let targetWidth = fullWidth ? width : cellWidth
                let height = cell.measureHeight(forWidth: targetWidth, fullWidth: fullWidth, textInset: textInset)

                if fullWidth {
                    flushCompactRow()
                    if y > 0 {
                        y += rowSpacing
                    }
                    cell.frame = CGRect(x: 0, y: y, width: width, height: height)
                    cell.setNeedsLayout()
                    cell.layoutIfNeeded()
                    y += height
                } else {
                    compactIndices.append(index)
                    compactRowHeight = max(compactRowHeight, height)
                    if compactIndices.count == columns {
                        flushCompactRow()
                    }
                }
            }
            flushCompactRow()
            scrollView.contentSize = CGSize(width: width, height: y)
        }
    }

    private static func displayHint(for item: JuxuCandidateItem) -> String? {
        switch item.badgeKind {
        case .pinyinLookup, .shortcode, .typoCorrected:
            let cleaned = cleanIOSCandidateAnnotation(for: item)
            return cleaned.isEmpty ? nil : cleaned
        default:
            return nil
        }
    }

    private func makeRailButton(title: String?, symbol: String?, action: @escaping () -> Void) -> UIButton {
        let b = IOSExpandedCellButton(type: .system)
        b.layer.cornerRadius = IOSKeyboardLayoutEngine.keyCornerRadius
        b.layer.cornerCurve = .continuous
        if let s = symbol {
            b.setImage(UIImage(systemName: s), for: .normal)
        } else {
            b.setTitle(title, for: .normal)
            b.titleLabel?.font = .systemFont(ofSize: 14, weight: .regular)
        }
        b.addAction(UIAction { _ in action() }, for: .touchUpInside)
        return b
    }
}
#endif
