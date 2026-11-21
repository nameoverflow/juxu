import SwiftUI
#if os(iOS)
import UIKit
#endif

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
    @ObservedObject public var bridge: WubiBridge
    @State public var selectedTab: IOSHostTab
    @State private var sandboxText: String = ""
    @FocusState private var sandboxFocused: Bool
    @State private var newWord: String = ""
    @FocusState private var newWordFocused: Bool
    @State private var wordFeedback: String = ""

    public init(bridge: WubiBridge, initialTab: IOSHostTab = .home) {
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
                .onAppear { bridge.reloadTouchCalibration() }

            lexiconTab
                .tabItem { Label(IOSHostTab.lexicon.title, systemImage: IOSHostTab.lexicon.icon) }
                .tag(IOSHostTab.lexicon)
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
                            if !sandboxText.isEmpty {
                                Button("清空") {
                                    sandboxText = ""
                                }
                                .font(.system(size: 13))
                                .foregroundStyle(.secondary)
                            }
                            if sandboxFocused {
                                Button("完成") {
                                    dismissKeyboard()
                                }
                                .font(.system(size: 13.5, weight: .semibold))
                                .foregroundStyle(Color.accentColor)
                                .accessibilityIdentifier("PrisWubiSandboxDone")
                            }
                        }
                        TextField("点此弹出键盘试打汉字、长按空格滑动移光标…", text: $sandboxText)
                            .textFieldStyle(.roundedBorder)
                            .focused($sandboxFocused)
                            .submitLabel(.done)
                            .onSubmit {
                                dismissKeyboard()
                            }
                    }
                    .padding(14)
                    .background(iosCardBackground)

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
                    .padding(14)
                    .background(iosCardBackground)
                }
                .padding(16)
            }
            #if os(iOS)
            .scrollDismissesKeyboard(.interactively)
            .onTapGesture {
                if sandboxFocused {
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
        iosScreenContainer(title: "输入设置") {
            ScrollView {
                VStack(spacing: 14) {
                    // 1-Tap Schema Switcher Cards
                    VStack(alignment: .leading, spacing: 8) {
                        Text("输入方案快速切换")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(.secondary)

                        HStack(spacing: 8) {
                            compactSchemaButton(id: 20, name: "全拼", tag: "整句简拼")
                            compactSchemaButton(id: 21, name: "小鹤双拼", tag: "双拼整句")
                            compactSchemaButton(id: 10, name: "小鹤音形", tag: "双拼+鹤形")
                        }

                        HStack(spacing: 8) {
                            compactSchemaButton(id: 86, name: "86 版", tag: "经典五笔")
                            compactSchemaButton(id: 98, name: "98 版", tag: "规范码元")
                            compactSchemaButton(id: 6, name: "新世纪", tag: "06 国标")
                        }
                    }
                    .padding(14)
                    .background(iosCardBackground)

                    if !bridge.prefs.isPinyinFamily {
                        VStack(spacing: 12) {
                            iosToggleRow("四码唯一自动上屏", sub: "精确四码唯一匹配时直接上屏", isOn: binding(\.fourthCodeAutoCommit))
                            Divider()
                            iosToggleRow("第五码自动顶字", sub: "四码重码时敲第 5 码将首选词顶上屏", isOn: binding(\.fifthCodePushCommit))
                            Divider()
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text("四码空码行为")
                                        .font(.system(size: 15))
                                    Text("无匹配且无高置信度纠错时的动作")
                                        .font(.system(size: 12))
                                        .foregroundStyle(.secondary)
                                }
                                Spacer()
                                Picker("", selection: binding(\.fourthEmptyBehavior)) {
                                    Text("阻断第4码").tag("block")
                                    Text("清空重输").tag("clear")
                                }
                                .labelsHidden()
                            }
                        }
                        .padding(14)
                        .background(iosCardBackground)
                    }

                    VStack(spacing: 12) {
                        if bridge.prefs.isPinyinFamily {
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
                            iosToggleRow("最短编码（简码）优先", sub: "1–3 码简码始终排在补全项之前", isOn: binding(\.shortestCodeFirst))
                            Divider()
                            iosToggleRow("全码显示 ⚡ 简码提示", sub: "打全码时提示更短简码", isOn: binding(\.showShortcodeHint))
                            Divider()
                            iosToggleRow(
                                "出简不出全",
                                sub: bridge.prefs.spaceless
                                    ? "优先按首选简码切分单字，较长的编码仍可出字"
                                    : "有首选简码的单字在四码全码时后置",
                                isOn: binding(\.strictShortcode)
                            )
                            if bridge.prefs.schemaId != 10 {
                                Divider()
                                iosToggleRow("z 键通配与拼音反查", sub: "z 查码 + 输拼音反查五笔拆码", isOn: binding(\.zOmnipotent))
                            }
                            Divider()
                        }
                        iosToggleRow("中文标点与引号智能配对", sub: "自动转换全角标点并成对输入引号", isOn: binding(\.smartPunctuation))
                        Divider()
                        iosToggleRow("中英文之间自动加空格", sub: "中英混输、直输英文及剪贴板插入时自动补全空格", isOn: binding(\.autoSpaceZhEn))
                        if bridge.prefs.autoSpaceZhEn {
                            Divider()
                            iosToggleRow("数字与中文之间也加空格", sub: "在汉字与数字边界自动加空格（如「iOS 26 版」）", isOn: binding(\.autoSpaceDigits))
                        }
                        Divider()
                        iosToggleRow("候选栏剪贴板提示", sub: "空闲时在候选栏中央显示最近复制内容，并自动提取剪贴板验证码（需开启完全访问）", isOn: binding(\.clipboardSuggestion))
                    }
                    .padding(14)
                    .background(iosCardBackground)
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
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text("空码 / 边缘滑偏直推首选阈值")
                                    .font(.system(size: 14, weight: .medium))
                                Spacer()
                                Text("\(Int(bridge.prefs.promoteThreshold * 100))%")
                                    .font(.system(size: 13, weight: .semibold, design: .monospaced))
                                    .foregroundStyle(Color.accentColor)
                            }
                            Slider(value: binding(\.promoteThreshold), in: 0.60...0.96, step: 0.02)
                        }
                        Divider()
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text("次选纠错候选提示阈值")
                                    .font(.system(size: 14, weight: .medium))
                                Spacer()
                                Text("\(Int(bridge.prefs.suggestThreshold * 100))%")
                                    .font(.system(size: 13, weight: .semibold, design: .monospaced))
                                    .foregroundStyle(Color.accentColor)
                            }
                            Slider(value: binding(\.suggestThreshold), in: 0.15...0.70, step: 0.05)
                        }
                    }
                    .padding(14)
                    .background(iosCardBackground)
                }
                .padding(16)
            }
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
                    .padding(14)
                    .background(iosCardBackground)

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
                                Text(item.code)
                                    .font(.system(size: 12, design: .monospaced))
                                    .foregroundStyle(.secondary)
                                    .padding(.horizontal, 6)
                                    .padding(.vertical, 2)
                                    .background(Color.primary.opacity(0.07), in: Capsule())
                                Spacer()
                                Text("\(item.count) 次")
                                    .font(.system(size: 12))
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    .padding(14)
                    .background(iosCardBackground)
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

    private var iosCardBackground: some View {
        RoundedRectangle(cornerRadius: 13, style: .continuous)
            .fill(Color.primary.opacity(0.055))
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

    private func binding<T>(_ kp: WritableKeyPath<WubiPreferences, T>) -> Binding<T> {
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
        .padding(14)
        .background(
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .fill(Color.primary.opacity(0.055))
        )
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
