//! Embedded locations, normalized weather data, and equipment catalogs.

use pv_core::ids::{LocationId, WeatherSourceId};
use pv_core::units::{Angle, Length, Power, Temperature};
use pv_core::weather::{
    HourlyWeatherRecord, Location, LocationCatalog, Speed, WeatherDataError, WeatherDataset,
    WeatherProvider, WeatherQualityFlag, WeatherRepository, WeatherSourceMetadata,
};

const PVGIS_TMY_DOC_URL: &str = "https://joint-research-centre.ec.europa.eu/photovoltaic-geographical-information-system-pvgis/using-pvgis-5/pvgis-5-tools/pvgis-typical-meteorological-year-tmy-generator_en";
const NASA_POWER_HOURLY_DOC_URL: &str =
    "https://power.larc.nasa.gov/docs/services/api/temporal/hourly/";

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
}
