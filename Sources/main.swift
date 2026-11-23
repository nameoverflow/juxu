import AppKit
import Carbon
import InputMethodKit

if CommandLine.arguments.contains("--register-and-enable") {
    let bundle = Bundle.main
    let bundleURL = bundle.bundleURL
    let bundleId = bundle.bundleIdentifier ?? "im.juxu.inputmethod.Juxu"
    let modeId = ((bundle.infoDictionary?["ComponentInputModeDict"] as? [String: Any])?["tsVisibleInputModeOrderedArrayKey"] as? [String])?.first
        ?? "\(bundleId).Hans"
    let shouldSelect = CommandLine.arguments.contains("--select")

    // Disable any retired legacy input source IDs if still lingering in user preferences
    for legacyId in ["com.hcyue.juxu.inputmethod", "com.hcyue.juxu.inputmethod.juxu"] {
        if let oldList = TISCreateInputSourceList([kTISPropertyBundleID as String: legacyId] as CFDictionary, true)?.takeRetainedValue() as? [TISInputSource] {
            for src in oldList {
                _ = TISDisableInputSource(src)
            }
        }
    }

    var regStatus: OSStatus = noErr
    var modeEnabled = false
    for attempt in 0..<25 {
        if attempt == 0 || attempt == 5 || attempt == 12 {
            regStatus = TISRegisterInputSource(bundleURL as CFURL)
        }
        for (idx, filterDict) in [
            [kTISPropertyBundleID as String: bundleId],
            [kTISPropertyInputSourceID as String: bundleId],
            [kTISPropertyInputSourceID as String: modeId],
        ].enumerated() {
            if let list = TISCreateInputSourceList(filterDict as CFDictionary, true)?.takeRetainedValue() as? [TISInputSource] {
                for src in list {
                    _ = TISEnableInputSource(src)
                    if idx == 2 {
                        if let ptr = TISGetInputSourceProperty(src, kTISPropertyInputSourceIsEnabled) {
                            let val = Unmanaged<CFBoolean>.fromOpaque(ptr).takeUnretainedValue()
                            if CFBooleanGetValue(val) {
                                modeEnabled = true
                            }
                        }
                        if shouldSelect {
                            _ = TISSelectInputSource(src)
                        }
                    }
                }
            }
        }
        if modeEnabled {
            break
        }
        usleep(150_000)
    }
    if regStatus != noErr {
        fputs("TISRegisterInputSource returned \(regStatus)\n", stderr)
    }
    if !modeEnabled {
        fputs("Warning: input mode \(modeId) was not reported enabled after registration\n", stderr)
    }
    exit(0)
}

final class JuxuAppDelegate: NSObject, NSApplicationDelegate {
    private var imkServer: IMKServer?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let bundle = Bundle.main
        let bundleId = bundle.bundleIdentifier ?? "im.juxu.inputmethod.Juxu"

        // On macOS 27+, `+[IMKServer connectionNameFor:]` enforces `<bundleIdentifier>.Connection`
        // for all third-party input methods (rejecting custom names in IMKInputSession_Modern).
        let connSel = NSSelectorFromString("connectionNameFor:")
        let resolvedConnName: String
        if IMKServer.responds(to: connSel),
           let obj = IMKServer.perform(connSel, with: bundle)?.takeUnretainedValue() as? String,
           !obj.isEmpty {
            resolvedConnName = obj
        } else {
            resolvedConnName = (bundle.infoDictionary?["InputMethodConnectionName"] as? String)
                ?? "\(bundleId).Connection"
        }

        self.imkServer = IMKServer(name: resolvedConnName, bundleIdentifier: bundleId)

        // Initialize shared Rust mmap core eagerly (< 1ms) so first keystroke has 0ms cold latency
        _ = JuxuBridge.shared

        // Allow launching standalone settings UI via `--settings`
        if CommandLine.arguments.contains("--settings") {
            SettingsWindowController.shared.show()
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        JuxuBridge.shared.flushUserDataIfNeeded()
    }
}

let app = NSApplication.shared
let delegate = JuxuAppDelegate()
app.delegate = delegate
app.run()
