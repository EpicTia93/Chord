import NodeAPI

@_cdecl("napi_register_module_v1")
public func register(env: OpaquePointer, exports: OpaquePointer) -> OpaquePointer? {
    NodeModuleRegistrar(env).register { ["noop": try NodeFunction { () -> Void in }] }
}
