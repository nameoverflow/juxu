GEISHA ?= $(HOME)/.geisha/bin/geisha

.PHONY: all check macos-app ios-sim-app ios-app all-targets test clean

all: macos-app ios-sim-app

check:
	cd shared && $(GEISHA) check
	cd macos && $(GEISHA) check
	cd ios-keyboard && $(GEISHA) check
	cd ios-app && $(GEISHA) check

macos-app:
	cd macos && $(GEISHA) build --target macos-app -o build/Juxu.app

ios-sim-app:
	cd ios-keyboard && $(GEISHA) build --target ios-sim-app -o build/JuxuKeyboard.appex
	cd ios-app && $(GEISHA) build --target ios-sim-app -o build/Juxu.app

ios-app:
	cd ios-keyboard && $(GEISHA) build --target ios-app -o build/JuxuKeyboard-device.appex
	cd ios-app && $(GEISHA) build --target ios-app -o build/Juxu-device.app

all-targets: macos-app ios-sim-app ios-app

test:
	cd shared && $(GEISHA) test

clean:
	rm -rf shared/build macos/build ios-keyboard/build ios-app/build
