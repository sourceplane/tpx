# 🚀 Tpx v2 — Full Implementation Contract (From Scratch)

## 🎯 Goal

Implement tpx as a **modular runtime orchestration engine** with:

* OCI-native distribution
* Lazy execution model
* Pluggable runtimes
* Deterministic dependency graph
* Single-file authoring + multi-kind support

---

# 🧱 MODULE ARCHITECTURE

```
tpx/
 ├── cli/            # CLI entrypoints
 ├── parser/         # YAML → AST → Internal Model
 ├── core/           # resolver + graph engine
 ├── runtime/        # runtime interfaces + registry
 ├── runtimes/       # runtime implementations
 ├── store/          # OCI-like content store
 ├── shim/           # lazy execution layer
 ├── workspace/      # workspace management
 └── config/         # global config (tpx home)
```

---

# 1️⃣ CLI MODULE

## Responsibilities

* Command parsing
* User interaction
* Delegation to core engine

## Commands (MUST IMPLEMENT)

### Workspace

```bash
tpx init
tpx workspace list
tpx workspace use <name>
```

### Providers

```bash
tpx provider add <name>@<version>
tpx provider list
tpx provider remove <name>
```

### Tools

```bash
tpx run <tool> [args...]
tpx exec <tool> [args...]   # alias
tpx tools list
tpx tools inspect <tool>
```

### Runtime

```bash
tpx runtime list
tpx runtime inspect <name>
```

### Cache

```bash
tpx cache list
tpx cache clean
```

---

## CLI → Core Binding

```ts
core.run(toolName, args, workspaceCtx)
core.installProvider(name, version)
core.listTools()
```

---

# 2️⃣ PARSER MODULE

## Responsibilities

* Parse YAML (single or multi-doc)
* Validate schema
* Normalize into internal model

---

## Input Supported

### Mode 1: Inline

```yaml
kind: Provider
spec:
  tools:
    - name: kubectl
```

### Mode 2: Multi-Kind

```yaml
---
kind: Tool
metadata:
  name: kubectl
```

---

## Output (STRICT)

```ts
type InternalModel = {
  tools: Map<string, Tool>
  assets: Map<string, Asset>
  envs: Map<string, Environment>
  providers: Map<string, Provider>
}
```

---

## Binding

```ts
parser.parse(filePath) → InternalModel
```

---

# 3️⃣ CORE MODULE (Resolver Engine)

## Responsibilities

* Graph building (DAG)
* Dependency resolution
* Runtime selection
* Execution orchestration

---

## Core API

```ts
run(toolName: string, args: string[], ctx: Context): int

resolveTool(name: string): Tool

resolveDependencies(tool: Tool): Tool[]

buildExecutionPlan(tool: Tool): ExecutionPlan
```

---

## Execution Flow (STRICT)

```
run()
  → resolveTool()
  → resolveDependencies()
  → selectRuntime()
  → runtime.resolve()
  → runtime.isInstalled()
  → runtime.install() (if needed)
  → runtime.execute()
```

---

## Dependency Rules

* DAG only (no cycles)
* Tool-level dependencies
* Provider-level dependencies
* Parallel resolution allowed

---

# 4️⃣ RUNTIME MODULE (INTERFACE)

## Runtime Interface

```ts
interface Runtime {
  name(): string

  resolve(tool: Tool, ctx: Context): ResolvedTool

  install(resolved: ResolvedTool, ctx: Context): void

  execute(resolved: ResolvedTool, args: string[], ctx: Context): int

  isInstalled(resolved: ResolvedTool, ctx: Context): boolean
}
```

---

## Runtime Registry

```ts
register(runtime: Runtime)
get(name: string): Runtime
list(): Runtime[]
```

---

## Binding

```ts
runtime = registry.get(tool.runtime.type)
```

---

# 5️⃣ RUNTIMES MODULE (PLUGINS)

## Required Built-in Runtimes

### 1. local

* Executes binary from cache
* No install step

---

### 2. script

* Executes script
* Produces tool artifact
* Uses cache key

---

### 3. oci

* Pulls from OCI
* Extracts correct platform layer
* Stores in content store

---

## Future (Design for extension)

* deno
* wasm
* container

---

# 6️⃣ STORE MODULE (OCI-LIKE)

## Responsibilities

* Content-addressable storage
* Deduplication
* Layer management

---

## Structure

```
  ~/.tpx/
  store/
    blobs/<sha256>
    index/
```

---

## API

```ts
store.put(blob) → digest
store.get(digest) → blob
store.exists(digest) → bool
```

---

## Requirements

* SHA256-based
* platform-aware
* partial fetch support

---

# 7️⃣ SHIM MODULE

## Responsibilities

* Lazy execution trigger
* Transparent user experience

---

## Behavior

When user runs:

```bash
kubectl get pods
```

Shim:

```
if not installed:
  tpx core run kubectl
else:
  execute binary
```

---

## Shim Generation

```ts
shim.create(toolName, path)
```

---

## Requirements

* Must be fast
* Must not re-install repeatedly
* Must be OS compatible

---

# 8️⃣ WORKSPACE MODULE

## Responsibilities

* Workspace lifecycle
* Tool exposure
* Environment setup

---

## Structure

```
workspace/
  tpx.yaml
  .tpx/
    bin/      # shims
    env       # exported env
```

---

## API

```ts
workspace.load(path)
workspace.installProvider()
workspace.linkTool()
```

---

---

# 🔗 MODULE BINDINGS (CRITICAL)

| From             | To                | Contract |
| ---------------- | ----------------- | -------- |
| CLI → Core       | run(), install()  |          |
| Core → Runtime   | Runtime interface |          |
| Core → Parser    | InternalModel     |          |
| Runtime → Store  | get/put blobs     |          |
| Shim → Core      | run()             |          |
| Workspace → Core | context           |          |

---

# ⚡ EXECUTION CONTRACT (END-TO-END)

## tpx run kubectl

1. CLI → core.run("kubectl")
2. core:

   * resolve tool
   * resolve dependencies
   * select runtime
3. runtime:

   * resolve()
   * isInstalled()
   * install() if needed
   * execute()
4. store used for caching
5. shim updated

---

# 🧠 CACHING RULES

* Cache key MUST be deterministic
* Based on:

  * tool
  * version
  * inputs
* No duplicate installs

---

# 🧪 ERROR HANDLING

* Missing runtime → fail fast
* Missing dependency → fail
* Cyclic graph → fail
* Script failure → propagate exit code

---

# 🔐 SECURITY (MINIMAL BASELINE)

* Script runtime must support:

  * sandboxing (future)
  * checksum validation (future)

---

# 📦 FINAL DELIVERABLES

* Fully modular codebase
* Runtime plugin system
* Lazy execution working
* OCI-like store working
* CLI fully functional
* Example provider (kubectl)
* Example runtime (script + oci)

---

# 🎯 FINAL SYSTEM DEFINITION

> tpx is a pluggable runtime orchestration engine that resolves tools into executable environments using a graph-based model and lazy execution semantics.
