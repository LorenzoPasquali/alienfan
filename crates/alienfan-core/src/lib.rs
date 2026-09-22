//! Core of alienfan: domain model, config, curve engine and sysfs access.
//!
//! Nothing here talks to D-Bus. The daemon, the CLI and the boot service all
//! build on this crate, so every value that reaches the hardware is validated
//! here.

pub mod config;
pub mod controller;
pub mod curve;
pub mod error;
pub mod model;
pub mod plan;
pub mod sysfs;

pub use config::{Config, ConfigFile, PresetConfig, SensorRef};
pub use controller::{Controller, TickOutput};
pub use curve::{Curve, CurvePoint};
pub use error::{ConfigError, Error, Result};
pub use model::{
    Boost, Control, ControlKind, FanId, FanPair, Health, OverrideUntil, PowerSource, Preset,
    Profile,
};
pub use plan::{Target, Write};
pub use sysfs::{Hardware, SysfsRoot};
