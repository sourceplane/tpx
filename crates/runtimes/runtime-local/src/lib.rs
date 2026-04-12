use tpx_runtime::{Context, ResolvedTool, Result, Runtime, Tool};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LocalRuntime;

impl Runtime for LocalRuntime {
    fn name(&self) -> &str {
        "local"
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

    fn execute(&self, resolved: &ResolvedTool, _args: &[String], _ctx: &Context) -> Result<i32> {
        resolved.ensure_runtime(self.name())?;

        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_runtime_name() {
        assert_eq!(LocalRuntime.name(), "local");
    }

    #[test]
    fn resolves_tools_for_local_runtime() {
        let resolved = LocalRuntime
            .resolve(&Tool::default(), &Context::default())
            .expect("tool should resolve");

        assert_eq!(resolved.runtime(), "local");
        assert!(
            LocalRuntime
                .is_installed(&resolved, &Context::default())
                .expect("local runtime should report installation")
        );
    }
}
