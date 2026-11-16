import Foundation
import HubClient
import Observation
import os

/// The applet store as the app shows it (CONTROL-API.md §3 "Store").
///
/// The page lists the store's applets for one device (the one chosen, or the
/// only one paired), with each applet's `installed` and `update`; with no device
/// paired it lists the whole catalog. `known` holds the whole catalog by id so
/// other pages can give installed applets a title and an icon.
///
/// Hub answers a failed catalog fetch with the error `store`, never the older
/// catalog; the page then shows that failure instead of a list. While a list is
/// being refreshed for the same device, the list on screen stays until the
/// answer replaces it, so the many `store.changed` of a background prefetch do
/// not make the page flicker.
@MainActor
@Observable
public final class StoreModel {
    static let log = Logger(subsystem: "nexus.pocket.hub", category: "store")

    /// What the page shows for.
    public enum Context: Equatable, Sendable {
        /// No device is paired: the whole catalog, nothing can be installed.
        case noDevice
        /// A paired device; `knownTarget` is false before its first link.
        case device(id: String, knownTarget: Bool)
    }

    public enum Page: Equatable, Sendable {
        case idle
        case loading
        case loaded(StoreCatalog)
        case failed(UserFacingError)
    }

    /// An install in progress.
    public struct Install: Equatable, Sendable {
        public var appId: String
        public var deviceId: String
        /// `downloading`, then `sending`, `receiving`, `received`, `staged`.
        public var phase: String?
        public var fraction: Double?
    }

    public private(set) var page: Page = .idle
    /// A refresh of the list on screen is under way.
    public private(set) var isRefreshing = false
    /// The whole catalog by applet id, for titles and icons elsewhere. Kept
    /// across outages: it names applets, it does not describe live state.
    public private(set) var known: [String: StoreApp] = [:]
    /// Installs in progress, by device and applet.
    public private(set) var installs: [String: Install] = [:]
    /// The last install failure, for the page to show; cleared by `dismissFailure()`.
    public var installFailure: UserFacingError?
    /// An install that ended because the device's link ended under it. Whether
    /// the device holds the applet is unknown until it links again and reports
    /// its apps, so the page says that once, in place of "not connected", and
    /// no alert claims a failure. Cleared when the device links again.
    public private(set) var interrupted: InterruptedInstall?

    public struct InterruptedInstall: Equatable, Sendable {
        public var deviceId: String
        public var appTitle: String
        /// The link the install ran on; a device on another link has come back.
        var linkSince: EpochMilliseconds?
    }
    /// The device the owner chose with the picker; nil follows the default.
    public private(set) var chosenDevice: String?

    weak var model: HubModel?
    private let client: any HubCalling
    private var loadTask: Task<Void, Never>?
    private var loadGeneration = 0
    private var reloadPending = false
    private var installTasks: [String: Task<Void, Never>] = [:]

    init(client: any HubCalling, model: HubModel?) {
        self.client = client
        self.model = model
    }

    // MARK: - What the page is for

    /// The device the page shows: the chosen one while it is paired, else the
    /// only one, else the first linked one, else the first.
    public var deviceId: String? {
        let devices = model?.devices ?? []
        if let chosenDevice, devices.contains(where: { $0.id == chosenDevice }) { return chosenDevice }
        return devices.first(where: \.isLinked)?.id ?? devices.first?.id
    }

    public var context: Context {
        guard let id = deviceId, let device = model?.device(id) else { return .noDevice }
        return .device(id: id, knownTarget: device.hasKnownTarget)
    }

    /// Chooses the device of the page (the picker shown with several devices).
    public func choose(device id: String) {
        guard id != deviceId || chosenDevice != id else { return }
        chosenDevice = id
        reload(clearing: true)
    }

    /// The list on screen, when there is one.
    public var apps: [StoreApp] {
        if case .loaded(let catalog) = page { return catalog.apps }
        return []
    }

    // MARK: - Loading

    /// Fetches the page's list again. With `refresh`, Hub fetches the catalog
    /// from the store first (⌘R). `clearing` drops the list on screen first:
    /// for another device, the old list is not this device's.
    public func reload(refresh: Bool = false, clearing: Bool = false) {
        guard model?.isLive == true else { return }
        if loadTask != nil && !refresh && !clearing {
            reloadPending = true
            return
        }
        loadTask?.cancel()
        loadGeneration += 1
        let generation = loadGeneration
        let context = self.context
        if clearing || !isLoaded { page = .loading }
        isRefreshing = true
        reloadPending = false
        loadTask = Task { [weak self, client] in
            let result: Result<(StoreCatalog, StoreCatalog?), any Error>
            do {
                switch context {
                case .noDevice:
                    let all = try await client.storeApps(device: nil, refresh: refresh)
                    result = .success((all, all))
                case .device(let id, let knownTarget):
                    // A device that never linked has no known target and is
                    // offered nothing; the page says so without asking Hub.
                    let forDevice = knownTarget
                        ? try await client.storeApps(device: id, refresh: refresh)
                        : StoreCatalog(apps: [], fetchedAt: 0)
                    let all = try? await client.storeApps(device: nil, refresh: false)
                    result = .success((forDevice, all))
                }
            } catch {
                result = .failure(error)
            }
            self?.finishLoad(result, generation: generation)
        }
    }

    private var isLoaded: Bool {
        if case .loaded = page { return true }
        return false
    }

    private func finishLoad(_ result: Result<(StoreCatalog, StoreCatalog?), any Error>, generation: Int) {
        guard generation == loadGeneration else { return }
        loadTask = nil
        isRefreshing = false
        switch result {
        case .success(let (catalog, all)):
            page = .loaded(catalog)
            if let all { known = Dictionary(all.apps.map { ($0.id, $0) }, uniquingKeysWith: { $1 }) }
        case .failure(let error):
            if let hubError = error as? HubError, hubError.isCancelled { return }
            page = .failed(UserFacingError(storeError: error))
        }
        if reloadPending { reload() }
    }

    // MARK: - From HubModel

    func didSync() {
        reload(clearing: true)
    }

    func catalogDidChange() {
        reload()
    }

    func deviceDidChange(from old: Device?, to new: Device) {
        if let interrupted, interrupted.deviceId == new.id, let link = new.link, link.since != interrupted.linkSince {
            // Linked again: its apps report, not the ended call, says what it holds.
            self.interrupted = nil
            if new.id == deviceId { reload() }
        }
        guard new.id == deviceId else { return }
        // What the device holds decides `installed` and `update`; its first
        // link makes its target known.
        if old?.apps != new.apps || old?.hasKnownTarget != new.hasKnownTarget || old == nil
            || old?.isLinked != new.isLinked
        {
            reload(clearing: old?.hasKnownTarget != new.hasKnownTarget)
        }
    }

    func devicesDidChange() {
        // The page's device may have changed (removed, or the first paired).
        reload(clearing: true)
    }

    func dropLiveState() {
        loadTask?.cancel()
        loadTask = nil
        loadGeneration += 1
        page = .idle
        isRefreshing = false
        reloadPending = false
        for task in installTasks.values { task.cancel() }
        installTasks = [:]
        installs = [:]
        interrupted = nil
    }

    // MARK: - Installing

    static func key(_ device: String, _ app: String) -> String { "\(device)/\(app)" }

    public func install(for app: StoreApp, on device: String) -> Install? {
        installs[Self.key(device, app.id)]
    }

    /// Installs (or updates) `app` on `device`, with progress; a failure lands
    /// in `installFailure` in plain words.
    public func install(_ app: StoreApp, on device: String) {
        let key = Self.key(device, app.id)
        guard installTasks[key] == nil else { return }
        installs[key] = Install(appId: app.id, deviceId: device, phase: nil, fraction: nil)
        let deviceName = model?.device(device)?.displayName
        let deviceKind = model?.device(device)?.kind ?? DeviceKind(rawValue: "")
        let linkSince = model?.device(device)?.link?.since
        let title = app.title
        let progress: @Sendable (TransferProgress) -> Void = { [weak self] update in
            Task { @MainActor in
                guard let self, self.installs[key] != nil else { return }
                if self.installs[key]?.phase != update.phase {
                    Self.log.notice("install \(key, privacy: .public): \(update.phase ?? "-", privacy: .public)")
                }
                self.installs[key]?.phase = update.phase
                self.installs[key]?.fraction = update.fraction
            }
        }
        installTasks[key] = Task { [weak self, client] in
            do {
                _ = try await client.storeInstall(device: device, app: app.id, progress: progress)
                self?.installFinished(key, outcome: "installed", failure: nil)
            } catch {
                let cancelled = (error as? HubError)?.isCancelled == true
                if (error as? HubError)?.kind == .link {
                    self?.installInterrupted(key, device: device, appTitle: title, linkSince: linkSince, error: error)
                    return
                }
                self?.installFinished(key, outcome: cancelled ? "cancelled" : "failed: \(error)",
                                      failure: UserFacingError(storeInstallError: error, appTitle: title, deviceName: deviceName, kind: deviceKind))
            }
        }
    }

    /// Stops an install; Hub cancels the call and releases the device.
    public func cancelInstall(of app: StoreApp, on device: String) {
        installTasks[Self.key(device, app.id)]?.cancel()
    }

    /// The link ended under the install (`link`, e.g. `closed`). A device that
    /// is still on the link it installed on answered with an error: that is a
    /// failure like any other. One that is away, or already back on a new link,
    /// may hold the applet: the page waits for its apps report.
    private func installInterrupted(_ key: String, device: String, appTitle: String, linkSince: EpochMilliseconds?,
                                    error: any Error) {
        let current = model?.device(device)
        // `closed` is the link itself ending; the `device.unlinked` that says
        // so may reach this model a moment after the answer.
        let closed: Bool = if case .operation(let operation)? = error as? HubError { operation.linkCode == "closed" } else { false }
        let sameLink = current?.link != nil && current?.link?.since == linkSince
        if sameLink && !closed {
            installFinished(key, outcome: "failed: \(error)",
                            failure: UserFacingError(storeInstallError: error, appTitle: appTitle,
                                                     deviceName: current?.displayName,
                                                     kind: current?.kind ?? DeviceKind(rawValue: "")))
            return
        }
        if current?.link == nil || sameLink {
            interrupted = InterruptedInstall(deviceId: device, appTitle: appTitle, linkSince: linkSince)
        }
        installFinished(key, outcome: "link ended: \(error)", failure: nil)
    }

    private func installFinished(_ key: String, outcome: String, failure: UserFacingError?) {
        Self.log.notice("install \(key, privacy: .public) ended: \(outcome, privacy: .public)")
        installTasks[key] = nil
        installs[key] = nil
        if let failure { installFailure = failure }
        // The device's next apps report also reloads; this covers a device
        // that reports nothing new (the same version installed again).
        reload()
    }

    /// Opens an installed applet on the device.
    public func open(_ app: StoreApp, on device: String) async throws {
        guard let model else { return }
        try await model.launch(appId: app.id, on: device)
    }

    // MARK: - Opened from a link (docs/DEEP-LINKS.md §2)

    /// The store's entry for an applet id, out of the whole catalog; nil when
    /// the store does not list it. An id the catalog Hub holds lacks is asked
    /// for once more with a fresh catalog: the applet may have been published
    /// since Hub last fetched, and Hub answers from the catalog it holds for
    /// 10 minutes (CONTROL-API.md §3 "Store").
    public func lookUp(appId: String) async throws -> StoreApp? {
        let held = try await client.storeApps(device: nil, refresh: false)
        if let app = held.apps.first(where: { $0.id == appId }) { return app }
        return try await client.storeApps(device: nil, refresh: true).apps.first { $0.id == appId }
    }

    /// The paired devices that can run `app`, in the model's order (linked first).
    public func devices(running app: StoreApp) -> [Device] {
        (model?.devices ?? []).filter(app.runs(on:))
    }

    /// The device a link's applet is shown for: one that is linked before one
    /// that is not, since only a linked device can take the install now, and
    /// the device whose page is open (`shown`) before another. nil when no
    /// paired device can run the applet.
    public func device(for app: StoreApp, preferring shown: String?) -> Device? {
        let able = devices(running: app)
        let linked = able.filter(\.isLinked)
        return linked.first { $0.id == shown } ?? linked.first ?? able.first { $0.id == shown } ?? able.first
    }
}

extension Device {
    /// The device has linked at least once, so Hub knows its target.
    public var hasKnownTarget: Bool { !target.isEmpty }
}
