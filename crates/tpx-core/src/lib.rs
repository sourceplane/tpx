pub const CRATE_NAME: &str = "tpx-core";
pub const STAGE: &str = "stage-0";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Engine;

impl Engine {
    pub const fn new() -> Self {
        Self
    }

    pub const fn crate_name(&self) -> &'static str {
        CRATE_NAME
    }

    pub const fn stage(&self) -> &'static str {
        STAGE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_scaffold_metadata() {
        let engine = Engine::new();

        assert_eq!(engine.crate_name(), "tpx-core");
        assert_eq!(engine.stage(), "stage-0");
    }
}
