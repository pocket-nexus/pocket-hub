import AppKit
import HubAppModel
import HubClient
import SwiftUI
import UniformTypeIdentifiers

/// File operations on a screenshot Hub wrote. Reads happen off the main actor.
enum ScreenshotFile {
    static func loadImage(path: String) async -> NSImage? {
        let data = await Task.detached(priority: .userInitiated) { try? Data(contentsOf: URL(fileURLWithPath: path)) }.value
        return data.flatMap(NSImage.init(data:))
    }

    @MainActor
    static func copyToPasteboard(path: String) async {
        guard let image = await loadImage(path: path) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.writeObjects([image])
    }

    /// Asks where to save, then copies the PNG there.
    @MainActor
    static func save(path: String, suggestedName: String) {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.png]
        panel.nameFieldStringValue = suggestedName
        panel.canCreateDirectories = true
        guard panel.runModal() == .OK, let destination = panel.url else { return }
        Task.detached(priority: .userInitiated) {
            let fm = FileManager.default
            try? fm.removeItem(at: destination)
            try? fm.copyItem(at: URL(fileURLWithPath: path), to: destination)
        }
    }

    static func suggestedName(device: String, date: Date = Date()) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd 'at' HH.mm.ss"
        return "\(device) \(formatter.string(from: date)).png"
    }
}

/// The latest screenshot of a device, with Save and Copy.
struct ScreenshotView: View {
    let device: Device
    let shot: ScreenshotResult?
    let isTaking: Bool
    let take: () -> Void
    @State private var image: NSImage?

    var body: some View {
        VStack(spacing: 12) {
            if let shot {
                Group {
                    if let image {
                        Image(nsImage: image)
                            .resizable()
                            .interpolation(.none)
                            .aspectRatio(CGFloat(shot.width) / CGFloat(max(shot.height, 1)), contentMode: .fit)
                            .frame(maxWidth: CGFloat(shot.width), maxHeight: CGFloat(shot.height))
                            .clipShape(RoundedRectangle(cornerRadius: 6))
                            .shadow(radius: 2, y: 1)
                            .accessibilityLabel(Text("Screenshot of \(device.displayName)"))
                            .draggable(image)
                    } else {
                        ProgressView().frame(height: 200)
                    }
                }
                // The image takes the room the buttons leave, so they stay in view.
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .task(id: shot.path) { image = await ScreenshotFile.loadImage(path: shot.path) }
                HStack {
                    Button("Copy", systemImage: "doc.on.doc") {
                        Task { await ScreenshotFile.copyToPasteboard(path: shot.path) }
                    }
                    Button("Save…", systemImage: "square.and.arrow.down") {
                        ScreenshotFile.save(path: shot.path, suggestedName: ScreenshotFile.suggestedName(device: device.displayName))
                    }
                    Spacer()
                    Button("Take Another", systemImage: "camera", action: take)
                        .disabled(!device.isLinked || isTaking)
                }
                .labelStyle(.titleAndIcon)
            } else {
                ContentUnavailableView {
                    Label("No Screenshot Yet", systemImage: "camera")
                } description: {
                    Text("Capture both screens of the console as they are now.")
                } actions: {
                    Button("Take Screenshot", action: take)
                        .disabled(!device.isLinked || isTaking)
                }
                .frame(minHeight: 200)
            }
        }
        .overlay {
            if isTaking { ProgressView().controlSize(.large) }
        }
    }
}
