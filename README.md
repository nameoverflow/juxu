# 形序 (Formordo)

面向 **macOS (`InputMethodKit`)** 与 **iOS (`UIInputViewController` 虚拟键盘扩展 + SwiftUI 宿主 App)** 的极速精简原生五笔输入法。

## 核心特性

1. **零外部依赖、零拷贝 `mmap` 通用内核 (`Core/juxu-core`)**：
   - 纯 Rust `std` + POSIX `mmap`，`[dependencies]` 为零，同时编译为 `aarch64-apple-darwin`、`aarch64-apple-ios` 与 `aarch64-apple-ios-sim` 静态库。
   - 1–4 码五笔采用保序 Base-27 `u32` 定长分桶，编译生成紧凑只读词典（`wubi86.wbdict` / `wubi98.wbdict` / `wubi06.wbdict` + `wubi_aux.wbaux` `WBAUX003`）。
   - 冷启动映射 `~1 ms`，单次击键含 24-beam 误触解码平均耗时 **< 0.1 ms**，只读 `mmap` 的 `INT8` 神经语言模型权重 Dirty RSS = 0（循环状态含 8 步退格回滚环仅 ~600 KB）。
2. **86 / 98 / 新世纪 (06) 三版本与跨版本动态组词**：
   - 运行时毫秒级热切换三套标准码表。
   - 个人词库与连打自动组词仅保存词条文本，按当前激活版本的单字字根（`stem2`）实时遵循 `AaAbBaBb` / `AaBaCaCb` / `AaBaCaZa` 推导编码，切版本无需重造词。
3. **Micro-RWKV-7 神经循环状态 + 连续触控热区自适应纠错 (`Core/src/typo.rs` + `Core/src/neural.rs`)**：
   - 融合 **2D 连续触点高斯对数似然** + **`Micro-RWKV7-Small`（2.52M 参数，ARM64 NEON `INT8` 零拷贝推理，`MTP-2` 快筛 + 竞争项精确展开）** + **高 PMI N-gram 残差** + **个人历史 Unigram/Bigram**，经 Softmax 产出归一化后验置信度 $p \in [0, 1]$。
   - 支持 **第 1 码预测性键缝热区与第 2–4 码合法邻键键缝连续先验消歧 (`nextKeyPriors`)**、**空码救回**、**两键交界滑偏重判 (Soft Voronoi Hitbox)**、**双指快速换位纠错 (`ab -> ba`)**，以及 **每键个人拇指偏移 EMA 在线自适应校准**（限幅 `±0.20` 键宽防跑偏）。
   - 默认对非精确纠错项抑制「四码唯一盲目自动上屏」，绝不乱顶错字。
4. **贴合苹果原生的双端界面**：
   - **macOS (`Sources/`)**：无边框 `NSPanel` 纯 AppKit 单行候选浮窗（横/竖排、深/浅色自适应）、系统设置风格偏好面板、输入法菜单栏快捷切表与 Shift 中英切换 HUD。
   - **iOS (`iOS/`)**：
     - **虚拟键盘扩展 (`iOS/KeyboardExtension/`)**：1:1 对齐 iOS 原生 4 行键帽几何（`273pt` 总高）、三层键盘（`ABC` 26 键 / `123` 数字与中英标点 / `#+=` 符号）、多指 Rollover 连打、按键放大气泡（Key Callout）、长按/滑动空格二维光标触控板、长按退格加速连删、候选栏中央剪贴板提示（自动提取剪贴板短信/邮件验证码）、横向候选条 + `⌄` 四列全屏展开候选面板。
     - **宿主应用 (`iOS/HostApp/`)**：三步启用引导与实时打字试手区、五笔规则与剪贴板提示配置、**26 键个人触控热区偏移图**、个人词库管理，并通过同 Team Keychain 分组与键盘扩展同步设置。

## 构建与验证

```sh
make dicts         # 从 Resources/RawTables/ 重新编译 Resources/Dicts/ 二进制词典（字节级可复现）
make test          # 运行 Rust 核心测试 + macOS Swift 桥接测试 + iOS 键盘几何与剪贴板测试
make app           # 编译并签名 macOS 输入法 build/PrisWubi.app
make install       # 安装并注册至 ~/Library/Input Methods/PrisWubi.app
make ios-core      # 交叉编译 iOS 真机 (aarch64-apple-ios) 与模拟器静态库
make ios-project   # 使用 XcodeGen 生成 iOS/PrisWubi.xcodeproj
make previews      # 渲染 macOS 2x Retina 候选窗与设置页预览图
make ios-previews  # 渲染 iPhone 2x Retina 虚拟键盘与宿主 App 预览图
```

### 真机 / 模拟器运行 iOS 端
在装有完整 `Xcode.app` 的 Mac 上打开 `iOS/PrisWubi.xcodeproj`，选择 `JuxuApp` Scheme 并在 Signing & Capabilities 选定你的开发 Team，按 `⌘R` 即可自动触发 `iOS/Scripts/build-rust-ios.sh` 编译 Rust 静态库并安装到 iPhone 或模拟器。

## 开源协议与词库来源

- **仓库本体源代码**：采用 **[WTFPL v2](LICENSE)** 协议。
- **第三方词库与派生词典**：`Resources/Dicts/` 中的二进制词典与拼音/Bigram 索引由 `Resources/RawTables/` 中的第三方开源词表经 `Core/src/bin/build_dicts.rs` 编译生成，所有原始词表及协议原文均已随仓库提供并沿用各自上游协议，详见 [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)：

  - **五笔 86 码表**：[rime/rime-wubi](https://github.com/rime/rime-wubi)（`LGPL-3.0`）
  - **五笔 98 码表**：[lotem/rime-wubi98](https://github.com/lotem/rime-wubi98)（`GPL-3.0`）
  - **新世纪（06）五笔码表**：[g178253/rime-wubi06](https://github.com/g178253/rime-wubi06)（`Apache-2.0`）
  - **拼音反查与规范字表**：[mozillazg/pinyin-data](https://github.com/mozillazg/pinyin-data)（`MIT`）
