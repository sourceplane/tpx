pub use anyhow::Result;
pub use tpx_core::Context;
pub use tpx_parser::Tool;

use anyhow::bail;
use std::{collections::BTreeMap, fmt, sync::Arc};

pub trait Runtime: Send + Sync {
    fn name(&self) -> &str;

    fn resolve(&self, tool: &Tool, ctx: &Context) -> Result<ResolvedTool>;

    fn is_installed(&self, resolved: &ResolvedTool, ctx: &Context) -> Result<bool>;

    fn install(&self, resolved: &ResolvedTool, ctx: &Context) -> Result<()>;

    fn execute(&self, resolved: &ResolvedTool, args: &[String], ctx: &Context) -> Result<i32>;
}

pub const BUILTIN_RUNTIMES: &[&str] = &["local", "script", "oci"];

pub type RuntimeRef = Arc<dyn Runtime>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTool {
    runtime: String,
    tool: Tool,
}

impl ResolvedTool {
    pub fn new(runtime: impl Into<String>, tool: Tool) -> Self {
        Self {
            runtime: runtime.into(),
            tool,
        }
    }

    pub fn runtime(&self) -> &str {
        &self.runtime
    }

    pub fn tool(&self) -> &Tool {
        &self.tool
    }

    pub fn ensure_runtime(&self, runtime: &str) -> Result<()> {
        if self.runtime == runtime {
            return Ok(());
        }

        bail!(
            "resolved tool belongs to runtime '{}' not '{}'",
            self.runtime,
            runtime
        )
    }
}

#[derive(Default, Clone)]
pub struct RuntimeRegistry {
    runtimes: BTreeMap<String, RuntimeRef>,
}

impl RuntimeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<R>(&mut self, runtime: R) -> Option<RuntimeRef>
    where
        R: Runtime + 'static,
    {
        self.register_ref(Arc::new(runtime))
    }

    pub fn register_ref(&mut self, runtime: RuntimeRef) -> Option<RuntimeRef> {
        let name = runtime.name().to_owned();

        self.runtimes.insert(name, runtime)
    }

    pub fn get(&self, name: &str) -> Option<RuntimeRef> {
        self.runtimes.get(name).cloned()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.runtimes.contains_key(name)
    }

    pub fn len(&self) -> usize {
        self.runtimes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.runtimes.is_empty()
    }

    pub fn names(&self) -> Vec<String> {
        self.runtimes.keys().cloned().collect()
    }
}

impl fmt::Debug for RuntimeRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeRegistry")
            .field("runtimes", &self.names())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRuntime {
        name: &'static str,
    }

    impl Runtime for TestRuntime {
        fn name(&self) -> &str {
            self.name
        }

        fn resolve(&self, tool: &Tool, _ctx: &Context) -> Result<ResolvedTool> {
            Ok(ResolvedTool::new(self.name(), tool.clone()))
        }

        fn is_installed(&self, resolved: &ResolvedTool, _ctx: &Context) -> Result<bool> {
            resolved.ensure_runtime(self.name())?;

            Ok(true)
        }

        fn install(&self, resolved: &ResolvedTool, _ctx: &Context) -> Result<()> {
            resolved.ensure_runtime(self.name())
        }

        fn execute(
            &self,
            resolved: &ResolvedTool,
            _args: &[String],
            _ctx: &Context,
        ) -> Result<i32> {
            resolved.ensure_runtime(self.name())?;

            Ok(0)
        }
    }

    #[test]
    fn exposes_runtime_names() {
        let runtime = TestRuntime { name: "test" };

        assert_eq!(runtime.name(), "test");
        assert_eq!(BUILTIN_RUNTIMES, ["local", "script", "oci"]);
    }

    #[test]
    fn registers_and_gets_runtimes() {
        let mut registry = RuntimeRegistry::new();

        assert!(registry.is_empty());
        assert!(registry.register(TestRuntime { name: "test" }).is_none());
        assert!(registry.contains("test"));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.names(), vec![String::from("test")]);
        assert_eq!(
            registry
                .get("test")
                .expect("runtime should be registered")
                .name(),
            "test"
        );
    }

    #[test]
    fn register_replaces_existing_runtime_by_name() {
        let mut registry = RuntimeRegistry::new();

        registry.register(TestRuntime { name: "test" });
        let replaced = registry.register(TestRuntime { name: "test" });

        assert!(replaced.is_some());
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.get("test").expect("runtime should exist").name(),
            "test"
        );
    }

    #[test]
    fn resolved_tool_rejects_runtime_mismatches() {
        let resolved = ResolvedTool::new("oci", Tool::default());
        let error = resolved
            .ensure_runtime("script")
            .expect_err("runtime mismatch should fail");

        assert_eq!(
            error.to_string(),
            "resolved tool belongs to runtime 'oci' not 'script'"
        );
    }
}
