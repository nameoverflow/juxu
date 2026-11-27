# 句序 Juxu (Pure Geisha Rewrite — `geisha-rewrite`)

100% pure [Geisha](https://github.com/geisha-lang/geisha) rewrite of the Juxu Chinese/English input method for **macOS** and **iOS**, directly depending on:
- [`juxu_core`](https://github.com/nameoverflow/juxu-core) (`v0.1.9`, idiomatic Geisha API — zero C FFI / zero JSON on hot paths)
- [`objc`](https://github.com/geisha-lang/objc.gsa) (`v0.1.0`, native Objective-C Runtime & Apple Blocks bridge)

Zero Swift, zero Rust, zero Xcode project files required.

## Packages

| Package | Artifact | Description |
|---|---|---|
| `shared/` | `juxu_shared` | Apple CoreGraphics/ObjC geometry, Chinese punctuation pairing, ZH/EN auto-spacing, and high-level `JuxuSession` |
| `macos/` | `macos/build/Juxu.app` | Pure AppKit + InputMethodKit macOS Input Method (`IMKServer`, `JuxuInputController`, single-row AppKit `CandidatePanel`, `FloatingHUD`, AppKit `SettingsWindow`, `ShiftTapDetector`, `--register-system-cache`) |
| `ios-keyboard/` | `ios-keyboard/build/JuxuKeyboard.appex` | Pure UIKit + CoreGraphics iOS Keyboard Extension (`KeyboardViewController`, 273pt portrait layout, atomic `setMarkedText`+`unmarkText` session, flat keycaps & direct-draw candidate ribbon) |
| `ios-app/` | `ios-app/build/Juxu.app` | Pure UIKit iOS Host App embedding `PlugIns/JuxuKeyboard.appex` |

## Build & Test

```sh
make check        # Typecheck all 4 Geisha packages
make macos-app    # Build macOS Juxu.app (arm64-apple-macosx14.0)
make ios-sim-app  # Build iOS Simulator JuxuKeyboard.appex + Juxu.app
make ios-app      # Build iOS Device JuxuKeyboard.appex + Juxu.app (arm64-apple-ios16.0)
make test         # Run Geisha shared & end-to-end tests
```
