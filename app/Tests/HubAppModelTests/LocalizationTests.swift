import Foundation
import Testing

/// The String Catalog is the one place the app's words are kept: a key is the
/// English sentence, and the catalog holds its Simplified Chinese. This test
/// guards the file. Which keys the source uses is settled by
/// scripts/check-strings.sh, which asks the compiler, syncs the catalog and
/// then runs this test: a key it added has no Chinese yet, and a key the
/// source no longer uses is marked stale. Both fail here, by name.
@Suite("String Catalog")
struct LocalizationTests {
    static let catalogURL = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appending(path: "Localization/Localizable.xcstrings")

    @Test func everyKeyIsUsedAndHasSimplifiedChinese() throws {
        let data = try Data(contentsOf: Self.catalogURL)
        let catalog = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        #expect(catalog["sourceLanguage"] as? String == "en")
        let strings = try #require(catalog["strings"] as? [String: [String: Any]])
        #expect(strings.count > 200)
        for (key, entry) in strings {
            if entry["extractionState"] as? String == "stale" {
                Issue.record("“\(key)” is no longer in the source: delete it from the catalog")
                continue
            }
            let localizations = entry["localizations"] as? [String: Any] ?? [:]
            // The key is the English. An `en` unit is there for a string with
            // a count, as its plural forms, and may repeat the key otherwise.
            if let english = localizations["en"] as? [String: Any] {
                Self.expectFilled(english, "en", key)
            }
            guard let chinese = localizations["zh-Hans"] as? [String: Any] else {
                Issue.record("“\(key)” has no Simplified Chinese: add its zh-Hans to the catalog")
                continue
            }
            Self.expectFilled(chinese, "zh-Hans", key)
            // Format specifiers must survive translation (positional ones may reorder).
            if let zh = (chinese["stringUnit"] as? [String: Any])?["value"] as? String {
                #expect(Self.specifiers(key) == Self.specifiers(zh), "specifiers differ in “\(key)”")
            }
        }
    }

    /// A language's unit of a key: a translated value, or variations (plural forms).
    static func expectFilled(_ unit: [String: Any], _ language: String, _ key: String) {
        if let stringUnit = unit["stringUnit"] as? [String: Any] {
            #expect(stringUnit["state"] as? String == "translated", "\(language) “\(key)”")
            #expect((stringUnit["value"] as? String)?.isEmpty == false, "\(language) “\(key)”")
        } else {
            #expect(unit["variations"] != nil, "\(language) “\(key)” has neither a value nor variations")
        }
    }

    /// The multiset of format specifier kinds (%@, %lld), positions ignored.
    static func specifiers(_ text: String) -> [String] {
        let pattern = /%(?:\d+\$)?(@|lld|ld|d|f)/
        return text.matches(of: pattern).map { String($0.output.1) }.sorted()
    }
}
