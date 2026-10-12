import AppKit
import Foundation

// Pass the environment through Launch Services, never through command-line
// --env arguments (which would expose credentials in process listings).
let arguments = Array(CommandLine.arguments.dropFirst())
guard let path = arguments.first else {
    fputs("Missing development app bundle\n", stderr)
    exit(1)
}
let configuration = NSWorkspace.OpenConfiguration()
configuration.createsNewApplicationInstance = true
configuration.allowsRunningApplicationSubstitution = false
configuration.activates = false
configuration.addsToRecentItems = false
configuration.promptsUserIfNeeded = false
configuration.arguments = Array(arguments.dropFirst())
configuration.environment = ProcessInfo.processInfo.environment.filter { key, _ in
    // These belong to the parent terminal/process, not the launched app.
    !key.hasPrefix("DYLD_") && !key.hasPrefix("XPC_")
        && key != "__CFBundleIdentifier" && key != "LaunchInstanceID"
}

var application: NSRunningApplication?
var failure: Error?
NSWorkspace.shared.openApplication(at: URL(fileURLWithPath: path), configuration: configuration) { app, error in
    application = app
    failure = error
}
let deadline = Date().addingTimeInterval(15)
while application == nil && failure == nil && Date() < deadline {
    RunLoop.current.run(until: Date().addingTimeInterval(0.025))
}
guard let app = application else {
    fputs("Development app launch failed: \(failure?.localizedDescription ?? "timed out")\n", stderr)
    exit(1)
}
let pidURL = URL(fileURLWithPath: path).deletingLastPathComponent().appendingPathComponent("launch.pid")
if app.processIdentifier > 1 {
    do {
        try "\(app.processIdentifier)\n".write(to: pidURL, atomically: true, encoding: .utf8)
    } catch {
        fputs("Could not publish development app PID: \(error.localizedDescription)\n", stderr)
        app.terminate()
        exit(1)
    }
}
while !app.isTerminated {
    RunLoop.current.run(until: Date().addingTimeInterval(0.05))
}
