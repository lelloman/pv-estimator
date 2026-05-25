use serde::{Deserialize, Serialize};
use std::ops::{Add, Div, Mul, Sub};

macro_rules! quantity {
    ($name:ident, $field:ident) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
        pub struct $name {
            $field: f64,
        }

        impl $name {
            pub const fn from_base_units($field: f64) -> Self {
                Self { $field }
            }

            pub const fn as_base_units(self) -> f64 {
                self.$field
            }
        }

        impl Add for $name {
            type Output = Self;

            fn add(self, rhs: Self) -> Self::Output {
                Self::from_base_units(self.$field + rhs.$field)
            }
        }

        impl Sub for $name {
            type Output = Self;

            fn sub(self, rhs: Self) -> Self::Output {
                Self::from_base_units(self.$field - rhs.$field)
            }
        }

        impl Mul<f64> for $name {
            type Output = Self;

            fn mul(self, rhs: f64) -> Self::Output {
                Self::from_base_units(self.$field * rhs)
            }
        }

        impl Div<f64> for $name {
            type Output = Self;

            fn div(self, rhs: f64) -> Self::Output {
                Self::from_base_units(self.$field / rhs)
            }
        }

        impl Div for $name {
            type Output = f64;

            fn div(self, rhs: Self) -> Self::Output {
                self.$field / rhs.$field
            }
        }
    };
}

quantity!(Voltage, volts);
quantity!(Current, amperes);
quantity!(Power, watts);
quantity!(Energy, watt_hours);
quantity!(Temperature, degrees_celsius);
quantity!(Length, meters);
quantity!(Area, square_meters);
quantity!(Angle, radians);
quantity!(TimeSpan, hours);

impl Voltage {
    pub const fn from_volts(volts: f64) -> Self {
        Self::from_base_units(volts)
    }

    pub const fn from_kilovolts(kilovolts: f64) -> Self {
        Self::from_base_units(kilovolts * 1_000.0)
    }

    pub const fn as_volts(self) -> f64 {
        self.as_base_units()
    }

    pub const fn as_kilovolts(self) -> f64 {
        self.as_base_units() / 1_000.0
    }
}

impl Current {
    pub const fn from_amperes(amperes: f64) -> Self {
        Self::from_base_units(amperes)
    }

    pub const fn from_milliamperes(milliamperes: f64) -> Self {
        Self::from_base_units(milliamperes / 1_000.0)
    }

    pub const fn as_amperes(self) -> f64 {
        self.as_base_units()
    }

    pub const fn as_milliamperes(self) -> f64 {
        self.as_base_units() * 1_000.0
    }
}

impl Power {
    pub const fn from_watts(watts: f64) -> Self {
        Self::from_base_units(watts)
    }

    pub const fn from_kilowatts(kilowatts: f64) -> Self {
        Self::from_base_units(kilowatts * 1_000.0)
    }

    pub const fn as_watts(self) -> f64 {
        self.as_base_units()
    }

    pub const fn as_kilowatts(self) -> f64 {
        self.as_base_units() / 1_000.0
    }

    pub fn from_voltage_current(voltage: Voltage, current: Current) -> Self {
        Self::from_watts(voltage.as_volts() * current.as_amperes())
    }

    pub fn energy_over(self, duration: TimeSpan) -> Energy {
        Energy::from_watt_hours(self.as_watts() * duration.as_hours())
    }
}

impl Energy {
    pub const fn from_watt_hours(watt_hours: f64) -> Self {
        Self::from_base_units(watt_hours)
    }

    pub const fn from_kilowatt_hours(kilowatt_hours: f64) -> Self {
        Self::from_base_units(kilowatt_hours * 1_000.0)
    }

    pub const fn from_joules(joules: f64) -> Self {
        Self::from_base_units(joules / 3_600.0)
    }

    pub const fn as_watt_hours(self) -> f64 {
        self.as_base_units()
    }

    pub const fn as_kilowatt_hours(self) -> f64 {
        self.as_base_units() / 1_000.0
    }

    pub const fn as_joules(self) -> f64 {
        self.as_base_units() * 3_600.0
    }

    pub fn average_power_over(self, duration: TimeSpan) -> Power {
        Power::from_watts(self.as_watt_hours() / duration.as_hours())
    }
}

impl Temperature {
    const KELVIN_OFFSET: f64 = 273.15;

    pub const fn from_celsius(degrees_celsius: f64) -> Self {
        Self::from_base_units(degrees_celsius)
    }

    pub const fn from_kelvin(kelvin: f64) -> Self {
        Self::from_base_units(kelvin - Self::KELVIN_OFFSET)
    }

    pub const fn as_celsius(self) -> f64 {
        self.as_base_units()
    }

    pub const fn as_kelvin(self) -> f64 {
        self.as_base_units() + Self::KELVIN_OFFSET
    }
}

impl Length {
    pub const fn from_meters(meters: f64) -> Self {
        Self::from_base_units(meters)
    }

    pub const fn from_millimeters(millimeters: f64) -> Self {
        Self::from_base_units(millimeters / 1_000.0)
    }

    pub const fn as_meters(self) -> f64 {
        self.as_base_units()
    }

    pub const fn as_millimeters(self) -> f64 {
        self.as_base_units() * 1_000.0
    }
}

impl Area {
    pub const fn from_square_meters(square_meters: f64) -> Self {
        Self::from_base_units(square_meters)
    }

    pub const fn from_square_millimeters(square_millimeters: f64) -> Self {
        Self::from_base_units(square_millimeters / 1_000_000.0)
    }

    pub const fn as_square_meters(self) -> f64 {
        self.as_base_units()
    }

    pub const fn as_square_millimeters(self) -> f64 {
        self.as_base_units() * 1_000_000.0
    }
}

impl Angle {
    pub const fn from_radians(radians: f64) -> Self {
        Self::from_base_units(radians)
    }

    pub fn from_degrees(degrees: f64) -> Self {
        Self::from_base_units(degrees.to_radians())
    }

    pub const fn as_radians(self) -> f64 {
        self.as_base_units()
    }

    pub fn as_degrees(self) -> f64 {
        self.as_base_units().to_degrees()
    }
}

impl TimeSpan {
    pub const fn from_hours(hours: f64) -> Self {
        Self::from_base_units(hours)
    }

    pub const fn from_seconds(seconds: f64) -> Self {
        Self::from_base_units(seconds / 3_600.0)
    }

    pub const fn as_hours(self) -> f64 {
        self.as_base_units()
    }

    pub const fn as_seconds(self) -> f64 {
        self.as_base_units() * 3_600.0
    }
}

impl Mul<Current> for Voltage {
    type Output = Power;

    fn mul(self, rhs: Current) -> Self::Output {
        Power::from_voltage_current(self, rhs)
    }
}

impl Mul<Voltage> for Current {
    type Output = Power;

    fn mul(self, rhs: Voltage) -> Self::Output {
        Power::from_voltage_current(rhs, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn almost_eq(left: f64, right: f64) -> bool {
        (left - right).abs() < 1e-9
    }

    #[test]
    fn converts_voltage_current_power_and_energy() {
        assert!(almost_eq(Voltage::from_kilovolts(1.2).as_volts(), 1_200.0));
        assert!(almost_eq(
            Current::from_milliamperes(500.0).as_amperes(),
            0.5
        ));
        assert!(almost_eq(Power::from_kilowatts(3.5).as_watts(), 3_500.0));
        assert!(almost_eq(
            Energy::from_kilowatt_hours(2.4).as_watt_hours(),
            2_400.0
        ));
        assert!(almost_eq(Energy::from_joules(3_600.0).as_watt_hours(), 1.0));
    }

    #[test]
    fn converts_temperature_length_area_angle_and_time() {
        assert!(almost_eq(
            Temperature::from_celsius(25.0).as_kelvin(),
            298.15
        ));
        assert!(almost_eq(
            Temperature::from_kelvin(300.0).as_celsius(),
            26.85
        ));
        assert!(almost_eq(
            Length::from_millimeters(1_500.0).as_meters(),
            1.5
        ));
        assert!(almost_eq(
            Area::from_square_millimeters(6.0).as_square_meters(),
            0.000_006
        ));
        assert!(almost_eq(
            Angle::from_degrees(180.0).as_radians(),
            std::f64::consts::PI
        ));
        assert!(almost_eq(TimeSpan::from_seconds(7_200.0).as_hours(), 2.0));
    }

    #[test]
    fn computes_derived_energy_and_power() {
        let voltage = Voltage::from_volts(40.0);
        let current = Current::from_amperes(10.0);
        let power = voltage * current;
        let energy = power.energy_over(TimeSpan::from_hours(5.0));

        assert!(almost_eq(power.as_watts(), 400.0));
        assert!(almost_eq(energy.as_kilowatt_hours(), 2.0));
        assert!(almost_eq(
            energy
                .average_power_over(TimeSpan::from_hours(5.0))
                .as_watts(),
            400.0
        ));
    }

    #[test]
    fn arithmetic_preserves_units() {
        let total = Voltage::from_volts(20.0) + Voltage::from_volts(30.0);
        let scaled = total * 2.0;
        let ratio = scaled / Voltage::from_volts(25.0);

        assert!(almost_eq(total.as_volts(), 50.0));
        assert!(almost_eq(scaled.as_volts(), 100.0));
        assert!(almost_eq(ratio, 4.0));
    }
}
