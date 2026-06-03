//! Embedded locations, normalized weather data, and equipment catalogs.

use std::collections::HashMap;
use std::sync::OnceLock;

use pv_core::ids::{LocationId, WeatherSourceId};
use pv_core::source_model::{
    ClimateNormalTarget, SourceModelCoverage, SourceModelMetadata, SourceModelRegistry,
};
use pv_core::units::{Angle, Length, Power, Temperature};
use pv_core::weather::{
    HourlyWeatherRecord, Location, LocationCatalog, Speed, WeatherDataError, WeatherDataset,
    WeatherProvider, WeatherQualityFlag, WeatherRepository, WeatherSourceMetadata,
};

const PVGIS_TMY_DOC_URL: &str = "https://joint-research-centre.ec.europa.eu/photovoltaic-geographical-information-system-pvgis/using-pvgis-5/pvgis-5-tools/pvgis-typical-meteorological-year-tmy-generator_en";
const NASA_POWER_HOURLY_DOC_URL: &str =
    "https://power.larc.nasa.gov/docs/services/api/temporal/hourly/";
const SOURCE_MODEL_FAMILY: &str = "monthly-hourly climate-normal residual MLP";
const SOURCE_MODEL_INPUT_FEATURES: u16 = 66;
const SOURCE_MODEL_PARAMETERS: u64 = 9_522_442;

#[derive(Debug, Clone)]
pub struct EmbeddedData {
    locations: Vec<Location>,
    weather_datasets: Vec<WeatherDataset>,
}

impl EmbeddedData {
    pub fn new_fixture() -> Self {
        let pvgis = weather_source(
            "pvgis-tmy",
            "PVGIS TMY fixture",
            WeatherProvider::Pvgis,
            PVGIS_TMY_DOC_URL,
        );
        let nasa = weather_source(
            "nasa-power",
            "NASA POWER fixture",
            WeatherProvider::NasaPower,
            NASA_POWER_HOURLY_DOC_URL,
        );
        let sources = vec![
            pvgis.weather_source_id.clone(),
            nasa.weather_source_id.clone(),
        ];
        let rome_id = LocationId::new("it-rome").expect("valid fixture id");

        Self {
            locations: vec![
                ItalianCapitalFixture {
                    location_id: "it-rome",
                    display_name: "Rome",
                    region: "Lazio",
                    province: "RM",
                    latitude_degrees: 41.9028,
                    longitude_degrees: 12.4964,
                    elevation_meters: Some(21.0),
                    available_weather_sources: sources.clone(),
                }
                .into_location(),
                ItalianCapitalFixture {
                    location_id: "it-milan",
                    display_name: "Milan",
                    region: "Lombardy",
                    province: "MI",
                    latitude_degrees: 45.4642,
                    longitude_degrees: 9.19,
                    elevation_meters: Some(122.0),
                    available_weather_sources: sources.clone(),
                }
                .into_location(),
                ItalianCapitalFixture {
                    location_id: "it-palermo",
                    display_name: "Palermo",
                    region: "Sicily",
                    province: "PA",
                    latitude_degrees: 38.1157,
                    longitude_degrees: 13.3615,
                    elevation_meters: Some(14.0),
                    available_weather_sources: sources,
                }
                .into_location(),
            ],
            weather_datasets: vec![
                WeatherDataset {
                    location_id: rome_id.clone(),
                    source: pvgis,
                    records: vec![
                        hourly_record(
                            0,
                            0.0,
                            Some(0.0),
                            Some(0.0),
                            7.0,
                            Some(2.1),
                            vec![WeatherQualityFlag::TypicalMeteorologicalYear],
                        ),
                        hourly_record(
                            12,
                            520.0,
                            Some(680.0),
                            Some(120.0),
                            13.0,
                            Some(3.5),
                            vec![WeatherQualityFlag::TypicalMeteorologicalYear],
                        ),
                    ],
                },
                WeatherDataset {
                    location_id: rome_id,
                    source: nasa,
                    records: vec![
                        hourly_record(
                            0,
                            0.0,
                            None,
                            None,
                            8.0,
                            None,
                            vec![
                                WeatherQualityFlag::Climatology,
                                WeatherQualityFlag::MissingDirectNormalIrradiance,
                                WeatherQualityFlag::MissingDiffuseHorizontalIrradiance,
                                WeatherQualityFlag::MissingWindSpeed,
                            ],
                        ),
                        hourly_record(
                            12,
                            500.0,
                            None,
                            None,
                            14.0,
                            None,
                            vec![
                                WeatherQualityFlag::Climatology,
                                WeatherQualityFlag::MissingDirectNormalIrradiance,
                                WeatherQualityFlag::MissingDiffuseHorizontalIrradiance,
                                WeatherQualityFlag::MissingWindSpeed,
                            ],
                        ),
                    ],
                },
            ],
        }
    }
}

impl Default for EmbeddedData {
    fn default() -> Self {
        Self::new_fixture()
    }
}

impl LocationCatalog for EmbeddedData {
    fn locations(&self) -> &[Location] {
        &self.locations
    }
}

impl WeatherRepository for EmbeddedData {
    fn get_weather_dataset(
        &self,
        location_id: &LocationId,
        weather_source_id: &WeatherSourceId,
    ) -> Result<&WeatherDataset, WeatherDataError> {
        if self.get_location(location_id).is_none() {
            return Err(WeatherDataError::UnknownLocation(location_id.clone()));
        }

        self.weather_datasets
            .iter()
            .find(|dataset| {
                &dataset.location_id == location_id
                    && &dataset.source.weather_source_id == weather_source_id
            })
            .ok_or_else(|| WeatherDataError::UnknownWeatherSource {
                location_id: location_id.clone(),
                weather_source_id: weather_source_id.clone(),
            })
    }
}

pub fn embedded_data() -> EmbeddedData {
    EmbeddedData::default()
}

pub fn locations() -> Vec<Location> {
    embedded_data().locations
}

pub fn weather_dataset(
    location_id: &LocationId,
    weather_source_id: &WeatherSourceId,
) -> Result<WeatherDataset, WeatherDataError> {
    embedded_data()
        .get_weather_dataset(location_id, weather_source_id)
        .cloned()
}

const EMBEDDED_CITY_CATALOG_ZSTD: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/geonames_city_catalog.bin.zst"));
const CITY_CATALOG_MAGIC: &[u8] = b"PVCITYCAT1\n";
static CITY_CATALOG: OnceLock<CityCatalog> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct CityCatalogMetadata {
    pub variant: String,
    pub city_count: usize,
    pub compressed_bytes: usize,
    pub uncompressed_bytes: usize,
    pub alias_cap: u16,
}

#[derive(Debug, Clone)]
pub struct CitySearchResult {
    pub geoname_id: u32,
    pub display_name: String,
    pub country_code: String,
    pub latitude_degrees: f64,
    pub longitude_degrees: f64,
    pub population: u32,
    pub feature_code: String,
    pub matched_name: String,
    pub match_kind: CityMatchKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CityMatchKind {
    ExactPrimary,
    ExactAlias,
    PrefixPrimary,
    PrefixAlias,
    SubstringPrimary,
    SubstringAlias,
    FuzzyPrimary,
    FuzzyAlias,
}

#[derive(Debug)]
struct CityCatalog {
    metadata: CityCatalogMetadata,
    cities: Vec<CityRecord>,
    names: Vec<CitySearchName>,
    prefix_index: HashMap<String, Vec<usize>>,
}

#[derive(Debug)]
struct CityRecord {
    geoname_id: u32,
    name: String,
    latitude_degrees: f64,
    longitude_degrees: f64,
    country_code: String,
    population: u32,
    feature_code: String,
}

#[derive(Debug)]
struct CitySearchName {
    city_index: usize,
    name: String,
    normalized_name: String,
    is_alias: bool,
}

#[derive(Debug, Clone)]
struct RankedCityMatch {
    city_index: usize,
    result: CitySearchResult,
    rank_class: u8,
    edit_distance: usize,
}

pub fn city_catalog_metadata() -> &'static CityCatalogMetadata {
    &city_catalog().metadata
}

pub fn search_cities(query: &str, limit: usize) -> Vec<CitySearchResult> {
    if limit == 0 {
        return Vec::new();
    }
    let normalized_query = normalize_city_search_text(query);
    if normalized_query.is_empty() {
        return Vec::new();
    }

    let catalog = city_catalog();
    let mut matches = collect_city_matches(catalog, &normalized_query, limit);
    matches.sort_by(compare_city_matches);
    matches.into_iter().map(|ranked| ranked.result).collect()
}

fn city_catalog() -> &'static CityCatalog {
    CITY_CATALOG.get_or_init(|| {
        let decoded = zstd::stream::decode_all(EMBEDDED_CITY_CATALOG_ZSTD)
            .expect("embedded GeoNames city catalog decompresses");
        parse_city_catalog(&decoded).expect("embedded GeoNames city catalog parses")
    })
}

fn parse_city_catalog(bytes: &[u8]) -> Result<CityCatalog, String> {
    let mut reader = CatalogReader::new(bytes);
    reader.expect_bytes(CITY_CATALOG_MAGIC)?;
    let variant = reader.read_null_terminated_string()?;
    let include_ascii_name = reader.read_u8()? != 0;
    let include_rank_fields = reader.read_u8()? != 0;
    let alias_cap = reader.read_u16()?;
    let city_count = reader.read_u32()? as usize;
    if !include_ascii_name || !include_rank_fields {
        return Err("embedded city catalog must include ASCII and ranking fields".to_string());
    }

    let mut cities = Vec::with_capacity(city_count);
    let mut names = Vec::with_capacity(city_count * 4);
    for _ in 0..city_count {
        let geoname_id = reader.read_u32()?;
        let name = reader.read_string()?;
        let ascii_name = reader.read_string()?;
        let latitude_degrees = f64::from(reader.read_i32()?) / 1_000_000.0;
        let longitude_degrees = f64::from(reader.read_i32()?) / 1_000_000.0;
        let country_code = reader.read_string()?;
        let population = reader.read_u32()?;
        let feature_code = reader.read_string()?;
        let alias_count = reader.read_u16()? as usize;
        let mut aliases = Vec::with_capacity(alias_count);
        for _ in 0..alias_count {
            aliases.push(reader.read_string()?);
        }
        let city_index = cities.len();
        push_city_search_name(&mut names, city_index, &name, false);
        if ascii_name != name {
            push_city_search_name(&mut names, city_index, &ascii_name, false);
        }
        for alias in aliases {
            push_city_search_name(&mut names, city_index, &alias, true);
        }
        cities.push(CityRecord {
            geoname_id,
            name,
            latitude_degrees,
            longitude_degrees,
            country_code,
            population,
            feature_code,
        });
    }

    Ok(CityCatalog {
        metadata: CityCatalogMetadata {
            variant,
            city_count,
            compressed_bytes: EMBEDDED_CITY_CATALOG_ZSTD.len(),
            uncompressed_bytes: bytes.len(),
            alias_cap,
        },
        prefix_index: build_city_prefix_index(&names),
        cities,
        names,
    })
}

fn build_city_prefix_index(names: &[CitySearchName]) -> HashMap<String, Vec<usize>> {
    let mut index: HashMap<String, Vec<usize>> = HashMap::new();
    for (name_index, name) in names.iter().enumerate() {
        if let Some(key) = search_prefix_key(&name.normalized_name) {
            index.entry(key).or_default().push(name_index);
        }
    }
    index
}

fn push_city_search_name(
    names: &mut Vec<CitySearchName>,
    city_index: usize,
    name: &str,
    is_alias: bool,
) {
    let normalized_name = normalize_city_search_text(name);
    if !normalized_name.is_empty() {
        names.push(CitySearchName {
            city_index,
            name: name.to_string(),
            normalized_name,
            is_alias,
        });
    }
}

fn collect_city_matches(
    catalog: &CityCatalog,
    normalized_query: &str,
    limit: usize,
) -> Vec<RankedCityMatch> {
    let Some(prefix_key) = search_prefix_key(normalized_query) else {
        return Vec::new();
    };
    let Some(name_indexes) = catalog.prefix_index.get(&prefix_key) else {
        return Vec::new();
    };

    let mut best_by_city: HashMap<usize, RankedCityMatch> = HashMap::new();
    for name_index in name_indexes {
        let search_name = &catalog.names[*name_index];
        let Some(candidate) = city_name_match(search_name, normalized_query, false) else {
            continue;
        };
        let ranked = ranked_city_match(catalog, search_name, candidate);
        keep_best_city_match(
            best_by_city
                .entry(search_name.city_index)
                .or_insert_with(|| ranked.clone()),
            ranked,
        );
    }

    let mut matches = best_by_city.into_values().collect::<Vec<_>>();
    matches.sort_by(compare_city_matches);
    let should_try_fuzzy = normalized_query.chars().count() >= 4
        && matches
            .first()
            .is_none_or(|best| best.rank_class > 2 || best.result.population < 100_000);
    if should_try_fuzzy {
        for name_index in name_indexes {
            let search_name = &catalog.names[*name_index];
            if search_name.is_alias || best_by_city_has_city(&matches, search_name.city_index) {
                continue;
            }
            let Some(candidate) = city_name_match(search_name, normalized_query, true) else {
                continue;
            };
            matches.push(ranked_city_match(catalog, search_name, candidate));
        }
    }
    matches.sort_by(compare_city_matches);
    matches.dedup_by_key(|ranked| ranked.result.geoname_id);
    matches.truncate(limit);
    matches
}

fn best_by_city_has_city(matches: &[RankedCityMatch], city_index: usize) -> bool {
    matches
        .iter()
        .any(|matched| matched.city_index == city_index)
}

fn keep_best_city_match(slot: &mut RankedCityMatch, candidate: RankedCityMatch) {
    if compare_city_matches(&candidate, slot).is_lt() {
        *slot = candidate;
    }
}

fn ranked_city_match(
    catalog: &CityCatalog,
    search_name: &CitySearchName,
    matched: CandidateCityMatch,
) -> RankedCityMatch {
    let city = &catalog.cities[search_name.city_index];
    RankedCityMatch {
        city_index: search_name.city_index,
        result: CitySearchResult {
            geoname_id: city.geoname_id,
            display_name: city.name.clone(),
            country_code: city.country_code.clone(),
            latitude_degrees: city.latitude_degrees,
            longitude_degrees: city.longitude_degrees,
            population: city.population,
            feature_code: city.feature_code.clone(),
            matched_name: search_name.name.clone(),
            match_kind: matched.kind,
        },
        rank_class: matched.rank_class,
        edit_distance: matched.edit_distance,
    }
}

fn compare_city_matches(left: &RankedCityMatch, right: &RankedCityMatch) -> std::cmp::Ordering {
    left.rank_class
        .cmp(&right.rank_class)
        .then_with(|| right.result.population.cmp(&left.result.population))
        .then_with(|| left.edit_distance.cmp(&right.edit_distance))
        .then_with(|| {
            left.result
                .display_name
                .len()
                .cmp(&right.result.display_name.len())
        })
        .then_with(|| left.result.display_name.cmp(&right.result.display_name))
        .then_with(|| left.result.country_code.cmp(&right.result.country_code))
}

struct CandidateCityMatch {
    kind: CityMatchKind,
    rank_class: u8,
    edit_distance: usize,
}

fn city_name_match(
    search_name: &CitySearchName,
    normalized_query: &str,
    allow_fuzzy: bool,
) -> Option<CandidateCityMatch> {
    let normalized_name = search_name.normalized_name.as_str();
    let (kind, rank_class, edit_distance) = if normalized_name == normalized_query {
        if search_name.is_alias {
            (CityMatchKind::ExactAlias, 0, 0)
        } else {
            (CityMatchKind::ExactPrimary, 0, 0)
        }
    } else if normalized_name.starts_with(normalized_query) {
        if search_name.is_alias {
            (CityMatchKind::PrefixAlias, 3, 0)
        } else {
            (CityMatchKind::PrefixPrimary, 2, 0)
        }
    } else if normalized_name.contains(normalized_query) {
        if search_name.is_alias {
            (CityMatchKind::SubstringAlias, 5, 0)
        } else {
            (CityMatchKind::SubstringPrimary, 4, 0)
        }
    } else if allow_fuzzy {
        let distance = levenshtein_bounded(
            normalized_name,
            normalized_query,
            fuzzy_distance_limit(normalized_query),
        )?;
        if search_name.is_alias {
            (CityMatchKind::FuzzyAlias, 7, distance)
        } else {
            (CityMatchKind::FuzzyPrimary, 2, distance)
        }
    } else {
        return None;
    };

    Some(CandidateCityMatch {
        kind,
        rank_class,
        edit_distance,
    })
}

fn search_prefix_key(value: &str) -> Option<String> {
    let mut chars = value.chars();
    let first = chars.next()?;
    let second = chars.next()?;
    let mut key = String::with_capacity(first.len_utf8() + second.len_utf8());
    key.push(first);
    key.push(second);
    Some(key)
}

fn fuzzy_distance_limit(normalized_query: &str) -> usize {
    match normalized_query.chars().count() {
        0..=4 => 1,
        5..=8 => 2,
        _ => 3,
    }
}

fn normalize_city_search_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|char| char.is_alphanumeric())
        .collect()
}

fn levenshtein_bounded(left: &str, right: &str, max_distance: usize) -> Option<usize> {
    let left_chars = left.chars().collect::<Vec<_>>();
    let right_chars = right.chars().collect::<Vec<_>>();
    if left_chars.len().abs_diff(right_chars.len()) > max_distance {
        return None;
    }

    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut current = vec![0; right_chars.len() + 1];
    for (left_index, left_char) in left_chars.iter().enumerate() {
        current[0] = left_index + 1;
        let mut row_min = current[0];
        for (right_index, right_char) in right_chars.iter().enumerate() {
            let substitution_cost = usize::from(left_char != right_char);
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + substitution_cost);
            row_min = row_min.min(current[right_index + 1]);
        }
        if row_min > max_distance {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let distance = previous[right_chars.len()];
    (distance <= max_distance).then_some(distance)
}

struct CatalogReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> CatalogReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn expect_bytes(&mut self, expected: &[u8]) -> Result<(), String> {
        let actual = self.read_bytes(expected.len())?;
        if actual == expected {
            Ok(())
        } else {
            Err("invalid city catalog magic".to_string())
        }
    }

    fn read_null_terminated_string(&mut self) -> Result<String, String> {
        let start = self.offset;
        while self.offset < self.bytes.len() && self.bytes[self.offset] != 0 {
            self.offset += 1;
        }
        if self.offset >= self.bytes.len() {
            return Err("unterminated string in city catalog".to_string());
        }
        let value = std::str::from_utf8(&self.bytes[start..self.offset])
            .map_err(|error| error.to_string())?
            .to_string();
        self.offset += 1;
        Ok(value)
    }

    fn read_string(&mut self) -> Result<String, String> {
        let len = self.read_u16()? as usize;
        let bytes = self.read_bytes(len)?;
        Ok(std::str::from_utf8(bytes)
            .map_err(|error| error.to_string())?
            .to_string())
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        Ok(*self
            .read_bytes(1)?
            .first()
            .ok_or_else(|| "unexpected EOF".to_string())?)
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        let bytes = self.read_array::<2>()?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        let bytes = self.read_array::<4>()?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_i32(&mut self) -> Result<i32, String> {
        let bytes = self.read_array::<4>()?;
        Ok(i32::from_le_bytes(bytes))
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let bytes = self.read_bytes(N)?;
        bytes
            .try_into()
            .map_err(|_| "invalid fixed-width field".to_string())
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| "city catalog offset overflow".to_string())?;
        if end > self.bytes.len() {
            return Err("unexpected EOF in city catalog".to_string());
        }
        let bytes = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }
}

pub fn source_model_registry() -> SourceModelRegistry {
    SourceModelRegistry {
        version: 1,
        model_family: SOURCE_MODEL_FAMILY.to_string(),
        input_features: SOURCE_MODEL_INPUT_FEATURES,
        output_targets: climate_normal_targets(),
        sources: vec![
            source_model(SourceModelSeed {
                weather_source_id: "nasa_power",
                label: "NASA POWER",
                coverage_rule: SourceModelCoverage::Global,
                checkpoint_uri: "rtx.homelab:~/pv-estimator-gpu/results/full_climate_normals_compressor_holdout_768x8/best_model.pt",
                training_locations: 7_056,
                training_rows: 309_391_488,
                best_epoch: 77,
                best_validation_mae_mean: 5.101154327392578,
            }),
            source_model(SourceModelSeed {
                weather_source_id: "pvgis_era5",
                label: "PVGIS-ERA5",
                coverage_rule: SourceModelCoverage::GlobalLandPvgisGateway,
                checkpoint_uri: "rtx.homelab:~/pv-estimator-gpu/results/pvgis_era5_climate_normals_compressor_768x8/best_model.pt",
                training_locations: 1_972,
                training_rows: 328_408_996,
                best_epoch: 75,
                best_validation_mae_mean: 8.333486728010506,
            }),
            source_model(SourceModelSeed {
                weather_source_id: "pvgis_sarah3",
                label: "PVGIS-SARAH3",
                coverage_rule: SourceModelCoverage::EmpiricalGridMask {
                    mask_path: "experiments/ml-weather/config/source_coverage/pvgis_sarah3_empirical_grid_mask.json".to_string(),
                },
                checkpoint_uri: "rtx.homelab:~/pv-estimator-gpu/results/pvgis_sarah3_climate_normals_compressor_768x8/best_model.pt",
                training_locations: 787,
                training_rows: 131_063_835,
                best_epoch: 80,
                best_validation_mae_mean: 7.889469525492784,
            }),
        ],
    }
}

fn climate_normal_targets() -> Vec<ClimateNormalTarget> {
    vec![
        ClimateNormalTarget::GhiMean,
        ClimateNormalTarget::DniMean,
        ClimateNormalTarget::DhiMean,
        ClimateNormalTarget::TemperatureMean,
        ClimateNormalTarget::WindMean,
        ClimateNormalTarget::GhiStd,
        ClimateNormalTarget::DniStd,
        ClimateNormalTarget::DhiStd,
        ClimateNormalTarget::TemperatureStd,
        ClimateNormalTarget::WindStd,
    ]
}

struct SourceModelSeed {
    weather_source_id: &'static str,
    label: &'static str,
    coverage_rule: SourceModelCoverage,
    checkpoint_uri: &'static str,
    training_locations: u32,
    training_rows: u64,
    best_epoch: u32,
    best_validation_mae_mean: f64,
}

fn source_model(seed: SourceModelSeed) -> SourceModelMetadata {
    SourceModelMetadata {
        weather_source_id: WeatherSourceId::new(seed.weather_source_id)
            .expect("valid source model id"),
        label: seed.label.to_string(),
        active: true,
        coverage_rule: seed.coverage_rule,
        checkpoint_uri: seed.checkpoint_uri.to_string(),
        training_locations: seed.training_locations,
        training_rows: seed.training_rows,
        best_epoch: seed.best_epoch,
        best_validation_mae_mean: seed.best_validation_mae_mean,
        parameters: SOURCE_MODEL_PARAMETERS,
    }
}

struct ItalianCapitalFixture {
    location_id: &'static str,
    display_name: &'static str,
    region: &'static str,
    province: &'static str,
    latitude_degrees: f64,
    longitude_degrees: f64,
    elevation_meters: Option<f64>,
    available_weather_sources: Vec<WeatherSourceId>,
}

impl ItalianCapitalFixture {
    fn into_location(self) -> Location {
        Location {
            location_id: LocationId::new(self.location_id).expect("valid fixture id"),
            display_name: self.display_name.to_string(),
            country_code: "IT".to_string(),
            region: Some(self.region.to_string()),
            province: Some(self.province.to_string()),
            latitude: Angle::from_degrees(self.latitude_degrees),
            longitude: Angle::from_degrees(self.longitude_degrees),
            elevation: self.elevation_meters.map(Length::from_meters),
            timezone: "Europe/Rome".to_string(),
            available_weather_sources: self.available_weather_sources,
        }
    }
}

fn weather_source(
    weather_source_id: &str,
    name: &str,
    provider: WeatherProvider,
    documentation_url: &str,
) -> WeatherSourceMetadata {
    WeatherSourceMetadata {
        weather_source_id: WeatherSourceId::new(weather_source_id).expect("valid fixture id"),
        name: name.to_string(),
        provider,
        documentation_url: documentation_url.to_string(),
        imported_at: Some("2026-05-26".to_string()),
        notes: Some("Small fixture dataset for format and lookup tests".to_string()),
    }
}

fn hourly_record(
    hour_of_year: u16,
    ghi_watts_per_square_meter: f64,
    dni_watts_per_square_meter: Option<f64>,
    dhi_watts_per_square_meter: Option<f64>,
    temperature_celsius: f64,
    wind_speed_meters_per_second: Option<f64>,
    quality_flags: Vec<WeatherQualityFlag>,
) -> HourlyWeatherRecord {
    HourlyWeatherRecord {
        hour_of_year,
        global_horizontal_irradiance: Power::from_watts(ghi_watts_per_square_meter),
        direct_normal_irradiance: dni_watts_per_square_meter.map(Power::from_watts),
        diffuse_horizontal_irradiance: dhi_watts_per_square_meter.map(Power::from_watts),
        ambient_temperature: Temperature::from_celsius(temperature_celsius),
        wind_speed: wind_speed_meters_per_second.map(Speed::from_meters_per_second),
        quality_flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_can_be_listed() {
        let data = EmbeddedData::default();
        let location_ids: Vec<_> = data
            .locations()
            .iter()
            .map(|location| location.location_id.as_str())
            .collect();

        assert_eq!(data.locations().len(), 3);
        assert!(location_ids.contains(&"it-rome"));
        assert!(location_ids.contains(&"it-milan"));
        assert!(location_ids.contains(&"it-palermo"));
    }

    #[test]
    fn weather_data_can_be_retrieved_by_location_and_source() {
        let data = EmbeddedData::default();
        let dataset = data
            .get_weather_dataset(
                &LocationId::new("it-rome").expect("valid id"),
                &WeatherSourceId::new("pvgis-tmy").expect("valid id"),
            )
            .expect("fixture dataset exists");

        assert_eq!(dataset.records.len(), 2);
        assert_eq!(dataset.source.provider, WeatherProvider::Pvgis);
        assert_eq!(
            dataset.records[1].global_horizontal_irradiance.as_watts(),
            520.0
        );
    }

    #[test]
    fn missing_location_returns_clear_error() {
        let data = EmbeddedData::default();
        let error = data
            .get_weather_dataset(
                &LocationId::new("it-unknown").expect("valid id"),
                &WeatherSourceId::new("pvgis-tmy").expect("valid id"),
            )
            .expect_err("location is absent");

        assert_eq!(
            error,
            WeatherDataError::UnknownLocation(LocationId::new("it-unknown").expect("valid id"))
        );
    }

    #[test]
    fn missing_source_returns_clear_error() {
        let data = EmbeddedData::default();
        let error = data
            .get_weather_dataset(
                &LocationId::new("it-rome").expect("valid id"),
                &WeatherSourceId::new("unknown-source").expect("valid id"),
            )
            .expect_err("source is absent");

        assert_eq!(
            error,
            WeatherDataError::UnknownWeatherSource {
                location_id: LocationId::new("it-rome").expect("valid id"),
                weather_source_id: WeatherSourceId::new("unknown-source").expect("valid id"),
            }
        );
    }

    #[test]
    fn source_model_registry_lists_active_ensemble_sources() {
        let registry = source_model_registry();
        let source_ids: Vec<_> = registry
            .sources
            .iter()
            .map(|source| source.weather_source_id.as_str())
            .collect();

        assert_eq!(registry.version, 1);
        assert_eq!(registry.input_features, 66);
        assert_eq!(registry.output_targets.len(), 10);
        assert_eq!(registry.sources.len(), 3);
        assert!(source_ids.contains(&"nasa_power"));
        assert!(source_ids.contains(&"pvgis_era5"));
        assert!(source_ids.contains(&"pvgis_sarah3"));
        assert!(registry.sources.iter().all(|source| source.active));
    }

    #[test]
    fn embedded_city_catalog_has_expected_size_profile() {
        let metadata = city_catalog_metadata();

        assert_eq!(metadata.variant, "ranked_aliases_10");
        assert!(metadata.city_count > 160_000);
        assert!(metadata.compressed_bytes < 7 * 1024 * 1024);
        assert!(metadata.uncompressed_bytes < 17 * 1024 * 1024);
        assert_eq!(metadata.alias_cap, 10);
    }

    #[test]
    fn city_search_returns_coordinates_and_country() {
        let results = search_cities("Milan", 5);
        let first = results.first().expect("Milan is indexed");

        assert_eq!(first.display_name, "Milan");
        assert_eq!(first.country_code, "IT");
        assert!((first.latitude_degrees - 45.46427).abs() < 0.01);
        assert!((first.longitude_degrees - 9.18951).abs() < 0.01);
    }

    #[test]
    fn city_search_uses_aliases_and_fuzzy_matching() {
        let roma = search_cities("Roma", 3);
        assert!(
            roma.iter()
                .any(|city| city.display_name == "Rome" && city.country_code == "IT")
        );

        let miln = search_cities("Miln", 3);
        assert!(
            miln.iter()
                .any(|city| city.display_name == "Milan" && city.country_code == "IT")
        );
    }

    #[test]
    #[ignore = "manual city-search timing smoke"]
    fn city_search_perf_smoke() {
        let _ = city_catalog_metadata();
        for query in ["mi", "mila", "milan", "miln", "roma"] {
            let start = std::time::Instant::now();
            let results = search_cities(query, 30);
            eprintln!(
                "query={query} elapsed_ms={} results={}",
                start.elapsed().as_millis(),
                results.len()
            );
        }
    }
}
