// Deliberately uses the old NodeSwift async bridge to reproduce NodeExecutor.enqueue's
// fatal missing-queue branch after a MainActor suspension with multiple add-on environments.
import Foundation
import NodeAPI

@MainActor func hop() async throws {
    try await Task.sleep(for: .milliseconds(1))
}

@_cdecl("napi_register_module_v1")
public func register(env: OpaquePointer, exports: OpaquePointer) -> OpaquePointer? {
    NodeModuleRegistrar(env).register {
        ["hop": try NodeFunction { () async throws -> Void in try await hop() }]
    }
}
