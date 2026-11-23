.PHONY: all dicts context-capture rust-test swift-test ios-test test app pkg install previews ios-previews ios-project ios-core clean

CERT_NAME ?= Talus Local Code Signing
CODESIGN_IDENTITY ?= auto
RAW_TABLES_DIR ?= Resources/RawTables

export MACOSX_DEPLOYMENT_TARGET := 14.0
MACOS_TARGET = -target arm64-apple-macosx14.0

CORE_LIB = Core/target/release/libjuxu_core.a
CORE_HEADER = Core/include/juxu_core.h
CORE_FLAGS = -import-objc-header $(CORE_HEADER) $(CORE_LIB)
CORE_SOURCES = Core/Cargo.toml $(wildcard Core/src/*.rs)

SHARED_SOURCES = Shared/JuxuModels.swift Shared/JuxuBridge.swift Shared/TextFormatting.swift Shared/SharedSettingsComponents.swift
SHARED_CORE_SOURCES = Shared/JuxuModels.swift Shared/JuxuBridge.swift Shared/TextFormatting.swift
IOS_SHARED_SOURCES = iOS/Shared/IOSInlineMarkedTextSession.swift iOS/Shared/IOSKeyboardLayoutEngine.swift iOS/Shared/IOSKeyboardGesturesAndEditing.swift iOS/Shared/IOSClipboardSuggestion.swift
VOICE_SOURCES = Sources/VoicePreferences.swift Sources/VoiceContextCapture.swift Sources/VoiceLocalInstaller.swift Sources/VoiceEngine.swift Sources/VoiceWhisperServer.swift Sources/VoicePartialStabilizer.swift Sources/VoiceSessionTimeline.swift Sources/VoiceQwenHelper.swift Sources/VoiceASREngine.swift Sources/VoiceGlobalHotkey.swift Sources/VoiceHUDPanel.swift Sources/VoiceSessionController.swift Sources/VoiceSettingsView.swift
UI_SOURCES = $(SHARED_SOURCES) Sources/FloatingHUDPanel.swift Sources/CandidatePanel.swift $(VOICE_SOURCES) Sources/SettingsWindow.swift
APP_SOURCES = $(UI_SOURCES) Sources/ShiftTapDetector.swift Sources/InputController.swift Sources/main.swift
FRAMEWORKS = -framework Cocoa -framework SwiftUI -framework InputMethodKit -framework Carbon -framework AVFoundation -framework Speech -framework Security -framework Accelerate -framework ApplicationServices

all: build/Juxu

$(CORE_LIB): $(CORE_SOURCES)
	cargo build --release --manifest-path Core/Cargo.toml --lib

CORPUS_LM ?= $(if $(JUXU_CORPUS_LM),$(JUXU_CORPUS_LM),$(FORMORDO_CORPUS_LM))

dicts:
	cargo build --release --manifest-path Core/Cargo.toml --bin build-dicts
	@if [ -n "$(CORPUS_LM)" ]; then \
	  Core/target/release/build-dicts "$(RAW_TABLES_DIR)" Resources/Dicts --corpus-lm "$(CORPUS_LM)"; \
	else \
	  Core/target/release/build-dicts "$(RAW_TABLES_DIR)" Resources/Dicts; \
	fi

build/Juxu: $(APP_SOURCES) $(CORE_LIB) $(CORE_HEADER)
	mkdir -p build
	swiftc -O $(MACOS_TARGET) $(APP_SOURCES) $(CORE_FLAGS) $(FRAMEWORKS) -o $@

context-capture: Sources/VoiceContextCapture.swift Scripts/ContextCaptureCLI.swift
	mkdir -p build
	swiftc -O $(MACOS_TARGET) Sources/VoiceContextCapture.swift Scripts/ContextCaptureCLI.swift -framework Cocoa -framework ApplicationServices -framework Carbon -o build/context-capture

build/AppIcon.icns build/menu_icon.pdf: Scripts/DrawAssets.swift
	mkdir -p build/AppIcon.iconset
	find build/AppIcon.iconset -type f -delete
	swift Scripts/DrawAssets.swift build/icon-master.png build/menu_icon.pdf
	@for spec in 16x16:16 16x16@2x:32 32x32:32 32x32@2x:64 128x128:128 128x128@2x:256 256x256:256 256x256@2x:512 512x512:512 512x512@2x:1024; do \
	  name=$${spec%:*}; size=$${spec#*:}; \
	  sips -z $$size $$size build/icon-master.png --out build/AppIcon.iconset/icon_$${name}.png >/dev/null; \
	done
	iconutil -c icns build/AppIcon.iconset -o build/AppIcon.icns

QWEN_HELPER = build/qwen/release/juxu-qwen-asr
QWEN_SOURCES = Helpers/JuxuQwenASR/Package.swift $(wildcard Helpers/JuxuQwenASR/Sources/juxu-qwen-asr/*.swift) $(wildcard Helpers/JuxuQwenASR/Sources/juxu-qwen-asr/Vendor/*.swift)

$(QWEN_HELPER): $(QWEN_SOURCES) Scripts/build-qwen-helper.sh
	sh Scripts/build-qwen-helper.sh

app: build/Juxu Resources/Info.plist build/AppIcon.icns build/menu_icon.pdf $(QWEN_HELPER)
	rm -rf build/Juxu.app
	mkdir -p build/Juxu.app/Contents/MacOS \
	         build/Juxu.app/Contents/Resources/Dicts \
	         build/Juxu.app/Contents/Resources/zh-Hans.lproj
	cp build/Juxu build/Juxu.app/Contents/MacOS/Juxu
	cp $(QWEN_HELPER) build/qwen/release/mlx.metallib build/Juxu.app/Contents/MacOS/
	cp Resources/Info.plist build/Juxu.app/Contents/Info.plist
	cp Resources/zh-Hans.lproj/InfoPlist.strings build/Juxu.app/Contents/Resources/zh-Hans.lproj/InfoPlist.strings
	cp build/AppIcon.icns build/Juxu.app/Contents/Resources/AppIcon.icns
	cp build/menu_icon.pdf build/Juxu.app/Contents/Resources/menu_icon.pdf
	cp Resources/Dicts/*.wbdict Resources/Dicts/*.pydict Resources/Dicts/*.wbaux Resources/Dicts/*.endict Resources/Dicts/*.tsv build/Juxu.app/Contents/Resources/Dicts/
	@id="$$(sh Scripts/resolve-codesign-identity.sh "$(CODESIGN_IDENTITY)" "$(CERT_NAME)")"; \
	echo "Signing build/Juxu.app with identity: $$id"; \
	codesign --force --sign "$$id" build/Juxu.app/Contents/MacOS/juxu-qwen-asr; \
	codesign --force --deep --sign "$$id" build/Juxu.app

pkg: app
	chmod +x Scripts/build-macos-pkg.sh
	@ver="$$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' Resources/Info.plist)"; \
	sh Scripts/build-macos-pkg.sh build/Juxu.app "$$ver" "build/Juxu-$$ver-macOS-arm64.pkg"

install: app
	sh Scripts/install-macos.sh --local-app build/Juxu.app

rust-test:
	cargo test --release --manifest-path Core/Cargo.toml

swift-test: $(CORE_LIB)
	mkdir -p build
	swiftc -Onone $(MACOS_TARGET) $(UI_SOURCES) Sources/ShiftTapDetector.swift Tests/SwiftBridgeTests.swift $(CORE_FLAGS) $(FRAMEWORKS) -o build/swift-bridge-tests
	build/swift-bridge-tests

test: rust-test swift-test ios-test

ios-test: $(CORE_LIB)
	mkdir -p build
	swiftc -Onone $(MACOS_TARGET) $(SHARED_CORE_SOURCES) $(IOS_SHARED_SOURCES) Tests/IOSKeyboardTests.swift $(CORE_FLAGS) -framework Foundation -framework CoreGraphics -framework Security -o build/ios-keyboard-tests
	build/ios-keyboard-tests

previews: $(CORE_LIB)
	mkdir -p build/previews
	swiftc -O $(MACOS_TARGET) $(UI_SOURCES) Scripts/RenderPreviews.swift $(CORE_FLAGS) $(FRAMEWORKS) -o build/render-previews
	build/render-previews build/previews

ios-previews: $(CORE_LIB)
	mkdir -p build/ios-previews
	swiftc -O $(MACOS_TARGET) $(SHARED_SOURCES) $(IOS_SHARED_SOURCES) iOS/HostApp/HostAppViews.swift Scripts/RenderIOSPreviews.swift $(CORE_FLAGS) -framework Cocoa -framework SwiftUI -o build/render-ios-previews
	build/render-ios-previews build/ios-previews

ios-project:
	chmod +x iOS/Scripts/build-rust-ios.sh
	cd iOS && xcodegen generate

ios-core:
	cargo build --release --manifest-path Core/Cargo.toml --lib --target aarch64-apple-ios
	cargo build --release --manifest-path Core/Cargo.toml --lib --target aarch64-apple-ios-sim

clean:
	rm -rf build
	cargo clean --manifest-path Core/Cargo.toml
