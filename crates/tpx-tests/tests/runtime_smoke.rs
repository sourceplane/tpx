use runtime_local::LocalRuntime;
use runtime_oci::OciRuntime;
use runtime_script::ScriptRuntime;
use tpx_core::Engine;
use tpx_runtime::{BUILTIN_RUNTIMES, Runtime};

#[test]
fn stage_zero_workspace_wires_core_and_runtimes() {
    let engine = Engine::new();

    assert_eq!(engine.crate_name(), "tpx-core");
    assert_eq!(engine.stage(), "stage-0");
    assert_eq!(LocalRuntime.name(), "local");
    assert_eq!(ScriptRuntime.name(), "script");
    assert_eq!(OciRuntime.name(), "oci");
    assert_eq!(BUILTIN_RUNTIMES, ["local", "script", "oci"]);
}
