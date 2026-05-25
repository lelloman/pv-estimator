//! Embedded locations, normalized weather data, and equipment catalogs.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_loads() {
        assert_eq!(env!("CARGO_PKG_NAME"), "pv-data");
    }
}
