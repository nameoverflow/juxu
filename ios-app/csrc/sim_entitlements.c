/*
 * Embeds Simulator Keychain access-group entitlements in Mach-O `(__TEXT,__entitlements)`
 * when targeting the iOS Simulator (`TARGET_OS_SIMULATOR`).
 * Physical iOS device builds (`TARGET_OS_IOS && !TARGET_OS_SIMULATOR`) ignore this file
 * and receive signed entitlements via `JuxuApp.entitlements` + provisioning profile.
 */
#if defined(__APPLE__)
#include <TargetConditionals.h>
#if TARGET_OS_SIMULATOR
__asm__(
    ".section __TEXT,__entitlements\n"
    ".ascii \"<?xml version=\\\"1.0\\\" encoding=\\\"UTF-8\\\"?>\\n\"\n"
    ".ascii \"<!DOCTYPE plist PUBLIC \\\"-//Apple//DTD PLIST 1.0//EN\\\" \\\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\\\">\\n\"\n"
    ".ascii \"<plist version=\\\"1.0\\\">\\n<dict>\\n\"\n"
    ".ascii \"\\t<key>application-identifier</key>\\n\\t<string>LBDWT733YM.im.juxu.app</string>\\n\"\n"
    ".ascii \"\\t<key>keychain-access-groups</key>\\n\\t<array>\\n\"\n"
    ".ascii \"\\t\\t<string>LBDWT733YM.im.juxu.settings</string>\\n\"\n"
    ".ascii \"\\t\\t<string>im.juxu.settings</string>\\n\"\n"
    ".ascii \"\\t</array>\\n\"\n"
    ".ascii \"</dict>\\n</plist>\\n\"\n"
);
#endif
#endif
