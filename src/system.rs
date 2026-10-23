use crate::{
    battery,
    config::WeatherSettings,
    weather::{self, WeatherDisplay},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default)]
pub struct Metrics {
    pub cpu: f64,
    pub memory: f64,
    pub disk: f64,
    pub download: f64,
    pub upload: f64,
    pub gpu: Option<(f64, f64, f64)>,
    pub battery: Option<(f64, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Volume {
    pub level: f64,
    pub muted: bool,
}

#[derive(Debug)]
pub enum Update {
    Metrics(Metrics),
    Volume(Volume),
    Brightness(f64),
    Weather(WeatherDisplay),
}

pub fn output(program: &str, args: &[&str], timeout: Duration) -> io::Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let result = (|| {
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing stdout"))?;
        // Read without blocking so the deadline also covers inherited pipes.
        let fd = stdout.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = Vec::new();
        let start = Instant::now();
        let mut eof = false;
        loop {
            let mut buffer = [0; 8192];
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) => {
                        eof = true;
                        break;
                    }
                    Ok(size) => {
                        bytes.extend_from_slice(&buffer[..size]);
                        if bytes.len() > 1_048_576 {
                            return Err(io::Error::other("command output exceeded 1 MiB"));
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            if let Some(status) = child.try_wait()?
                && eof
            {
                if !status.success() {
                    return Err(io::Error::other(format!("{program}: {status}")));
                }
                return Ok(String::from_utf8_lossy(&bytes).trim().to_owned());
            }
            if start.elapsed() >= timeout {
                return Err(io::Error::new(io::ErrorKind::TimedOut, program.to_owned()));
            }
            thread::sleep(Duration::from_millis(20));
        }
    })();
    if result.is_err() {
        // The helper owns this process group, including descendants holding stdout.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.wait();
    }
    result
}

pub fn execute(argv: Vec<String>) {
    thread::spawn(move || {
        match Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                if let Err(e) = child.wait() {
                    eprintln!("Command failed: {e}");
                }
            }
            Err(e) => eprintln!("Cannot execute {}: {e}", argv[0]),
        }
    });
}

pub fn cpu_totals(text: &str) -> (u64, u64) {
    let values: Vec<u64> = text
        .lines()
        .find(|l| l.starts_with("cpu "))
        .unwrap_or("")
        .split_whitespace()
        .skip(1)
        .take(8)
        .filter_map(|v| v.parse().ok())
        .collect();
    let total = values.iter().sum::<u64>();
    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    (total.saturating_sub(idle), total)
}

pub fn memory_usage(text: &str) -> f64 {
    let mut total = 0.;
    let mut available = 0.;
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let key = words.next().unwrap_or("");
        let value = words.next().unwrap_or("0").parse::<f64>().unwrap_or(0.);
        match key {
            "MemTotal:" => total = value,
            "MemAvailable:" => available = value,
            _ => {}
        }
    }
    if total > 0. {
        ((total - available) / total).clamp(0., 1.)
    } else {
        0.
    }
}

pub fn net_counters(text: &str) -> BTreeMap<String, (u64, u64)> {
    text.lines()
        .filter_map(|line| {
            let (interface, rest) = line.split_once(':')?;
            let interface = interface.trim();
            if interface == "lo" {
                return None;
            }
            let values: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            Some((interface.into(), (*values.first()?, *values.get(8)?)))
        })
        .collect()
}

pub fn network_rates(
    current: &BTreeMap<String, (u64, u64)>,
    previous: &BTreeMap<String, (u64, u64)>,
    seconds: f64,
) -> (f64, f64) {
    if seconds <= 0. {
        return (0., 0.);
    }
    let (mut down, mut up) = (0_u64, 0_u64);
    for (name, &(rx, tx)) in current {
        if let Some(&(old_rx, old_tx)) = previous.get(name) {
            down = down.saturating_add(rx.saturating_sub(old_rx));
            up = up.saturating_add(tx.saturating_sub(old_tx));
        }
    }
    (down as f64 / seconds, up as f64 / seconds)
}

fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn disk_usage() -> f64 {
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // statvfs initializes the entire structure on success.
    if unsafe { libc::statvfs(c"/".as_ptr(), stats.as_mut_ptr()) } != 0 {
        return 0.;
    }
    let stats = unsafe { stats.assume_init() };
    if stats.f_blocks == 0 {
        return 0.;
    }
    (stats.f_blocks - stats.f_bfree) as f64 / stats.f_blocks as f64
}

fn battery() -> Option<(f64, String)> {
    battery::read_all(Path::new("/sys/class/power_supply"))
}

fn brightness() -> Option<f64> {
    read_brightness(Path::new("/sys/class/backlight"))
}

pub fn read_brightness(root: &Path) -> Option<f64> {
    let mut devices: Vec<_> = fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|d| d.path())
        .collect();
    devices.sort();
    for path in devices {
        let current = battery::number(&path, "actual_brightness")
            .or_else(|| battery::number(&path, "brightness"));
        if let Some((current, max)) = current.zip(battery::number(&path, "max_brightness"))
            && max > 0.
        {
            return Some((current / max).clamp(0., 1.));
        }
    }
    None
}

pub fn parse_volume(text: &str) -> Option<Volume> {
    let level = text
        .strip_prefix("Volume:")?
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()?;
    if !level.is_finite() {
        return None;
    }
    Some(Volume {
        level: level.max(0.),
        muted: text.contains("[MUTED]"),
    })
}

fn start_levels(tx: Sender<Update>) {
    let brightness_tx = tx.clone();
    thread::spawn(move || {
        let mut last_brightness = None;
        loop {
            if let Some(value) = brightness()
                && last_brightness != Some(value)
            {
                last_brightness = Some(value);
                if brightness_tx.send(Update::Brightness(value)).is_err() {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
    thread::spawn(move || {
        let mut last_volume = None;
        loop {
            let started = Instant::now();
            if let Some(value) = output(
                "wpctl",
                &["get-volume", "@DEFAULT_AUDIO_SINK@"],
                Duration::from_secs(1),
            )
            .ok()
            .and_then(|text| parse_volume(&text))
                && last_volume.as_ref() != Some(&value)
            {
                last_volume = Some(value.clone());
                if tx.send(Update::Volume(value)).is_err() {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(100).saturating_sub(started.elapsed()));
        }
    });
}

pub fn start() -> Receiver<Update> {
    let (tx, rx) = mpsc::channel();
    // Keep slow GPU and disk queries out of the volume/brightness response path.
    start_levels(tx.clone());
    thread::spawn(move || {
        let mut previous_cpu = cpu_totals(&read("/proc/stat"));
        let mut previous_net = net_counters(&read("/proc/net/dev"));
        let mut previous_time = Instant::now();
        let mut last_metrics = Instant::now() - Duration::from_secs(2);
        loop {
            if last_metrics.elapsed() >= Duration::from_secs(2) {
                let cpu = cpu_totals(&read("/proc/stat"));
                let cpu_load = if cpu.1 > previous_cpu.1 {
                    cpu.0.saturating_sub(previous_cpu.0) as f64 / (cpu.1 - previous_cpu.1) as f64
                } else {
                    0.
                };
                previous_cpu = cpu;
                let net = net_counters(&read("/proc/net/dev"));
                let (download, upload) =
                    network_rates(&net, &previous_net, previous_time.elapsed().as_secs_f64());
                previous_time = Instant::now();
                previous_net = net;
                let gpu = if std::path::Path::new("/proc/driver/nvidia").exists() {
                    output(
                        "nvidia-smi",
                        &[
                            "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu",
                            "--format=csv,noheader,nounits",
                        ],
                        Duration::from_secs(2),
                    )
                    .ok()
                    .and_then(|text| {
                        let values: Vec<f64> = text
                            .lines()
                            .next()?
                            .split(',')
                            .filter_map(|v| v.trim().parse().ok())
                            .collect();
                        (values.len() == 4 && values[2] > 0.)
                            .then(|| (values[0] / 100., values[1] / values[2], values[3]))
                    })
                } else {
                    None
                };
                if tx
                    .send(Update::Metrics(Metrics {
                        cpu: cpu_load.clamp(0., 1.),
                        memory: memory_usage(&read("/proc/meminfo")),
                        disk: disk_usage(),
                        download,
                        upload,
                        gpu,
                        battery: battery(),
                    }))
                    .is_err()
                {
                    return;
                }
                last_metrics = Instant::now();
            }
            thread::sleep(Duration::from_millis(500));
        }
    });
    rx
}

pub fn start_weather(updates: Sender<Update>, settings: Receiver<WeatherSettings>) {
    thread::spawn(move || {
        let mut config = WeatherSettings::default();
        let mut last = Instant::now() - Duration::from_secs(300);
        loop {
            while let Ok(next) = settings.try_recv() {
                config = next;
                last = Instant::now() - Duration::from_secs(300);
            }
            if config.enable && last.elapsed() >= Duration::from_secs(300) {
                if let Some(display) = weather(&config)
                    && updates.send(Update::Weather(display)).is_err()
                {
                    return;
                }
                last = Instant::now();
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
}

fn fetch_json(url: &str) -> Option<Value> {
    let text = output(
        "curl",
        &[
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            "12",
            url,
        ],
        Duration::from_secs(15),
    )
    .ok()?;
    serde_json::from_str(&text).ok()
}

fn weather(settings: &WeatherSettings) -> Option<WeatherDisplay> {
    let (lat, lon) = match (settings.latitude, settings.longitude) {
        (Some(lat), Some(lon)) => (lat, lon),
        _ => {
            let location = fetch_json("https://ipwho.is/")?;
            (
                location["latitude"].as_f64()?,
                location["longitude"].as_f64()?,
            )
        }
    };
    if !(-90. ..=90.).contains(&lat) || !(-180. ..=180.).contains(&lon) {
        return None;
    }
    let data = fetch_json(&format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&current=temperature_2m,is_day,weather_code"
    ))?;
    let current = &data["current"];
    let temperature = current["temperature_2m"].as_f64()?;
    let code = current["weather_code"].as_u64()?;
    let day = current["is_day"].as_u64()? != 0;
    let date = gtk::glib::DateTime::now_local().ok()?;
    Some(weather::display(
        temperature,
        code,
        day,
        (date.year(), date.month() as u32, date.day_of_month() as u32),
    ))
}

pub fn format_rate(bytes: f64) -> String {
    let units = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes.max(0.);
    let mut index = 0;
    while value >= 1024. && index < units.len() - 1 {
        value /= 1024.;
        index += 1;
    }
    format!("{value:.1} {}/s", units[index])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_excludes_guest_time_and_memory_uses_available() {
        assert_eq!(cpu_totals("cpu 10 20 30 100 5 2 3 4 999 999\n"), (69, 174));
        assert!((memory_usage("MemTotal: 1000 kB\nMemAvailable: 400 kB\n") - 0.6).abs() < 1e-6);
    }
    #[test]
    fn interface_churn_and_counter_reset_do_not_spike_network_rates() {
        let old = BTreeMap::from([("eth0".into(), (1000, 500)), ("wlan0".into(), (2000, 700))]);
        let new = BTreeMap::from([
            ("eth0".into(), (1200, 550)),
            ("tun0".into(), (999999, 999999)),
        ]);
        assert_eq!(network_rates(&new, &old, 2.), (100., 25.));
        let reset = BTreeMap::from([("eth0".into(), (2, 3))]);
        assert_eq!(network_rates(&reset, &old, 1.), (0., 0.));
    }
    #[test]
    fn muted_and_boosted_volume_remain_distinguishable() {
        assert_eq!(
            parse_volume("Volume: 1.25 [MUTED]"),
            Some(Volume {
                level: 1.25,
                muted: true
            })
        );
        assert!(parse_volume("Volume: NaN").is_none());
        assert!(parse_volume("no sink").is_none());
    }
}
#[test]
fn captures_replies_larger_than_a_pipe_buffer() {
    let text = output(
        "bash",
        &["-c", "printf '%100000s' x"],
        Duration::from_secs(3),
    )
    .unwrap();
    assert_eq!(text, "x");
}

#[test]
fn command_deadline_covers_inherited_stdout_and_output_limit() {
    let start = Instant::now();
    assert_eq!(
        output("bash", &["-c", "sleep 5 &"], Duration::from_millis(100))
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut,
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        output(
            "bash",
            &["-c", "printf '%1100000s' x"],
            Duration::from_secs(3)
        )
        .unwrap_err()
        .to_string()
        .contains("exceeded 1 MiB")
    );
}

#[test]
fn backlight_skips_invalid_devices_and_reads_actual_values() {
    let root = std::env::temp_dir().join(format!("astal-backlight-{}", std::process::id()));
    fs::create_dir_all(root.join("a-broken")).unwrap();
    fs::create_dir_all(root.join("b-panel")).unwrap();
    fs::write(root.join("b-panel/actual_brightness"), "30").unwrap();
    fs::write(root.join("b-panel/brightness"), "80").unwrap();
    fs::write(root.join("b-panel/max_brightness"), "100").unwrap();
    assert_eq!(read_brightness(&root), Some(0.3));
    fs::remove_dir_all(root).unwrap();
}
