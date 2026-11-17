import AppKit
import HubAppModel
import HubClient
import SwiftUI
import WebKit

/// The store's applet player, carried in the app: pocket-nexus/applets
/// `store/site/public/play` (`index.html`, `player.js`, PocketJS's core as
/// `pocketjs.wasm`), from the submodule `vendor/applets`, copied into
/// `Contents/Resources/Player` by `scripts/embed-player.sh`. It runs an
/// applet's package on PocketJS's core in WebAssembly and answers every
/// request the applet makes from the version's sample data, with no network:
/// nothing reaches a device or an account. The page that frames it posts it
/// the package, the sample data and the core, and hears back what the applet
/// did (`store/play/player.ts`). Here that page is the app's own, served with
/// the player and Hub's files from a scheme of this web view alone.
enum AppletPlayer {
    /// The variant the player runs, as the store's website runs it
    /// (`components/AppPlayer.vue` TARGET): Omarchy Pocket's.
    static let target = "rg-ds-plus"
    /// Its surface, 512 × 384 points (`SURFACE`).
    static let surface = (width: 512, height: 384)
    /// The scheme the web view's pages are served from; WebKit hands it to
    /// this web view's handler alone, and nothing outside the app knows it.
    static let scheme = "hub-player"

    /// Whether Try It can run the applet: the store has sample data for its
    /// version, and its package has the variant the player runs.
    static func plays(_ app: StoreApp) -> Bool {
        app.sample && app.targets.contains(target)
    }
}

/// Try It on an applet's page: the player in a 4:3 surface, started by the
/// owner. Hub downloads the package and the sample data first when its
/// cache lacks them (`store.sample`).
struct AppletPlayerSection: View {
    let app: StoreApp
    @Environment(AppState.self) private var state
    @State private var phase: Phase = .idle
    /// Each start makes a new web view.
    @State private var run = 0
    @State private var controller = PlayerController()

    private enum Phase: Equatable {
        case idle
        case preparing(Double?)
        /// The files are here; `started` once the applet drew its first frame.
        case playing(StoreSample, started: Bool)
        case failed(String)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ZStack {
                Color.black
                switch phase {
                case .idle:
                    VStack(spacing: 14) {
                        StoreIcon(path: app.icon, size: 56)
                        Button("Try It", systemImage: "play.fill", action: start)
                            .buttonStyle(.borderedProminent)
                            .controlSize(.large)
                    }
                case .preparing(let fraction):
                    VStack(spacing: 10) {
                        if let fraction {
                            ProgressView(value: fraction).frame(width: 180)
                        } else {
                            ProgressView().controlSize(.large)
                        }
                        Text("Downloading…").foregroundStyle(.white.opacity(0.8))
                    }
                case .playing(let files, let started):
                    PlayerWebView(files: files, controller: controller) { event in hear(event) }
                        .id(run)
                    if !started {
                        ProgressView().controlSize(.large)
                    }
                case .failed(let message):
                    VStack(spacing: 8) {
                        Text("The applet stopped.").font(.headline).foregroundStyle(.white)
                        Text(verbatim: message)
                            .font(.callout)
                            .foregroundStyle(.white.opacity(0.8))
                            .multilineTextAlignment(.center)
                            .textSelection(.enabled)
                        Button("Try Again", action: start)
                    }
                    .padding()
                }
            }
            .aspectRatio(CGFloat(AppletPlayer.surface.width) / CGFloat(AppletPlayer.surface.height), contentMode: .fit)
            .frame(maxWidth: 640)
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            .frame(maxWidth: .infinity)
            HStack {
                Text("On sample data: it reaches no device and no account.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Spacer()
                if case .playing = phase {
                    // The deck's Back, which the player turns into Cross (`button` "back").
                    Button("Back") { controller.back() }
                        .help("The device's Back button")
                    Button("Stop") { phase = .idle }
                }
            }
        }
        .padding(.vertical, 4)
        // Another applet's page in the same place starts again from Try It.
        .onChange(of: app.id) { phase = .idle }
        #if DEBUG
        .onChange(of: state.debugTries) { start() }
        #endif
    }

    private func start() {
        phase = .preparing(nil)
        let store = state.model.store
        Task {
            do {
                let files = try await store.sample(of: app) { fraction in
                    Task { @MainActor in
                        if case .preparing = phase { phase = .preparing(fraction) }
                    }
                }
                run += 1
                phase = .playing(files, started: false)
            } catch {
                phase = .failed(UserFacingError(storeError: error).title)
            }
        }
    }

    private func hear(_ event: PlayerEvent) {
        switch event {
        case .running:
            if case .playing(let files, _) = phase { phase = .playing(files, started: true) }
        case .stopped(let message):
            phase = .failed(message)
        }
    }
}

/// What the player said, as the app's page passes it on.
enum PlayerEvent {
    /// The applet drew its first frame.
    case running
    /// It could not start, or stopped: the player's words.
    case stopped(String)
}

/// Sends the player what the page's own buttons do.
@MainActor
final class PlayerController {
    weak var webView: WKWebView?

    func back() {
        webView?.evaluateJavaScript("back()")
    }
}

/// The web view: the app's page from `AppletPlayer.scheme`, which frames
/// the player in a sandboxed frame as the store's website does.
struct PlayerWebView: NSViewRepresentable {
    let files: StoreSample
    let controller: PlayerController
    let hear: (PlayerEvent) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(hear: hear) }

    func makeNSView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.setURLSchemeHandler(PlayerSchemeHandler(files: files), forURLScheme: AppletPlayer.scheme)
        configuration.userContentController.add(context.coordinator, name: "player")
        // Nothing of a run is kept: no cookies, storage or cache on disk.
        configuration.websiteDataStore = .nonPersistent()
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.underPageBackgroundColor = .black
        controller.webView = view
        view.load(URLRequest(url: URL(string: "\(PlayerSchemeHandler.origin)/")!))
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {
        context.coordinator.hear = hear
    }

    static func dismantleNSView(_ view: WKWebView, coordinator: Coordinator) {
        view.configuration.userContentController.removeScriptMessageHandler(forName: "player")
    }

    /// Hears the page's `webkit.messageHandlers.player` (the player's
    /// `running` and `error`).
    final class Coordinator: NSObject, WKScriptMessageHandler {
        var hear: (PlayerEvent) -> Void

        init(hear: @escaping (PlayerEvent) -> Void) {
            self.hear = hear
        }

        func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
            guard let body = message.body as? [String: Any], let type = body["type"] as? String else { return }
            switch type {
            case "running": hear(.running)
            case "error": hear(.stopped(body["message"] as? String ?? ""))
            default: break
            }
        }
    }
}

/// Serves the web view's pages: the app's own page at `/`, the player's
/// files at `/play/` from the app's resources, and the applet's package and
/// sample data at `/local/` from Hub's cache, as `store/play/serve.ts`
/// serves them to a developer's browser. Each page has the policy the store
/// gives it: the player's frame reaches nothing, not even this scheme's
/// files, which the app's page fetches and posts to it.
final class PlayerSchemeHandler: NSObject, WKURLSchemeHandler {
    let files: StoreSample
    private let player = Bundle.main.resourceURL?.appending(path: "Player", directoryHint: .isDirectory)

    init(files: StoreSample) {
        self.files = files
    }

    /// The frame's policy, as the store's `store/site/public/_headers` gives
    /// it on applets.pocket.nexus (`store/play/serve.ts` PLAYER_POLICY), with
    /// this scheme's origin written out where the store writes `'self'`: the
    /// frame is sandboxed, its origin is opaque, and WebKit matches `'self'`
    /// against no origin of a custom scheme, so the player's script did not
    /// load (measured).
    static let playerPolicy = "default-src 'none'; script-src \(origin) 'unsafe-eval' 'wasm-unsafe-eval'; style-src 'unsafe-inline'; img-src 'none'; connect-src 'none'; frame-ancestors \(origin)"
    /// The origin of the app's page and the player's files.
    static let origin = "\(AppletPlayer.scheme)://applet"
    /// The app's page: its own inline script, the frame, and fetches of this scheme's files.
    static let pagePolicy = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; frame-src 'self'; connect-src 'self'"

    /// The app's page: the frame, and what `serve.ts`'s page does with it,
    /// the requests list left out. The surface and the variant are the
    /// website's.
    static let page = """
    <!doctype html>
    <html><head><meta charset="utf-8">
    <style>html, body { margin: 0; height: 100%; background: #000; overflow: hidden; } iframe { border: 0; width: 100%; height: 100%; display: block; }</style>
    </head><body>
    <iframe id="player" sandbox="allow-scripts" src="/play/"></iframe>
    <script>
    const frame = document.getElementById("player");
    const tell = (message) => webkit.messageHandlers.player.postMessage(message);
    addEventListener("message", async (event) => {
      if (event.source !== frame.contentWindow) return;
      const heard = event.data || {};
      if (heard.type === "ready") {
        try {
          const [pkg, wasm, mock] = await Promise.all([
            fetch("/local/package").then((answer) => answer.arrayBuffer()),
            fetch("/play/pocketjs.wasm").then((answer) => answer.arrayBuffer()),
            fetch("/local/mock").then((answer) => answer.json()),
          ]);
          frame.contentWindow.postMessage({ type: "boot", package: pkg, wasm, mock, size: [\(AppletPlayer.surface.width), \(AppletPlayer.surface.height)], target: "\(AppletPlayer.target)" }, "*", [pkg, wasm]);
        } catch (error) {
          tell({ type: "error", message: String(error) });
        }
      } else if (heard.type === "running") {
        tell({ type: "running" });
        frame.focus();
      } else if (heard.type === "error") {
        tell({ type: "error", message: heard.message || "" });
      }
    });
    function back() { frame.contentWindow.postMessage({ type: "button", name: "back" }, "*"); }
    </script>
    </body></html>
    """

    func webView(_ webView: WKWebView, start task: any WKURLSchemeTask) {
        guard let url = task.request.url else { return }
        let found: (body: Data, type: String, policy: String?)?
        switch url.path(percentEncoded: false) {
        case "", "/": found = (Data(Self.page.utf8), "text/html; charset=utf-8", Self.pagePolicy)
        case "/play", "/play/", "/play/index.html": found = resource("index.html").map { ($0, "text/html; charset=utf-8", Self.playerPolicy) }
        case "/play/player.js": found = resource("player.js").map { ($0, "text/javascript", nil) }
        case "/play/pocketjs.wasm": found = resource("pocketjs.wasm").map { ($0, "application/wasm", nil) }
        case "/local/package": found = (try? Data(contentsOf: URL(fileURLWithPath: files.package))).map { ($0, "application/octet-stream", nil) }
        case "/local/mock": found = (try? Data(contentsOf: URL(fileURLWithPath: files.mock))).map { ($0, "application/json", nil) }
        default: found = nil
        }
        var headers = ["Content-Type": found?.type ?? "text/plain", "Cache-Control": "no-store"]
        if let policy = found?.policy { headers["Content-Security-Policy"] = policy }
        let response = HTTPURLResponse(url: url, statusCode: found == nil ? 404 : 200, httpVersion: "HTTP/1.1", headerFields: headers)!
        task.didReceive(response)
        task.didReceive(found?.body ?? Data("not found".utf8))
        task.didFinish()
    }

    func webView(_ webView: WKWebView, stop task: any WKURLSchemeTask) {}

    private func resource(_ name: String) -> Data? {
        player.flatMap { try? Data(contentsOf: $0.appending(path: name)) }
    }
}
