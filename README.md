# 句序 (Juxu)

面向 **macOS (`InputMethodKit`)** 与 **iOS (`UIInputViewController` 虚拟键盘扩展 + SwiftUI 宿主 App)** 的原生多方案输入法，统一驱动**全拼（26 键 / 九宫格 T9）、小鹤双拼、小鹤音形、86 / 98 / 新世纪五笔与仓颉三 / 五 / 六代**。

## 核心特性

1. **零外部依赖、零拷贝 `mmap` 通用内核 (`Core/juxu-core`)**：
   - 纯 Rust `std` + POSIX `mmap`，`[dependencies]` 为零，同时编译为 `aarch64-apple-darwin`、`aarch64-apple-ios` 与 `aarch64-apple-ios-sim` 静态库。
   - 定长形码/音形词典（`*.wbdict`）、精简汉字读音表（`pinyin.pydict`，`~0.8 MB`）、英文词库（`english.endict`）与量化语言模型（`wubi_aux.wbaux`）全部只读 `mmap` 映射，模型权重零 Dirty RSS。
   - 冷启动映射 `~1 ms`；五笔定长逐键 `< 0.25 ms`，26 键/T9 全拼纯神经束搜索逐键 `~1.6–1.8 ms`，满足 120Hz ProMotion 帧预算。
2. **三大体系共 9 套输入方案毫秒级热切换**：
   - **拼音体系**：全拼（支持 26 键与 iOS 九宫格 T9、声母简拼、中英混输）、小鹤双拼、小鹤音形。
   - **五笔体系**：86 版五笔、98 版五笔、新世纪（06）五笔（支持定长四码唯一上屏、第五码顶字与无空格整句连打，跨版本共享个人词条并实时重推导编码）。
   - **仓颉体系**：仓颉三代、五代、六代（支持五码规则、键帽字根提示与 `z` 万能反查）。
3. **Micro-RWKV-7 纯神经整句解码 + 概率层一次学习 + 连续触控热区纠错 (`Core/src/neural.rs` + `Core/src/pinyin.rs` + `Core/src/typo.rs`)**：
   - 内置 7 层 `Micro-RWKV-7`（`d=320`，ARM64 NEON `sdot` 寄存器分块批处理 `INT8` 推理 + `MTP-2` 双音节前瞻救回），全拼/双拼无需庞大静态词库即可完成整句解码。
   - 概率层在线个性化（`PersonalContinuations`）：手动选字或造词后立即在概率空间插值生效，一次选词即可记住新词与专有名词，同时保留 8 步提交状态回滚环。
   - 融合 **2D 连续触点高斯似然** + **神经下一键热区先验 (`nextKeyPriors`)** + **双指换位/漏键/多键纠错** + **每键个人拇指偏移 EMA 在线校准**（限幅 `±0.20` 键宽防跑偏）。
4. **贴合苹果原生的双端体验与后台无阻塞持久化**：
   - **macOS (`Sources/`)**：无边框 `NSPanel` 纯 AppKit 单行候选浮窗（横/竖排、深/浅色自适应）、系统设置风格偏好面板、中英文与数字自动空格、成对标点、Emoji 次要候选，以及可选按需加载的语音输入模块。
   - **iOS (`iOS/`)**：
     - **虚拟键盘扩展 (`iOS/KeyboardExtension/`)**：对齐 iOS 原生 4 行键帽几何（竖屏 `273pt`）、26 键与九宫格 T9 布局、多指 Rollover 连打、按键放大气泡（Key Callout）、上滑符号/长按派生字符、退格上滑清空整行与下滑撤回、空格长按二维光标触控板、剪贴板与短信验证码提取胶囊、横向候选条 + `⌄` 展开候选面板；Keychain 与词库落盘全部异步串行执行，主线程零 I/O 卡顿，空闲自动休眠触觉硬件省电。
     - **宿主应用 (`iOS/HostApp/`)**：启用引导与实时试打区、输入方案与规则配置、**26 键个人触控热区偏移图**、个人词库与评测统计，通过同 Team Keychain 分组与键盘扩展同步。

## 构建与验证

```sh
make dicts         # 从 Resources/RawTables/ 重新编译 Resources/Dicts/ 二进制词典（字节级可复现）
make test          # 运行 Rust 核心测试 + macOS Swift 桥接测试 + iOS 键盘几何与内联上屏测试
make app           # 编译并签名 macOS 输入法 build/Juxu.app
make install       # 安装并注册至 ~/Library/Input Methods/Juxu.app
make ios-core      # 交叉编译 iOS 真机 (aarch64-apple-ios) 与模拟器静态库
make ios-project   # 使用 XcodeGen 生成 iOS/Juxu.xcodeproj
make previews      # 渲染 macOS 2x Retina 候选窗与设置页预览图
make ios-previews  # 渲染 iPhone 2x Retina 虚拟键盘与宿主 App 预览图
```

### 真机 / 模拟器运行 iOS 端
在装有完整 `Xcode.app` 的 Mac 上打开 `iOS/Juxu.xcodeproj`，选择 `JuxuApp` Scheme 并在 Signing & Capabilities 选定你的开发 Team，按 `⌘R` 即可自动触发 `iOS/Scripts/build-rust-ios.sh` 编译 Rust 静态库并安装到 iPhone 或模拟器。

## 开源协议与词库来源

- **仓库本体源代码**：采用 **[WTFPL v2](LICENSE)** 协议。
- **第三方词库与派生词典**：`Resources/Dicts/` 中的二进制词典、读音表、Emoji 词表与语言模型统计由 `Resources/RawTables/` 中的第三方开源数据经 `Core/src/bin/build_dicts.rs` 编译生成，所有原始词表及协议原文均已随仓库提供并沿用各自上游协议，详见 [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)：

  - **五笔 86 码表**：[rime/rime-wubi](https://github.com/rime/rime-wubi)（`LGPL-3.0`）
  - **五笔 98 码表**：[lotem/rime-wubi98](https://github.com/lotem/rime-wubi98)（`GPL-3.0`）
  - **新世纪（06）五笔码表**：[g178253/rime-wubi06](https://github.com/g178253/rime-wubi06)（`Apache-2.0`）
  - **仓颉三代 / 五代 / 六代码表**：[cangjie-system/rime-cangjie-integrated](https://github.com/cangjie-system/rime-cangjie-integrated)（`LGPL-3.0`）、[rime/rime-cangjie](https://github.com/rime/rime-cangjie)（`LGPL-3.0`）、[rime-aca/rime-cangjie6](https://github.com/rime-aca/rime-cangjie6)（`GPL-3.0`）
  - **拼音读音与规范字表**：[mozillazg/pinyin-data](https://github.com/mozillazg/pinyin-data)（`MIT`）
  - **Emoji 候选词表**：[iDvel/rime-ice](https://github.com/iDvel/rime-ice)（`GPL-3.0`）
