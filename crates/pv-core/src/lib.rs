//! UI-agnostic PV system domain, validation, simulation, and reporting core.

pub mod ids;
pub mod issues;
pub mod units;

pub mod prelude {
    //! Commonly used core types for adapters and downstream crates.

    pub use crate::ids::{
        CatalogItemId, ComponentId, EndpointId, IdError, LocationId, ProjectId, WeatherSourceId,
    };
    pub use crate::issues::{Issue, IssueCode, IssueSeverity};
    pub use crate::units::{
        Angle, Area, Current, Energy, Length, Power, Temperature, TimeSpan, Voltage,
    };
}
