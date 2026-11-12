import Foundation
import Testing
@testable import HubAppModel

/// The app's language is the per-app `AppleLanguages` of its own defaults domain.
@Suite("App language")
@MainActor
struct AppLanguageTests {
    /// A defaults suite of its own, removed afterwards.
    private func withSuite(_ body: (UserDefaults, String) throws -> Void) rethrows {
        let name = "nexus.pocket.hub.tests.language-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: name)!
        defer { defaults.removePersistentDomain(forName: name) }
        try body(defaults, name)
    }

    @Test func noChoiceFollowsTheSystem() {
        withSuite { defaults, name in
            let language = AppLanguage(defaults: defaults, domain: name)
            #expect(language.choice == .system)
            #expect(!language.needsRestart)
        }
    }

    @Test func aChoiceIsTheAppsAppleLanguagesAndWantsARestart() {
        withSuite { defaults, name in
            let language = AppLanguage(defaults: defaults, domain: name)
            language.choose(.simplifiedChinese)
            #expect(defaults.persistentDomain(forName: name)?["AppleLanguages"] as? [String] == ["zh-Hans"])
            #expect(language.needsRestart)
            // Choosing what the app started with again needs no restart.
            language.choose(.system)
            #expect(defaults.persistentDomain(forName: name)?["AppleLanguages"] == nil)
            #expect(!language.needsRestart)
        }
    }

    /// What System Settings writes for the app (a region may come with it) reads as the same choice.
    @Test func readsTheSystemSettingsValue() {
        for (stored, expected) in [("en", AppLanguage.Choice.english), ("en-GB", .english),
                                   ("zh-Hans", .simplifiedChinese), ("zh-Hans-CN", .simplifiedChinese),
                                   ("zh-CN", .simplifiedChinese),
                                   // Traditional Chinese is not a language the app has.
                                   ("zh-Hant", .system), ("zh-TW", .system)] {
            withSuite { defaults, name in
                defaults.set([stored], forKey: "AppleLanguages")
                let language = AppLanguage(defaults: defaults, domain: name)
                #expect(language.choice == expected, "\(stored)")
                #expect(language.choiceAtLaunch == expected)
            }
        }
    }
}
