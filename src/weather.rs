#[derive(Clone, Debug, PartialEq)]
pub struct WeatherDisplay {
    pub text: String,
    pub icon: Option<&'static str>,
    pub tooltip: String,
}

pub fn moon_phase(year: i32, month: u32, day: u32) -> (usize, &'static str, &'static str) {
    let (year, month) = if month < 3 {
        (year - 1, month + 12)
    } else {
        (year, month)
    };
    // Keep the original shell's mean synodic-month approximation.
    let cycles = (365.25 * f64::from(year) + 30.6 * f64::from(month + 1) + f64::from(day)
        - 694_039.09)
        / 29.530_588_2;
    let phase = (cycles.rem_euclid(1.) * 8.).round() as usize % 8;
    const PHASES: [(&str, &str); 8] = [
        ("New", "\u{1f311}"),
        ("Waxing Crescent", "\u{1f312}"),
        ("Quarter Moon", "\u{1f313}"),
        ("Waxing Gibbous", "\u{1f314}"),
        ("Full", "\u{1f315}"),
        ("Waning Gibbous", "\u{1f316}"),
        ("Last Quarter", "\u{1f317}"),
        ("Waning Crescent", "\u{1f318}"),
    ];
    (phase, PHASES[phase].0, PHASES[phase].1)
}

pub fn condition(code: u64) -> (&'static str, &'static str) {
    match code {
        0 => ("Clear sky", "weather-clear"),
        1 => ("Mainly clear", "weather-few-clouds"),
        2 => ("Partly cloudy", "weather-few-clouds"),
        3 => ("Overcast", "weather-overcast"),
        45 => ("Fog", "weather-fog"),
        48 => ("Depositing rime fog", "weather-fog"),
        51 => ("Drizzle: Light", "weather-showers"),
        53 => ("Drizzle: Moderate", "weather-showers"),
        55 => ("Drizzle: Dense intensity", "weather-showers"),
        56 => ("Freezing Drizzle: Light", "weather-snow"),
        57 => ("Freezing Drizzle: Dense intensity", "weather-snow"),
        61 => ("Rain: Slight", "weather-showers"),
        63 => ("Rain: Moderate", "weather-showers"),
        65 => ("Rain: Heavy intensity", "weather-showers"),
        66 => ("Freezing Rain: Light", "weather-snow"),
        67 => ("Freezing Rain: Heavy intensity", "weather-snow"),
        71 => ("Snow fall: Slight", "weather-snow"),
        73 => ("Snow fall: Moderate", "weather-snow"),
        75 => ("Snow fall: Heavy intensity", "weather-snow"),
        77 => ("Snow grains", "weather-snow"),
        80 => ("Rain showers: Slight", "weather-showers"),
        81 => ("Rain showers: Moderate", "weather-showers"),
        82 => ("Rain showers: Violent", "weather-showers"),
        85 => ("Snow showers slight", "weather-snow"),
        86 => ("Snow showers heavy", "weather-snow"),
        95 => ("Thunderstorm: Slight or moderate", "weather-storm"),
        96 => ("Thunderstorm with slight hail", "weather-storm"),
        99 => ("Thunderstorm with heavy hail", "weather-storm"),
        _ => ("Unknown weather code", "weather-severe-alert"),
    }
}

pub fn display(temperature: f64, code: u64, day: bool, date: (i32, u32, u32)) -> WeatherDisplay {
    let (description, icon) = condition(code);
    if day {
        WeatherDisplay {
            text: format!("{temperature:.0}\u{b0}C"),
            icon: Some(icon),
            tooltip: description.into(),
        }
    } else {
        let (_, phase, moon) = moon_phase(date.0, date.1, date.2);
        WeatherDisplay {
            text: format!("{moon} {temperature:.0}\u{b0}C"),
            icon: None,
            tooltip: format!("{description}\n{phase} moon"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_known_new_and_full_moons_and_all_months() {
        assert_eq!(moon_phase(2024, 1, 11).0, 0);
        assert_eq!(moon_phase(2024, 1, 25).0, 4);
        for month in 1..=12 {
            assert!(moon_phase(2024, month, 15).0 < 8);
        }
    }

    #[test]
    fn night_uses_actual_phase_and_preserves_weather_description() {
        let night = display(13., 95, false, (2024, 1, 25));
        assert_eq!(night.icon, None);
        assert!(night.text.starts_with("\u{1f315}"));
        assert!(night.tooltip.contains("Full moon"));
        assert!(night.tooltip.contains("Thunderstorm"));
        assert_eq!(
            display(13., 0, true, (2024, 1, 25)).icon,
            Some("weather-clear")
        );
    }

    #[test]
    fn preserves_wmo_details_and_unknown_codes() {
        assert_eq!(condition(56).0, "Freezing Drizzle: Light");
        assert_eq!(condition(99).0, "Thunderstorm with heavy hail");
        assert_eq!(condition(999).0, "Unknown weather code");
    }
}
