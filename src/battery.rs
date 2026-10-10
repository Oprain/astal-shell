use std::{fs, path::Path};

#[derive(Clone, Debug)]
pub struct Battery {
    pub name: String,
    pub fraction: f64,
    pub status: String,
    pub energy: Option<f64>,
    pub full: Option<f64>,
    pub health: Option<f64>,
    pub power: Option<f64>,
    pub voltage: Option<f64>,
    pub temperature: Option<f64>,
    pub technology: String,
    pub vendor: String,
    pub model: String,
}

pub fn number(path: &Path, key: &str) -> Option<f64> {
    fs::read_to_string(path.join(key))
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

fn text(path: &Path, key: &str) -> String {
    fs::read_to_string(path.join(key))
        .unwrap_or_default()
        .trim()
        .into()
}

impl Battery {
    pub fn read(path: &Path) -> Option<Self> {
        if text(path, "type") != "Battery" || number(path, "present") == Some(0.) {
            return None;
        }
        let voltage = number(path, "voltage_now")
            .filter(|v| *v > 0.)
            .or_else(|| number(path, "voltage_min_design").filter(|v| *v > 0.))
            .map(|v| v / 1_000_000.);
        let energy_value = |energy, charge| {
            number(path, energy)
                .map(|v| v / 1_000_000.)
                .or_else(|| Some(number(path, charge)? * voltage? / 1_000_000.))
                .filter(|v| *v >= 0.)
        };
        let energy = energy_value("energy_now", "charge_now");
        let full = energy_value("energy_full", "charge_full").filter(|v| *v > 0.);
        let design = energy_value("energy_full_design", "charge_full_design").filter(|v| *v > 0.);
        let fraction = number(path, "capacity")
            .map(|v| v / 100.)
            .or_else(|| Some(energy? / full?))?
            .clamp(0., 1.);
        let power = number(path, "power_now")
            .map(|v| v.abs() / 1_000_000.)
            .or_else(|| Some(number(path, "current_now")?.abs() * voltage? / 1_000_000.));
        Some(Self {
            name: path.file_name()?.to_string_lossy().into(),
            fraction,
            status: text(path, "status"),
            energy,
            full,
            health: full.zip(design).map(|(f, d)| f / d * 100.),
            power,
            voltage,
            temperature: number(path, "temp").map(|v| v / 10.),
            technology: text(path, "technology"),
            vendor: text(path, "manufacturer"),
            model: text(path, "model_name"),
        })
    }

    pub fn tooltip(&self) -> String {
        let mut lines = vec![
            format!("{}: {:.0}%", self.name, self.fraction * 100.),
            format!("Status: {}", self.status),
        ];
        if let Some((energy, power)) = self.energy.zip(self.power).filter(|(_, p)| *p > 0.) {
            let remaining = if self.status == "Charging" {
                self.full
                    .map(|full| ("Time to full", (full - energy).max(0.)))
            } else if self.status == "Discharging" {
                Some(("Time to empty", energy))
            } else {
                None
            };
            if let Some((label, energy)) = remaining {
                let minutes = (energy / power * 60.).round() as u64;
                lines.push(format!("{label}: {}h {}m", minutes / 60, minutes % 60));
            }
        }
        if let Some(health) = self.health {
            lines.push(format!("Capacity: {health:.0}%"));
        }
        if let Some((energy, full)) = self.energy.zip(self.full) {
            lines.push(format!("Energy: {energy:.1}/{full:.1} Wh"));
        }
        if let Some(power) = self.power {
            lines.push(format!("Power draw: {power:.1} W"));
        }
        if let Some(voltage) = self.voltage {
            lines.push(format!("Voltage: {voltage:.2} V"));
        }
        if let Some(temp) = self.temperature {
            lines.push(format!("Temperature: {temp:.1}\u{b0}C"));
        }
        if !self.technology.is_empty() {
            lines.push(format!("Technology: {}", self.technology));
        }
        if !self.vendor.is_empty() || !self.model.is_empty() {
            lines.push(format!("Device: {} {}", self.vendor, self.model));
        }
        lines.join("\n")
    }
}

pub fn read_all(root: &Path) -> Option<(f64, String)> {
    let mut batteries: Vec<_> = fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|entry| Battery::read(&entry.path()))
        .collect();
    batteries.sort_by(|a, b| a.name.cmp(&b.name));
    if batteries.is_empty() {
        return None;
    }
    let energies: Option<Vec<_>> = batteries.iter().map(|b| b.energy.zip(b.full)).collect();
    let fraction = match energies {
        Some(energies) => {
            energies.iter().map(|e| e.0).sum::<f64>() / energies.iter().map(|e| e.1).sum::<f64>()
        }
        None => batteries.iter().map(|b| b.fraction).sum::<f64>() / batteries.len() as f64,
    };
    Some((
        fraction.clamp(0., 1.),
        batteries
            .iter()
            .map(Battery::tooltip)
            .collect::<Vec<_>>()
            .join("\n\n"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_energy_health_eta_and_device_details() {
        let root = std::env::temp_dir().join(format!("astal-battery-{}", std::process::id()));
        let battery = root.join("BAT0");
        fs::create_dir_all(&battery).unwrap();
        for (key, value) in [
            ("type", "Battery"),
            ("status", "Charging"),
            ("capacity", "50"),
            ("energy_now", "20000000"),
            ("energy_full", "40000000"),
            ("energy_full_design", "50000000"),
            ("power_now", "10000000"),
            ("voltage_now", "12000000"),
            ("temp", "305"),
            ("technology", "Li-ion"),
            ("manufacturer", "Fixture"),
            ("model_name", "Notebook"),
        ] {
            fs::write(battery.join(key), value).unwrap();
        }
        let (fraction, details) = read_all(&root).unwrap();
        assert_eq!(fraction, 0.5);
        for expected in [
            "Time to full: 2h 0m",
            "Capacity: 80%",
            "20.0/40.0 Wh",
            "10.0 W",
            "12.00 V",
            "30.5\u{b0}C",
            "Li-ion",
            "Fixture Notebook",
        ] {
            assert!(details.contains(expected), "{details}");
        }
        fs::write(battery.join("present"), "0").unwrap();
        assert!(read_all(&root).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn charge_based_batteries_use_voltage_and_weighted_aggregation() {
        let root = std::env::temp_dir().join(format!("astal-charge-{}", std::process::id()));
        for (name, now, full) in [
            ("BAT0", "1000000", "2000000"),
            ("BAT1", "6000000", "6000000"),
        ] {
            let path = root.join(name);
            fs::create_dir_all(&path).unwrap();
            for (key, value) in [
                ("type", "Battery"),
                ("status", "Discharging"),
                ("charge_now", now),
                ("charge_full", full),
                ("charge_full_design", full),
                ("voltage_now", "10000000"),
                ("current_now", "-1000000"),
            ] {
                fs::write(path.join(key), value).unwrap();
            }
        }
        let (fraction, details) = read_all(&root).unwrap();
        assert_eq!(fraction, 0.875);
        assert!(details.contains("Time to empty: 1h 0m"));
        fs::remove_dir_all(root).unwrap();
    }
}
