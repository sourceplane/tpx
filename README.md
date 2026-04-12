# TPX

TPX is a pluggable runtime orchestration engine for resolving tools into executable environments with a deterministic dependency model and lazy execution semantics.

## Stage 0

This repository currently contains the project scaffold for the first implementation stage:

- a Rust workspace rooted at `tpx`
- crate boundaries for CLI, core, parser, runtime, store, shim, and workspace modules
- built-in runtime crates for `local`, `script`, and `oci`
- a dedicated test crate for workspace-level smoke coverage
- GitHub Actions CI for `cargo build` and `cargo test`

## Workspace Layout

```text
crates/
├── tpx-cli
├── tpx-core
├── tpx-parser
├── tpx-runtime
├── tpx-store
├── tpx-shim
├── tpx-workspace
├── runtimes/
│   ├── runtime-local
│   ├── runtime-script
│   └── runtime-oci
└── tpx-tests
```

## Architecture Direction

- `tpx-cli` owns command entrypoints and delegates into the engine.
- `tpx-core` will host dependency resolution and orchestration.
- `tpx-parser` will normalize YAML contract files into an internal model.
- `tpx-runtime` defines runtime interfaces and shared runtime types.
- `tpx-store` will back the OCI-like content store.
- `tpx-shim` will handle lazy execution entrypoints.
- `tpx-workspace` will manage workspace state and tool exposure.
- `runtime-*` crates provide built-in runtime implementations.
