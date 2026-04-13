# TPX

TPX is a pluggable runtime orchestration engine for resolving tools into executable environments with a deterministic dependency model and lazy execution semantics.

## Current CLI

The repository now exposes a tinx-style CLI through the `tpx` binary in the `tpx-cli` crate.

Core commands currently wired:

- `tpx init`
- `tpx install <ref> [as <alias>]`
- `tpx exec <command> [args...]`
- `tpx shell`
- `tpx status`
- `tpx pack --manifest <provider-manifest>`
- `tpx release --manifest <provider-manifest> [--push <file://...|path>]`
- `tpx workspace <create|list|current|use|delete>`
- `tpx provider <add|list|remove|update>`
- `tpx use <workspace>`
- `tpx add <provider> [as <alias>]`
- `tpx remove <provider-or-alias>`
- `tpx update [provider-or-alias...]`
- `tpx list [workspace|default]`
- `tpx -- <command> [args...]`
- `tpx run <provider-or-alias> [args...]` as deprecated compatibility

`tpx exec` and `tpx shell` build workspace shell state under `.workspace/` and lazy alias shims under `.tpx/`.
`tpx pack` and `tpx release` now write local OCI image layouts with `oci-layout`, `index.json`, and content-addressed blobs.

## Build And Test

Build the CLI:

```bash
cargo build -p tpx-cli
```

Run the CLI from source:

```bash
cargo run -p tpx-cli -- --help
```

Run the CLI crate tests:

```bash
cargo test -p tpx-cli
```

Run the full workspace suite:

```bash
cargo test --workspace
```

## Quickstart

Workspace flow, close to tinx usage:

```bash
tpx init
tpx add ./bin/tool.sh as tool
tpx status
tpx exec tool --version
tpx -- tool --version
tpx shell
```

Workspace management flow:

```bash
tpx workspace create ./demo
tpx workspace use ./demo
tpx provider list workspace
tpx workspace current
```

Provider author flow with OCI image layout output:

```bash
tpx pack --manifest tpx.yaml --artifact-root dist --output oci
tpx release --manifest tpx.yaml --dist dist --output oci --push file://./published/demo
```

Consume a local OCI layout from a workspace:

```bash
tpx add ./oci as demo
tpx exec demo
```

Or run the same flows from source without installing `tpx` globally:

```bash
cargo run -p tpx-cli -- init
cargo run -p tpx-cli -- add ./bin/tool.sh as tool
cargo run -p tpx-cli -- status
cargo run -p tpx-cli -- exec tool --version
cargo run -p tpx-cli -- pack --manifest tpx.yaml --artifact-root dist --output oci
```

Local `file://` layout push is supported today. Remote OCI registry push is not wired yet.

## Workspace Layout

- a Rust workspace rooted at `tpx`
- crate boundaries for CLI, core, parser, runtime, store, shim, and workspace modules
- built-in runtime crates for `local`, `script`, and `oci`
- a dedicated test crate for workspace-level smoke coverage
- GitHub Actions CI for `cargo build` and `cargo test`

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
