//! Presentation units. Sensors, saved curves and control calculations use Celsius.
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
pub enum Unit {
    #[default]
    Fahrenheit,
    Celsius,
}

static USE_FAHRENHEIT: AtomicBool = AtomicBool::new(true);

impl Unit {
    pub fn convert(self, celsius: f64) -> f64 {
        match self {
            Self::Fahrenheit => celsius * 1.8 + 32.0,
            Self::Celsius => celsius,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Fahrenheit => "°F",
            Self::Celsius => "°C",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Fahrenheit => "fahrenheit",
            Self::Celsius => "celsius",
        }
    }
}

/// Set once when the CLI starts, before creating controllers or rendering output.
pub fn set(unit: Unit) {
    USE_FAHRENHEIT.store(matches!(unit, Unit::Fahrenheit), Ordering::Relaxed);
}

pub fn current() -> Unit {
    if USE_FAHRENHEIT.load(Ordering::Relaxed) {
        Unit::Fahrenheit
    } else {
        Unit::Celsius
    }
}

pub fn value(celsius: f64) -> f64 {
    current().convert(celsius)
}

pub fn display(celsius: f64) -> String {
    let unit = current();
    format!("{:.1}{}", unit.convert(celsius), unit.symbol())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fahrenheit_is_default_and_converts_reference_temperatures() {
        assert_eq!(Unit::default().name(), "fahrenheit");
        for (celsius, fahrenheit) in [(-40.0, -40.0), (0.0, 32.0), (100.0, 212.0)] {
            assert_eq!(Unit::Fahrenheit.convert(celsius), fahrenheit);
            assert_eq!(Unit::Celsius.convert(celsius), celsius);
        }
    }
}
