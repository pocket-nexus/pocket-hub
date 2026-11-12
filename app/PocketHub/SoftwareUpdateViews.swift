import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// The device's software update, a card of its page: offered when Hub knows a
/// newer release (`program.update`), then its progress, the restart as an
/// expected step, and how it ended. No card when there is no update.
struct SoftwareUpdateSection: View {
    let device: Device
    @Environment(AppState.self) private var state

    var body: some View {
        let model = state.model
        if let update = model.programUpdates[device.id] {
            Section {
                switch update.stage {
                case .finished(let version):
                    finished(version: version)
                case .failed(let failure):
                    failed(failure)
                default:
                    progress(update)
                }
            }
        } else if device.offersSoftwareUpdate, let latest = device.program.latest {
            Section {
                available(latest: latest)
            }
        }
    }

    // MARK: - Offered

    private func available(latest: String) -> some View {
        let unavailable = unavailableReason
        return BannerBox {
            Image(systemName: "arrow.down.circle.fill")
                .font(.title)
                .foregroundStyle(Color.accentColor)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text("An update for your 3DS's software is available").font(.headline)
                if let current = device.program.version {
                    Text("Version \(latest) · you have \(current)").font(.callout).foregroundStyle(.secondary)
                } else {
                    Text("Version \(latest)").font(.callout).foregroundStyle(.secondary)
                }
                Text(verbatim: unavailable ?? String(localized: "Your 3DS restarts during the update and reconnects by itself."))
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            Button("Update Now") { state.model.updateProgram(on: device.id) }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(unavailable != nil)
        }
    }

    /// Why the update cannot start now, in the owner's words; nil when it can.
    private var unavailableReason: String? {
        if !device.isLinked { return String(localized: "Turn on your 3DS and wait until it shows as connected to update it.") }
        if let busy = device.busy { return busy.displayText }
        return nil
    }

    // MARK: - Running

    private func progress(_ update: ProgramUpdate) -> some View {
        BannerBox {
            Image(systemName: update.stage == .restarting || update.stage == .reconnected
                  ? "arrow.triangle.2.circlepath" : "arrow.down.circle")
                .font(.title)
                .foregroundStyle(Color.accentColor)
                .symbolEffect(.pulse, isActive: update.stage == .restarting || update.stage == .reconnected)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                if let version = update.version {
                    Text("Updating your 3DS's software to version \(version)").font(.headline)
                } else {
                    Text("Updating your 3DS's software").font(.headline)
                }
                Text(verbatim: update.stage.displayText).font(.callout)
                if let fraction = update.fraction {
                    ProgressView(value: fraction).frame(maxWidth: 360)
                } else {
                    ProgressView().progressViewStyle(.linear).frame(maxWidth: 360)
                }
                Group {
                    switch update.stage {
                    case .restarting:
                        Text("This is expected and takes up to a minute. Keep your 3DS on and nearby.")
                    case .reconnected:
                        EmptyView()
                    default:
                        Text("Keep your 3DS on and nearby until it reconnects.")
                    }
                }
                .font(.caption).foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
    }

    // MARK: - Ended

    private func finished(version: String?) -> some View {
        BannerBox {
            Image(systemName: "checkmark.circle.fill")
                .font(.title)
                .foregroundStyle(.green)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text("Your 3DS's software is up to date").font(.headline)
                if let version {
                    Text("It reconnected with version \(version).").font(.callout).foregroundStyle(.secondary)
                } else {
                    Text("It reconnected with the new version.").font(.callout).foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 12)
            Button("OK") { state.model.dismissProgramUpdate(of: device.id) }
        }
    }

    private func failed(_ failure: UserFacingError) -> some View {
        BannerBox {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.title)
                .foregroundStyle(.orange)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: failure.title).font(.headline)
                Text(verbatim: failure.advice).font(.callout).foregroundStyle(.secondary)
                if let detail = failure.detail {
                    Text(verbatim: detail).font(.caption).foregroundStyle(.tertiary).textSelection(.enabled)
                }
            }
            Spacer(minLength: 12)
            VStack(alignment: .trailing, spacing: 6) {
                if device.offersSoftwareUpdate {
                    Button("Try Again") { state.model.updateProgram(on: device.id) }
                        .disabled(unavailableReason != nil)
                        .help(unavailableReason ?? "")
                }
                Button("Close") { state.model.dismissProgramUpdate(of: device.id) }
            }
        }
    }
}

/// The row the update's states share: a symbol in the state's colour, the
/// words, the action. The card around it is the grouped form's own, as the
/// cards of the page's other sections are; the states differ by the symbol.
private struct BannerBox<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            content
        }
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The device's details: who it is, its software version, its connection.
struct DeviceDetailsForm: View {
    let device: Device

    var body: some View {
        Form {
            Section {
                LabeledContent("Name") { Text(verbatim: device.displayName) }
                if !device.model.isEmpty {
                    LabeledContent("Model") { Text(verbatim: device.model) }
                }
                LabeledContent("Software version") {
                    VStack(alignment: .trailing, spacing: 2) {
                        if let version = device.program.version {
                            Text(verbatim: version).textSelection(.enabled)
                        } else {
                            Text("Unknown until it connects")
                        }
                        if device.offersSoftwareUpdate, let latest = device.program.latest {
                            Text("Version \(latest) is available").font(.caption).foregroundStyle(Color.accentColor)
                        } else if device.program.version != nil, device.program.latest != nil {
                            Text("Up to date").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
            }
            Section {
                LabeledContent("Connection") {
                    if let link = device.link {
                        Text("Connected over \(link.carrier.displayName)")
                    } else {
                        Text("Not connected")
                    }
                }
                if let address = device.lastAddress {
                    LabeledContent("Address") { Text(verbatim: address).textSelection(.enabled) }
                }
                if !device.isLinked, let seen = device.lastSeen {
                    LabeledContent("Last seen") { Text(seen.date.formatted(date: .abbreviated, time: .shortened)) }
                }
                if let apps = device.apps {
                    LabeledContent("Applets") { Text(verbatim: "\(apps.filter { !$0.isHome }.count)") }
                }
                LabeledContent("Identifier") { Text(verbatim: device.id).textSelection(.enabled) }
            }
        }
        .formStyle(.grouped)
    }
}
