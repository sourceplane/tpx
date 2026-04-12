# TPX v2 - Tinx Parity Contract

## Goal

Implement TPX as a Rust reimplementation of tinx.

The system must be:

- workspace-first
- OCI-native
- lazy in runtime materialization
- deterministic in locking and shell construction
- simple in execution: binaries on `PATH`, no RPC, no plugin protocol

This contract intentionally moves TPX away from a generic orchestration engine and toward a faithful tinx-style provider runtime.

## Design Principles

### Workspace-first execution

- All execution flows through a workspace.
- Canonical execution is `tpx exec`, `tpx shell`, or `tpx -- <command>`.
- `tpx run` is deprecated compatibility only.

### OCI-native provider model

- Providers are OCI artifacts.
- Local OCI layouts and remote registries are both first-class sources.
- Provider metadata, OCI content, and materialized binaries are separate states.

### Deterministic runtime shell

- Runtime output is generated into `.workspace/`.
- Alias ordering is deterministic.
- Environment conflicts fail fast.
- Workspace aliases override host binaries through `PATH` ordering.

### Lazy materialization

- Metadata install must work without extracting binaries.
- Binary extraction happens only when required by runtime execution.
- Missing blobs may be restored by remote hydration if metadata is already available.

## Canonical Concepts

### Workspace

- The unit of execution.
- Defines aliases, provider sources, and the desired tool environment.

### Provider

- The unit of distribution.
- A versioned OCI artifact that packages one binary entrypoint plus optional assets and metadata.

### Alias

- The command name a workspace exposes on `PATH`.
- Aliases map workspace intent to provider sources.

### Runtime shell

- The generated shell environment that resolves aliases, builds `PATH`, exports env, and launches commands.

### TPX home

- Shared global cache and state root.
- Equivalent of tinx home.

## Canonical Files and Directories

### Workspace root

```text
<workspace>/
  tpx.yaml
  tpx.lock
  .workspace/
    env
    path
    bin/
      <alias>
```

### TPX home

```text
$TPX_HOME/
  config.yaml
  providers/
    <namespace>/<name>/<version>/
  store/
    <storeID>/
      oci/
      bin/<os>/<arch>/<entrypoint>
      assets/
```

### Path rules

- Default home is `~/.tpx`.
- `--tpx-home` overrides `TPX_HOME`.
- `.workspace/` is generated output, not source of truth.

## Module Architecture

```text
tpx/
  crates/
    tpx-cli          # root CLI, workspace targeting, command passthrough
    tpx-parser       # manifests, lock file, config parsing and validation
    tpx-core         # workspace sync and shell planning
    tpx-runtime      # binary runtime shell execution
    tpx-store        # provider metadata store, OCI layout store, materialization
    tpx-shim         # shim generation and exec wrappers
    tpx-workspace    # workspace discovery, active workspace, runtime state build
    runtimes/        # internal execution/materialization drivers only if needed
```

Important note:

- `tpx-runtime` is not a public plugin framework in the tinx sense.
- The primary public provider runtime is `binary`.
- Any extra runtimes are internal implementation details until parity is complete.

## CLI Contract

### Root commands required for parity

```bash
tpx init
tpx install
tpx exec <command> [args...]
tpx shell
tpx status
tpx pack
tpx release
tpx version
tpx workspace <subcommand>
tpx provider <subcommand>
tpx use <workspace>
tpx add <provider> [as <alias>]
tpx remove <provider-or-alias>
tpx update [provider-or-alias...]
tpx list
tpx -- <command> [args...]
```

### `tpx run`

```bash
tpx run <provider-or-alias> [args...]
```

Required behavior:

- Keep only as deprecated compatibility.
- Return guidance toward `tpx exec` or `tpx -- ...`.
- Do not make it the main execution model.

### Workspace commands

```bash
tpx workspace create [path|manifest]
tpx workspace list [--active|--ready|--missing|--short]
tpx workspace current
tpx workspace use <workspace> [-- command...]
tpx workspace delete <workspace>
```

### Provider commands

```bash
tpx provider add <provider> [as <alias>]
tpx provider list [workspace|default]
tpx provider remove <provider-or-alias>
tpx provider update [provider-or-alias...]
```

### Install and packaging commands

```bash
tpx install <ref> [as <alias>]
tpx pack --manifest tpx.yaml
tpx release --manifest tpx.yaml [--push <ref>]
```

### Global flags

```bash
--tpx-home <path>
--workspace, -w <workspace>
--version
```

## Parser Module

### Responsibilities

- Parse typed YAML documents.
- Validate workspace, provider, and lock schemas.
- Normalize alias and provider references.
- Preserve deterministic ordering where possible.

### Canonical documents

#### Workspace manifest

```yaml
apiVersion: tpx.io/v1
kind: Workspace
workspace: dev
providers:
  node:
    source: core/node
  kubectl:
    source: ghcr.io/acme/kubectl:v1.31.0
    plainHTTP: false
```

#### Provider manifest

```yaml
apiVersion: tpx.io/v1
kind: Provider
metadata:
  namespace: acme
  name: node
  version: v20.19.0
spec:
  runtime: binary
  entrypoint: node
  platforms:
    - os: linux
      arch: amd64
      binary: bin/linux/amd64/node
  capabilities:
    build:
      description: Compile the application
  env:
    WORKSPACE_ROOT: ${workspace_root}
  path:
    - tools/bin
  layers:
    assets:
      root: assets
```

#### Lock file

```yaml
apiVersion: tpx.io/v1
kind: WorkspaceLock
workspace: dev
providers:
  - alias: node
    provider: core/node
    source: core/node
    version: v20.19.0
    resolved: ghcr.io/acme/node-provider@sha256:...
    store: 4f3f...
```

### Internal model guidance

TPX should not treat generic `Tool` objects as the primary external model.

Preferred internal model:

```rust
pub enum ParsedDocument {
    Workspace(WorkspaceManifest),
    Provider(ProviderManifest),
    WorkspaceLock(WorkspaceLock),
}
```

If a generic `Tool` abstraction remains, it must represent a workspace alias ready for execution, not the user-facing manifest model.

### Current parser extension mode

Multi-kind and inline authoring can remain as an extension, but parity work must prioritize:

- `Workspace`
- `Provider`
- `WorkspaceLock`

## Workspace Module

### Responsibilities

- Resolve which workspace is active.
- Load and normalize manifest plus lock.
- Register and unregister known workspaces.
- Build `.workspace/` runtime state.

### Workspace resolution order

1. explicit `--workspace`
2. upward discovery from current working directory
3. active workspace from `$TPX_HOME/config.yaml`

### Runtime build outputs

- `.workspace/env`
- `.workspace/path`
- `.workspace/bin/<alias>`

### Required behaviors

- recreate `.workspace/bin` on each build
- sort aliases before writing shell artifacts
- preserve cwd if invocation happens inside the workspace tree
- otherwise execute from the workspace root
- mark stale registered workspaces as missing when paths disappear

## Core Module

### Responsibilities

- orchestrate workspace sync
- resolve provider aliases to exact locked refs
- detect environment conflicts
- plan shell build and command execution
- expose deterministic execution plan APIs

### Core API direction

```rust
pub fn sync_workspace(ctx: &WorkspaceContext) -> Result<WorkspaceLock>
pub fn build_shell(ctx: &WorkspaceContext) -> Result<ShellPlan>
pub fn exec(command: &str, args: &[String], ctx: &WorkspaceContext) -> Result<i32>
```

### Important constraint

The core module is not primarily a provider DAG resolver.

- Provider dependencies are not the main tinx abstraction.
- A graph planner may exist for future workflow features.
- It must not replace workspace alias resolution, lock generation, or shell planning.

### Execution plan

```rust
pub struct ShellPlan {
    pub workspace_root: PathBuf,
    pub env_file: PathBuf,
    pub path_file: PathBuf,
    pub aliases: Vec<AliasPlan>,
    pub env: BTreeMap<String, String>,
    pub path_entries: Vec<PathBuf>,
}
```

## Runtime Module

### Responsibilities

- materialize current platform binary when missing
- assemble environment and `PATH`
- resolve alias from generated `PATH`
- `exec` the final child process

### Runtime model

Public provider runtime support for parity:

- `binary`

Internal source/materialization drivers may exist for:

- local OCI layout
- remote OCI registry

But these are not a public plugin framework.

### Runtime-generated environment variables

```text
TPX_HOME
TPX_WORKSPACE_ROOT
TPX_WORKSPACE_HOME
TPX_WORKSPACE_ENV_FILE
TPX_WORKSPACE_PATH_FILE
TPX_WORKSPACE_PROVIDERS
TPX_PROVIDER_<ALIAS>_REF
TPX_PROVIDER_<ALIAS>_HOME
TPX_PROVIDER_<ALIAS>_BINARY
```

### Template variables supported in provider manifests

```text
${cwd}
${workspace_root}
${workspace_home}
${provider_alias}
${provider_ref}
${provider_namespace}
${provider_name}
${provider_version}
${provider_home}
${provider_root}
${provider_binary}
${provider_assets}
```

Unknown template variables should remain unchanged.

## Store Module

### Responsibilities

- store provider metadata
- store OCI layouts
- materialize binaries and assets lazily
- support remote hydration for partial cache states

### Required layout

```text
$TPX_HOME/providers/<namespace>/<name>/<version>/
$TPX_HOME/store/<storeID>/oci/
$TPX_HOME/store/<storeID>/bin/<os>/<arch>/<entrypoint>
$TPX_HOME/store/<storeID>/assets/
```

### Required rules

- `storeID` must be deterministic from provider identity plus manifest digest
- local OCI install must validate requested identity against actual layout content
- metadata-only install is valid
- remote hydration must recover missing blobs without discarding metadata

## Shim Module

### Responsibilities

- create workspace alias shims under `.workspace/bin`
- forward execution to the materialized binary
- preserve exit code, signals, and process behavior

### Requirement

Shims must `exec` the real binary rather than spawn nested shells where possible.

## Packaging and Release Module

### Responsibilities

- build binaries for each declared platform
- validate declared binary paths exist
- assemble OCI image layout deterministically
- optionally push provider artifact to registry

### Required commands

```bash
tpx pack --manifest tpx.yaml
tpx release --manifest tpx.yaml [--push <ref>]
```

### Packaging pipeline

1. build every declared platform
2. create provider metadata config
3. add binary and asset layers
4. write OCI image layout
5. optionally push through OCI registry APIs

## Execution Contract

## `tpx -- node build`

1. CLI resolves workspace target
2. workspace module loads `tpx.yaml` and `tpx.lock`
3. core syncs provider metadata and rewrites lock if needed
4. store ensures OCI layout is available
5. runtime materializes the current platform binary lazily
6. workspace module writes `.workspace/env`, `.workspace/path`, and `.workspace/bin/node`
7. runtime prepends `.workspace/bin` and provider path entries to `PATH`
8. runtime resolves `node` from the generated `PATH`
9. runtime `exec`s the real provider binary with merged env

## Caching Rules

- metadata cache and binary cache are distinct
- no eager binary extraction during sync-only flows
- identical provider ref plus manifest digest must reuse store state
- local OCI layouts are reused as local sources and are not rehydrated from remote

## Error Handling

- no workspace selected and none discoverable -> fail
- selected workspace path missing -> fail
- provider source mismatch with local OCI layout -> fail
- requested platform binary missing -> fail
- provider env conflict -> fail
- requested command absent from constructed `PATH` -> fail
- unresolved remote hydration -> fail

## Security Baseline

- default to HTTPS registry access
- allow plain HTTP only per provider source
- reserve `TPX_` prefix for runtime-generated variables
- verify provider identity when installing from local OCI layouts
- plan checksum and signature verification as future hardening stages

## CI and Deployment Contract

- support explicit `TPX_HOME` for deterministic CI caches
- cache `providers/` and `store/` between CI jobs
- support env-based registry authentication
- prefer `tpx exec` and `tpx -- ...` in CI
- use `tpx shell` only for interactive local workflows

## Required Changes to the Current Code Direction

### 1. Parser

- Add canonical `Workspace`, `Provider`, and `WorkspaceLock` models.
- Support shorthand and expanded provider refs.
- Treat current generic `Tool` parsing as secondary.

### 2. Core

- Do not let the current DAG resolver become the main workspace engine.
- Refocus core on workspace sync, lock generation, shell build, env conflict detection, and command lookup.

### 3. Runtime

- Narrow the public runtime contract to `binary` first.
- Reclassify local/OCI behaviors as source/materialization paths rather than end-user runtime types.

### 4. Workspace state

- Prefer `.workspace/` over `.tpx/` for runtime artifacts.
- Add active workspace tracking and missing-workspace handling.

### 5. CLI

- Add `status`, `shell`, `install`, `pack`, `release`, `workspace current`, `workspace delete`, and `provider update`.
- Deprecate `run` once `exec` and `tpx -- ...` are stable.

## Final System Definition

> TPX is a workspace-centric OCI provider runtime that resolves aliases into deterministic shell environments, materializes binaries lazily, and executes commands through normal PATH-based process execution.
