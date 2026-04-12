use tpx_runtime::Runtime;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LocalRuntime;

impl Runtime for LocalRuntime {
    fn name(&self) -> &'static str {
        "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_runtime_name() {
        assert_eq!(LocalRuntime.name(), "local");
    }
}
