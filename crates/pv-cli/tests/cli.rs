use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn pv() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pv"))
}

fn assert_success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "command failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn assert_failure(output: Output) -> Output {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn assert_json_number_close(actual: &Value, expected: f64) {
    let actual = actual.as_f64().expect("JSON number");
    assert!(
        (actual - expected).abs() < 1.0e-6,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn estimate_uses_embedded_defaults() {
    let output = assert_success(
        pv().args([
            "estimate",
            "--lat",
            "40.4168",
            "--lon=-3.7038",
            "--format",
            "json",
        ])
        .output()
        .expect("run pv estimate"),
    );
    let document: Value = serde_json::from_slice(&output.stdout).expect("estimate JSON");

    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["location"]["latitude"], 40.4168);
    assert_eq!(document["location"]["longitude"], -3.7038);
    assert!(
        document["ensemble_estimate"]["monthly_estimates"]
            .as_array()
            .is_some_and(|rows| rows.len() == 12)
    );
}

#[test]
fn estimate_madrid_json_matches_runtime_regression_fixture() {
    let output = assert_success(
        pv().args([
            "estimate",
            "--lat",
            "40.4168",
            "--lon=-3.7038",
            "--name",
            "Madrid",
            "--region",
            "ES",
            "--kwp",
            "4.5",
            "--loss-pct",
            "13",
            "--tilt-deg",
            "28",
            "--azimuth-deg",
            "5",
            "--storage-kwh",
            "5.5",
            "--format",
            "json",
        ])
        .output()
        .expect("run Madrid regression estimate"),
    );
    let document: Value = serde_json::from_slice(&output.stdout).expect("estimate JSON");

    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["location"]["name"], "Madrid");
    assert_eq!(document["location"]["region"], "ES");
    assert_eq!(document["system"]["storage_usable_kwh"], 5.5);
    assert_eq!(
        document["coverage"]["applicable_sources"],
        serde_json::json!(["nasa_power", "pvgis_era5", "pvgis_sarah3"])
    );
    assert_json_number_close(
        &document["ensemble_estimate"]["annual_energy"]["mean"]["watt_hours"],
        7_284_074.992219012,
    );
    assert_json_number_close(
        &document["ensemble_estimate"]["annual_energy"]["low"]["watt_hours"],
        6_971_517.213198024,
    );
    assert_json_number_close(
        &document["ensemble_estimate"]["annual_energy"]["high"]["watt_hours"],
        7_532_027.85505303,
    );
    assert_json_number_close(
        &document["ensemble_estimate"]["monthly_estimates"][0]["energy"]["mean"]["watt_hours"],
        425_479.9841724254,
    );
    assert_json_number_close(
        &document["ensemble_estimate"]["monthly_estimates"][11]["energy"]["mean"]["watt_hours"],
        387_656.9706862636,
    );
}

#[test]
fn estimate_includes_storage_metadata_when_set() {
    let output = assert_success(
        pv().args([
            "estimate",
            "--lat",
            "40.4168",
            "--lon=-3.7038",
            "--storage-kwh",
            "5.5",
            "--format",
            "json",
        ])
        .output()
        .expect("run pv estimate with storage"),
    );
    let document: Value = serde_json::from_slice(&output.stdout).expect("estimate JSON");

    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["system"]["storage_usable_kwh"], 5.5);
}

#[test]
fn estimate_omits_storage_metadata_when_unset() {
    let output = assert_success(
        pv().args([
            "estimate",
            "--lat",
            "40.4168",
            "--lon=-3.7038",
            "--format",
            "json",
        ])
        .output()
        .expect("run pv estimate without storage"),
    );
    let document: Value = serde_json::from_slice(&output.stdout).expect("estimate JSON");

    assert!(document["system"].get("storage_usable_kwh").is_none());
}

#[test]
fn estimate_rejects_invalid_storage() {
    for storage_arg in ["--storage-kwh=0", "--storage-kwh=-1", "--storage-kwh=NaN"] {
        let output = assert_failure(
            pv().args([
                "estimate",
                "--lat",
                "45.4642",
                "--lon",
                "9.1900",
                storage_arg,
            ])
            .output()
            .expect("run pv estimate with invalid storage"),
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("storage usable kWh must be positive")
                || stderr.contains("invalid value"),
            "stderr:\n{stderr}"
        );
    }
}

#[test]
fn estimate_accepts_explicit_model_dir() {
    let model_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("artifacts/source-models-768x8-int8");
    let output = assert_success(
        pv().args([
            "estimate",
            "--lat",
            "40.4168",
            "--lon=-3.7038",
            "--model-dir",
            model_dir.to_str().expect("UTF-8 model dir"),
            "--format",
            "json",
        ])
        .output()
        .expect("run pv estimate with model dir"),
    );
    let document: Value = serde_json::from_slice(&output.stdout).expect("estimate JSON");

    assert_eq!(document["schema_version"], 1);
    assert!(
        document["coverage"]["applicable_sources"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
    );
}

#[test]
fn estimate_accepts_multiple_arrays() {
    let output = assert_success(
        pv().args([
            "estimate",
            "--lat",
            "45.4642",
            "--lon",
            "9.1900",
            "--array",
            "1.5,30,0",
            "--array",
            "west roof,2.25,20,-90",
            "--format",
            "json",
        ])
        .output()
        .expect("run pv estimate with arrays"),
    );
    let document: Value = serde_json::from_slice(&output.stdout).expect("estimate JSON");

    assert_eq!(document["system"]["peak_power_kwp"], 3.75);
    assert_eq!(document["references"]["arrays"][0]["peak_power_kwp"], 1.5);
    assert!(document["references"]["arrays"][0].get("name").is_none());
    assert_eq!(document["references"]["arrays"][1]["name"], "west roof");
    assert_eq!(document["references"]["arrays"][1]["azimuth_deg"], -90.0);
}

#[test]
fn estimate_milan_multi_array_json_matches_runtime_regression_fixture() {
    let output = assert_success(
        pv().args([
            "estimate",
            "--lat",
            "45.4642",
            "--lon",
            "9.1900",
            "--array",
            "1.5,30,0",
            "--array",
            "west roof,2.25,20,-90",
            "--format",
            "json",
        ])
        .output()
        .expect("run Milan multi-array regression estimate"),
    );
    let document: Value = serde_json::from_slice(&output.stdout).expect("estimate JSON");

    assert_eq!(document["system"]["peak_power_kwp"], 3.75);
    assert_eq!(document["system"]["tilt_deg"], 24.0);
    assert_eq!(document["system"]["aspect_deg"], -54.0);
    assert_eq!(document["references"]["arrays"][1]["name"], "west roof");
    assert_json_number_close(
        &document["ensemble_estimate"]["annual_energy"]["mean"]["watt_hours"],
        4_500_427.349125953,
    );
    assert_json_number_close(
        &document["ensemble_estimate"]["annual_energy"]["low"]["watt_hours"],
        4_402_404.427571193,
    );
    assert_json_number_close(
        &document["ensemble_estimate"]["annual_energy"]["high"]["watt_hours"],
        4_584_262.65135788,
    );
}

#[test]
fn estimate_accepts_semicolon_array_list() {
    let output = assert_success(
        pv().args([
            "estimate",
            "--lat",
            "45.4642",
            "--lon",
            "9.1900",
            "--array",
            "1.5,30,0; 2.25,20,-90",
            "--format",
            "json",
        ])
        .output()
        .expect("run pv estimate with array list"),
    );
    let document: Value = serde_json::from_slice(&output.stdout).expect("estimate JSON");

    assert_eq!(document["system"]["peak_power_kwp"], 3.75);
    assert_eq!(
        document["references"]["arrays"]
            .as_array()
            .expect("arrays reference")
            .len(),
        2
    );
}

#[test]
fn estimate_rejects_malformed_arrays() {
    let output = assert_failure(
        pv().args([
            "estimate", "--lat", "45.4642", "--lon", "9.1900", "--array", "1.5,30",
        ])
        .output()
        .expect("run pv estimate with invalid array"),
    );

    assert!(
        String::from_utf8_lossy(&output.stderr).contains("array 1 must be [NAME,]KWP,TILT,AZIMUTH"),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn estimate_rejects_empty_array_list() {
    let output = assert_failure(
        pv().args([
            "estimate", "--lat", "45.4642", "--lon", "9.1900", "--array", ";",
        ])
        .output()
        .expect("run pv estimate with empty array list"),
    );

    assert!(
        String::from_utf8_lossy(&output.stderr).contains("at least one --array entry is required"),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn search_milan_json_returns_city_fields() {
    let output = assert_success(
        pv().args(["search", "Milan", "--format", "json"])
            .output()
            .expect("run pv search"),
    );
    let rows: Value = serde_json::from_slice(&output.stdout).expect("search JSON");
    let first = rows
        .as_array()
        .and_then(|rows| rows.first())
        .expect("first city");

    assert_eq!(first["display_name"], "Milan");
    assert_eq!(first["country_code"], "IT");
    assert_eq!(first["latitude"], 45.46427);
    assert_eq!(first["longitude"], 9.18951);
    assert!(first["geoname_id"].as_u64().is_some());
    assert!(
        first["population"]
            .as_u64()
            .is_some_and(|population| population > 0)
    );
    assert_eq!(first["feature_code"], "PPLA");
    assert_eq!(first["matched_name"], "Milan");
    assert_eq!(first["match_kind"], "exact_primary");
}

#[test]
fn search_rejects_too_short_queries() {
    let output = assert_failure(pv().args(["search", "M"]).output().expect("run pv search"));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("at least 2 characters"),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn search_rejects_invalid_limits() {
    for limit in ["0", "51"] {
        let output = assert_failure(
            pv().args(["search", "Milan", "--limit", limit])
                .output()
                .expect("run pv search"),
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("limit must be in 1..=50"),
            "stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
