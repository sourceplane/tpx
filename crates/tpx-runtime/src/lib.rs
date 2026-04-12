pub trait Runtime {
    fn name(&self) -> &'static str;
}

pub const BUILTIN_RUNTIMES: &[&str] = &["local", "script", "oci"];

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRuntime;

    impl Runtime for TestRuntime {
        fn name(&self) -> &'static str {
            "test"
        }
    }

    #[test]
    fn exposes_runtime_names() {
        let runtime = TestRuntime;

        assert_eq!(runtime.name(), "test");
        assert_eq!(BUILTIN_RUNTIMES, ["local", "script", "oci"]);
    }
}
