//! UI-agnostic PV system domain, validation, simulation, and reporting core.

pub mod catalog;
pub mod ids;
pub mod issues;
pub mod project;
pub mod simulation;
pub mod source_model;
pub mod units;
pub mod weather;

pub mod prelude {
    //! Commonly used core types for adapters and downstream crates.

    pub use crate::catalog::{
        BatterySpec, BlockingDiodeSpec, BmsSpec, CableMaterial, CableSpec, CatalogItem,
        CatalogItemMetadata, CatalogSource, Equipment, EquipmentCatalog, EquipmentCategory,
        InMemoryEquipmentCatalog, InverterSpec, InverterType, ModuleDimensions, MpptInputSpec,
        PanelSpec, ProjectEquipmentCatalog, VoltageRange, custom_equipment_catalog,
    };
    pub use crate::ids::{
        CatalogItemId, ComponentId, EndpointId, IdError, LocationId, ProjectId, WeatherSourceId,
    };
    pub use crate::issues::{Issue, IssueCode, IssueSeverity};
    pub use crate::project::{
        ComponentInstance, ComponentKind, CustomEquipmentDefinition, EquipmentReference,
        LoadProfileReference, MountingGroup, ProjectLoadError, PvSystemProject, SchemaVersion,
        SimulationSettings, StringDefinition, SystemLocation, TopologyConnection,
        load_project_json, save_project_json,
    };
    pub use crate::simulation::{
        BuiltInLoadShapeId, LoadProfile, LoadShape, MetricSummary, MonthlyProductionBand,
        ProductionProfile, SimulationMetricSummaries, SimulationOptions, SimulationRequest,
        SimulationResult, SimulationRunMetrics, StorageConfig, simulate,
        simulate_with_cancellation,
    };
    pub use crate::source_model::{
        AnnualPvEnsembleEstimate, CalibratedEnergyBand, CalibratedIrradiationBand,
        ClimateNormalTarget, EnergyEstimateBand, EstimateCoverage, EstimateLocation,
        EstimateSystem, EstimateUncertainty, Irradiation, IrradiationEstimateBand, MonthOfYear,
        MonthlyPvEnsembleEstimate, SourceAnnualPvEstimate, SourceEnsembleEstimateDocument,
        SourceModelCoverage, SourceModelMetadata, SourceModelRegistry, SourceMonthlyPvEstimate,
    };
    pub use crate::units::{
        Angle, Area, Current, Energy, Length, Power, Temperature, TimeSpan, Voltage,
    };
    pub use crate::weather::{
        HourlyWeatherRecord, Location, LocationCatalog, Speed, WeatherDataError, WeatherDataset,
        WeatherProvider, WeatherQualityFlag, WeatherRepository, WeatherSourceMetadata,
    };
}
