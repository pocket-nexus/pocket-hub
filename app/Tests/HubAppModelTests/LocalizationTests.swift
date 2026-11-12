import Foundation
import Testing

/// The String Catalog holds English and Simplified Chinese for every key.
/// (Which keys exist is checked against the source by scripts/check-strings.sh,
/// which asks the compiler; this test guards the catalog file itself.)
@Suite("String Catalog")
struct LocalizationTests {
    static let catalogURL = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appending(path: "Localization/Localizable.xcstrings")

    @Test func everyKeyHasEnglishAndSimplifiedChinese() throws {
        let data = try Data(contentsOf: Self.catalogURL)
        let catalog = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        #expect(catalog["sourceLanguage"] as? String == "en")
        let strings = try #require(catalog["strings"] as? [String: [String: Any]])
        #expect(strings.count > 200)
        for (key, entry) in strings where entry["extractionState"] as? String != "stale" {
            let localizations = entry["localizations"] as? [String: Any] ?? [:]
            for language in ["en", "zh-Hans"] {
                guard let unit = localizations[language] as? [String: Any] else {
                    Issue.record("\(language) missing for “\(key)”")
                    continue
                }
                if let stringUnit = unit["stringUnit"] as? [String: Any] {
                    #expect(stringUnit["state"] as? String == "translated", "\(language) “\(key)”")
                    #expect((stringUnit["value"] as? String)?.isEmpty == false, "\(language) “\(key)”")
                } else {
                    #expect(unit["variations"] != nil, "\(language) “\(key)” has neither a value nor variations")
                }
            }
            // Format specifiers must survive translation (positional ones may reorder).
            if let zh = ((localizations["zh-Hans"] as? [String: Any])?["stringUnit"] as? [String: Any])?["value"] as? String {
                #expect(Self.specifiers(key) == Self.specifiers(zh), "specifiers differ in “\(key)”")
            }
        }
    }

    /// The multiset of format specifier kinds (%@, %lld), positions ignored.
    static func specifiers(_ text: String) -> [String] {
        let pattern = /%(?:\d+\$)?(@|lld|ld|d|f)/
        return text.matches(of: pattern).map { String($0.output.1) }.sorted()
    }
}
