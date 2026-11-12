import Foundation
import Observation

/// The language of the app's own words: the system's, or one the owner picks
/// for Pocket Hub alone.
///
/// macOS keeps a per-app language as `AppleLanguages` in the app's own
/// defaults domain (what System Settings › General › Language & Region ›
/// Applications writes), and an app reads it once, when it starts. So a
/// choice here is that same key, the two settings never disagree, and the
/// new language shows once the app has restarted.
@MainActor
@Observable
public final class AppLanguage {
    public enum Choice: String, CaseIterable, Sendable {
        case system
        case english = "en"
        case simplifiedChinese = "zh-Hans"
    }

    public private(set) var choice: Choice
    /// The choice the running app started with: its words stay in that
    /// language until it restarts.
    public let choiceAtLaunch: Choice

    private let defaults: UserDefaults
    private let domain: String?
    static let key = "AppleLanguages"

    /// `domain` is the defaults domain `defaults` writes to: the bundle
    /// identifier for `.standard`, the suite's name for a suite.
    public init(defaults: UserDefaults = .standard, domain: String? = Bundle.main.bundleIdentifier) {
        self.defaults = defaults
        self.domain = domain
        let stored = Self.read(defaults, domain)
        choice = stored
        choiceAtLaunch = stored
    }

    /// Whether the app shows another language than the one chosen.
    public var needsRestart: Bool { choice != choiceAtLaunch }

    public func choose(_ choice: Choice) {
        switch choice {
        case .system: defaults.removeObject(forKey: Self.key)
        case .english, .simplifiedChinese: defaults.set([choice.rawValue], forKey: Self.key)
        }
        self.choice = choice
    }

    /// The app's own value only: `defaults` also answers with the system's
    /// list of languages, which is not a choice made for this app.
    private static func read(_ defaults: UserDefaults, _ domain: String?) -> Choice {
        guard let domain,
              let first = (defaults.persistentDomain(forName: domain)?[key] as? [String])?.first
        else { return .system }
        // The likely script filled in, so zh-CN reads as Simplified and
        // zh-TW as Traditional, which the app does not have.
        let language = Locale.Language(identifier: Locale.Language(identifier: first).maximalIdentifier)
        switch (language.languageCode?.identifier, language.script?.identifier) {
        case ("en", _): return .english
        case ("zh", "Hans"): return .simplifiedChinese
        default: return .system
        }
    }
}
