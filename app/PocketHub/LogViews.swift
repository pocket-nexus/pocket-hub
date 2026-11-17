import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// The device's live log: a switch that turns `devices.logs` on and off, and
/// the `device.log` lines, newest at the bottom, following the end.
struct DeviceLogView: View {
    let device: Device
    @Environment(AppState.self) private var state

    var body: some View {
        let model = state.model
        let isOn = model.logsOn.contains(device.id)
        let entries = model.deviceLogs[device.id]?.entries ?? []
        VStack(spacing: 0) {
            HStack {
                // Hub accepts the request while the device is away; the lines
                // start when it links, and Hub keeps them on across relinks.
                Toggle("Live Log", isOn: Binding(get: { isOn }, set: { setLogs($0) }))
                    .toggleStyle(.switch)
                Spacer()
                Button("Copy All", systemImage: "doc.on.doc") {
                    let text = entries.map { "[\($0.line.level)] \($0.line.source): \($0.line.text)" }.joined(separator: "\n")
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(text, forType: .string)
                }
                .disabled(entries.isEmpty)
                Button("Clear", systemImage: "trash") { model.clearLog(of: device.id) }
                    .disabled(entries.isEmpty)
            }
            .labelStyle(.iconOnly)
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            if entries.isEmpty {
                ContentUnavailableView {
                    Label("No Log Lines", systemImage: "text.alignleft")
                } description: {
                    if isOn && !device.isLinked {
                        Text("The lines start when the device connects.")
                    } else if isOn {
                        Text("Lines appear here while the applet writes them.")
                    } else {
                        Text("Turn on the live log to see what the device and its applets write.")
                    }
                }
                // The empty state takes the room below the header, so the
                // header stays at the top instead of being centred with it.
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                LogList(entries: entries) { entry in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        LevelBadge(level: entry.line.level)
                        Text(verbatim: entry.line.source).foregroundStyle(.secondary).frame(width: 40, alignment: .leading)
                        Text(verbatim: entry.line.text).textSelection(.enabled)
                        if let dropped = entry.line.dropped, dropped > 0 {
                            Text("(\(dropped) lines dropped)").foregroundStyle(.orange)
                        }
                    }
                }
                // The lines sit in a text area's own background, rounded as
                // the grouped pages' cards are, rather than under a divider.
                .scrollContentBackground(.hidden)
                .background(Color(nsColor: .textBackgroundColor))
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                .padding([.horizontal, .bottom], 20)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }

    private func setLogs(_ on: Bool) {
        Task {
            do { try await state.model.setLogs(on, for: device.id) } catch {
                state.presentedError = UserFacingError(error, deviceName: device.displayName, kind: device.kind)
            }
        }
    }
}

struct LevelBadge: View {
    let level: String

    var body: some View {
        Text(verbatim: level.uppercased())
            .font(.caption2.monospaced().bold())
            .foregroundStyle(color)
            .frame(width: 44, alignment: .leading)
    }

    private var color: Color {
        switch level {
        case "error", "stderr": .red
        case "warn": .orange
        case "debug": .secondary
        case "hub": .blue
        default: .primary
        }
    }
}

/// A monospaced list that keeps the newest line in view while the reader is at the end.
struct LogList<Line: Sendable & Hashable, Row: View>: View {
    let entries: [LogBuffer<Line>.Entry]
    @ViewBuilder let row: (LogBuffer<Line>.Entry) -> Row

    var body: some View {
        ScrollViewReader { proxy in
            List(entries) { entry in
                row(entry)
                    .font(.system(.callout, design: .monospaced))
                    .id(entry.id)
                    .listRowSeparator(.hidden)
            }
            .listStyle(.plain)
            .onChange(of: entries.last?.id) { _, last in
                if let last { proxy.scrollTo(last, anchor: .bottom) }
            }
            .onAppear {
                if let last = entries.last?.id { proxy.scrollTo(last, anchor: .bottom) }
            }
        }
    }
}

/// A service's output: the tail from `services.logs`, then `service.log` lines.
struct ServiceLogView: View {
    let service: Service
    @Environment(AppState.self) private var state

    var body: some View {
        let entries = state.model.serviceLogs[service.id]?.entries ?? []
        Group {
            if entries.isEmpty {
                ContentUnavailableView("No Output", systemImage: "text.alignleft",
                                       description: Text("This service hasn't written anything yet."))
            } else {
                LogList(entries: entries) { entry in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(entry.line.at.date.formatted(date: .omitted, time: .standard))
                            .foregroundStyle(.secondary)
                        if entry.line.stream != "stdout" { LevelBadge(level: entry.line.stream) }
                        Text(verbatim: entry.line.text).textSelection(.enabled)
                    }
                }
            }
        }
        .task(id: service.id) {
            do { try await state.model.loadServiceLog(service.id) } catch {
                state.presentedError = UserFacingError(error)
            }
        }
    }
}
