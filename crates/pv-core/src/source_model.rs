use crate::ids::WeatherSourceId;
use crate::units::Energy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceModelRegistry {
    pub version: u32,
    pub model_family: String,
    pub input_features: u16,
    pub output_targets: Vec<ClimateNormalTarget>,
    pub sources: Vec<SourceModelMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceModelMetadata {
    pub weather_source_id: WeatherSourceId,
    pub label: String,
    pub active: bool,
    pub coverage_rule: SourceModelCoverage,
    pub checkpoint_uri: String,
    pub training_locations: u32,
    pub training_rows: u64,
    pub best_epoch: u32,
    pub best_validation_mae_mean: f64,
    pub parameters: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceEnsembleEstimateDocument {
    pub schema_version: u32,
    pub location: EstimateLocation,
    pub system: EstimateSystem,
    pub coverage: EstimateCoverage,
    pub ensemble_estimate: AnnualPvEnsembleEstimate,
    #[serde(default)]
    pub references: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimateLocation {
    pub location_id: String,
    pub name: String,
    pub region: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimateSystem {
    pub peak_power_kwp: f64,
    pub loss_pct: f64,
    pub tilt_deg: f64,
    pub aspect_deg: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_usable_kwh: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimateCoverage {
    pub pvgis_sarah3_applicable: bool,
    pub applicable_sources: Vec<WeatherSourceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClimateNormalTarget {
    GhiMean,
    DniMean,
    DhiMean,
    TemperatureMean,
    WindMean,
    GhiStd,
    DniStd,
    DhiStd,
    TemperatureStd,
    WindStd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceModelCoverage {
    Global,
    GlobalLandPvgisGateway,
    EmpiricalGridMask { mask_path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MonthOfYear(u8);

impl MonthOfYear {
    pub fn new(value: u8) -> Option<Self> {
        (1..=12).contains(&value).then_some(Self(value))
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Irradiation {
    kilowatt_hours_per_square_meter: f64,
}

impl Irradiation {
    pub const fn from_kilowatt_hours_per_square_meter(
        kilowatt_hours_per_square_meter: f64,
    ) -> Self {
        Self {
            kilowatt_hours_per_square_meter,
        }
    }

    pub const fn as_kilowatt_hours_per_square_meter(self) -> f64 {
        self.kilowatt_hours_per_square_meter
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceAnnualPvEstimate {
    pub weather_source_id: WeatherSourceId,
    pub annual_energy: Energy,
    pub annual_in_plane_irradiation: Irradiation,
    pub annual_global_horizontal_irradiation: Irradiation,
    pub monthly_estimates: Vec<SourceMonthlyPvEstimate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceMonthlyPvEstimate {
    pub month: MonthOfYear,
    pub energy: Energy,
    pub in_plane_irradiation: Irradiation,
    pub global_horizontal_irradiation: Irradiation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnualPvEnsembleEstimate {
    pub source_estimates: Vec<SourceAnnualPvEstimate>,
    pub annual_energy: EnergyEstimateBand,
    pub annual_in_plane_irradiation: IrradiationEstimateBand,
    pub annual_global_horizontal_irradiation: IrradiationEstimateBand,
    pub uncertainty: EstimateUncertainty,
    pub monthly_estimates: Vec<MonthlyPvEnsembleEstimate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonthlyPvEnsembleEstimate {
    pub month: MonthOfYear,
    pub energy: EnergyEstimateBand,
    pub in_plane_irradiation: IrradiationEstimateBand,
    pub global_horizontal_irradiation: IrradiationEstimateBand,
    pub uncertainty: EstimateUncertainty,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EnergyEstimateBand {
    pub mean: Energy,
    pub low: Energy,
    pub high: Energy,
    pub half_spread: Energy,
    pub spread_fraction: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IrradiationEstimateBand {
    pub mean: Irradiation,
    pub low: Irradiation,
    pub high: Irradiation,
    pub half_spread: Irradiation,
    pub spread_fraction: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimateUncertainty {
    pub method: UncertaintyMethod,
    pub multiplier: f64,
    pub source_count: u16,
    pub calibrated: bool,
    pub annual_energy: Option<CalibratedEnergyBand>,
    pub annual_in_plane_irradiation: Option<CalibratedIrradiationBand>,
    pub annual_global_horizontal_irradiation: Option<CalibratedIrradiationBand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyMethod {
    SourceSpreadMultiplier,
    InsufficientSources,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CalibratedEnergyBand {
    pub low: Energy,
    pub high: Energy,
    pub half_width: Energy,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CalibratedIrradiationBand {
    pub low: Irradiation,
    pub high: Irradiation,
    pub half_width: Irradiation,
}

impl AnnualPvEnsembleEstimate {
    pub fn from_source_estimates(source_estimates: Vec<SourceAnnualPvEstimate>) -> Option<Self> {
        Self::from_source_estimates_with_uncertainty(source_estimates, 2.0)
    }

    pub fn from_source_estimates_with_uncertainty(
        source_estimates: Vec<SourceAnnualPvEstimate>,
        uncertainty_multiplier: f64,
    ) -> Option<Self> {
        if source_estimates.is_empty() {
            return None;
        }

        let annual_energy = EnergyEstimateBand::from_values(
            source_estimates
                .iter()
                .map(|estimate| estimate.annual_energy.as_kilowatt_hours()),
        );
        let annual_in_plane_irradiation =
            IrradiationEstimateBand::from_values(source_estimates.iter().map(|estimate| {
                estimate
                    .annual_in_plane_irradiation
                    .as_kilowatt_hours_per_square_meter()
            }));
        let annual_global_horizontal_irradiation =
            IrradiationEstimateBand::from_values(source_estimates.iter().map(|estimate| {
                estimate
                    .annual_global_horizontal_irradiation
                    .as_kilowatt_hours_per_square_meter()
            }));

        Some(Self {
            uncertainty: EstimateUncertainty::from_bands(
                source_estimates.len(),
                uncertainty_multiplier,
                annual_energy,
                annual_in_plane_irradiation,
                annual_global_horizontal_irradiation,
            ),
            annual_energy,
            annual_in_plane_irradiation,
            annual_global_horizontal_irradiation,
            monthly_estimates: monthly_estimate_bands(&source_estimates, uncertainty_multiplier),
            source_estimates,
        })
    }

    pub fn source_count(&self) -> usize {
        self.source_estimates.len()
    }
}

fn monthly_estimate_bands(
    source_estimates: &[SourceAnnualPvEstimate],
    uncertainty_multiplier: f64,
) -> Vec<MonthlyPvEnsembleEstimate> {
    (1..=12)
        .filter_map(|month| {
            let month = MonthOfYear::new(month).expect("valid month");
            let estimates: Vec<_> = source_estimates
                .iter()
                .flat_map(|source| source.monthly_estimates.iter())
                .filter(|estimate| estimate.month == month)
                .collect();

            (!estimates.is_empty()).then(|| {
                let energy = EnergyEstimateBand::from_values(
                    estimates
                        .iter()
                        .map(|estimate| estimate.energy.as_kilowatt_hours()),
                );
                let in_plane_irradiation =
                    IrradiationEstimateBand::from_values(estimates.iter().map(|estimate| {
                        estimate
                            .in_plane_irradiation
                            .as_kilowatt_hours_per_square_meter()
                    }));
                let global_horizontal_irradiation =
                    IrradiationEstimateBand::from_values(estimates.iter().map(|estimate| {
                        estimate
                            .global_horizontal_irradiation
                            .as_kilowatt_hours_per_square_meter()
                    }));
                MonthlyPvEnsembleEstimate {
                    month,
                    uncertainty: EstimateUncertainty::from_bands(
                        estimates.len(),
                        uncertainty_multiplier,
                        energy,
                        in_plane_irradiation,
                        global_horizontal_irradiation,
                    ),
                    energy,
                    in_plane_irradiation,
                    global_horizontal_irradiation,
                }
            })
        })
        .collect()
}

impl EnergyEstimateBand {
    pub fn from_values(values: impl IntoIterator<Item = f64>) -> Self {
        let band = ScalarEstimateBand::from_values(values);
        Self {
            mean: Energy::from_kilowatt_hours(band.mean),
            low: Energy::from_kilowatt_hours(band.low),
            high: Energy::from_kilowatt_hours(band.high),
            half_spread: Energy::from_kilowatt_hours(band.half_spread),
            spread_fraction: band.spread_fraction,
        }
    }
}

impl IrradiationEstimateBand {
    pub fn from_values(values: impl IntoIterator<Item = f64>) -> Self {
        let band = ScalarEstimateBand::from_values(values);
        Self {
            mean: Irradiation::from_kilowatt_hours_per_square_meter(band.mean),
            low: Irradiation::from_kilowatt_hours_per_square_meter(band.low),
            high: Irradiation::from_kilowatt_hours_per_square_meter(band.high),
            half_spread: Irradiation::from_kilowatt_hours_per_square_meter(band.half_spread),
            spread_fraction: band.spread_fraction,
        }
    }
}

impl EstimateUncertainty {
    pub fn from_bands(
        source_count: usize,
        multiplier: f64,
        energy: EnergyEstimateBand,
        in_plane_irradiation: IrradiationEstimateBand,
        global_horizontal_irradiation: IrradiationEstimateBand,
    ) -> Self {
        let calibrated = source_count >= 2;
        Self {
            method: if calibrated {
                UncertaintyMethod::SourceSpreadMultiplier
            } else {
                UncertaintyMethod::InsufficientSources
            },
            multiplier,
            source_count: source_count as u16,
            calibrated,
            annual_energy: calibrated
                .then(|| CalibratedEnergyBand::from_raw_band(energy, multiplier)),
            annual_in_plane_irradiation: calibrated.then(|| {
                CalibratedIrradiationBand::from_raw_band(in_plane_irradiation, multiplier)
            }),
            annual_global_horizontal_irradiation: calibrated.then(|| {
                CalibratedIrradiationBand::from_raw_band(global_horizontal_irradiation, multiplier)
            }),
        }
    }
}

impl CalibratedEnergyBand {
    pub fn from_raw_band(raw: EnergyEstimateBand, multiplier: f64) -> Self {
        let mean = raw.mean.as_kilowatt_hours();
        let half_width = raw.half_spread.as_kilowatt_hours() * multiplier;
        Self {
            low: Energy::from_kilowatt_hours((mean - half_width).max(0.0)),
            high: Energy::from_kilowatt_hours(mean + half_width),
            half_width: Energy::from_kilowatt_hours(half_width),
        }
    }
}

impl CalibratedIrradiationBand {
    pub fn from_raw_band(raw: IrradiationEstimateBand, multiplier: f64) -> Self {
        let mean = raw.mean.as_kilowatt_hours_per_square_meter();
        let half_width = raw.half_spread.as_kilowatt_hours_per_square_meter() * multiplier;
        Self {
            low: Irradiation::from_kilowatt_hours_per_square_meter((mean - half_width).max(0.0)),
            high: Irradiation::from_kilowatt_hours_per_square_meter(mean + half_width),
            half_width: Irradiation::from_kilowatt_hours_per_square_meter(half_width),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ScalarEstimateBand {
    mean: f64,
    low: f64,
    high: f64,
    half_spread: f64,
    spread_fraction: f64,
}

impl ScalarEstimateBand {
    fn from_values(values: impl IntoIterator<Item = f64>) -> Self {
        let values: Vec<_> = values.into_iter().collect();
        assert!(
            !values.is_empty(),
            "estimate bands require at least one source value"
        );

        let low = values.iter().copied().fold(f64::INFINITY, f64::min);
        let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let spread = high - low;

        Self {
            mean,
            low,
            high,
            half_spread: spread / 2.0,
            spread_fraction: if mean == 0.0 { 0.0 } else { spread / mean },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_estimate(
        source_id: &str,
        energy_kwh: f64,
        poa_kwh_m2: f64,
    ) -> SourceAnnualPvEstimate {
        SourceAnnualPvEstimate {
            weather_source_id: WeatherSourceId::new(source_id).expect("valid source id"),
            annual_energy: Energy::from_kilowatt_hours(energy_kwh),
            annual_in_plane_irradiation: Irradiation::from_kilowatt_hours_per_square_meter(
                poa_kwh_m2,
            ),
            annual_global_horizontal_irradiation: Irradiation::from_kilowatt_hours_per_square_meter(
                poa_kwh_m2 - 200.0,
            ),
            monthly_estimates: vec![
                monthly_estimate(1, energy_kwh / 10.0, poa_kwh_m2 / 10.0),
                monthly_estimate(2, energy_kwh / 8.0, poa_kwh_m2 / 8.0),
            ],
        }
    }

    fn monthly_estimate(month: u8, energy_kwh: f64, poa_kwh_m2: f64) -> SourceMonthlyPvEstimate {
        SourceMonthlyPvEstimate {
            month: MonthOfYear::new(month).expect("valid month"),
            energy: Energy::from_kilowatt_hours(energy_kwh),
            in_plane_irradiation: Irradiation::from_kilowatt_hours_per_square_meter(poa_kwh_m2),
            global_horizontal_irradiation: Irradiation::from_kilowatt_hours_per_square_meter(
                poa_kwh_m2 - 20.0,
            ),
        }
    }

    #[test]
    fn ensemble_bands_capture_source_disagreement() {
        let ensemble = AnnualPvEnsembleEstimate::from_source_estimates(vec![
            source_estimate("nasa-power", 1450.0, 1700.0),
            source_estimate("pvgis-era5", 1500.0, 1760.0),
            source_estimate("pvgis-sarah3", 1350.0, 1620.0),
        ])
        .expect("source estimates exist");

        assert_eq!(ensemble.source_count(), 3);
        assert_eq!(
            ensemble.annual_energy.mean.as_kilowatt_hours(),
            1433.3333333333333
        );
        assert_eq!(ensemble.annual_energy.low.as_kilowatt_hours(), 1350.0);
        assert_eq!(ensemble.annual_energy.high.as_kilowatt_hours(), 1500.0);
        assert_eq!(ensemble.annual_energy.half_spread.as_kilowatt_hours(), 75.0);
        assert!((ensemble.annual_energy.spread_fraction - 0.10465116279069768).abs() < 1e-12);
        assert!(ensemble.uncertainty.calibrated);
        assert_eq!(ensemble.uncertainty.multiplier, 2.0);
        let display_band = ensemble.uncertainty.annual_energy.expect("calibrated band");
        assert_eq!(display_band.low.as_kilowatt_hours(), 1283.3333333333333);
        assert_eq!(display_band.high.as_kilowatt_hours(), 1583.3333333333333);
        assert_eq!(display_band.half_width.as_kilowatt_hours(), 150.0);
        assert_eq!(
            ensemble
                .annual_in_plane_irradiation
                .mean
                .as_kilowatt_hours_per_square_meter(),
            1693.3333333333333
        );
        assert_eq!(ensemble.monthly_estimates.len(), 2);
        assert_eq!(ensemble.monthly_estimates[0].month.value(), 1);
        assert_eq!(
            ensemble.monthly_estimates[0]
                .energy
                .mean
                .as_kilowatt_hours(),
            143.33333333333334
        );
    }

    #[test]
    fn month_rejects_invalid_values() {
        assert!(MonthOfYear::new(0).is_none());
        assert!(MonthOfYear::new(13).is_none());
        assert_eq!(MonthOfYear::new(12).expect("valid month").value(), 12);
    }

    #[test]
    fn estimate_document_deserializes_from_inference_json_shape() {
        let json = r#"
        {
          "schema_version": 1,
          "location": {
            "location_id": "it_potenza_user",
            "name": "Potenza",
            "region": "Italy",
            "latitude": 40.65,
            "longitude": 15.643
          },
          "system": {
            "peak_power_kwp": 1.0,
            "loss_pct": 14.0,
            "tilt_deg": 30.0,
            "aspect_deg": 0.0
          },
          "coverage": {
            "pvgis_sarah3_applicable": true,
            "applicable_sources": ["nasa_power", "pvgis_era5", "pvgis_sarah3"]
          },
          "ensemble_estimate": {
            "source_estimates": [
              {
                "weather_source_id": "nasa_power",
                "annual_energy": {"watt_hours": 1450000.0},
                "annual_in_plane_irradiation": {"kilowatt_hours_per_square_meter": 1700.0},
                "annual_global_horizontal_irradiation": {"kilowatt_hours_per_square_meter": 1500.0},
                "monthly_estimates": [
                  {
                    "month": 1,
                    "energy": {"watt_hours": 100000.0},
                    "in_plane_irradiation": {"kilowatt_hours_per_square_meter": 120.0},
                    "global_horizontal_irradiation": {"kilowatt_hours_per_square_meter": 100.0}
                  }
                ]
              }
            ],
            "annual_energy": {
              "mean": {"watt_hours": 1450000.0},
              "low": {"watt_hours": 1450000.0},
              "high": {"watt_hours": 1450000.0},
              "half_spread": {"watt_hours": 0.0},
              "spread_fraction": 0.0
            },
            "annual_in_plane_irradiation": {
              "mean": {"kilowatt_hours_per_square_meter": 1700.0},
              "low": {"kilowatt_hours_per_square_meter": 1700.0},
              "high": {"kilowatt_hours_per_square_meter": 1700.0},
              "half_spread": {"kilowatt_hours_per_square_meter": 0.0},
              "spread_fraction": 0.0
            },
            "annual_global_horizontal_irradiation": {
              "mean": {"kilowatt_hours_per_square_meter": 1500.0},
              "low": {"kilowatt_hours_per_square_meter": 1500.0},
              "high": {"kilowatt_hours_per_square_meter": 1500.0},
              "half_spread": {"kilowatt_hours_per_square_meter": 0.0},
              "spread_fraction": 0.0
            },
            "uncertainty": {
              "method": "insufficient_sources",
              "multiplier": 2.0,
              "source_count": 1,
              "calibrated": false,
              "annual_energy": null,
              "annual_in_plane_irradiation": null,
              "annual_global_horizontal_irradiation": null
            },
            "monthly_estimates": [
              {
                "month": 1,
                "energy": {
                  "mean": {"watt_hours": 100000.0},
                  "low": {"watt_hours": 100000.0},
                  "high": {"watt_hours": 100000.0},
                  "half_spread": {"watt_hours": 0.0},
                  "spread_fraction": 0.0
                },
                "in_plane_irradiation": {
                  "mean": {"kilowatt_hours_per_square_meter": 120.0},
                  "low": {"kilowatt_hours_per_square_meter": 120.0},
                  "high": {"kilowatt_hours_per_square_meter": 120.0},
                  "half_spread": {"kilowatt_hours_per_square_meter": 0.0},
                  "spread_fraction": 0.0
                },
                "global_horizontal_irradiation": {
                  "mean": {"kilowatt_hours_per_square_meter": 100.0},
                  "low": {"kilowatt_hours_per_square_meter": 100.0},
                  "high": {"kilowatt_hours_per_square_meter": 100.0},
                  "half_spread": {"kilowatt_hours_per_square_meter": 0.0},
                  "spread_fraction": 0.0
                },
                "uncertainty": {
                  "method": "insufficient_sources",
                  "multiplier": 2.0,
                  "source_count": 1,
                  "calibrated": false,
                  "annual_energy": null,
                  "annual_in_plane_irradiation": null,
                  "annual_global_horizontal_irradiation": null
                }
              }
            ]
          },
          "references": {}
        }
        "#;

        let document: SourceEnsembleEstimateDocument =
            serde_json::from_str(json).expect("estimate JSON matches core contract");

        assert_eq!(document.schema_version, 1);
        assert_eq!(document.location.location_id, "it_potenza_user");
        assert_eq!(document.coverage.applicable_sources.len(), 3);
        assert_eq!(document.ensemble_estimate.monthly_estimates.len(), 1);
        assert_eq!(
            document
                .ensemble_estimate
                .annual_energy
                .mean
                .as_kilowatt_hours(),
            1450.0
        );
    }

    #[test]
    fn empty_ensemble_is_not_valid() {
        assert!(AnnualPvEnsembleEstimate::from_source_estimates(Vec::new()).is_none());
    }
}
