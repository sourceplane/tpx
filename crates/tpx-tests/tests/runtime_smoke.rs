use runtime_local::LocalRuntime;
use runtime_oci::OciRuntime;
use runtime_script::ScriptRuntime;
use tpx_core::Engine;
use tpx_runtime::{BUILTIN_RUNTIMES, Runtime};

#[test]
fn stage_two_workspace_wires_core_and_runtimes() {
    let engine = Engine::new();
    let script_runtime = ScriptRuntime::default();
    let oci_runtime = OciRuntime::default();

    assert_eq!(engine.crate_name(), "tpx-core");
    assert_eq!(engine.stage(), "stage-2");
    assert_eq!(LocalRuntime.name(), "local");
    assert_eq!(script_runtime.name(), "script");
    assert_eq!(oci_runtime.name(), "oci");
    assert_eq!(BUILTIN_RUNTIMES, ["local", "script", "oci"]);
}
