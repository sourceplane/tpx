pub const TEST_CRATE_NAME: &str = "tpx-tests";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_test_crate_name() {
        assert_eq!(TEST_CRATE_NAME, "tpx-tests");
    }
}
