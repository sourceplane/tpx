pub const COMPONENT_NAME: &str = "tpx-workspace";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Workspace;

impl Workspace {
    pub const fn component_name(&self) -> &'static str {
        COMPONENT_NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_component_name() {
        assert_eq!(Workspace.component_name(), "tpx-workspace");
    }
}
