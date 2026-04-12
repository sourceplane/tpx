pub const COMPONENT_NAME: &str = "tpx-shim";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ShimManager;

impl ShimManager {
    pub const fn component_name(&self) -> &'static str {
        COMPONENT_NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_component_name() {
        assert_eq!(ShimManager.component_name(), "tpx-shim");
    }
}
