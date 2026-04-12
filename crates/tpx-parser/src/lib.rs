pub const COMPONENT_NAME: &str = "tpx-parser";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Parser;

impl Parser {
    pub const fn component_name(&self) -> &'static str {
        COMPONENT_NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_component_name() {
        assert_eq!(Parser.component_name(), "tpx-parser");
    }
}
