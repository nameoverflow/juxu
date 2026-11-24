#if os(iOS)
import CoreHaptics
import UIKit

/// Low-latency Taptic feedback controller combining `CoreHaptics` crisp high-sharpness transients
/// (`playsHapticsOnly = true`, zero audio-session ducking) with a pre-warmed `.rigid` fallback ring
/// and system keyboard click audio playback.
@MainActor
final class IOSKeyboardFeedbackController {
    private var engine: CHHapticEngine?
    private var keyTapPlayer: CHHapticPatternPlayer?
    private var softTickPlayer: CHHapticPatternPlayer?
    private var engineRunning = false

    private var rigidRing: [UIImpactFeedbackGenerator] = []
    private var ringIndex = 0
    private let selectionGenerator = UISelectionFeedbackGenerator()

    init(attachingTo view: UIView? = nil) {
        for _ in 0..<3 {
            if #available(iOS 17.5, *), let view {
                rigidRing.append(UIImpactFeedbackGenerator(style: .rigid, view: view))
            } else {
                rigidRing.append(UIImpactFeedbackGenerator(style: .rigid))
            }
        }
        setupCoreHapticsIfSupported()
    }

    private func setupCoreHapticsIfSupported() {
        guard CHHapticEngine.capabilitiesForHardware().supportsHaptics else { return }
        do {
            let eng = try CHHapticEngine()
            eng.playsHapticsOnly = true
            // Allow CoreHaptics to power-gate the Taptic hardware channel after ~2s idle while the keyboard
            // stays visible on screen (eliminating static idle battery drain), auto-waking on `player.start`.
            eng.isAutoShutdownEnabled = true
            eng.stoppedHandler = { [weak self] reason in
                Task { @MainActor in
                    guard let self else { return }
                    if reason != .idleTimeout {
                        self.engineRunning = false
                    }
                }
            }
            eng.resetHandler = { [weak self] in
                Task { @MainActor in
                    self?.rebuildPlayersAndStart()
                }
            }
            self.engine = eng
            rebuildPlayersAndStart()
        } catch {
            self.engine = nil
            self.engineRunning = false
        }
    }

    private func rebuildPlayersAndStart() {
        guard let eng = engine else { return }
        do {
            let keyEvent = CHHapticEvent(
                eventType: .hapticTransient,
                parameters: [
                    CHHapticEventParameter(parameterID: .hapticIntensity, value: 0.45),
                    CHHapticEventParameter(parameterID: .hapticSharpness, value: 0.88),
                ],
                relativeTime: 0
            )
            let softEvent = CHHapticEvent(
                eventType: .hapticTransient,
                parameters: [
                    CHHapticEventParameter(parameterID: .hapticIntensity, value: 0.31),
                    CHHapticEventParameter(parameterID: .hapticSharpness, value: 0.80),
                ],
                relativeTime: 0
            )
            keyTapPlayer = try eng.makePlayer(with: CHHapticPattern(events: [keyEvent], parameters: []))
            softTickPlayer = try eng.makePlayer(with: CHHapticPattern(events: [softEvent], parameters: []))
            try eng.start()
            engineRunning = true
        } catch {
            engineRunning = false
        }
    }

    func prepare() {
        for gen in rigidRing {
            gen.prepare()
        }
        selectionGenerator.prepare()
        if let eng = engine, !engineRunning {
            do {
                try eng.start()
                engineRunning = true
            } catch {
                engineRunning = false
            }
        }
    }

    func suspend() {
        engine?.stop(completionHandler: nil)
        engineRunning = false
    }

    func fireKeyDown() {
        UIDevice.current.playInputClick()
        if playTransient(keyTapPlayer) {
            return
        }
        let gen = rigidRing[ringIndex % rigidRing.count]
        ringIndex &+= 1
        gen.impactOccurred(intensity: 0.42)
        gen.prepare()
    }

    func fireSoftStep(playClick: Bool) {
        if playClick {
            UIDevice.current.playInputClick()
        }
        if playTransient(softTickPlayer) {
            return
        }
        selectionGenerator.selectionChanged()
        selectionGenerator.prepare()
    }

    private func playTransient(_ player: CHHapticPatternPlayer?) -> Bool {
        guard let player, let eng = engine else { return false }
        if !engineRunning {
            try? eng.start()
            engineRunning = true
        }
        do {
            try player.start(atTime: CHHapticTimeImmediate)
            return true
        } catch {
            if (try? eng.start()) != nil {
                engineRunning = true
                if (try? player.start(atTime: CHHapticTimeImmediate)) != nil {
                    return true
                }
            }
            engineRunning = false
            return false
        }
    }
}
#endif
