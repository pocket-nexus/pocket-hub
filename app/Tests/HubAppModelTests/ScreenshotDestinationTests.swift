import Foundation
import Testing
@testable import HubAppModel

/// Where a device's screenshot is kept: where macOS keeps the owner's own.
@Suite("Screenshot destination")
struct ScreenshotDestinationTests {
    let home = URL(fileURLWithPath: "/Users/someone", isDirectory: true)

    @Test func theFolderIsMacOSsOwn() {
        #expect(ScreenshotDestination.folder(location: nil, home: home).path == "/Users/someone/Desktop", "macOS's default")
        #expect(ScreenshotDestination.folder(location: "", home: home).path == "/Users/someone/Desktop")
        #expect(ScreenshotDestination.folder(location: "~/Pictures/Screenshots", home: home).path == "/Users/someone/Pictures/Screenshots")
        #expect(ScreenshotDestination.folder(location: "/Volumes/Shots/", home: home).path == "/Volumes/Shots")
    }

    @Test func aNameIsTheDevicesAndTheTimesAndNeverTaken() {
        let folder = URL(fileURLWithPath: "/tmp/shots", isDirectory: true)
        let date = Date(timeIntervalSince1970: 1_794_884_553) // 2026-11-17 03:02:33 UTC
        let utc = TimeZone(identifier: "UTC")!
        let first = ScreenshotDestination.file(in: folder, device: "ewind", date: date, timeZone: utc) { _ in false }
        #expect(first.path == "/tmp/shots/ewind 2026-11-17 03.02.33.png")
        var taken: Set<String> = [first.path, "/tmp/shots/ewind 2026-11-17 03.02.33 2.png"]
        let third = ScreenshotDestination.file(in: folder, device: "ewind", date: date, timeZone: utc) { taken.contains($0.path) }
        #expect(third.lastPathComponent == "ewind 2026-11-17 03.02.33 3.png")
        taken = []
        let odd = ScreenshotDestination.file(in: folder, device: "A/B: C", date: date, timeZone: utc) { taken.contains($0.path) }
        #expect(odd.lastPathComponent == "A-B- C 2026-11-17 03.02.33.png")
    }
}
