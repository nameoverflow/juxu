# 句序 Juxu (Pure Geisha Rewrite — `geisha-rewrite`)

100% line-for-line [Geisha](https://github.com/geisha-lang/geisha) rewrite of the Juxu Chinese/English input method across **macOS** and **iOS** (`HostApp` + `KeyboardExtension`), directly powered by:
- [`juxu_core`](https://github.com/nameoverflow/juxu-core) (`v0.1.9`, idiomatic Geisha API — zero C FFI / zero JSON on hot paths, `Micro-RWKV-7` + `WBDICT03` / `PYDICT03` `mmap` zero-copy core)
- [`objc`](https://github.com/geisha-lang/objc.gsa) (`v0.1.0`, native Objective-C Runtime & Apple Blocks bridge)

## Packages

| Package | Artifact | Description |
|---|---|---|
| `shared/` | `juxu_shared` | 1:1 port of `Shared/` (`JuxuModels`, `TextFormatting`, `SharedSettingsComponents`, `JuxuBridge` with iOS Keychain + `zlib` user-data/eval-ring/touch-profile sync & legacy `PrisWubi` migration) |
| `macos/` | `macos/build/Juxu.app` | 1:1 AppKit + InputMethodKit macOS Input Method (`IMKServer`, `JuxuInputController`, single-row AppKit `CandidatePanel`, `FloatingHUD` with `NSGlassEffectView`, 5-section `SettingsWindow` + interactive 26-key touch simulator, complete 14-module Voice subsystem with `AVAudioEngine` + `vDSP` 11-band spectrum `VoiceHUDPanel` + Cloud BYOK / Local Whisper / Qwen3-ASR / Apple Speech + AX context capture & privacy gates, `--register-system-cache`, `--render-previews`) |
| `ios-keyboard/` | `ios-keyboard/build/JuxuKeyboard.appex` | 1:1 UIKit + CoreGraphics iOS Keyboard Extension (`KeyboardViewController`, `273pt` portrait / `204pt` landscape layout, causal multi-touch rollover, Wubi/Cangjie keycap radicals, 9-key T9 Pinyin + syllable sidebar, docked/floating key callouts & long-press alternates, whole-keyboard trackpad, backspace swipe-clear + undo, clipboard pill & OTP extraction, direct-draw `IOSCandidateRibbonView` & wrapped `IOSExpandedCandidateView`, atomic `setMarkedText`+`unmarkText` session & cross-instance orphan sweep) |
| `ios-app/` | `ios-app/build/Juxu.app` | 1:1 UIKit iOS Host App (4 tabs: `启用指南` + dual/WebView test sandboxes, `输入设置`, `误触与热区` 26-key visualizer & eval log dashboard, `个人词库`, embedding `PlugIns/JuxuKeyboard.appex`) |

## Build, Test & Install

```sh
make check        # Typecheck all 4 Geisha packages (shared, macos, ios-keyboard, ios-app)
make test         # Run all Geisha parity test suites (shared, ios-keyboard, macos + voice + voice_ui)
make macos-app    # Build & codesign macOS Juxu.app (arm64-apple-macosx14.0)
make previews     # Render all 6 Retina 2x macOS UI previews into build/previews/
make install      # Install macOS Juxu.app into /Library/Input Methods/Juxu.app
make ios-sim-app  # Build & ad-hoc sign iOS Simulator JuxuKeyboard.appex + Juxu.app
make ios-app      # Build iOS physical device JuxuKeyboard-device.appex + Juxu-device.app (arm64-apple-ios16.0)
make all-targets  # Build macOS + iOS Simulator + iOS Device bundles
```
