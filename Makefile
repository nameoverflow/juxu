GEISHA ?= $(HOME)/.geisha/bin/geisha
CERT_NAME ?= Talus Local Code Signing
CODESIGN_IDENTITY ?= auto

export MACOSX_DEPLOYMENT_TARGET := 14.0
export DEVELOPER_DIR ?= /Applications/Xcode.app/Contents/Developer

QWEN_HELPER = build/qwen/release/juxu-qwen-asr
QWEN_SOURCES = Helpers/JuxuQwenASR/Package.swift $(wildcard Helpers/JuxuQwenASR/Sources/juxu-qwen-asr/*.swift) $(wildcard Helpers/JuxuQwenASR/Sources/juxu-qwen-asr/Vendor/*.swift)

.PHONY: all check macos-app qwen-helper previews install pkg ios-sim-app ios-app all-targets test clean

all: macos-app ios-sim-app

check:
	cd shared && $(GEISHA) check
	cd macos && $(GEISHA) check
	cd ios-keyboard && $(GEISHA) check
	cd ios-app && $(GEISHA) check

qwen-helper: $(QWEN_HELPER)

$(QWEN_HELPER): $(QWEN_SOURCES) Scripts/build-qwen-helper.sh
	sh Scripts/build-qwen-helper.sh

macos-app:
	cd macos && MACOSX_DEPLOYMENT_TARGET=14.0 $(GEISHA) build --target macos-app -o build/Juxu.app
	@if [ -f "$(QWEN_HELPER)" ] && [ -f "build/qwen/release/mlx.metallib" ]; then \
	  cp "$(QWEN_HELPER)" build/qwen/release/mlx.metallib macos/build/Juxu.app/Contents/MacOS/; \
	fi
	@id="$$(sh Scripts/resolve-codesign-identity.sh "$(CODESIGN_IDENTITY)" "$(CERT_NAME)")"; \
	echo "Signing macos/build/Juxu.app with identity: $$id"; \
	if [ -f macos/build/Juxu.app/Contents/MacOS/mlx.metallib ]; then \
	  codesign --force --sign "$$id" macos/build/Juxu.app/Contents/MacOS/mlx.metallib; \
	fi; \
	if [ -f macos/build/Juxu.app/Contents/MacOS/juxu-qwen-asr ]; then \
	  codesign --force --sign "$$id" macos/build/Juxu.app/Contents/MacOS/juxu-qwen-asr; \
	fi; \
	codesign --force --deep --sign "$$id" macos/build/Juxu.app

previews: macos-app
	mkdir -p build/previews
	./macos/build/Juxu.app/Contents/MacOS/Juxu --render-previews build/previews

pkg: macos-app
	chmod +x Scripts/build-macos-pkg.sh
	mkdir -p build
	@ver="$$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' macos/Info.plist)"; \
	sh Scripts/build-macos-pkg.sh macos/build/Juxu.app "$$ver" "build/Juxu-$$ver-macOS-arm64.pkg"

install: macos-app
	sh Scripts/install-macos.sh --local-app macos/build/Juxu.app

ios-sim-app:
	cd ios-keyboard && $(GEISHA) build --target ios-sim-app -o build/JuxuKeyboard.appex
	plutil -replace CFBundleSupportedPlatforms -json '["iPhoneSimulator"]' ios-keyboard/build/JuxuKeyboard.appex/Info.plist
	plutil -replace DTPlatformName -string "iphonesimulator" ios-keyboard/build/JuxuKeyboard.appex/Info.plist
	plutil -replace MinimumOSVersion -string "16.0" ios-keyboard/build/JuxuKeyboard.appex/Info.plist
	plutil -replace JuxuSettingsGroup -string "LBDWT733YM.im.juxu.settings" ios-keyboard/build/JuxuKeyboard.appex/Info.plist
	codesign --force --sign - ios-keyboard/build/JuxuKeyboard.appex
	cd ios-app && $(GEISHA) build --target ios-sim-app -o build/Juxu.app
	plutil -replace CFBundleSupportedPlatforms -json '["iPhoneSimulator"]' ios-app/build/Juxu.app/PlugIns/JuxuKeyboard.appex/Info.plist
	plutil -replace DTPlatformName -string "iphonesimulator" ios-app/build/Juxu.app/PlugIns/JuxuKeyboard.appex/Info.plist
	plutil -replace MinimumOSVersion -string "16.0" ios-app/build/Juxu.app/PlugIns/JuxuKeyboard.appex/Info.plist
	plutil -replace JuxuSettingsGroup -string "LBDWT733YM.im.juxu.settings" ios-app/build/Juxu.app/PlugIns/JuxuKeyboard.appex/Info.plist
	plutil -replace CFBundleSupportedPlatforms -json '["iPhoneSimulator"]' ios-app/build/Juxu.app/Info.plist
	plutil -replace DTPlatformName -string "iphonesimulator" ios-app/build/Juxu.app/Info.plist
	plutil -replace MinimumOSVersion -string "16.0" ios-app/build/Juxu.app/Info.plist
	plutil -replace JuxuSettingsGroup -string "LBDWT733YM.im.juxu.settings" ios-app/build/Juxu.app/Info.plist
	codesign --force --sign - ios-app/build/Juxu.app/PlugIns/JuxuKeyboard.appex
	codesign --force --sign - ios-app/build/Juxu.app

ios-app:
	cd ios-keyboard && $(GEISHA) build --target ios-app -o build/JuxuKeyboard.appex
	cp -R ios-keyboard/build/JuxuKeyboard.appex ios-keyboard/build/JuxuKeyboard-device.appex
	cd ios-app && $(GEISHA) build --target ios-app -o build/Juxu-device.app

all-targets: macos-app ios-sim-app ios-app

test:
	cd shared && $(GEISHA) test
	cd ios-keyboard && $(GEISHA) test
	cd macos && $(GEISHA) test

clean:
	rm -rf build shared/build macos/build ios-keyboard/build ios-app/build
