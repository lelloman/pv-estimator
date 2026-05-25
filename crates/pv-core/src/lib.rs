//! UI-agnostic PV system domain, validation, simulation, and reporting core.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_loads() {
        assert_eq!(env!("CARGO_PKG_NAME"), "pv-core");
    }
}
