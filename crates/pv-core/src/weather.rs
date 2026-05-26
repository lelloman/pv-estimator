use crate::ids::{LocationId, WeatherSourceId};
use crate::units::{Angle, Energy, Length, Power, Temperature};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub location_id: LocationId,
    pub display_name: String,
    pub country_code: String,
    pub region: Option<String>,
    pub province: Option<String>,
    pub latitude: Angle,
    pub longitude: Angle,
    pub elevation: Option<Length>,
    pub timezone: String,
    pub available_weather_sources: Vec<WeatherSourceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeatherSourceMetadata {
    pub weather_source_id: WeatherSourceId,
    pub name: String,
    pub provider: WeatherProvider,
    pub documentation_url: String,
    pub imported_at: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherProvider {
    Pvgis,
    NasaPower,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherDataset {
    pub location_id: LocationId,
    pub source: WeatherSourceMetadata,
    pub records: Vec<HourlyWeatherRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HourlyWeatherRecord {
    pub hour_of_year: u16,
    pub global_horizontal_irradiance: Power,
    pub direct_normal_irradiance: Option<Power>,
    pub diffuse_horizontal_irradiance: Option<Power>,
    pub ambient_temperature: Temperature,
    pub wind_speed: Option<Speed>,
    pub quality_flags: Vec<WeatherQualityFlag>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Speed {
    meters_per_second: f64,
}

impl Speed {
    pub const fn from_meters_per_second(meters_per_second: f64) -> Self {
        Self { meters_per_second }
    }

    pub const fn from_kilometers_per_hour(kilometers_per_hour: f64) -> Self {
        Self {
            meters_per_second: kilometers_per_hour / 3.6,
        }
    }

    pub const fn as_meters_per_second(self) -> f64 {
        self.meters_per_second
    }

    pub const fn as_kilometers_per_hour(self) -> f64 {
        self.meters_per_second * 3.6
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherQualityFlag {
    TypicalMeteorologicalYear,
    Climatology,
    MissingDirectNormalIrradiance,
    MissingDiffuseHorizontalIrradiance,
    MissingWindSpeed,
    Estimated,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeatherDataError {
    UnknownLocation(LocationId),
    UnknownWeatherSource {
        location_id: LocationId,
        weather_source_id: WeatherSourceId,
    },
}

impl fmt::Display for WeatherDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownLocation(location_id) => write!(f, "unknown location {location_id}"),
            Self::UnknownWeatherSource {
                location_id,
                weather_source_id,
            } => write!(
                f,
                "unknown weather source {weather_source_id} for location {location_id}"
            ),
        }
    }
}

impl Error for WeatherDataError {}

pub trait LocationCatalog {
    fn locations(&self) -> &[Location];

    fn get_location(&self, location_id: &LocationId) -> Option<&Location> {
        self.locations()
            .iter()
            .find(|location| &location.location_id == location_id)
    }
}

pub trait WeatherRepository {
    fn get_weather_dataset(
        &self,
        location_id: &LocationId,
        weather_source_id: &WeatherSourceId,
    ) -> Result<&WeatherDataset, WeatherDataError>;
}

impl HourlyWeatherRecord {
    pub fn irradiance_energy_over_one_hour(&self) -> Energy {
        self.global_horizontal_irradiance
            .energy_over(crate::units::TimeSpan::from_hours(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_converts_between_common_units() {
        let speed = Speed::from_kilometers_per_hour(18.0);

        assert_eq!(speed.as_meters_per_second(), 5.0);
        assert_eq!(
            Speed::from_meters_per_second(5.0).as_kilometers_per_hour(),
            18.0
        );
    }

    #[test]
    fn hourly_record_can_express_one_hour_irradiance_energy() {
        let record = HourlyWeatherRecord {
            hour_of_year: 12,
            global_horizontal_irradiance: Power::from_watts(800.0),
            direct_normal_irradiance: None,
            diffuse_horizontal_irradiance: None,
            ambient_temperature: Temperature::from_celsius(25.0),
            wind_speed: None,
            quality_flags: vec![WeatherQualityFlag::MissingWindSpeed],
        };

        assert_eq!(
            record.irradiance_energy_over_one_hour().as_watt_hours(),
            800.0
        );
    }
}
