import CoreGraphics
import Foundation

public enum IOSKeyPlane: Equatable {
    case letters
    case numbers
    case symbols
}

public enum IOSShiftState: Equatable {
    case lower
    case upperOnce
    case capsLocked

    public var isUppercase: Bool {
        self != .lower
    }
}

public enum IOSReturnKeyRole: Equatable {
    case normal
    case go
    case search
    case send
    case next
    case done
    case join
    case continueRole

    public var title: String {
        switch self {
        case .normal: return ""
        case .go: return "前往"
        case .search: return "搜索"
        case .send: return "发送"
        case .next: return "下一项"
        case .done: return "完成"
        case .join: return "加入"
        case .continueRole: return "继续"
        }
    }

    public var isBlueProminent: Bool {
        self != .normal
    }
}

public enum IOSKeyKind: Equatable {
    case letter(UInt8) // 'a'..'z'
    case symbol(String)
    case shift
    case backspace
    case planeSwitch(IOSKeyPlane, label: String)
    case globe
    case modeToggle // 中/英切换
    case space
    case returnKey
}

public struct IOSKeySpec: Identifiable, Equatable {
    public let id: String
    public let kind: IOSKeyKind
    public let frame: CGRect
    public let primaryLabel: String
    /// Secondary muted inline suffix (e.g. `"/英"` or `"/中"` on the mode switch key).
    public let secondaryInlineLabel: String?
    /// Corner subscript label (e.g. `"五"` in bottom-right of iOS 26 native Wubi spacebar).
    public let cornerSubscript: String?
    public let systemSymbol: String?
    public let isBlueAccent: Bool

    public init(
        id: String,
        kind: IOSKeyKind,
        frame: CGRect,
        primaryLabel: String,
        secondaryInlineLabel: String? = nil,
        cornerSubscript: String? = nil,
        systemSymbol: String? = nil,
        isBlueAccent: Bool = false
    ) {
        self.id = id
        self.kind = kind
        self.frame = frame
        self.primaryLabel = primaryLabel
        self.secondaryInlineLabel = secondaryInlineLabel
        self.cornerSubscript = cornerSubscript
        self.systemSymbol = systemSymbol
        self.isBlueAccent = isBlueAccent
    }
}

/// Exact iOS 26 native Wubi keyboard geometric & state engine, calibrated 1:1 from iOS 26 metrics:
/// - Uniform pure-white rounded-squircle keycaps (`cornerRadius = 7.8pt`, continuous) in Light mode
/// - Lowercase key legends (`q w e r t y...`) by default, uppercase only when Shift/CapsLock active
/// - Ergonomic thumb downward bias (`2.6pt`) + Wubi dictionary valid-next-key dynamic hitbox expansion
public struct IOSKeyboardLayoutEngine: Equatable {
    public static let keyCornerRadius: CGFloat = 7.8
    public static let trayTopCornerRadius: CGFloat = 26.0
    public static let candidateBarHeight: CGFloat = 53.0
    public static let landscapeCandidateBarHeight: CGFloat = 40.0
    /// Thumbs naturally strike ~2.6pt below visual keycap center on handheld phones.
    public static let ergonomicThumbYBias: CGFloat = 2.6

    public var plane: IOSKeyPlane = .letters
    public var shiftState: IOSShiftState = .lower
    public var asciiMode: Bool = false
    public var schemaId: UInt32 = 86
    public var cangjieKeycapRadicals: Bool = true
    /// True only on Home-button devices where system does not draw its own bottom Globe bar.
    public var showInlineGlobeKey: Bool = false
    public var returnRole: IOSReturnKeyRole = .normal
    public var isComposing: Bool = false
    public var isTrackpadActive: Bool = false
    /// 26-bit mask of valid next Wubi letters (`0x03FF_FFFF` when unconstrained).
    public var validNextMask: UInt32 = 0x03FF_FFFF
    /// 26-element continuous neural prior weights (`0` = dead, `100` = neutral, `50..180` = RWKV weighted).
    public var nextKeyPriors: [UInt8] = Array(repeating: 100, count: 26)
    /// 52-element per-key adaptive touch offsets `[dx0, dy0, ..., dx25, dy25]` in normalized key-pitch units.
    public var touchOffsets: [Float] = Array(repeating: 0, count: 52)

    private var lastShiftTapTime: TimeInterval = 0

    public init() {}

    public mutating func tapShift(at timestamp: TimeInterval) {
        switch shiftState {
        case .lower:
            shiftState = .upperOnce
            lastShiftTapTime = timestamp
        case .upperOnce:
            if timestamp - lastShiftTapTime <= 0.34 {
                shiftState = .capsLocked
            } else {
                shiftState = .lower
            }
            lastShiftTapTime = 0
        case .capsLocked:
            shiftState = .lower
            lastShiftTapTime = 0
        }
    }

    public mutating func consumeOneShotShiftIfNeeded() {
        if shiftState == .upperOnce {
            shiftState = .lower
        }
    }

    public mutating func switchPlane(_ target: IOSKeyPlane) {
        plane = target
        if target != .letters && shiftState == .upperOnce {
            shiftState = .lower
        }
    }

    /// Subtle bottom-right subscript on the spacebar reflecting active input schema.
    public var spaceCornerTag: String? {
        if isTrackpadActive {
            return nil
        }
        if asciiMode {
            return "EN"
        }
        switch schemaId {
        case 20: return "拼"
        case 21: return "双"
        case 10: return "鹤"
        case 33, 35, 36: return "仓"
        default: return "五"
        }
    }

    public struct LayoutSnapshot {
        public let bounds: CGRect
        public let keys: [IOSKeySpec]
        public let centers52: [Float]
        public let originX: Float
        public let originY: Float
        public let pitchX: Float
        public let pitchY: Float
        public let bottomRowMinY: CGFloat
    }

    public func computeLayout(in size: CGSize) -> LayoutSnapshot {
        let w = max(280, size.width)
        let isLandscape = w > 540
        // Guard against transient full-screen container heights (e.g. 903pt before XPC heightConstraint settles
        // or while a system paste-permission modal interrupts layout) so Row 0 never stretches to fill the tray.
        let maxReasonableH: CGFloat = isLandscape ? 220.0 : 225.0
        let h = min(max(168, size.height), maxReasonableH)

        // Exact iOS 26 proportions (`440pt` -> outerMargin `6.67pt`, gapX `6.00pt`, keyW `37.27pt`, pitchX `43.27pt`,
        // `keyH = 45.0pt`, `rowGap = 11.0pt`, total `53 + 220 = 273pt` matching native `608..881pt` tray).
        // `bottomPad = 7.0pt` preserves generous bottom catch-margin inside the `220pt` XPC key canvas.
        let outerX: CGFloat = isLandscape ? 8.0 : round(w * (20.0 / 1320.0) * 3.0) / 3.0
        let gapX: CGFloat = isLandscape ? 6.5 : round(w * (18.0 / 1320.0) * 3.0) / 3.0
        let topPad: CGFloat = isLandscape ? 2.0 : 0.0
        let bottomPad: CGFloat = isLandscape ? 4.5 : 7.0
        let rowGap: CGFloat = isLandscape ? 8.0 : 11.0

        let usableH = h - topPad - bottomPad - rowGap * 3.0
        let keyH = max(34.0, floor(usableH / 4.0))
        let pitchY = keyH + rowGap

        // 10 keys + 9 gaps = w - 2 * outerX
        let stdKeyW = (w - outerX * 2.0 - gapX * 9.0) / 10.0
        let pitchX = stdKeyW + gapX

        let rowY: [CGFloat] = [
            topPad,
            topPad + pitchY,
            topPad + pitchY * 2.0,
            topPad + pitchY * 3.0,
        ]

        // Precompute exact Row 1 (`a..l`) x-origins because Row 2 (`z..m`) and Row 3 (`Space`/`Return`) lock to them!
        let row1StartX = outerX + pitchX * 0.5
        let colRow1X = (0..<9).map { i in row1StartX + CGFloat(i) * pitchX }

        var specs: [IOSKeySpec] = []

        switch plane {
        case .letters:
            // Row 0: q w e r t y u i o p
            for (c, b) in Array("qwertyuiop".utf8).enumerated() {
                let x = outerX + CGFloat(c) * pitchX
                specs.append(
                    IOSKeySpec(
                        id: "L_\(b)",
                        kind: .letter(b),
                        frame: CGRect(x: x, y: rowY[0], width: stdKeyW, height: keyH),
                        primaryLabel: formatLetter(b)
                    )
                )
            }

            // Row 1: a s d f g h j k l
            for (c, b) in Array("asdfghjkl".utf8).enumerated() {
                specs.append(
                    IOSKeySpec(
                        id: "L_\(b)",
                        kind: .letter(b),
                        frame: CGRect(x: colRow1X[c], y: rowY[1], width: stdKeyW, height: keyH),
                        primaryLabel: formatLetter(b)
                    )
                )
            }

            // Row 2: [⇧] + z x c v b n m (aligned with colRow1X[1..7] = s..k!) + [⌫]
            let sideKeyW = stdKeyW * 1.342 // 49.33pt on 440pt width
            let shiftSym: String
            switch shiftState {
            case .lower: shiftSym = "shift"
            case .upperOnce: shiftSym = "shift.fill"
            case .capsLocked: shiftSym = "capslock.fill"
            }
            specs.append(
                IOSKeySpec(
                    id: "K_shift",
                    kind: .shift,
                    frame: CGRect(x: outerX, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: "",
                    systemSymbol: isTrackpadActive ? nil : shiftSym
                )
            )

            for (c, b) in Array("zxcvbnm".utf8).enumerated() {
                specs.append(
                    IOSKeySpec(
                        id: "L_\(b)",
                        kind: .letter(b),
                        frame: CGRect(x: colRow1X[c + 1], y: rowY[2], width: stdKeyW, height: keyH),
                        primaryLabel: formatLetter(b)
                    )
                )
            }

            specs.append(
                IOSKeySpec(
                    id: "K_backspace",
                    kind: .backspace,
                    frame: CGRect(x: w - outerX - sideKeyW, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: "",
                    systemSymbol: isTrackpadActive ? nil : "delete.backward"
                )
            )

        case .numbers:
            let r0 = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]
            appendTenRow(r0, rowIdx: 0, rowY: rowY[0], outerX: outerX, pitchX: pitchX, keyW: stdKeyW, keyH: keyH, into: &specs)

            let r1 = asciiMode
                ? ["-", "/", ":", ";", "(", ")", "$", "&", "@", "\""]
                : ["-", "/", "：", "；", "（", "）", "￥", "@", "“", "”"]
            appendTenRow(r1, rowIdx: 1, rowY: rowY[1], outerX: outerX, pitchX: pitchX, keyW: stdKeyW, keyH: keyH, into: &specs)

            let sideKeyW = stdKeyW * 1.342
            specs.append(
                IOSKeySpec(
                    id: "P_symbols",
                    kind: .planeSwitch(.symbols, label: "#+="),
                    frame: CGRect(x: outerX, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : "#+="
                )
            )
            let puncMid = asciiMode
                ? [".", ",", "?", "!", "'"]
                : ["。", "，", "、", "？", "！", "."]
            appendSpanSymbols(
                puncMid,
                rowY: rowY[2],
                minX: colRow1X[1],
                maxX: colRow1X[7] + stdKeyW,
                gapX: gapX,
                keyH: keyH,
                into: &specs
            )
            specs.append(
                IOSKeySpec(
                    id: "K_backspace",
                    kind: .backspace,
                    frame: CGRect(x: w - outerX - sideKeyW, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: "",
                    systemSymbol: isTrackpadActive ? nil : "delete.backward"
                )
            )

        case .symbols:
            let r0 = ["[", "]", "{", "}", "#", "%", "^", "*", "+", "="]
            appendTenRow(r0, rowIdx: 0, rowY: rowY[0], outerX: outerX, pitchX: pitchX, keyW: stdKeyW, keyH: keyH, into: &specs)

            let r1 = asciiMode
                ? ["_", "\\", "|", "~", "<", ">", "€", "£", "¥", "•"]
                : ["——", "\\", "|", "～", "《", "》", "【", "】", "…", "·"]
            appendTenRow(r1, rowIdx: 1, rowY: rowY[1], outerX: outerX, pitchX: pitchX, keyW: stdKeyW, keyH: keyH, into: &specs)

            let sideKeyW = stdKeyW * 1.342
            specs.append(
                IOSKeySpec(
                    id: "P_numbers",
                    kind: .planeSwitch(.numbers, label: "123"),
                    frame: CGRect(x: outerX, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : "123"
                )
            )
            let symMid = ["…", "‘", "’", "〈", "〉"]
            appendSpanSymbols(
                symMid,
                rowY: rowY[2],
                minX: colRow1X[1],
                maxX: colRow1X[7] + stdKeyW,
                gapX: gapX,
                keyH: keyH,
                into: &specs
            )
            specs.append(
                IOSKeySpec(
                    id: "K_backspace",
                    kind: .backspace,
                    frame: CGRect(x: w - outerX - sideKeyW, y: rowY[2], width: sideKeyW, height: keyH),
                    primaryLabel: "",
                    systemSymbol: isTrackpadActive ? nil : "delete.backward"
                )
            )
        }

        // Row 3: Exact iOS 26 4-Key Grid!
        let leftPairSpan = (colRow1X[1] + stdKeyW) - outerX
        let leftEachW = (leftPairSpan - gapX) * 0.5

        let targetPlane: IOSKeyPlane = (plane == .letters) ? .numbers : .letters
        let planeLabel = (plane == .letters) ? "123" : "ABC"
        specs.append(
            IOSKeySpec(
                id: "B_plane",
                kind: .planeSwitch(targetPlane, label: planeLabel),
                frame: CGRect(x: outerX, y: rowY[3], width: leftEachW, height: keyH),
                primaryLabel: isTrackpadActive ? "" : planeLabel
            )
        )

        let secondLeftX = outerX + leftEachW + gapX
        if showInlineGlobeKey {
            specs.append(
                IOSKeySpec(
                    id: "B_globe",
                    kind: .globe,
                    frame: CGRect(x: secondLeftX, y: rowY[3], width: leftEachW, height: keyH),
                    primaryLabel: "",
                    systemSymbol: isTrackpadActive ? nil : "globe"
                )
            )
        } else {
            // Crisp typographic 中/英 mode indicator instead of jarring smiley icon
            specs.append(
                IOSKeySpec(
                    id: "B_mode",
                    kind: .modeToggle,
                    frame: CGRect(x: secondLeftX, y: rowY[3], width: leftEachW, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : (asciiMode ? "EN" : "中"),
                    secondaryInlineLabel: isTrackpadActive ? nil : (asciiMode ? "/中" : "/英")
                )
            )
        }

        // Spacebar: `colRow1X[2] .. (colRow1X[6] + stdKeyW)` (`d.minX .. j.maxX`)!
        let spaceMinX = colRow1X[2]
        let spaceMaxX = colRow1X[6] + stdKeyW
        specs.append(
            IOSKeySpec(
                id: "B_space",
                kind: .space,
                frame: CGRect(x: spaceMinX, y: rowY[3], width: spaceMaxX - spaceMinX, height: keyH),
                primaryLabel: "",
                cornerSubscript: spaceCornerTag
            )
        )

        // Return Key: `colRow1X[7] .. (w - outerX)` (`k.minX .. p.maxX`)!
        let returnMinX = colRow1X[7]
        let returnMaxX = w - outerX
        let useBlue = returnRole.isBlueProminent
        specs.append(
            IOSKeySpec(
                id: "B_return",
                kind: .returnKey,
                frame: CGRect(x: returnMinX, y: rowY[3], width: returnMaxX - returnMinX, height: keyH),
                primaryLabel: isTrackpadActive ? "" : (useBlue ? returnRole.title : ""),
                systemSymbol: isTrackpadActive ? nil : (useBlue ? nil : "return.left"),
                isBlueAccent: useBlue
            )
        )

        // Compute exact 26-key normalized coordinates matching our actual Row 0/1/2 geometric centers.
        // Ergonomic thumb Y-bias is applied consistently via `compensatedLetterPoint(_:in:)` before
        // both UI `hitTest` and Rust `pushTouch`, keeping frontend and backend coordinates 1:1 aligned.
        let qMidX = outerX + stdKeyW * 0.5
        let qMidY = rowY[0] + keyH * 0.5
        var centers52 = [Float](repeating: 0, count: 52)
        for (c, b) in Array("qwertyuiop".utf8).enumerated() {
            let idx = Int(b - UInt8(ascii: "a"))
            centers52[idx * 2] = Float(c)
            centers52[idx * 2 + 1] = 0.0
        }
        for (c, b) in Array("asdfghjkl".utf8).enumerated() {
            let idx = Int(b - UInt8(ascii: "a"))
            centers52[idx * 2] = Float(c) + 0.5
            centers52[idx * 2 + 1] = 1.0
        }
        for (c, b) in Array("zxcvbnm".utf8).enumerated() {
            let idx = Int(b - UInt8(ascii: "a"))
            centers52[idx * 2] = Float(c) + 1.5
            centers52[idx * 2 + 1] = 2.0
        }

        return LayoutSnapshot(
            bounds: CGRect(origin: .zero, size: CGSize(width: w, height: h)),
            keys: specs,
            centers52: centers52,
            originX: Float(qMidX),
            originY: Float(qMidY),
            pitchX: Float(pitchX),
            pitchY: Float(pitchY),
            bottomRowMinY: rowY[3]
        )
    }

    /// Apply consistent row-aware ergonomic upward thumb compensation for letter taps,
    /// shared by both frontend `hitTest` and backend `pushTouch` coordinate normalization.
    public static func compensatedLetterPoint(_ point: CGPoint, in snapshot: LayoutSnapshot) -> CGPoint {
        var qPt = point
        let row0MidY = CGFloat(snapshot.originY)
        if point.y > row0MidY && point.y < snapshot.bottomRowMinY - 5.0 {
            qPt.y -= ergonomicThumbYBias
        }
        return qPt
    }

    public static func hitTest(
        point: CGPoint,
        in snapshot: LayoutSnapshot,
        validNextMask: UInt32 = 0x03FF_FFFF,
        nextKeyPriors: [UInt8] = [],
        touchOffsets: [Float] = []
    ) -> IOSKeySpec? {
        guard snapshot.bounds.insetBy(dx: -18, dy: -28).contains(point) else {
            return nil
        }

        // 1. Functional keys (`Space`, `⌫`, `⇧`, `123`, `中/英`, `Globe`, `Return`) are checked against
        //    unbiased `point` first so top-edge taps on `⌫`/`⇧`/`Space` are never pulled up into letters.
        for k in snapshot.keys {
            switch k.kind {
            case .letter, .symbol:
                continue
            default:
                if k.frame.contains(point) {
                    return k
                }
            }
        }

        // 2. Apply ergonomic upward compensation strictly inside the 3 letter/symbol rows (never near Row 3
        //    `Space`/`123`/`Return`, so top-edge Space taps never get pulled into `c`/`v`/`b`/`n`!).
        let qPt = compensatedLetterPoint(point, in: snapshot)

        let hasMaskWeights = (validNextMask != 0x03FF_FFFF) && (validNextMask != 0)
        let hasNeuralPriors = (nextKeyPriors.count == 26) && nextKeyPriors.contains(where: { $0 != 100 })
        let hasTouchOffsets = (touchOffsets.count == 52) && touchOffsets.contains(where: { abs($0) > 1e-4 })
        let hasDynamicWeights = hasMaskWeights || hasNeuralPriors || hasTouchOffsets

        let personalShift: (UInt8) -> (CGFloat, CGFloat) = { ascii in
            guard hasTouchOffsets, ascii >= 97, ascii <= 122 else { return (0, 0) }
            let idx = Int(ascii - 97)
            let dx = CGFloat(max(-0.20, min(0.20, touchOffsets[idx * 2]))) * CGFloat(snapshot.pitchX)
            let dy = CGFloat(max(-0.20, min(0.20, touchOffsets[idx * 2 + 1]))) * CGFloat(snapshot.pitchY)
            return (dx, dy)
        }

        if !hasDynamicWeights {
            if let direct = snapshot.keys.first(where: { $0.frame.contains(qPt) }) {
                return direct
            }
        } else {
            // Sacred Keycap Core: any tap landing cleanly inside the visual interior (`>= 3.0pt` from X bevel,
            // `>= 2.6pt` from Y bevel, shifted gently by personal TouchProfile) ALWAYS belongs to that keycap
            // even if `validNextMask` marks it 0.
            // Dynamic weighting only arbitrates border/bevel (`0..3pt`) & inter-key gutter taps!
            for k in snapshot.keys {
                switch k.kind {
                case .letter(let ascii):
                    let (px, py) = personalShift(ascii)
                    let core = k.frame.offsetBy(dx: px * 0.5, dy: py * 0.5).insetBy(dx: 3.0, dy: 2.6)
                    if core.contains(qPt) {
                        return k
                    }
                case .symbol:
                    if k.frame.contains(qPt) {
                        return k
                    }
                default:
                    break
                }
            }
        }

        var best: IOSKeySpec?
        var bestScore = CGFloat.greatestFiniteMagnitude
        let maxLetterBottomY = snapshot.bottomRowMinY - 6.0

        for k in snapshot.keys {
            var effRect = k.frame
            var deadKeyPenalty: CGFloat = 0
            var evalPt = qPt

            switch k.kind {
            case .space:
                // Evaluate Row 3 against raw `point` and give Spacebar generous top (`4.5pt`), side (`2.2pt`),
                // and bottom (`10pt`) cushions so light grazing thumb taps on any rim of Spacebar never miss!
                evalPt = point
                effRect = CGRect(
                    x: k.frame.minX - 2.2,
                    y: k.frame.minY - 4.5,
                    width: k.frame.width + 4.4,
                    height: k.frame.height + 14.5
                )
            case .planeSwitch(_, let lbl) where k.frame.minY >= snapshot.bottomRowMinY - 1.0 && lbl != "#+=":
                evalPt = point
                effRect = CGRect(
                    x: k.frame.minX - 2.0,
                    y: k.frame.minY - 3.0,
                    width: k.frame.width + 2.0,
                    height: k.frame.height + 12.0
                )
            case .modeToggle, .globe, .returnKey:
                evalPt = point
                effRect = CGRect(
                    x: k.frame.minX,
                    y: k.frame.minY - 3.0,
                    width: k.frame.width,
                    height: k.frame.height + 12.0
                )
            case .shift, .backspace:
                evalPt = point
            case .letter(let ascii) where hasDynamicWeights && ascii >= 97 && ascii <= 122:
                let (px, py) = personalShift(ascii)
                effRect = effRect.offsetBy(dx: px, dy: py)
                let idx = Int(ascii - 97)
                let bit = UInt32(1) << UInt32(idx)
                let pVal = (nextKeyPriors.count == 26) ? Int(nextKeyPriors[idx]) : ((validNextMask & bit) != 0 ? 100 : 0)
                if (hasMaskWeights && (validNextMask & bit) == 0) || pVal == 0 {
                    effRect = effRect.insetBy(dx: 2.8, dy: 2.2)
                    deadKeyPenalty = 4.2
                } else {
                    // Modulate gutter/bevel reach continuously by `Micro-RWKV-7` next-key log-prior (`pVal = 54..176`, `100` = neutral)
                    let bias = CGFloat(pVal - 100) / 25.0
                    let expX: CGFloat
                    let expY: CGFloat
                    if !hasMaskWeights {
                        // Key #1 predictive gutter mode (`validNextMask == ALL_26`)
                        expX = max(-1.6, min(3.8, bias * 1.45))
                        expY = max(-1.0, min(2.4, bias * 0.95))
                        deadKeyPenalty = -bias * 0.85
                    } else {
                        // Keys #2..#4 valid continuation mode (breaks valid-vs-valid gutter ties!)
                        expX = max(1.2, min(6.2, 4.8 + bias * 1.15))
                        expY = max(0.8, min(4.0, 3.2 + bias * 0.75))
                        deadKeyPenalty = -bias * 1.15
                    }
                    effRect = effRect.insetBy(dx: -expX, dy: -expY)
                    if effRect.maxY > maxLetterBottomY {
                        effRect.size.height = max(k.frame.height, maxLetterBottomY - effRect.minY)
                    }
                }
            default:
                break
            }

            let dx = max(effRect.minX - evalPt.x, 0, evalPt.x - effRect.maxX)
            let dy = max(effRect.minY - evalPt.y, 0, evalPt.y - effRect.maxY)
            let score: CGFloat
            if dx == 0 && dy == 0 {
                // Point is inside `effRect`: negative signed depth to nearest edge breaks ties cleanly!
                let innerDepth = min(
                    evalPt.x - effRect.minX,
                    effRect.maxX - evalPt.x,
                    evalPt.y - effRect.minY,
                    effRect.maxY - evalPt.y
                )
                score = -innerDepth + deadKeyPenalty * 0.25
            } else {
                score = hypot(dx, dy) + deadKeyPenalty
            }

            if score < bestScore {
                bestScore = score
                best = k
            }
        }
        return best
    }

    /// Native iOS 26 Chinese keyboard stay-vs-return rules for `123` and `#+=` planes:
    /// - On `123`: digits (`0-9`) and numeric/time/currency/path connectors (`- / : ： . ￥ ¥ $`) stay on `123`;
    ///   sentence punctuation, quotes, brackets, `@`, `&`, `;` return to `.letters`.
    /// - On `#+=`: `~`, `～`, `…` stay on `#+=`; brackets, math, and punctuation symbols return to `.letters`.
    public static func shouldReturnToLettersAfterSymbol(_ sym: String, on plane: IOSKeyPlane) -> Bool {
        if sym.allSatisfy(\.isNumber) {
            return false
        }
        switch plane {
        case .numbers:
            let staySet: Set<String> = ["-", "/", ":", "：", ".", "￥", "¥", "$"]
            return !staySet.contains(sym)
        case .symbols:
            let staySet: Set<String> = ["~", "～", "…"]
            return !staySet.contains(sym)
        case .letters:
            return false
        }
    }

    public static let cangjieRadicalTable: [String] = [
        "日", "月", "金", "木", "水", "火", "土", "竹", "戈", "十", "大", "中", "一",
        "弓", "人", "心", "手", "口", "尸", "廿", "山", "女", "田", "難", "卜", "重",
    ]

    public static func cangjieRadical(for ascii: UInt8) -> String? {
        let idx: Int
        if ascii >= UInt8(ascii: "a") && ascii <= UInt8(ascii: "z") {
            idx = Int(ascii - UInt8(ascii: "a"))
        } else if ascii >= UInt8(ascii: "A") && ascii <= UInt8(ascii: "Z") {
            idx = Int(ascii - UInt8(ascii: "A"))
        } else {
            return nil
        }
        return cangjieRadicalTable[idx]
    }

    public var showsCangjieRadicals: Bool {
        cangjieKeycapRadicals
            && !asciiMode
            && !shiftState.isUppercase
            && (schemaId == 33 || schemaId == 35 || schemaId == 36)
    }

    private func formatLetter(_ ascii: UInt8) -> String {
        if isTrackpadActive {
            return ""
        }
        if showsCangjieRadicals, let rad = Self.cangjieRadical(for: ascii) {
            return rad
        }
        let s = String(UnicodeScalar(ascii))
        return shiftState.isUppercase ? s.uppercased() : s.lowercased()
    }

    private func appendTenRow(
        _ labels: [String],
        rowIdx: Int,
        rowY: CGFloat,
        outerX: CGFloat,
        pitchX: CGFloat,
        keyW: CGFloat,
        keyH: CGFloat,
        into specs: inout [IOSKeySpec]
    ) {
        for (c, s) in labels.enumerated() {
            let x = outerX + CGFloat(c) * pitchX
            specs.append(
                IOSKeySpec(
                    id: "S_\(rowIdx)_\(c)",
                    kind: .symbol(s),
                    frame: CGRect(x: x, y: rowY, width: keyW, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : s
                )
            )
        }
    }

    private func appendSpanSymbols(
        _ labels: [String],
        rowY: CGFloat,
        minX: CGFloat,
        maxX: CGFloat,
        gapX: CGFloat,
        keyH: CGFloat,
        into specs: inout [IOSKeySpec]
    ) {
        guard !labels.isEmpty else { return }
        let n = CGFloat(labels.count)
        let kw = ((maxX - minX) - gapX * (n - 1)) / n
        for (i, s) in labels.enumerated() {
            let x = minX + CGFloat(i) * (kw + gapX)
            specs.append(
                IOSKeySpec(
                    id: "SM_\(i)",
                    kind: .symbol(s),
                    frame: CGRect(x: x, y: rowY, width: kw, height: keyH),
                    primaryLabel: isTrackpadActive ? "" : s
                )
            )
        }
    }
}

public struct SpaceTrackpadAccumulator {
    public private(set) var isActive: Bool = false
    private var anchorX: CGFloat = 0
    private var anchorY: CGFloat = 0
    private var lastUpdateTimestamp: TimeInterval = 0
    public let stepPoints: CGFloat = 9.5
    public let lineStepPoints: CGFloat = 24.0

    public init() {}

    public mutating func begin(at x: CGFloat) {
        begin(at: CGPoint(x: x, y: 0))
    }

    public mutating func begin(at point: CGPoint, timestamp: TimeInterval = 0) {
        isActive = true
        anchorX = point.x
        anchorY = point.y
        lastUpdateTimestamp = timestamp
    }

    public mutating func update(x: CGFloat) -> Int {
        update(point: CGPoint(x: x, y: anchorY)).dxSteps
    }

    public mutating func update(point: CGPoint, timestamp: TimeInterval = 0) -> (dxSteps: Int, dySteps: Int) {
        guard isActive else { return (0, 0) }
        let diffX = point.x - anchorX
        let diffY = point.y - anchorY

        var rawXSteps = Int((diffX / stepPoints).rounded(.towardZero))
        if rawXSteps != 0 {
            anchorX += CGFloat(rawXSteps) * stepPoints
            // Gentle velocity boost when sweeping fast horizontally across the trackpad
            if timestamp > 0 && lastUpdateTimestamp > 0 {
                let dt = max(0.004, timestamp - lastUpdateTimestamp)
                let speed = abs(diffX) / CGFloat(dt)
                if speed > 520 && abs(rawXSteps) == 1 {
                    rawXSteps *= 2
                }
            }
        }

        let dySteps = Int((diffY / lineStepPoints).rounded(.towardZero))
        if dySteps != 0 {
            anchorY += CGFloat(dySteps) * lineStepPoints
        }

        if rawXSteps != 0 || dySteps != 0 {
            lastUpdateTimestamp = timestamp
        }
        return (rawXSteps, dySteps)
    }

    public mutating func end() {
        isActive = false
    }
}
