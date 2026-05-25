//! WASM adapter for the PV Estimator core.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_loads() {
        assert_eq!(env!("CARGO_PKG_NAME"), "pv-wasm");
    }
}
