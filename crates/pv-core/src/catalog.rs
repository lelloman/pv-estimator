use crate::ids::CatalogItemId;
use crate::issues::{Issue, IssueCode, IssueSeverity};
use crate::project::{CustomEquipmentDefinition, EquipmentReference};
use crate::units::{Area, Current, Energy, Length, Power, Temperature, Voltage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogItem {
    pub catalog_item_id: CatalogItemId,
    pub metadata: CatalogItemMetadata,
    pub equipment: Equipment,
}

impl CatalogItem {
    pub fn category(&self) -> EquipmentCategory {
        self.equipment.category()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogItemMetadata {
    pub manufacturer: String,
    pub model: String,
    pub source: Option<CatalogSource>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSource {
    pub source_url: String,
    pub extraction_date: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "category", rename_all = "snake_case")]
pub enum Equipment {
    Panel(PanelSpec),
    Inverter(InverterSpec),
    Battery(BatterySpec),
    Bms(BmsSpec),
    BlockingDiode(BlockingDiodeSpec),
    Cable(CableSpec),
}

impl Equipment {
    pub const fn category(&self) -> EquipmentCategory {
        match self {
            Self::Panel(_) => EquipmentCategory::Panel,
            Self::Inverter(_) => EquipmentCategory::Inverter,
            Self::Battery(_) => EquipmentCategory::Battery,
            Self::Bms(_) => EquipmentCategory::Bms,
            Self::BlockingDiode(_) => EquipmentCategory::BlockingDiode,
            Self::Cable(_) => EquipmentCategory::Cable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentCategory {
    Panel,
    Inverter,
    Battery,
    Bms,
    BlockingDiode,
    Cable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelSpec {
    pub dimensions: ModuleDimensions,
    pub nominal_power: Power,
    pub area: Area,
    pub module_efficiency: Option<f64>,
    pub vmp: Voltage,
    pub imp: Current,
    pub voc: Voltage,
    pub isc: Current,
    pub temperature_coefficient_power_per_celsius: Option<f64>,
    pub temperature_coefficient_voc_per_celsius: Option<f64>,
    pub temperature_coefficient_isc_per_celsius: Option<f64>,
    pub noct: Option<Temperature>,
    pub nmot: Option<Temperature>,
    pub maximum_system_voltage: Option<Voltage>,
    pub maximum_series_fuse_rating: Option<Current>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleDimensions {
    pub width: Length,
    pub height: Length,
    pub thickness: Option<Length>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InverterSpec {
    pub inverter_type: InverterType,
    pub ac_nominal_power: Power,
    pub ac_max_power: Option<Power>,
    pub mppt_inputs: Vec<MpptInputSpec>,
    pub startup_voltage: Option<Voltage>,
    pub maximum_dc_voltage: Voltage,
    pub supported_battery_voltage_range: Option<VoltageRange>,
    pub weighted_efficiency: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InverterType {
    String,
    Hybrid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MpptInputSpec {
    pub input_index: u16,
    pub voltage_range: VoltageRange,
    pub maximum_input_current: Current,
    pub maximum_short_circuit_current: Option<Current>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VoltageRange {
    pub min: Voltage,
    pub max: Voltage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatterySpec {
    pub nominal_voltage: Voltage,
    pub usable_capacity: Energy,
    pub total_capacity: Option<Energy>,
    pub maximum_charge_power: Option<Power>,
    pub maximum_discharge_power: Option<Power>,
    pub maximum_charge_current: Option<Current>,
    pub maximum_discharge_current: Option<Current>,
    pub minimum_soc_fraction: Option<f64>,
    pub maximum_soc_fraction: Option<f64>,
    pub round_trip_efficiency: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BmsSpec {
    pub nominal_voltage: Option<Voltage>,
    pub voltage_range: Option<VoltageRange>,
    pub maximum_charge_current: Current,
    pub maximum_discharge_current: Current,
    pub maximum_charge_power: Option<Power>,
    pub maximum_discharge_power: Option<Power>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockingDiodeSpec {
    pub maximum_reverse_voltage: Voltage,
    pub maximum_forward_current: Current,
    pub forward_voltage_drop: Voltage,
    pub maximum_power_dissipation: Option<Power>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CableSpec {
    pub material: CableMaterial,
    pub cross_section: Area,
    pub current_rating: Option<Current>,
    pub resistance_ohm_per_meter: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CableMaterial {
    Copper,
    Aluminum,
}

pub trait EquipmentCatalog {
    fn get(&self, catalog_item_id: &CatalogItemId) -> Option<&CatalogItem>;
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InMemoryEquipmentCatalog {
    items: BTreeMap<CatalogItemId, CatalogItem>,
}

impl InMemoryEquipmentCatalog {
    pub fn new(items: impl IntoIterator<Item = CatalogItem>) -> Self {
        Self {
            items: items
                .into_iter()
                .map(|item| (item.catalog_item_id.clone(), item))
                .collect(),
        }
    }

    pub fn insert(&mut self, item: CatalogItem) -> Option<CatalogItem> {
        self.items.insert(item.catalog_item_id.clone(), item)
    }

    pub fn items(&self) -> impl Iterator<Item = &CatalogItem> {
        self.items.values()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl EquipmentCatalog for InMemoryEquipmentCatalog {
    fn get(&self, catalog_item_id: &CatalogItemId) -> Option<&CatalogItem> {
        self.items.get(catalog_item_id)
    }
}

pub struct ProjectEquipmentCatalog<'a> {
    pub embedded: &'a dyn EquipmentCatalog,
    pub custom: &'a dyn EquipmentCatalog,
}

impl ProjectEquipmentCatalog<'_> {
    pub fn resolve_reference(&self, reference: &EquipmentReference) -> Result<&CatalogItem, Issue> {
        match reference {
            EquipmentReference::Catalog { catalog_item_id } => self
                .embedded
                .get(catalog_item_id)
                .ok_or_else(|| missing_catalog_item_issue(catalog_item_id, "catalog")),
            EquipmentReference::Custom {
                custom_equipment_id,
            } => self
                .custom
                .get(custom_equipment_id)
                .ok_or_else(|| missing_catalog_item_issue(custom_equipment_id, "custom")),
        }
    }
}

pub fn custom_equipment_catalog(
    definitions: &[CustomEquipmentDefinition],
) -> InMemoryEquipmentCatalog {
    InMemoryEquipmentCatalog::new(definitions.iter().map(|definition| definition.item.clone()))
}

pub fn missing_catalog_item_issue(catalog_item_id: &CatalogItemId, source: &str) -> Issue {
    Issue::new(IssueCode::new("catalog.missing_item"), IssueSeverity::Error)
        .with_parameter("catalog_item_id", catalog_item_id.as_str())
        .with_parameter("source", source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_resolves_embedded_and_custom_equipment() {
        let embedded_item = inverter_item("catalog.inverter.1");
        let custom_item = panel_item("custom.panel.1");
        let embedded = InMemoryEquipmentCatalog::new([embedded_item.clone()]);
        let custom = InMemoryEquipmentCatalog::new([custom_item.clone()]);
        let catalog = ProjectEquipmentCatalog {
            embedded: &embedded,
            custom: &custom,
        };

        let resolved_embedded = catalog
            .resolve_reference(&EquipmentReference::Catalog {
                catalog_item_id: embedded_item.catalog_item_id.clone(),
            })
            .expect("embedded item exists");
        let resolved_custom = catalog
            .resolve_reference(&EquipmentReference::Custom {
                custom_equipment_id: custom_item.catalog_item_id.clone(),
            })
            .expect("custom item exists");

        assert_eq!(resolved_embedded, &embedded_item);
        assert_eq!(resolved_custom, &custom_item);
    }

    #[test]
    fn missing_catalog_reference_returns_validation_issue() {
        let embedded = InMemoryEquipmentCatalog::default();
        let custom = InMemoryEquipmentCatalog::default();
        let catalog = ProjectEquipmentCatalog {
            embedded: &embedded,
            custom: &custom,
        };
        let missing_id = CatalogItemId::new("missing.item").expect("valid id");

        let issue = catalog
            .resolve_reference(&EquipmentReference::Catalog {
                catalog_item_id: missing_id,
            })
            .expect_err("item is missing");

        assert_eq!(issue.code().as_str(), "catalog.missing_item");
        assert_eq!(
            issue.parameters().get("catalog_item_id"),
            Some(&"missing.item".to_string())
        );
    }

    #[test]
    fn required_panel_fields_are_typed() {
        let item = panel_item("panel.1");
        assert_eq!(item.category(), EquipmentCategory::Panel);

        let Equipment::Panel(panel) = &item.equipment else {
            panic!("expected panel");
        };

        assert_eq!(panel.nominal_power.as_watts(), 450.0);
        assert_eq!(panel.voc.as_volts(), 49.0);
        assert_eq!(panel.isc.as_amperes(), 11.5);
    }

    #[test]
    fn required_inverter_fields_are_typed() {
        let item = inverter_item("inverter.1");
        assert_eq!(item.category(), EquipmentCategory::Inverter);

        let Equipment::Inverter(inverter) = &item.equipment else {
            panic!("expected inverter");
        };

        assert_eq!(inverter.ac_nominal_power.as_watts(), 5_000.0);
        assert_eq!(inverter.maximum_dc_voltage.as_volts(), 1_000.0);
        assert_eq!(inverter.mppt_inputs.len(), 1);
    }

    #[test]
    fn required_storage_and_balance_of_system_fields_are_typed() {
        assert_eq!(
            battery_item("battery.1").category(),
            EquipmentCategory::Battery
        );
        assert_eq!(bms_item("bms.1").category(), EquipmentCategory::Bms);
        assert_eq!(
            blocking_diode_item("diode.1").category(),
            EquipmentCategory::BlockingDiode
        );
        assert_eq!(cable_item("cable.1").category(), EquipmentCategory::Cable);
    }

    fn metadata(model: &str) -> CatalogItemMetadata {
        CatalogItemMetadata {
            manufacturer: "Example".to_string(),
            model: model.to_string(),
            source: Some(CatalogSource {
                source_url: "https://example.com/datasheet.pdf".to_string(),
                extraction_date: "2026-05-26".to_string(),
            }),
            notes: None,
        }
    }

    fn panel_item(id: &str) -> CatalogItem {
        CatalogItem {
            catalog_item_id: CatalogItemId::new(id).expect("valid id"),
            metadata: metadata("Example Panel"),
            equipment: Equipment::Panel(PanelSpec {
                dimensions: ModuleDimensions {
                    width: Length::from_meters(1.13),
                    height: Length::from_meters(1.76),
                    thickness: Some(Length::from_millimeters(30.0)),
                },
                nominal_power: Power::from_watts(450.0),
                area: Area::from_square_meters(1.9888),
                module_efficiency: Some(0.226),
                vmp: Voltage::from_volts(41.0),
                imp: Current::from_amperes(10.98),
                voc: Voltage::from_volts(49.0),
                isc: Current::from_amperes(11.5),
                temperature_coefficient_power_per_celsius: Some(-0.0029),
                temperature_coefficient_voc_per_celsius: Some(-0.0025),
                temperature_coefficient_isc_per_celsius: Some(0.0004),
                noct: Some(Temperature::from_celsius(45.0)),
                nmot: None,
                maximum_system_voltage: Some(Voltage::from_volts(1_500.0)),
                maximum_series_fuse_rating: Some(Current::from_amperes(25.0)),
            }),
        }
    }

    fn inverter_item(id: &str) -> CatalogItem {
        CatalogItem {
            catalog_item_id: CatalogItemId::new(id).expect("valid id"),
            metadata: metadata("Example Hybrid Inverter"),
            equipment: Equipment::Inverter(InverterSpec {
                inverter_type: InverterType::Hybrid,
                ac_nominal_power: Power::from_watts(5_000.0),
                ac_max_power: Some(Power::from_watts(5_500.0)),
                mppt_inputs: vec![MpptInputSpec {
                    input_index: 1,
                    voltage_range: VoltageRange {
                        min: Voltage::from_volts(120.0),
                        max: Voltage::from_volts(850.0),
                    },
                    maximum_input_current: Current::from_amperes(13.5),
                    maximum_short_circuit_current: Some(Current::from_amperes(20.0)),
                }],
                startup_voltage: Some(Voltage::from_volts(150.0)),
                maximum_dc_voltage: Voltage::from_volts(1_000.0),
                supported_battery_voltage_range: Some(VoltageRange {
                    min: Voltage::from_volts(150.0),
                    max: Voltage::from_volts(600.0),
                }),
                weighted_efficiency: Some(0.975),
            }),
        }
    }

    fn battery_item(id: &str) -> CatalogItem {
        CatalogItem {
            catalog_item_id: CatalogItemId::new(id).expect("valid id"),
            metadata: metadata("Example Battery"),
            equipment: Equipment::Battery(BatterySpec {
                nominal_voltage: Voltage::from_volts(200.0),
                usable_capacity: Energy::from_kilowatt_hours(10.0),
                total_capacity: Some(Energy::from_kilowatt_hours(11.0)),
                maximum_charge_power: Some(Power::from_watts(5_000.0)),
                maximum_discharge_power: Some(Power::from_watts(5_000.0)),
                maximum_charge_current: Some(Current::from_amperes(25.0)),
                maximum_discharge_current: Some(Current::from_amperes(25.0)),
                minimum_soc_fraction: Some(0.05),
                maximum_soc_fraction: Some(0.95),
                round_trip_efficiency: Some(0.94),
            }),
        }
    }

    fn bms_item(id: &str) -> CatalogItem {
        CatalogItem {
            catalog_item_id: CatalogItemId::new(id).expect("valid id"),
            metadata: metadata("Example BMS"),
            equipment: Equipment::Bms(BmsSpec {
                nominal_voltage: Some(Voltage::from_volts(200.0)),
                voltage_range: Some(VoltageRange {
                    min: Voltage::from_volts(150.0),
                    max: Voltage::from_volts(240.0),
                }),
                maximum_charge_current: Current::from_amperes(25.0),
                maximum_discharge_current: Current::from_amperes(25.0),
                maximum_charge_power: Some(Power::from_watts(5_000.0)),
                maximum_discharge_power: Some(Power::from_watts(5_000.0)),
            }),
        }
    }

    fn blocking_diode_item(id: &str) -> CatalogItem {
        CatalogItem {
            catalog_item_id: CatalogItemId::new(id).expect("valid id"),
            metadata: metadata("Example Blocking Diode"),
            equipment: Equipment::BlockingDiode(BlockingDiodeSpec {
                maximum_reverse_voltage: Voltage::from_volts(1_500.0),
                maximum_forward_current: Current::from_amperes(20.0),
                forward_voltage_drop: Voltage::from_volts(0.7),
                maximum_power_dissipation: Some(Power::from_watts(15.0)),
            }),
        }
    }

    fn cable_item(id: &str) -> CatalogItem {
        CatalogItem {
            catalog_item_id: CatalogItemId::new(id).expect("valid id"),
            metadata: metadata("Example Cable"),
            equipment: Equipment::Cable(CableSpec {
                material: CableMaterial::Copper,
                cross_section: Area::from_square_millimeters(6.0),
                current_rating: Some(Current::from_amperes(40.0)),
                resistance_ohm_per_meter: Some(0.00308),
            }),
        }
    }
}
