import Foundation
import SwiftUI

/// Shared formatting helpers for personal 26-key / T9 touch hitbox calibration offsets.
public enum SharedCalibrationFormatting {
    public static func activeCalibratedCount(in items: [TouchKeyCalibrationItem]) -> Int {
        items.reduce(0) { $0 + ($1.samples > 0 ? 1 : 0) }
    }

    public static func calibratedProgressText(calibratedCount: Int, totalKeys: Int = 26) -> String {
        "\(calibratedCount) / \(totalKeys)"
    }

    public static func formatSignedPitch(_ delta: Float) -> String {
        if abs(delta) < 0.005 {
            return "±0.00"
        }
        return String(format: "%+.2f", delta)
    }

    public static func formatSignedMillimeters(_ deltaPitch: Float, keyPitchMm: Float = 6.5) -> String {
        let mm = deltaPitch * keyPitchMm
        if abs(mm) < 0.005 {
            return "±0.00 mm"
        }
        return String(format: "%+.2f mm", mm)
    }

    public static func formatOffsetVector(dx: Float, dy: Float) -> String {
        "(\(formatSignedPitch(dx)), \(formatSignedPitch(dy)))"
    }
}

/// Shared display model for local evaluation log (`eval_log.jsonl`) summary statistics.
public struct EvalLogMetricItem: Identifiable, Equatable, Sendable {
    public let id: String
    public let title: String
    public let value: String

    public init(id: String, title: String, value: String) {
        self.id = id
        self.title = title
        self.value = value
    }

    public static func formatTop1Rate(_ summary: EvalLogSummary) -> String {
        summary.commits > 0 ? String(format: "%.1f%%", summary.top1RatePercent) : "—"
    }

    public static func metrics(
        from summary: EvalLogSummary,
        compactTop1Title: Bool = false
    ) -> [EvalLogMetricItem] {
        [
            EvalLogMetricItem(id: "commits", title: "总上屏", value: "\(summary.commits)"),
            EvalLogMetricItem(
                id: "top1",
                title: compactTop1Title ? "Top-1 率" : "Top-1 命中率",
                value: formatTop1Rate(summary)
            ),
            EvalLogMetricItem(id: "manual", title: "手选非首选", value: "\(summary.manualSelects)"),
            EvalLogMetricItem(id: "lm_overruled", title: "策略压制模型", value: "\(summary.lmOverruled)"),
            EvalLogMetricItem(id: "backspace", title: "退格重打", value: "\(summary.backspaceAfterCommit)"),
            EvalLogMetricItem(id: "typo", title: "纠错触发", value: "\(summary.typoTriggered)"),
        ]
    }
}

/// Shared schema code-length and candidate badge display helpers across macOS and iOS settings views.
public enum SharedSchemaDisplayHelpers {
    public static func maxCodeChineseNumeral(isCangjie: Bool) -> String {
        isCangjie ? "五" : "四"
    }

    public static func pushCodeChineseNumeral(isCangjie: Bool) -> String {
        isCangjie ? "六" : "五"
    }

    public static func maxCodeDigits(isCangjie: Bool) -> Int {
        isCangjie ? 5 : 4
    }

    public static func pushCodeDigits(isCangjie: Bool) -> Int {
        isCangjie ? 6 : 5
    }

    public static func option(for schemaId: UInt32) -> InputSchemaOption? {
        InputSchemaOption.all.first(where: { $0.id == schemaId })
    }

    public static func badgeSymbol(for kind: JuxuCandidateItem.BadgeKind) -> String? {
        switch kind {
        case .shortcode: return "⚡"
        case .typoCorrected: return "⌖"
        case .wildcard, .pinyinLookup: return "反查"
        case .userPhrase: return "词"
        case .emoji, .none: return nil
        }
    }
}

/// Cross-platform compact metric stat box for `EvalLogSummary` cards.
public struct SharedEvalMetricCell: View {
    public let item: EvalLogMetricItem
    public var valueFontSize: CGFloat = 14
    public var cornerRadius: CGFloat = 6

    public init(
        item: EvalLogMetricItem,
        valueFontSize: CGFloat = 14,
        cornerRadius: CGFloat = 6
    ) {
        self.item = item
        self.valueFontSize = valueFontSize
        self.cornerRadius = cornerRadius
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(item.title)
                .font(.system(size: 10.5))
                .foregroundStyle(.secondary)
            Text(item.value)
                .font(.system(size: valueFontSize, weight: .semibold, design: .monospaced))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(
            RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                .fill(Color.primary.opacity(0.045))
        )
    }
}

/// Cross-platform monospace code capsule for user phrase rows.
public struct SharedUserPhraseCodePill: View {
    public let code: String
    public let isAutoPhrase: Bool

    public init(code: String, isAutoPhrase: Bool = false) {
        self.code = code
        self.isAutoPhrase = isAutoPhrase
    }

    public var body: some View {
        HStack(spacing: 6) {
            Text(code)
                .font(.system(size: 11.5, design: .monospaced))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 6)
                .padding(.vertical, 1.5)
                .background(Color.primary.opacity(0.06), in: Capsule())
            if isAutoPhrase {
                Text("自动组词")
                    .font(.system(size: 10))
                    .foregroundStyle(.teal)
            }
        }
    }
}
