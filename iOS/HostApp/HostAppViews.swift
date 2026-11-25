import SwiftUI
#if os(iOS)
import UIKit
import WebKit

private struct SandboxWebView: UIViewRepresentable {
    func makeUIView(context: Context) -> WKWebView {
        let wv = WKWebView()
        wv.loadHTMLString("""
        <!doctype html><html><meta name="viewport" content="width=device-width,initial-scale=1">
        <body style="margin:0;padding:4px">
        <textarea id="wta" aria-label="WebBox" style="width:95%;height:44px;font-size:16px"></textarea>
        </body></html>
        """, baseURL: nil)
        return wv
    }
    func updateUIView(_ uiView: WKWebView, context: Context) {}
}
#endif

private extension View {
    func iosSettingsCard() -> some View {
        padding(14)
            .background(
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .fill(Color.primary.opacity(0.055))
            )
    }
}

public enum IOSHostTab: Int, CaseIterable, Identifiable {
    case home = 0
    case typing = 1
    case typo = 2
    case lexicon = 3

    public var id: Int { rawValue }

    public var title: String {
        switch self {
        case .home: return "首页与试手"
        case .typing: return "输入设置"
        case .typo: return "误触与热区"
        case .lexicon: return "个人词库"
        }
    }

    public var icon: String {
        switch self {
        case .home: return "keyboard.badge.ellipsis"
        case .typing: return "slider.horizontal.3"
        case .typo: return "hand.point.up.left"
        case .lexicon: return "text.book.closed"
        }
    }
}

public struct IOSHostRootView: View {
    @ObservedObject public var bridge: JuxuBridge
    @State public var selectedTab: IOSHostTab
    @State private var sandboxText: String = ""
    @State private var sandboxSecondText: String = ""
    @State private var sandboxEditorText: String = ""
    @FocusState private var sandboxFocused: Bool
    @FocusState private var sandboxSecondFocused: Bool
    @State private var newWord: String = ""
    @FocusState private var newWordFocused: Bool
    @State private var wordFeedback: String = ""
    @State private var exportedEvalLogURL: URL?

    public init(bridge: JuxuBridge, initialTab: IOSHostTab = .home) {
        self.bridge = bridge
        self._selectedTab = State(initialValue: initialTab)
    }

    public var body: some View {
        #if os(iOS)
        TabView(selection: $selectedTab) {
            homeTab
                .tabItem { Label(IOSHostTab.home.title, systemImage: IOSHostTab.home.icon) }
                .tag(IOSHostTab.home)

            typingTab
                .tabItem { Label(IOSHostTab.typing.title, systemImage: IOSHostTab.typing.icon) }
                .tag(IOSHostTab.typing)

            typoTab
                .tabItem { Label(IOSHostTab.typo.title, systemImage: IOSHostTab.typo.icon) }
                .tag(IOSHostTab.typo)
                .onAppear {
                    bridge.reloadTouchCalibration()
                    refreshEvalLogState()
                }

            lexiconTab
                .tabItem { Label(IOSHostTab.lexicon.title, systemImage: IOSHostTab.lexicon.icon) }
                .tag(IOSHostTab.lexicon)
                .onAppear {
                    bridge.loadConfigAndUser()
                }
        }
        .onOpenURL { url in
            handleDeepLink(url)
        }
        #else
        VStack(spacing: 0) {
            Group {
                switch selectedTab {
                case .home: homeTab
                case .typing: typingTab
                case .typo: typoTab
                case .lexicon: lexiconTab
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            Divider()
            HStack(spacing: 0) {
                ForEach(IOSHostTab.allCases) { tab in
                    Button {
                        selectedTab = tab
                    } label: {
                        VStack(spacing: 3) {
                            Image(systemName: tab.icon)
                                .font(.system(size: 17, weight: .medium))
                            Text(tab.title)
                                .font(.system(size: 10, weight: .medium))
                        }
                        .foregroundStyle(selectedTab == tab ? Color.blue : Color.secondary)
                        .frame(maxWidth: .infinity)
                        .padding(.top, 7)
                        .padding(.bottom, 3)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        #endif
    }

    // MARK: - Tab 1: Home, Enable Guide & Live Typing Sandbox

    private var homeTab: some View {
        iosScreenContainer(title: "") {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    // Hero Banner
                    HStack(spacing: 14) {
                        ZStack {
                            RoundedRectangle(cornerRadius: 13, style: .continuous)
                                .fill(
                                    LinearGradient(
                                        colors: [
                                            Color(red: 0.98, green: 0.97, blue: 0.94),
                                            Color(red: 0.91, green: 0.89, blue: 0.85),
                                        ],
                                        startPoint: .top,
                                        endPoint: .bottom
                                    )
                                )
                                .overlay(
                                    RoundedRectangle(cornerRadius: 13, style: .continuous)
                                        .strokeBorder(Color.black.opacity(0.08), lineWidth: 0.7)
                                )
                                .frame(width: 54, height: 54)

                            Canvas { ctx, size in
                                let s = min(size.width, size.height) / 100.0
                                var nib = Path()
                                nib.move(to: CGPoint(x: 50 * s, y: 14 * s))
                                nib.addLine(to: CGPoint(x: 78 * s, y: 54 * s))
                                nib.addLine(to: CGPoint(x: 68 * s, y: 78 * s))
                                nib.addLine(to: CGPoint(x: 32 * s, y: 78 * s))
                                nib.addLine(to: CGPoint(x: 22 * s, y: 54 * s))
                                nib.closeSubpath()
                                let ink = Color(red: 0.11, green: 0.12, blue: 0.15)
                                let vermilion = Color(red: 0.86, green: 0.27, blue: 0.21)
                                ctx.fill(nib, with: .color(ink))

                                var slit = Path()
                                slit.move(to: CGPoint(x: 50 * s, y: 15 * s))
                                slit.addLine(to: CGPoint(x: 50 * s, y: 49 * s))
                                ctx.stroke(slit, with: .color(vermilion), style: StrokeStyle(lineWidth: 4.5 * s, lineCap: .round))

                                let eye = CGRect(x: 43.5 * s, y: 45.5 * s, width: 13 * s, height: 13 * s)
                                ctx.fill(Path(ellipseIn: eye), with: .color(vermilion))

                                let bar = CGRect(x: 30 * s, y: 82 * s, width: 40 * s, height: 7 * s)
                                ctx.fill(Path(roundedRect: bar, cornerRadius: 3.5 * s), with: .color(ink))
                            }
                            .frame(width: 38, height: 38)
                        }
                        VStack(alignment: .leading, spacing: 3) {
                            Text("句序 Juxu")
                                .font(.system(size: 20, weight: .bold))
                            Text("当前激活：\(bridge.state.schemaName) · 零拷贝 mmap 内核")
                                .font(.system(size: 13))
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                    }
                    .padding(.top, 4)

                    // Live Typing Test Box (placed near top so focusing never scrolls hero under status bar)
                    VStack(alignment: .leading, spacing: 8) {
                        HStack {
                            Label("键盘实时试手区", systemImage: "character.cursor.ibeam")
                                .font(.system(size: 14, weight: .semibold))
                            Spacer()
                            if !sandboxText.isEmpty || !sandboxSecondText.isEmpty || !sandboxEditorText.isEmpty {
                                Button("清空") {
                                    sandboxText = ""
                                    sandboxSecondText = ""
                                    sandboxEditorText = ""
                                }
                                .font(.system(size: 13))
                                .foregroundStyle(.secondary)
                            }
                            if sandboxFocused || sandboxSecondFocused || ProcessInfo.processInfo.arguments.contains("--dual-sandbox") {
                                Button("完成") {
                                    dismissKeyboard()
                                }
                                .font(.system(size: 13.5, weight: .semibold))
                                .foregroundStyle(Color.accentColor)
                                .accessibilityIdentifier("JuxuSandboxDone")
                            }
                        }
                        TextField("点此弹出键盘试打汉字、长按空格滑动移光标…", text: $sandboxText)
                            .textFieldStyle(.roundedBorder)
                            .focused($sandboxFocused)
                            .accessibilityIdentifier("JuxuSandboxPrimary")
                            .submitLabel(.done)
                            .onSubmit {
                                dismissKeyboard()
                            }
                        if ProcessInfo.processInfo.arguments.contains("--dual-sandbox") {
                            TextField("第二个输入框", text: $sandboxSecondText)
                                .textFieldStyle(.roundedBorder)
                                .focused($sandboxSecondFocused)
                                .accessibilityIdentifier("JuxuSandboxSecondary")
                                .submitLabel(.done)
                                .onSubmit {
                                    dismissKeyboard()
                                }
                            TextEditor(text: $sandboxEditorText)
                                .frame(height: 54)
                                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.secondary.opacity(0.3)))
                                .accessibilityIdentifier("JuxuSandboxEditor")
                            #if os(iOS)
                            SandboxWebView()
                                .frame(height: 60)
                                .accessibilityIdentifier("JuxuSandboxWeb")
                            #endif
                        }
                    }
                    .iosSettingsCard()

                    // Enable Keyboard Steps
                    VStack(alignment: .leading, spacing: 11) {
                        HStack {
                            Label("三步启用系统键盘", systemImage: "checkmark.shield.fill")
                                .font(.system(size: 15, weight: .semibold))
                            Spacer()
                            Text("默认零联网")
                                .font(.system(size: 11, weight: .medium))
                                .foregroundStyle(.green)
                                .padding(.horizontal, 7)
                                .padding(.vertical, 2.5)
                                .background(Color.green.opacity(0.14), in: Capsule())
                        }

                        stepLine(num: "1", text: "进入「设置 → 通用 → 键盘 → 键盘 → 添加新键盘」")
                        stepLine(num: "2", text: "在第三方键盘中选择「句序」即可离线极速盲打")
                        stepLine(num: "3", text: "如需候选栏剪贴板提示，开启「允许完全访问」并在「设置 → App → 句序 → 从其他 App 粘贴」设为「允许」")

                        Button {
                            openSystemKeyboardSettings()
                        } label: {
                            HStack {
                                Spacer()
                                Image(systemName: "gearshape.fill")
                                Text("打开系统键盘设置")
                                    .fontWeight(.semibold)
                                Spacer()
                            }
                            .padding(.vertical, 10)
                            .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .foregroundStyle(.white)
                        }
                        .buttonStyle(.plain)
                        .padding(.top, 2)
                    }
                    .iosSettingsCard()
                }
                .padding(16)
            }
            #if os(iOS)
            .scrollDismissesKeyboard(.interactively)
            .onTapGesture {
                if sandboxFocused || sandboxSecondFocused {
                    dismissKeyboard()
                }
            }
            #endif
            .onAppear {
                if ProcessInfo.processInfo.arguments.contains("--autofocus") {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                        sandboxFocused = true
                    }
                }
            }
        }
    }

    // MARK: - Tab 2: Typing & Schema Rules

    private var typingTab: some View {
        let activeCategory = bridge.prefs.schemaCategory
        let isCangjie = bridge.prefs.isCangjieFamily
        let maxCodeLabel = SharedSchemaDisplayHelpers.maxCodeChineseNumeral(isCangjie: isCangjie)
        let pushCodeLabel = SharedSchemaDisplayHelpers.pushCodeChineseNumeral(isCangjie: isCangjie)
        let maxCodeDigit = "\(SharedSchemaDisplayHelpers.maxCodeDigits(isCangjie: isCangjie))"
        let pushCodeDigit = "\(SharedSchemaDisplayHelpers.pushCodeDigits(isCangjie: isCangjie))"
        return iosScreenContainer(title: "输入设置") {
            ScrollView {
                VStack(spacing: 14) {
                    // 2-Level Schema Switcher: Category (拼音/五笔/仓颉) -> Version
                    VStack(alignment: .leading, spacing: 12) {
                        VStack(alignment: .leading, spacing: 8) {
                            HStack {
                                Text("方案大类")
                                    .font(.system(size: 13, weight: .semibold))
                                    .foregroundStyle(.secondary)
                                Spacer()
                                Text(bridge.state.schemaName)
                                    .font(.system(size: 12, weight: .medium))
                                    .foregroundStyle(Color.accentColor)
                            }

                            HStack(spacing: 6) {
                                ForEach(InputSchemaCategory.allCases) { category in
                                    compactCategoryButton(category)
                                }
                            }
                            .padding(3)
                            .background(
                                RoundedRectangle(cornerRadius: 11, style: .continuous)
                                    .fill(Color.primary.opacity(0.06))
                            )
                        }

                        Divider()

                        VStack(alignment: .leading, spacing: 8) {
                            Text("\(activeCategory.rawValue)方案版本")
                                .font(.system(size: 13, weight: .semibold))
                                .foregroundStyle(.secondary)

                            HStack(spacing: 8) {
                                ForEach(InputSchemaOption.options(for: activeCategory)) { option in
                                    compactSchemaButton(id: option.id, name: option.shortTitle, tag: option.tag)
                                }
                            }
                        }
                    }
                    .iosSettingsCard()

                    if !bridge.prefs.isPinyinFamily {
                        VStack(spacing: 12) {
                            if isCangjie {
                                iosToggleRow(
                                    "键盘键帽显示仓颉码",
                                    sub: "将字母键替换为仓颉字根（日月金木水火土…），切英文或大写时自动恢复字母",
                                    isOn: binding(\.cangjieKeycapRadicals)
                                )
                                Divider()
                            } else if bridge.prefs.isWubiFamily {
                                iosToggleRow(
                                    "键盘键帽显示五笔字根",
                                    sub: "在字母键下方显示当前五笔版本（86 / 98 / 新世纪）的核心字根，切英文或大写时自动隐藏",
                                    isOn: binding(\.wubiKeycapRadicals)
                                )
                                Divider()
                            }
                            iosToggleRow("\(maxCodeLabel)码唯一自动上屏", sub: "精确\(maxCodeLabel)码唯一匹配时直接上屏", isOn: binding(\.fourthCodeAutoCommit))
                            Divider()
                            iosToggleRow("第\(pushCodeLabel)码自动顶字", sub: "\(maxCodeLabel)码重码时敲第 \(pushCodeDigit) 码将首选词顶上屏", isOn: binding(\.fifthCodePushCommit))
                            Divider()
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text("\(maxCodeLabel)码空码行为")
                                        .font(.system(size: 15))
                                    Text("无匹配且无高置信度纠错时的动作")
                                        .font(.system(size: 12))
                                        .foregroundStyle(.secondary)
                                }
                                Spacer()
                                Picker("", selection: binding(\.fourthEmptyBehavior)) {
                                    Text("阻断第\(maxCodeDigit)码").tag("block")
                                    Text("清空重输").tag("clear")
                                }
                                .labelsHidden()
                            }
                        }
                        .iosSettingsCard()
                    }

                    VStack(spacing: 12) {
                        if bridge.prefs.isPinyinFamily {
                            if bridge.prefs.schemaId == 20 {
                                iosToggleRow(
                                    "九宫格全拼键盘（T9）",
                                    sub: "触屏中文全拼切为九宫格，支持键级高斯触点纠错、动态热区与左侧音节筛选；切英文自动恢复 26 键",
                                    isOn: binding(\.pinyinT9Layout)
                                )
                                Divider()
                            }
                            iosToggleRow(
                                "声母简拼",
                                sub: bridge.prefs.schemaId == 21
                                    ? "支持单键声母简拼（u/i/v 对应 sh/ch/zh，如 urf → 输入法）"
                                    : "支持首字母简拼及混拼（如 srf / shurf → 输入法）",
                                isOn: binding(\.pinyinInitialAbbr)
                            )
                            Divider()
                        } else {
                            iosToggleRow("流式无空格连打（实验）", sub: "连续输入词组和简码。标点确认中文，回车输入原始字母", isOn: binding(\.spaceless))
                            Divider()
                            iosToggleRow(
                                "最短编码（简码）优先",
                                sub: isCangjie ? "1–4 码精确匹配始终排在补全项之前" : "1–3 码简码始终排在补全项之前",
                                isOn: binding(\.shortestCodeFirst)
                            )
                            Divider()
                            iosToggleRow("全码显示 ⚡ 简码提示", sub: "打全码时提示更短简码", isOn: binding(\.showShortcodeHint))
                            Divider()
                            iosToggleRow(
                                "出简不出全",
                                sub: bridge.prefs.spaceless
                                    ? "优先按首选简码切分单字，较长的编码仍可出字"
                                    : "有首选简码的单字在\(maxCodeLabel)码全码时后置",
                                isOn: binding(\.strictShortcode)
                            )
                            if bridge.prefs.isWubiFamily || isCangjie {
                                Divider()
                                iosToggleRow(
                                    isCangjie ? "z 键拼音反查" : "z 键通配与拼音反查",
                                    sub: isCangjie
                                        ? "首键按重(z)后自动切字母键帽，输拼音反查当前仓颉拆码"
                                        : "z 查码 + 输拼音反查五笔拆码",
                                    isOn: binding(\.zOmnipotent)
                                )
                            }
                            Divider()
                        }
                        iosToggleRow("中文标点与引号智能配对", sub: "打左引号、括号或书名号时自动补全右半边，光标停在中间", isOn: binding(\.smartPunctuation))
                        Divider()
                        iosToggleRow("中英文之间自动加空格", sub: "中英混输、直输英文及剪贴板插入时自动补全空格", isOn: binding(\.autoSpaceZhEn))
                        if bridge.prefs.autoSpaceZhEn {
                            Divider()
                            iosToggleRow("数字与中文之间也加空格", sub: "在汉字与数字边界自动加空格（如「iOS 26 版」）", isOn: binding(\.autoSpaceDigits))
                        }
                        Divider()
                        iosToggleRow("候选栏剪贴板提示", sub: "空闲时在候选栏中央显示最近复制内容，并自动提取剪贴板验证码（需开启完全访问）", isOn: binding(\.clipboardSuggestion))
                        Divider()
                        iosToggleRow("Emoji 候选", sub: "输入时，在匹配的词语后显示 Emoji 作为次要候选（如 思考 → 🤔）", isOn: binding(\.emojiCandidates))
                        Divider()
                        iosToggleRow("上滑输入数字和符号", sub: "在字母键上滑，输入键帽顶部的小字符", isOn: binding(\.keySwipeUpInput))
                        Divider()
                        iosToggleRow("空格旁逗号键（上滑句号）", sub: "在 26 键空格左侧显示逗号键，点按输入逗号，上滑输入句号；不受上一项开关影响", isOn: binding(\.spaceCommaKey))
                        Divider()
                        iosToggleRow("长按弹出更多字符", sub: "长按字母键后左右滑动，选择声调字母、大小写、数字或符号", isOn: binding(\.keyLongPressAlternates))
                        Divider()
                        iosToggleRow("退格键上滑清空本行", sub: "上滑删除光标左侧到行首的内容，下滑依次撤回；正在输入拼音时只清空拼音", isOn: binding(\.backspaceSwipeClear))
                        Divider()
                        iosToggleRow("全键盘长按移动光标", sub: "在键盘任意键上长按即可滑动光标；继续按住不放则会弹出按键的特殊字符", isOn: binding(\.keyboardHoldCursor))
                    }
                    .iosSettingsCard()
                }
                .padding(16)
            }
        }
    }

    // MARK: - Tab 3: Touch Hitbox Adaptation & Bayesian Typo Decoder

    private var typoTab: some View {
        iosScreenContainer(title: "误触与热区") {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    IOSTouchHitboxMapCard(
                        calibrations: bridge.touchCalibrations,
                        onReset: { bridge.resetTouchCalibration() }
                    )

                    VStack(spacing: 12) {
                        iosToggleRow("统计 + 触控高斯误触纠正", sub: "融合 2D 触点似然、语料 Bigram 与个人输入历史", isOn: binding(\.typoEnabled))
                        Divider()
                        iosToggleRow("纠错项抑制四码盲目上屏", sub: "非精确救回项留待空格或第 5 码确认，防顶错字", isOn: binding(\.suppressAutoCommitOnCorrection))
                        Divider()
                        iosToggleRow("语言模型漏空格切分纠错", sub: "简码/三码字漏按空格续打时智能顶屏切分，减少空码截停与误上屏", isOn: binding(\.missedSpaceCorrection))
                        Divider()
                        iosToggleRow("个人拇指热区 EMA 自适应", sub: "确认选词后微调每键判定中心（限幅 ±0.20 键宽）", isOn: binding(\.touchHitboxAdapt))
                        Divider()
                        sliderRow(
                            title: "空码 / 边缘滑偏直推首选阈值",
                            valueText: "\(Int(bridge.prefs.promoteThreshold * 100))%",
                            value: binding(\.promoteThreshold),
                            range: 0.60...0.96,
                            step: 0.02
                        )
                        Divider()
                        sliderRow(
                            title: "次选纠错候选提示阈值",
                            valueText: "\(Int(bridge.prefs.suggestThreshold * 100))%",
                            value: binding(\.suggestThreshold),
                            range: 0.15...0.70,
                            step: 0.05
                        )
                    }
                    .iosSettingsCard()

                    // Local Input Eval Log Card (for building real-world eval sets)
                    VStack(alignment: .leading, spacing: 11) {
                        iosToggleRow(
                            "记录输入与选词诊断日志",
                            sub: "仅本地保存手选非首选、纠错、退格重打与 2D 落点，用于制作 eval set 改进排序策略",
                            isOn: binding(\.evalLogging)
                        )
                        Divider()
                        let s = bridge.evalLogSummary
                        let metrics = EvalLogMetricItem.metrics(from: s, compactTop1Title: true)
                        HStack(spacing: 10) {
                            ForEach(metrics.prefix(3)) { metric in
                                SharedEvalMetricCell(item: metric, valueFontSize: 15, cornerRadius: 8)
                            }
                        }
                        HStack(spacing: 10) {
                            ForEach(metrics.dropFirst(3)) { metric in
                                SharedEvalMetricCell(item: metric, valueFontSize: 15, cornerRadius: 8)
                            }
                        }
                        HStack(spacing: 10) {
                            Button("刷新统计") {
                                refreshEvalLogState()
                            }
                            .buttonStyle(.bordered)
                            .controlSize(.small)

                            #if os(iOS)
                            if let exportURL = exportedEvalLogURL {
                                ShareLink(item: exportURL) {
                                    Label("导出 JSONL", systemImage: "square.and.arrow.up")
                                }
                                .buttonStyle(.borderedProminent)
                                .controlSize(.small)
                            }
                            #endif

                            Spacer()

                            Button("清空", role: .destructive) {
                                bridge.clearEvalLog()
                                if let oldURL = exportedEvalLogURL {
                                    try? FileManager.default.removeItem(at: oldURL)
                                }
                                exportedEvalLogURL = nil
                            }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                            .disabled(s.totalEvents == 0)
                        }
                    }
                    .iosSettingsCard()
                }
                .padding(16)
            }
        }
    }

    private func sliderRow(
        title: String,
        valueText: String,
        value: Binding<Double>,
        range: ClosedRange<Double>,
        step: Double
    ) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(title)
                    .font(.system(size: 14, weight: .medium))
                Spacer()
                Text(valueText)
                    .font(.system(size: 13, weight: .semibold, design: .monospaced))
                    .foregroundStyle(Color.accentColor)
            }
            Slider(value: value, in: range, step: step)
        }
    }

    // MARK: - Tab 4: Personal Lexicon & Auto-Phrase

    private var lexiconTab: some View {
        iosScreenContainer(title: "个人词库") {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    VStack(spacing: 12) {
                        iosToggleRow("连打单字自动组词", sub: "重复连打双字/三字组合后自动加入个人词库", isOn: binding(\.autoPhrase))
                        Divider()
                        iosToggleRow("词组动态调频", sub: "常用词组与上下文高频搭配自动前移", isOn: binding(\.dynamicWordFreq))
                        Divider()
                        iosToggleRow("单字动态调频", sub: "建议关闭以保持单字重码顺序稳定", isOn: binding(\.dynamicCharFreq))
                    }
                    .iosSettingsCard()

                    VStack(alignment: .leading, spacing: 10) {
                        Text("跨版本个人词条（切 86/98/新世纪自动重算编码）")
                            .font(.system(size: 14, weight: .semibold))

                        HStack(spacing: 8) {
                            TextField("输入新词条，如：句序", text: $newWord)
                                .textFieldStyle(.roundedBorder)
                                .focused($newWordFocused)
                                .submitLabel(.done)
                                .onSubmit {
                                    dismissKeyboard()
                                }
                            Button("添加") {
                                let code = bridge.addUserPhrase(newWord)
                                if !code.isEmpty {
                                    wordFeedback = "✓ 已添加「\(newWord)」(\(code))"
                                    newWord = ""
                                    dismissKeyboard()
                                }
                            }
                            .buttonStyle(.borderedProminent)
                            .disabled(newWord.trimmingCharacters(in: .whitespaces).count < 2)
                        }

                        if !wordFeedback.isEmpty {
                            Text(wordFeedback)
                                .font(.system(size: 12))
                                .foregroundStyle(Color.accentColor)
                        }

                        ForEach(bridge.userPhrases.prefix(20)) { item in
                            Divider()
                            HStack {
                                Text(item.text)
                                    .font(.system(size: 15, weight: .medium))
                                SharedUserPhraseCodePill(code: item.code, isAutoPhrase: item.auto)
                                Spacer()
                                Text("\(item.count) 次")
                                    .font(.system(size: 12))
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    .iosSettingsCard()
                }
                .padding(16)
            }
            #if os(iOS)
            .scrollDismissesKeyboard(.interactively)
            .onTapGesture {
                if newWordFocused {
                    dismissKeyboard()
                }
            }
            #endif
        }
    }

    // MARK: - Helpers

    @ViewBuilder
    private func iosScreenContainer<Content: View>(title: String, @ViewBuilder content: () -> Content) -> some View {
        #if os(iOS)
        NavigationStack {
            if title.isEmpty {
                content()
                    .toolbar(.hidden, for: .navigationBar)
            } else {
                content()
                    .navigationTitle(title)
            }
        }
        #else
        VStack(alignment: .leading, spacing: 0) {
            if !title.isEmpty {
                Text(title)
                    .font(.system(size: 27, weight: .bold))
                    .padding(.horizontal, 18)
                    .padding(.top, 6)
                    .padding(.bottom, 4)
            }
            content()
        }
        #endif
    }

    private func compactCategoryButton(_ category: InputSchemaCategory) -> some View {
        let active = bridge.prefs.schemaCategory == category
        return Button {
            if !active {
                bridge.switchSchema(category.defaultSchemaId)
            }
        } label: {
            HStack(spacing: 5) {
                Image(systemName: category.symbolName)
                    .font(.system(size: 12, weight: .semibold))
                Text(category.rawValue)
                    .font(.system(size: 14, weight: .semibold))
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
            .background(
                RoundedRectangle(cornerRadius: 8.5, style: .continuous)
                    .fill(active ? Color.accentColor : Color.clear)
            )
            .foregroundStyle(active ? Color.white : Color.primary)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func compactSchemaButton(id: UInt32, name: String, tag: String) -> some View {
        let active = bridge.prefs.schemaId == id
        return Button {
            bridge.switchSchema(id)
        } label: {
            VStack(spacing: 3) {
                Text(name)
                    .font(.system(size: 14, weight: .semibold))
                Text(tag)
                    .font(.system(size: 10.5))
                    .opacity(0.8)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 9)
            .background(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(active ? Color.accentColor : Color.primary.opacity(0.06))
            )
            .foregroundStyle(active ? Color.white : Color.primary)
        }
        .buttonStyle(.plain)
    }

    private func stepLine(num: String, text: String) -> some View {
        HStack(alignment: .top, spacing: 9) {
            Text(num)
                .font(.system(size: 11.5, weight: .bold, design: .rounded))
                .foregroundStyle(.white)
                .frame(width: 19, height: 19)
                .background(Color.accentColor, in: Circle())
            Text(text)
                .font(.system(size: 13))
                .foregroundStyle(.primary)
        }
    }

    private func iosToggleRow(_ title: String, sub: String, isOn: Binding<Bool>) -> some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 14.5, weight: .medium))
                Text(sub)
                    .font(.system(size: 11.5))
                    .foregroundStyle(.secondary)
            }
            Spacer()
            #if os(iOS)
            Toggle(title, isOn: isOn)
                .labelsHidden()
                .accessibilityLabel(title)
            #else
            Button {
                isOn.wrappedValue.toggle()
            } label: {
                ZStack(alignment: isOn.wrappedValue ? .trailing : .leading) {
                    Capsule(style: .continuous)
                        .fill(isOn.wrappedValue ? Color.green : Color.primary.opacity(0.18))
                        .frame(width: 44, height: 26)
                    Circle()
                        .fill(Color.white)
                        .shadow(color: .black.opacity(0.2), radius: 1.2, x: 0, y: 1)
                        .frame(width: 22, height: 22)
                        .padding(2)
                }
            }
            .buttonStyle(.plain)
            #endif
        }
    }

    private func binding<T>(_ kp: WritableKeyPath<JuxuPreferences, T>) -> Binding<T> {
        Binding(
            get: { bridge.prefs[keyPath: kp] },
            set: { v in
                var copy = bridge.prefs
                copy[keyPath: kp] = v
                bridge.applyAndSavePreferences(copy)
            }
        )
    }

    private func dismissKeyboard() {
        sandboxFocused = false
        sandboxSecondFocused = false
        newWordFocused = false
        #if os(iOS)
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
        #endif
    }

    private func handleDeepLink(_ url: URL) {
        guard url.scheme?.lowercased() == "juxu" else { return }
        dismissKeyboard()
        let dest = (url.host ?? url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))).lowercased()
        switch dest {
        case "settings", "typing", "schema":
            selectedTab = .typing
        case "typo", "hitbox":
            selectedTab = .typo
        case "lexicon", "phrases":
            selectedTab = .lexicon
        case "home", "sandbox":
            selectedTab = .home
        default:
            selectedTab = .typing
        }
    }

    private func refreshEvalLogState() {
        bridge.reloadEvalLogSummary()
        #if os(iOS)
        exportedEvalLogURL = bridge.evalLogSummary.totalEvents > 0 ? bridge.exportEvalLogFileURL() : nil
        #endif
    }

    private func openSystemKeyboardSettings() {
        #if os(iOS)
        if let url = URL(string: UIApplication.openSettingsURLString) {
            UIApplication.shared.open(url)
        }
        #endif
    }
}

/// Visualizes each of the 26 keys' learned personal thumb-offset vectors `(dx, dy)`!
public struct IOSTouchHitboxMapCard: View {
    public let calibrations: [TouchKeyCalibrationItem]
    public var onReset: () -> Void

    private let rows: [(String, CGFloat)] = [
        ("qwertyuiop", 0.0),
        ("asdfghjkl", 0.5),
        ("zxcvbnm", 1.5),
    ]

    public var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("26 键个人触控热区偏移图")
                        .font(.system(size: 14.5, weight: .semibold))
                    Text("用句序键盘选词后自动采样。橙点表示已学习的触点中心，偏移会参与误触纠正。")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button("重置") {
                    onReset()
                }
                .font(.system(size: 12, weight: .semibold))
                .buttonStyle(.bordered)
                .controlSize(.small)
            }

            GeometryReader { geo in
                let w = geo.size.width
                let pitchX = w / 10.0
                let keyH: CGFloat = 34
                let pitchY: CGFloat = 40

                ZStack(alignment: .topLeading) {
                    ForEach(0..<3, id: \.self) { r in
                        let (chars, indent) = rows[r]
                        ForEach(Array(chars.enumerated()), id: \.offset) { col, ch in
                            let keyStr = String(ch)
                            let cal = calibrations.first(where: { $0.key == keyStr })
                            let cx = (CGFloat(col) + indent + 0.5) * pitchX
                            let cy = 4 + CGFloat(r) * pitchY + keyH * 0.5
                            keyCell(
                                keyStr: keyStr,
                                cal: cal,
                                keyW: pitchX - 4,
                                keyH: keyH,
                                pitchX: pitchX,
                                pitchY: pitchY
                            )
                            .position(x: cx, y: cy)
                        }
                    }
                }
            }
            .frame(height: 124)
        }
        .iosSettingsCard()
    }

    @ViewBuilder
    private func keyCell(
        keyStr: String,
        cal: TouchKeyCalibrationItem?,
        keyW: CGFloat,
        keyH: CGFloat,
        pitchX: CGFloat,
        pitchY: CGFloat
    ) -> some View {
        let hasSamples = (cal?.samples ?? 0) > 0
        let dxPt = CGFloat(cal?.dx ?? 0) * pitchX * 3.0
        let dyPt = CGFloat(cal?.dy ?? 0) * pitchY * 3.0
        let startPt = CGPoint(x: keyW * 0.5, y: keyH * 0.5 + 6)
        let endPt = CGPoint(x: startPt.x + dxPt, y: startPt.y + dyPt)

        ZStack {
            RoundedRectangle(cornerRadius: 5, style: .continuous)
                .fill(Color.primary.opacity(hasSamples ? 0.11 : 0.05))
                .overlay(
                    RoundedRectangle(cornerRadius: 5, style: .continuous)
                        .strokeBorder(
                            hasSamples ? Color.orange.opacity(0.65) : Color.primary.opacity(0.10),
                            lineWidth: hasSamples ? 1.0 : 0.5
                        )
                )
                .frame(width: keyW, height: keyH)

            Text(keyStr.uppercased())
                .font(.system(size: 11, weight: .semibold, design: .rounded))
                .foregroundStyle(hasSamples ? .primary : .secondary)
                .offset(y: -7)

            Circle()
                .fill(Color.secondary.opacity(0.45))
                .frame(width: 3.5, height: 3.5)
                .offset(y: 6)

            if hasSamples {
                Path { p in
                    p.move(to: startPt)
                    p.addLine(to: endPt)
                }
                .stroke(Color.orange.opacity(0.85), lineWidth: 1.4)
                .frame(width: keyW, height: keyH)

                Circle()
                    .fill(Color.orange)
                    .frame(width: 6, height: 6)
                    .offset(x: dxPt, y: 6 + dyPt)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(keyStr.uppercased())
        .accessibilityValue(hasSamples ? "\(cal?.samples ?? 0) 次采样" : "尚未采样")
        .accessibilityIdentifier("TouchCalibration-\(keyStr)")
    }
}
