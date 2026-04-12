pub const COMPONENT_NAME: &str = "tpx-store";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ContentStore;

impl ContentStore {
    pub const fn component_name(&self) -> &'static str {
        COMPONENT_NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_component_name() {
        assert_eq!(ContentStore.component_name(), "tpx-store");
    }
}
