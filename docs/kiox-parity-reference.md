# TPX Reference: Reimplementing kiox in Rust

## Purpose

This document translates the published kiox documentation into an implementation reference for TPX.

The target is not a generic tool orchestrator. The target is a Rust implementation of the kiox model:

- workspace-first execution
- OCI-native provider distribution
- lazy materialization of runtime artifacts
- deterministic lock and shell generation
- normal PATH-based command execution with no RPC layer

This document is intended to guide the next implementation stages, including workspace sync, provider install, runtime shell generation, pack/release, and CI deployment behavior.

## Documentation Basis

The analysis for this reference was derived from the kiox docs site, especially these pages:

- `/`
- `/getting-started/installation`
- `/getting-started/quick-start`
- `/concepts/workspace`
- `/concepts/providers`
- `/concepts/runtime-shell`
- `/concepts/caching`
- `/concepts/execution-model`
- `/cli/kiox`
- `/cli/kiox-install`
- `/cli/kiox-run`
- `/cli/kiox-workspace`
- `/cli/kiox-provider`
- `/architecture/internals`
- `/architecture/workspace-runtime`
- `/architecture/provider-execution`
- `/reference/configuration`
- `/reference/environment-variables`
- `/providers/writing-providers`
- `/examples/ci-environment`
- `/examples/multi-provider-workspace`

## Non-Negotiable Parity Rules

These are the behavioral constraints that define kiox and therefore should define TPX.

### 1. Workspace-first execution

- Every executable command must run through a workspace.
- Direct provider execution is not the primary model.
- `tpx run` should exist only as a migration aid or explicit deprecation path.
- Canonical execution is `tpx exec <command>`, `tpx shell`, or `tpx -- <command> [args...]`.

### 2. Provider is the unit of distribution

- A provider is a versioned OCI artifact.
- A provider packages one command-line tool entrypoint plus optional assets and environment metadata.
- Providers are selected into a workspace through aliases.

### 3. Alias is the unit of execution

- Users run aliases, not provider IDs.
- The alias resolves to a provider and becomes the command name exposed on `PATH`.
- The workspace owns alias mapping and lock state.

### 4. Runtime is just a shell environment

- No RPC layer.
- No custom plugin protocol.
- No service bus.
- Runtime means: resolve workspace, sync providers, materialize binaries, build env, write shims, exec commands.

### 5. Metadata and binaries are separate states

- Sync and lock generation can work from provider metadata alone.
- Binary extraction is lazy and occurs only when the runtime needs the current platform binary.
- Partial cache state is valid and must be recoverable.

### 6. Determinism is more important than convenience

- Lock files are authoritative generated state.
- Alias order must be deterministic.
- Environment merge conflicts fail early.
- Missing workspaces, missing binaries, and missing commands fail early.
- No hidden fallback to global host tools.

## Concept Mapping: kiox to TPX

| kiox concept | Meaning | TPX recommendation |
| --- | --- | --- |
| Provider | OCI-packaged tool artifact | Keep as first-class concept |
| Alias | Workspace command name mapped to provider | Keep as first-class concept |
| Workspace | Selected composition of providers | Keep as first-class concept |
| Runtime shell | Generated execution environment | Keep as first-class concept |
| kiox home | Shared cache and metadata root | Implement as `TPX_HOME` |
| `kiox.yaml` | Manifest for `Workspace` or `Provider` | Use `tpx.yaml` with same semantics |
| `kiox.lock` | Resolved provider lock file | Use `tpx.lock` |
| `.workspace/` | Generated runtime artifacts | Prefer keeping `.workspace/` for parity |

## Canonical External Model

TPX should preserve the kiox model but adopt TPX naming.

### Workspace manifest

Canonical file: `tpx.yaml`

```yaml
apiVersion: tpx.io/v1
kind: Workspace
workspace: dev
metadata:
  name: dev
providers:
  node:
    source: core/node
  kubectl:
    source: ghcr.io/acme/kubectl:v1.31.0
    plainHTTP: false
```

Required semantics:

- `providers` is an alias-to-provider mapping.
- A provider entry may be shorthand string or full object.
- `plainHTTP` is per-provider, not global.
- Workspace selection must support explicit target, upward discovery, and active workspace record.

### Provider manifest

Canonical file: `tpx.yaml`

```yaml
apiVersion: tpx.io/v1
kind: Provider
metadata:
  namespace: acme
  name: node
  version: v20.19.0
  description: Node.js runtime provider
spec:
  runtime: binary
  entrypoint: node
  platforms:
    - os: darwin
      arch: arm64
      binary: bin/darwin/arm64/node
    - os: linux
      arch: amd64
      binary: bin/linux/amd64/node
  capabilities:
    build:
      description: Compile the application
  env:
    NODE_EXTRA_CA_CERTS: ${provider_assets}/certs/root-ca.pem
  path:
    - tools/bin
  layers:
    assets:
      root: assets
      includes:
        - certs/*.pem
```

Required semantics:

- `spec.runtime` is currently only `binary` for kiox parity.
- `entrypoint` must match the executable exposed by the provider.
- `platforms` determines build and runtime selection.
- `env` and `path` support template expansion.
- `layers.assets` is optional and defines the provider assets root.

### Lock file

Canonical file: `tpx.lock`

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

Required semantics:

- The lock file is generated output.
- It records exact resolved references, versions, and store IDs.
- TPX must rewrite the lock during sync.
- Lock serialization must be deterministic.

### Global home config

Canonical file: `$TPX_HOME/config.yaml`

```yaml
aliases:
  node: core/node@v20.19.0
activeWorkspace: /abs/path/to/workspace
workspaces:
  dev: /abs/path/to/workspace
```

Required semantics:

- Track active workspace.
- Track registered workspace roots.
- Track known provider aliases for inventory and UX.
- Treat this as mutable local state, not portable project state.

## Recommended Internal Model

TPX currently models generic `Tool` objects. That is useful as an internal execution primitive, but it is not the canonical kiox model.

The canonical Rust model should center on workspace aliases and provider manifests.

```rust
pub struct WorkspaceManifest {
    pub api_version: String,
    pub workspace: String,
    pub metadata: Option<WorkspaceMetadata>,
    pub providers: BTreeMap<String, WorkspaceProviderRef>,
}

pub struct WorkspaceProviderRef {
    pub source: String,
    pub plain_http: bool,
}

pub struct ProviderManifest {
    pub api_version: String,
    pub metadata: ProviderMetadata,
    pub spec: ProviderSpec,
}

pub struct ProviderSpec {
    pub runtime: ProviderRuntime,
    pub entrypoint: String,
    pub platforms: Vec<ProviderPlatform>,
    pub capabilities: BTreeMap<String, Capability>,
    pub env: BTreeMap<String, String>,
    pub path: Vec<String>,
    pub layers: Option<ProviderLayers>,
}

pub struct WorkspaceLock {
    pub workspace: String,
    pub providers: Vec<LockedProvider>,
}
```

Recommended normalization boundary:

- parser returns typed manifests and lock documents
- workspace/core normalize them into alias-resolved runtime planning structs
- generic `Tool` can survive internally only if it represents an alias-ready executable unit, not the user-facing manifest model

## Storage and Cache Model

TPX should copy the two-root storage model.

### TPX home

Default location: `~/.tpx`

Override sources:

- `--tpx-home`
- `TPX_HOME`

Recommended layout:

```text
$TPX_HOME/
  config.yaml
  providers/
    <namespace>/
      <name>/
        <version>/
          metadata.json
  store/
    <storeID>/
      oci/
      bin/
        <os>/
          <arch>/
            <entrypoint>
      assets/
```

Required rules:

- Provider metadata is stored separately from materialized binaries.
- `storeID` must be derived from provider identity plus manifest digest.
- OCI layout must be retained to support re-materialization and hydration.
- Metadata-only installs are valid.

### Workspace runtime state

Recommended layout:

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

Required rules:

- `.workspace/` is generated state.
- `.workspace/bin` is recreated on each shell build.
- `.workspace/env` is a shell-friendly export file.
- `.workspace/path` is newline-separated for inspection and tooling.

## Workspace Resolution and Lifecycle

TPX workspace resolution must follow the same priority order as kiox.

### Resolution order

1. `--workspace` explicit target
2. upward discovery from current directory looking for `tpx.yaml`
3. active workspace stored in `$TPX_HOME/config.yaml`

### Lifecycle

1. Declare providers in `tpx.yaml`
2. Resolve sources to local OCI layout or registry refs
3. Install or refresh provider metadata
4. Write deterministic `tpx.lock`
5. Build `.workspace/` artifacts
6. Execute commands through the workspace shell

### Working directory rule

- If TPX is launched inside the workspace tree, preserve the current working directory.
- If launched from outside, execute from the workspace root.

## Runtime Shell Contract

Runtime behavior should be planned as a deterministic shell build, not as an open-ended runtime plugin system.

### Pipeline

1. Resolve workspace
2. Load manifest and lock
3. Sync providers
4. Materialize platform binary and assets lazily
5. Build env files and shims
6. Construct final `PATH`
7. Resolve requested command on `PATH`
8. `exec` the real binary

### PATH layout

Required order:

1. `.workspace/bin`
2. provider `spec.path` entries, expanded relative to provider home
3. original host `PATH`

This guarantees workspace aliases win over host binaries.

### Environment merge rules

- Merge provider env deterministically.
- Reject conflicting values for the same key.
- Reserve the `TPX_` prefix for runtime-generated variables.
- Expand provider templates during shell build, not during parse.

### Runtime-generated environment variables

TPX should mirror kiox with renamed prefixes:

- `TPX_HOME`
- `TPX_WORKSPACE_ROOT`
- `TPX_WORKSPACE_HOME`
- `TPX_WORKSPACE_ENV_FILE`
- `TPX_WORKSPACE_PATH_FILE`
- `TPX_WORKSPACE_PROVIDERS`
- `TPX_PROVIDER_<ALIAS>_REF`
- `TPX_PROVIDER_<ALIAS>_HOME`
- `TPX_PROVIDER_<ALIAS>_BINARY`

### Template variables supported in provider manifests

- `${cwd}`
- `${workspace_root}`
- `${workspace_home}`
- `${provider_alias}`
- `${provider_ref}`
- `${provider_namespace}`
- `${provider_name}`
- `${provider_version}`
- `${provider_home}`
- `${provider_root}`
- `${provider_binary}`
- `${provider_assets}`

Unknown template variables should remain unchanged to match the kiox behavior described in the docs.

## OCI Install, Materialization, and Hydration

TPX needs three distinct flows.

### Install metadata

`tpx install` should:

- accept registry refs and local OCI layouts
- validate local layout identity against the requested `<namespace>/<name>`
- write provider metadata into `TPX_HOME`
- avoid treating install as execution

### Materialize runtime artifacts

When a shell needs a provider:

1. compute expected binary path
2. check executable exists
3. extract current platform binary if missing
4. extract assets if configured
5. retry through remote hydration if metadata exists but blobs do not

### Remote hydration

If metadata and remote reference exist but runtime blobs are missing:

- rehydrate the OCI layout into local store
- retry extraction
- do not discard existing metadata state

This is essential for partial cache restores and CI resume behavior.

## CLI Parity Model

TPX should move toward kiox command semantics.

### Root commands that should exist

```text
tpx init
tpx install
tpx exec
tpx shell
tpx status
tpx pack
tpx release
tpx version
tpx workspace ...
tpx provider ...
tpx use
tpx add
tpx remove
tpx update
tpx list
tpx -- <command> [args...]
```

### `tpx run`

Recommendation:

- keep only as deprecated compatibility
- print guidance toward workspace execution
- do not make it the canonical API

### Workspace commands

Required parity:

- `workspace create`
- `workspace list [--active|--ready|--missing|--short]`
- `workspace current`
- `workspace use <workspace> [-- command...]`
- `workspace delete <workspace>`

### Provider commands

Required parity:

- `provider add <provider> [as <alias>]`
- `provider list [workspace|default]`
- `provider remove <provider-or-alias>`
- `provider update [provider-or-alias...]`

### Packaging commands

Required parity:

- `pack`: package provider manifest plus binaries into OCI layout
- `release`: build, pack, and optionally push

## CI and Deployment Guidance

TPX deployment stages should preserve the CI model used by kiox.

### Non-interactive usage

- Prefer `tpx exec` or `tpx -- ...` in CI.
- Use `tpx shell` only for local debugging.

### Deterministic CI home

- CI should set `TPX_HOME` or `--tpx-home` explicitly.
- Example cache roots:
  - `.tpx-home/providers/`
  - `.tpx-home/store/`

### Registry auth inputs

TPX should support standard env-based auth patterns:

- `TPX_REGISTRY_USERNAME` / `TPX_REGISTRY_PASSWORD`
- `ORAS_USERNAME` / `ORAS_PASSWORD`
- `GITHUB_ACTOR` / `GITHUB_TOKEN` for `ghcr.io`

### Release pipeline expectations

- build every platform listed in provider manifest
- validate declared binaries exist
- assemble OCI layout deterministically
- optionally push through OCI registry APIs

## Risks in the Current TPX Direction

The current repository is heading toward a generic orchestration engine. That is not the same thing as kiox parity.

### Risk 1: generic `Tool` model as primary public abstraction

kiox is provider-and-alias centric, not tool-graph centric.

Recommendation:

- make `Workspace` and `Provider` the canonical external manifests
- keep `Tool` only as an internal execution unit if still useful

### Risk 2: runtime plugin architecture as a public contract

kiox explicitly avoids a plugin framework.

Recommendation:

- reduce the public runtime contract to the `binary` runtime first
- keep OCI layout handling and local materialization as internal source adapters, not user-facing runtimes

### Risk 3: DAG-based dependency resolution as the main execution model

kiox docs do not describe providers depending on each other through a DAG. They describe a merged workspace shell where providers coexist on `PATH`.

Recommendation:

- do not make provider graph resolution the central workspace mechanism
- if DAG logic stays, use it only for optional future workflow planning, not for core provider resolution

### Risk 4: `.tpx/` runtime directory

kiox behavior centers around `.workspace/` as rebuildable runtime state.

Recommendation:

- prefer `.workspace/` for parity
- if `.tpx/` is kept, hide it behind a compatibility abstraction and document the divergence clearly

## Concrete Suggestions for the Existing Codebase

### Parser crate

- Add explicit parsing for `apiVersion`, `Workspace`, `Provider`, and `WorkspaceLock`.
- Add alias-to-provider source parsing with string shorthand and expanded object form.
- Add provider metadata fields: `namespace`, `version`, `description`, `entrypoint`, `platforms`, `capabilities`, `env`, `path`, `layers.assets`.
- Treat current `Tool`, `Bundle`, `Environment`, and `Secret` support as secondary extension mode, not the main parity model.

### Core crate

- Reposition the current DAG resolver as an internal planner, not the primary workspace resolver.
- Replace `run(tool, args, ctx)` as the main execution contract with workspace-oriented execution that takes a resolved workspace shell context.
- Add workspace selection, alias ordering, env conflict detection, and command lookup on generated `PATH`.

### Runtime crate

- Narrow the first production runtime to `binary`.
- Model platform selection and lazy materialization before reintroducing any generalized runtime registry.
- Generate shims that `exec` the real binary to preserve signals and process semantics.

### Store crate

- Introduce provider metadata store, OCI layout store, binary cache, and assets root.
- Derive `storeID` from provider identity plus manifest digest.
- Implement rehydration from remote refs when metadata exists but blobs do not.

### Workspace crate

- Add `tpx.lock` generation and rewrite logic.
- Add active workspace tracking and upward discovery.
- Generate `.workspace/env`, `.workspace/path`, and `.workspace/bin/<alias>` deterministically.

### CLI crate

- Add `status`, `shell`, `install`, `pack`, `release`, `workspace current`, `workspace delete`, and `provider update`.
- Add top-level shortcuts `use`, `add`, `remove`, `update`, and `list`.
- Mark `run` as deprecated once `exec` and `tpx -- ...` exist.

## Recommended Next Implementation Stages

1. Realign parser and workspace manifests around kiox parity.
2. Implement `TPX_HOME` config and workspace discovery.
3. Implement provider install and metadata store.
4. Implement `tpx.lock` and deterministic sync.
5. Implement runtime shell build with `.workspace/` outputs.
6. Implement OCI materialization and remote hydration.
7. Implement `exec`, `shell`, `status`, and `tpx -- ...`.
8. Implement `pack` and `release` for provider authors.
9. Add CI-focused cache and auth flows.

## Bottom Line

TPX should stop thinking of itself primarily as a pluggable orchestration engine and start thinking of itself as a workspace-centric OCI provider runtime.

That is the actual architectural center of kiox, and replicating that center is what will make the Rust port accurate.