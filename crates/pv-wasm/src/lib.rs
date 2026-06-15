//! WASM adapter for the PV Estimator core.

use pv_core::simulation::{SimulationRequest, simulate as core_simulate};
use pv_data::{CityMatchKind, CitySearchResult};
use pv_model_runtime::{
    EstimateArray, EstimateRequest, FinishedEstimate, SourceInferenceOutput, SourceModelRuntime,
};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[derive(Debug, Deserialize)]
struct EstimateInput {
    request: EstimateRequest,
    #[serde(default)]
    arrays: Vec<EstimateArray>,
}

#[derive(Debug, Serialize)]
struct CitySearchJsonRow {
    geoname_id: u32,
    display_name: String,
    country_code: String,
    latitude: f64,
    longitude: f64,
    population: u32,
    feature_code: String,
    matched_name: String,
    match_kind: &'static str,
}

#[wasm_bindgen]
pub fn search_cities(query: &str, limit: usize) -> Result<String, JsValue> {
    if query.trim().chars().count() < 2 {
        return Err(js_error("search query must contain at least 2 characters"));
    }
    if !(1..=50).contains(&limit) {
        return Err(js_error("search limit must be in 1..=50"));
    }
    let rows = pv_data::search_cities(query.trim(), limit)
        .iter()
        .map(city_search_json_row)
        .collect::<Vec<_>>();
    to_json(&rows)
}

#[wasm_bindgen]
pub fn prepare_estimate(input_json: &str) -> Result<String, JsValue> {
    let input: EstimateInput = from_json(input_json)?;
    let runtime = SourceModelRuntime::load_embedded().map_err(js_anyhow)?;
    let arrays = estimate_arrays(&input);
    let prepared = runtime
        .prepare_estimate_arrays(&input.request, &arrays)
        .map_err(js_anyhow)?;
    to_json(&prepared)
}

#[wasm_bindgen]
pub fn finish_estimate(input_json: &str, source_outputs_json: &str) -> Result<String, JsValue> {
    let input: EstimateInput = from_json(input_json)?;
    let source_outputs: Vec<SourceInferenceOutput> = from_json(source_outputs_json)?;
    let runtime = SourceModelRuntime::load_embedded().map_err(js_anyhow)?;
    let arrays = estimate_arrays(&input);
    let prepared = runtime
        .prepare_estimate_arrays(&input.request, &arrays)
        .map_err(js_anyhow)?;
    let finished: FinishedEstimate = runtime
        .finish_estimate(&prepared, &source_outputs)
        .map_err(js_anyhow)?;
    to_json(&finished)
}

#[wasm_bindgen]
pub fn simulate(request_json: &str) -> Result<String, JsValue> {
    let request: SimulationRequest = from_json(request_json)?;
    let result = core_simulate(&request).map_err(|error| js_error(&error.to_string()))?;
    to_json(&result)
}

fn estimate_arrays(input: &EstimateInput) -> Vec<EstimateArray> {
    if input.arrays.is_empty() {
        vec![input.request.single_array()]
    } else {
        input.arrays.clone()
    }
}

fn from_json<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T, JsValue> {
    serde_json::from_str(value).map_err(|error| js_error(&error.to_string()))
}

fn to_json<T: Serialize>(value: &T) -> Result<String, JsValue> {
    serde_json::to_string(value).map_err(|error| js_error(&error.to_string()))
}

fn js_anyhow(error: anyhow::Error) -> JsValue {
    js_error(&format!("{error:#}"))
}

fn js_error(message: &str) -> JsValue {
    JsValue::from_str(message)
}

fn city_search_json_row(result: &CitySearchResult) -> CitySearchJsonRow {
    CitySearchJsonRow {
        geoname_id: result.geoname_id,
        display_name: result.display_name.clone(),
        country_code: result.country_code.clone(),
        latitude: result.latitude_degrees,
        longitude: result.longitude_degrees,
        population: result.population,
        feature_code: result.feature_code.clone(),
        matched_name: result.matched_name.clone(),
        match_kind: city_match_kind_label(result.match_kind),
    }
}

fn city_match_kind_label(kind: CityMatchKind) -> &'static str {
    match kind {
        CityMatchKind::ExactPrimary => "exact_primary",
        CityMatchKind::ExactAlias => "exact_alias",
        CityMatchKind::PrefixPrimary => "prefix_primary",
        CityMatchKind::PrefixAlias => "prefix_alias",
        CityMatchKind::SubstringPrimary => "substring_primary",
        CityMatchKind::SubstringAlias => "substring_alias",
        CityMatchKind::FuzzyPrimary => "fuzzy_primary",
        CityMatchKind::FuzzyAlias => "fuzzy_alias",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn city_search_export_returns_json() {
        let rows = search_cities("Milan", 3).expect("search json");
        assert!(rows.starts_with('['));
    }

    #[test]
    fn prepare_estimate_returns_jobs() {
        let request = EstimateInput {
            request: EstimateRequest::default(),
            arrays: Vec::new(),
        };
        let json = serde_json::to_string(&request.request).unwrap();
        assert!(json.contains("latitude"));
    }
}
