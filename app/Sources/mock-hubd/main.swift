// mock-hubd: the mock core as a process, so the app can be run against it.
//
//   mock-hubd [--home DIR] [--scenario demo|first-run|tab5] [--fail-setup-at STAGE]
//             [--fail-reason REASON] [--link-after SECONDS | --no-link] [--unconfirmed]
//
// It listens at `<home>/hubd.sock`, home from --home or `$POCKET_HUB_HOME`.
// It refuses to run without one of them: the default home belongs to the
// owner's real core. It reads commands on standard input, one per line:
//
//   link <id> [carrier]        unlink <id> [code]        forget <id>
//   log <id> <text>            service-log <id> <text>    fail <method> <kind> [message]
//   candidates <address> ...   drop                      quit
//   store-link-loss <app> ...  (the link ends under these applets' installs)
//   release <version>|none     (the newest release of the devices' program, learned at their next link)
//   update ok | fail <unreachable|not_found|rejected> | drop | launch-fails | no-relink | stale
//                              (what the next devices.installProgram does)
//   pair-ask [code]            (a Tab5 asks to be paired by the code it shows, 314159 without one)
//   pair-gone                  (that Tab5 leaves)
//
// It exits on `quit`, on `hub.shutdown`, and on SIGINT/SIGTERM, removing its socket.

import Foundation
import HubClient
import MockHubCore

func usage() -> Never {
    FileHandle.standardError.write(Data("""
    usage: mock-hubd [--home DIR] [--scenario demo|first-run|tab5] [--fail-setup-at STAGE]
                     [--fail-reason REASON] [--link-after SECONDS | --no-link] [--unconfirmed]

    """.utf8))
    exit(2)
}

var arguments = Array(CommandLine.arguments.dropFirst())
var home: URL? = ProcessInfo.processInfo.environment["POCKET_HUB_HOME"].flatMap {
    $0.isEmpty ? nil : URL(fileURLWithPath: ($0 as NSString).expandingTildeInPath, isDirectory: true)
}
var scenario = MockScenario.demo
while !arguments.isEmpty {
    let argument = arguments.removeFirst()
    func value() -> String {
        guard !arguments.isEmpty else { usage() }
        return arguments.removeFirst()
    }
    switch argument {
    case "--home": home = URL(fileURLWithPath: (value() as NSString).expandingTildeInPath, isDirectory: true)
    case "--scenario":
        switch value() {
        case "demo": scenario = .demo
        case "tab5": scenario = .withTab5
        case "first-run": scenario = .firstRun
        default: usage()
        }
    case "--fail-setup-at": scenario.setup.failAt = SetupStage(rawValue: value())
    case "--fail-reason": scenario.setup.failureReason = value()
    case "--link-after": scenario.setup.linkAfter = .seconds(Double(value()) ?? 3)
    case "--no-link": scenario.setup.linkAfter = nil
    case "--unconfirmed": scenario.setup.confirmed = false
    case "-h", "--help": usage()
    default: usage()
    }
}

guard let home else {
    FileHandle.standardError.write(Data("mock-hubd: give --home DIR or set POCKET_HUB_HOME; the default home is the real core's.\n".utf8))
    exit(2)
}
let core = MockHubCore(home: home, scenario: scenario)
await core.setOnShutdown {
    print("mock-hubd: hub.shutdown received, exiting")
    exit(0)
}
do {
    try await core.start()
} catch {
    FileHandle.standardError.write(Data("mock-hubd: cannot listen: \(error)\n".utf8))
    exit(1)
}
print("mock-hubd: listening at \(core.socketPath)")
fflush(stdout)

// Signals: remove the socket and exit.
signal(SIGINT, SIG_IGN)
signal(SIGTERM, SIG_IGN)
var signalSources: [DispatchSourceSignal] = []
for number in [SIGINT, SIGTERM] {
    let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
    source.setEventHandler {
        Task {
            await core.stop()
            exit(0)
        }
    }
    source.resume()
    signalSources.append(source)
}

// Commands on standard input, read on their own thread (readLine blocks) and
// handled in order. End of input leaves the core running (it may have been
// started with stdin closed); `quit` or a signal ends it.
let commands = AsyncStream<String> { continuation in
    Thread.detachNewThread {
        while let line = readLine() { continuation.yield(line) }
        continuation.finish()
    }
}
Task {
    for await line in commands {
        let words = line.split(separator: " ", omittingEmptySubsequences: true).map(String.init)
        guard let command = words.first else { continue }
        let rest = Array(words.dropFirst())
        switch command {
        case "link" where rest.count >= 1: await core.link(rest[0], carrier: rest.count > 1 ? rest[1] : "tcp")
        case "unlink" where rest.count >= 1: await core.unlink(rest[0], code: rest.count > 1 ? rest[1] : "going_away")
        case "forget" where rest.count == 1: await core.remove(rest[0])
        case "log" where rest.count >= 2: await core.emitDeviceLog(rest[0], text: rest.dropFirst().joined(separator: " "))
        case "service-log" where rest.count >= 2: await core.emitServiceLog(rest[0], text: rest.dropFirst().joined(separator: " "))
        case "fail" where rest.count >= 2:
            await core.failNext(rest[0], with: .init(kind: rest[1], message: rest.count > 2 ? rest.dropFirst(2).joined(separator: " ") : "Injected failure."))
        case "candidates": await core.setCandidates(rest.map { SetupCandidate(address: $0, port: 5000, banner: "220 Hello!", ftpd: true) })
        case "drop": await core.dropConnections()
        // A Tab5 asks to be paired: `pair-ask [code]`, 314159 without one; `pair-gone` takes it back.
        case "pair-ask":
            var request = MockPairRequest(device: "7ab5000000000002")
            request.name = "Kitchen Tab5"
            if let code = rest.first { request.code = code }
            await core.askToPair(request)
        case "pair-gone": await core.withdrawPairRequest("7ab5000000000002")
        case "store-link-loss": await core.setStoreLinkLoss(Set(rest))
        case "release" where rest.count == 1:
            await core.setProgramRelease(version: rest[0] == "none" ? nil : rest[0])
        case "update" where !rest.isEmpty:
            var behavior = MockProgramBehavior()
            switch rest[0] {
            case "ok": break
            case "fail" where rest.count == 2: behavior.releaseFailure = rest[1]
            case "drop": behavior.dropLinkWhileSending = true
            case "launch-fails": behavior.launchFails = true
            case "no-relink":
                behavior.relinkAfter = nil
                behavior.launchHoldLimit = .seconds(10)
            case "stale": behavior.reportedAfterRestart = "2026.11.2"
            default: print("mock-hubd: unknown update behavior: \(line)")
            }
            await core.setProgramBehavior(behavior)
        case "quit":
            await core.stop()
            exit(0)
        default: print("mock-hubd: unknown command: \(line)")
        }
        fflush(stdout)
    }
}

// Top-level code is async here, so the main queue is already being serviced
// (dispatchMain would trap); park this task for the life of the process.
await withUnsafeContinuation { (_: UnsafeContinuation<Void, Never>) in }
