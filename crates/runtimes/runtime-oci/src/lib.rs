use tpx_runtime::Runtime;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OciRuntime;

impl Runtime for OciRuntime {
    fn name(&self) -> &'static str {
        "oci"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_runtime_name() {
        assert_eq!(OciRuntime.name(), "oci");
    }
}
