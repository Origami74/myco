use serde::Deserialize;

/// Actions Kotlin dispatches into the reducer. Serialized internally-tagged:
/// `{"type": "snake_case", ...camelCaseFields}` — matching the FFI contract in
/// `docs/reference/ffi-surface.md`. P1 adds the node lifecycle and the BLE
/// master switch; site, peer, and settings actions arrive in later phases.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum NativeAppAction {
    /// Pure read; does not bump `rev`.
    GetState,
    /// Advance time-based work; bumps `rev`. (== `refresh()`.)
    Tick,
    /// Start the embedded FIPS node (spawns its transport loops).
    StartNode,
    /// Stop the embedded FIPS node.
    StopNode,
    /// Master switch for the BLE L2CAP transport. On Android this gates whether
    /// the node brings up its BLE backend; the actual radio lives in the
    /// foreground service (P1 M4).
    SetBleEnabled { enabled: bool },
}
