#if os(iOS)
import UIKit

/// Long-press strip of alternate characters; the finger slides horizontally to choose one.
final class IOSKeyAlternatesView: UIView {
    private var labels: [UILabel] = []
    private let highlight = UIView()
    private var items: [String] = []
    private(set) var selectedIndex: Int = 0
    private var isDarkStyle = false

    var selectedText: String? {
        items.indices.contains(selectedIndex) ? items[selectedIndex] : nil
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        isHidden = true
        layer.cornerRadius = 13.0
        layer.cornerCurve = .continuous
        layer.shadowColor = UIColor.black.cgColor
        layer.shadowOffset = CGSize(width: 0, height: 3.5)
        layer.shadowRadius = 11.0
        highlight.backgroundColor = .systemBlue
        highlight.layer.cornerRadius = 9.0
        highlight.layer.cornerCurve = .continuous
        addSubview(highlight)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present(items: [String], defaultIndex: Int, over keyFrame: CGRect, containerWidth: CGFloat, dark: Bool) {
        self.items = items
        labels.forEach { $0.removeFromSuperview() }
        frame = IOSKeyAlternates.stripFrame(
            keyFrame: keyFrame,
            count: items.count,
            defaultIndex: defaultIndex,
            containerWidth: containerWidth
        )
        backgroundColor = dark ? UIColor(red: 0.29, green: 0.29, blue: 0.30, alpha: 1.0) : .white
        layer.shadowOpacity = dark ? 0.34 : 0.16
        layer.shadowPath = UIBezierPath(roundedRect: bounds, cornerRadius: 13.0).cgPath
        let cell = bounds.width / CGFloat(max(items.count, 1))
        labels = items.enumerated().map { idx, text in
            let label = UILabel(frame: CGRect(x: CGFloat(idx) * cell, y: 0, width: cell, height: bounds.height))
            label.text = text
            label.textAlignment = .center
            label.font = .systemFont(ofSize: 23.0)
            label.adjustsFontSizeToFitWidth = true
            label.minimumScaleFactor = 0.5
            addSubview(label)
            return label
        }
        isHidden = false
        isDarkStyle = dark
        select(defaultIndex)
    }

    /// Returns `true` when the highlighted character changed.
    @discardableResult
    func update(xInSuperview x: CGFloat) -> Bool {
        let idx = IOSKeyAlternates.index(atX: x, in: frame, count: items.count)
        guard idx != selectedIndex else { return false }
        select(idx)
        return true
    }

    private func select(_ idx: Int) {
        selectedIndex = idx
        guard labels.indices.contains(idx) else { return }
        highlight.frame = labels[idx].frame.insetBy(dx: 2.0, dy: 4.0)
        for (i, label) in labels.enumerated() {
            label.textColor = i == idx ? .white : (isDarkStyle ? .white : .black)
        }
    }

    func hide() {
        isHidden = true
        items = []
    }
}

/// Full-size iOS 26 Key Callout Bubble with spring micro-motion:
/// - Row 1 & Row 2: standalone `61.33 x 65.0pt` continuous squircle hovering `4.0pt` above pressed keycap.
/// - Row 0 (`q..p`): full `61.33pt` head (`36.5pt` glyph, zero shrinking!) seamlessly tapering into the
///   pressed keycap base so it neither gets clipped by the XPC viewport nor cuts the underlying glyph in half.
final class IOSKeyCalloutView: UIView {
    private let shapeLayer = CAShapeLayer()
    private let glyphLabel = UILabel()
    private var dismissWorkItem: DispatchWorkItem?
    private(set) var coveredKeyId: String?

    override init(frame: CGRect) {
        super.init(frame: CGRect(x: 0, y: 0, width: 61.33, height: 65.0))
        isOpaque = false
        isUserInteractionEnabled = false
        clipsToBounds = false

        shapeLayer.shadowColor = UIColor.black.cgColor
        shapeLayer.shadowOffset = CGSize(width: 0, height: 3.5)
        shapeLayer.shadowRadius = 11.0
        layer.addSublayer(shapeLayer)

        glyphLabel.font = .systemFont(ofSize: 36.5, weight: .regular)
        glyphLabel.textAlignment = .center
        glyphLabel.adjustsFontSizeToFitWidth = true
        glyphLabel.minimumScaleFactor = 0.45
        addSubview(glyphLabel)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func hideImmediately() {
        dismissWorkItem?.cancel()
        dismissWorkItem = nil
        layer.removeAllAnimations()
        coveredKeyId = nil
        alpha = 1.0
        transform = .identity
        isHidden = true
    }

    func releasePreview(onCleared: (() -> Void)? = nil) {
        dismissWorkItem?.cancel()
        let item = DispatchWorkItem { [weak self] in
            guard let self else { return }
            UIView.animate(
                withDuration: 0.052,
                delay: 0,
                options: [.curveEaseIn, .beginFromCurrentState, .allowUserInteraction],
                animations: {
                    self.alpha = 0.0
                    self.transform = CGAffineTransform(scaleX: 0.94, y: 0.94).translatedBy(x: 0, y: 2.0)
                },
                completion: { finished in
                    if finished {
                        self.coveredKeyId = nil
                        self.isHidden = true
                        self.alpha = 1.0
                        self.transform = .identity
                        onCleared?()
                    }
                }
            )
        }
        dismissWorkItem = item
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.038, execute: item)
    }

    /// Presents the full-size callout and returns `true` when Row 0's docked skirt covers the underlying keycap glyph.
    @discardableResult
    func present(label: String, keyId: String, over keyFrameInSuperview: CGRect, dark: Bool) -> Bool {
        dismissWorkItem?.cancel()
        dismissWorkItem = nil

        let wasHidden = isHidden || alpha < 0.2
        layer.removeAllAnimations()

        let headW: CGFloat = max(61.33, keyFrameInSuperview.width * 1.66)
        let headH: CGFloat = 65.0
        let hoverGap: CGFloat = 4.0
        let superW = superview?.bounds.width ?? 440

        let headMinX = min(
            max(3.5, keyFrameInSuperview.midX - headW * 0.5),
            max(3.5, superW - headW - 3.5)
        )
        let rawFloatingY = keyFrameInSuperview.minY - hoverGap - headH

        let fillCol = dark
            ? UIColor(red: 0.29, green: 0.29, blue: 0.30, alpha: 1.0)
            : UIColor.white
        let strokeCol = dark
            ? UIColor.white.withAlphaComponent(0.14)
            : UIColor.black.withAlphaComponent(0.06)

        CATransaction.begin()
        CATransaction.setDisableActions(true)

        let isDockedRow0 = (rawFloatingY < 4.0)

        if !isDockedRow0 {
            // Row 1 & Row 2: pure standalone floating iOS 26 squircle!
            coveredKeyId = nil
            let targetFrame = CGRect(x: headMinX, y: rawFloatingY, width: headW, height: headH)
            UIView.performWithoutAnimation {
                self.transform = .identity
                self.frame = targetFrame
                let p = UIBezierPath(roundedRect: CGRect(origin: .zero, size: targetFrame.size), cornerRadius: 13.5)
                shapeLayer.frame = bounds
                shapeLayer.path = p.cgPath
                shapeLayer.shadowPath = p.cgPath
                glyphLabel.font = .systemFont(ofSize: 36.5, weight: .regular)
                glyphLabel.frame = CGRect(x: 0, y: -1.0, width: headW, height: headH)
            }
        } else {
            // Row 0 (`q..p`): full-size callout docked seamlessly into exact keycap base!
            // Align outer edge flush with `q`/`p` keycap (`6.67pt`) so it sits cleanly inside the 26pt tray corner,
            // and place waist at `49.0..62.0pt` so descenders (`q`, `y`, `p`) have generous room above the taper.
            coveredKeyId = keyId
            let topY: CGFloat = 6.0
            let botY: CGFloat = keyFrameInSuperview.maxY
            let isLeftEdge = (keyFrameInSuperview.minX < 18.0)
            let isRightEdge = (keyFrameInSuperview.maxX > superW - 18.0)
            let row0HeadW: CGFloat = (isLeftEdge || isRightEdge) ? 55.0 : headW
            let row0HeadMinX: CGFloat
            if isLeftEdge {
                row0HeadMinX = keyFrameInSuperview.minX
            } else if isRightEdge {
                row0HeadMinX = keyFrameInSuperview.maxX - row0HeadW
            } else {
                row0HeadMinX = keyFrameInSuperview.midX - row0HeadW * 0.5
            }
            let row0HeadMaxX = row0HeadMinX + row0HeadW

            let unionMinX = min(row0HeadMinX, keyFrameInSuperview.minX)
            let unionMaxX = max(row0HeadMaxX, keyFrameInSuperview.maxX)
            let totalW = unionMaxX - unionMinX
            let totalH = botY - topY

            let hL = row0HeadMinX - unionMinX
            let hR = row0HeadMaxX - unionMinX
            let kL = keyFrameInSuperview.minX - unionMinX
            let kR = keyFrameInSuperview.maxX - unionMinX

            let waistTop: CGFloat = 49.0
            let waistBot: CGFloat = 62.0
            let path = Self.makeDockedBalloonPath(
                headLeft: hL,
                headRight: hR,
                keyLeft: kL,
                keyRight: kR,
                waistTop: waistTop,
                waistBot: waistBot,
                totalHeight: totalH,
                topRadius: 13.0,
                bottomRadius: IOSKeyboardLayoutEngine.keyCornerRadius
            )

            UIView.performWithoutAnimation {
                self.transform = .identity
                self.frame = CGRect(x: unionMinX, y: topY, width: totalW, height: totalH)
                shapeLayer.frame = bounds
                shapeLayer.path = path
                shapeLayer.shadowPath = path
                glyphLabel.font = .systemFont(ofSize: 35.5, weight: .regular)
                glyphLabel.frame = CGRect(x: hL, y: -1.5, width: row0HeadW, height: 51.0)
            }
        }

        shapeLayer.fillColor = fillCol.cgColor
        shapeLayer.strokeColor = strokeCol.cgColor
        shapeLayer.lineWidth = 0.5
        shapeLayer.shadowOpacity = dark ? 0.34 : 0.14
        CATransaction.commit()

        glyphLabel.text = label
        glyphLabel.textColor = dark ? .white : .black
        alpha = 1.0
        isHidden = false

        if wasHidden {
            transform = CGAffineTransform(scaleX: 0.91, y: 0.91).translatedBy(x: 0, y: 3.5)
            UIView.animate(
                withDuration: 0.072,
                delay: 0,
                usingSpringWithDamping: 0.84,
                initialSpringVelocity: 0.5,
                options: [.beginFromCurrentState, .allowUserInteraction],
                animations: {
                    self.transform = .identity
                }
            )
        }
        return isDockedRow0
    }

    private static func makeDockedBalloonPath(
        headLeft: CGFloat,
        headRight: CGFloat,
        keyLeft: CGFloat,
        keyRight: CGFloat,
        waistTop: CGFloat,
        waistBot: CGFloat,
        totalHeight: CGFloat,
        topRadius rT: CGFloat,
        bottomRadius rB: CGFloat
    ) -> CGPath {
        let p = UIBezierPath()
        let midY = (waistTop + waistBot) * 0.5

        p.move(to: CGPoint(x: headLeft + rT, y: 0))
        p.addLine(to: CGPoint(x: headRight - rT, y: 0))
        p.addQuadCurve(to: CGPoint(x: headRight, y: rT), controlPoint: CGPoint(x: headRight, y: 0))
        p.addLine(to: CGPoint(x: headRight, y: waistTop))

        // Right waist curve into keycap right edge
        p.addCurve(
            to: CGPoint(x: keyRight, y: waistBot),
            controlPoint1: CGPoint(x: headRight, y: midY),
            controlPoint2: CGPoint(x: keyRight, y: midY)
        )
        p.addLine(to: CGPoint(x: keyRight, y: totalHeight - rB))
        p.addQuadCurve(
            to: CGPoint(x: keyRight - rB, y: totalHeight),
            controlPoint: CGPoint(x: keyRight, y: totalHeight)
        )
        p.addLine(to: CGPoint(x: keyLeft + rB, y: totalHeight))
        p.addQuadCurve(
            to: CGPoint(x: keyLeft, y: totalHeight - rB),
            controlPoint: CGPoint(x: keyLeft, y: totalHeight)
        )
        p.addLine(to: CGPoint(x: keyLeft, y: waistBot))

        // Left waist curve back up into head left edge
        p.addCurve(
            to: CGPoint(x: headLeft, y: waistTop),
            controlPoint1: CGPoint(x: keyLeft, y: midY),
            controlPoint2: CGPoint(x: headLeft, y: midY)
        )
        p.addLine(to: CGPoint(x: headLeft, y: rT))
        p.addQuadCurve(to: CGPoint(x: headLeft + rT, y: 0), controlPoint: CGPoint(x: headLeft, y: 0))
        p.close()
        return p.cgPath
    }
}
#endif
