use tpx_runtime::Runtime;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScriptRuntime;

impl Runtime for ScriptRuntime {
    fn name(&self) -> &'static str {
        "script"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_runtime_name() {
        assert_eq!(ScriptRuntime.name(), "script");
    }
}
