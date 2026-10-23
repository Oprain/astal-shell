use gtk::{gdk_pixbuf::Pixbuf, gio, glib, prelude::*};
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PIXELS: i64 = 4096 * 4096;
const MAX_ENTRIES: usize = 64;
const MAX_DISK_FILES: usize = 256;
const MAX_DISK_BYTES: u64 = 32 * 1024 * 1024;

struct Entry {
    path: Option<PathBuf>,
    retry: Instant,
    pending: bool,
    used: bool,
    active: Arc<AtomicBool>,
}

struct Job {
    key: String,
    uri: String,
    source: Option<PathBuf>,
    destination: PathBuf,
    side: i32,
    active: Arc<AtomicBool>,
}

pub struct ArtworkCache {
    directory: PathBuf,
    entries: HashMap<String, Entry>,
    jobs: SyncSender<Job>,
    completed: Receiver<(Job, Option<PathBuf>)>,
}

fn filename(value: &str) -> String {
    let mut hash = glib::Checksum::new(glib::ChecksumType::Sha256).unwrap();
    hash.update(value.as_bytes());
    hash.string().unwrap()
}

fn image_size(path: &Path) -> Option<(i32, i32)> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let (_, width, height) = Pixbuf::file_info(path)?;
    (width > 0
        && height > 0
        && width <= 8192
        && height <= 8192
        && i64::from(width) * i64::from(height) <= MAX_PIXELS)
        .then_some((width, height))
}

fn thumbnail(path: &Path, side: i32) -> Option<Pixbuf> {
    let (width, height) = image_size(path)?;
    // Bound both the decoder input and its scaled output, including panoramic covers.
    let scale = (f64::from(side) / f64::from(width.min(height)))
        .min(f64::from(side * 4) / f64::from(width.max(height)))
        .min(1.);
    let image = Pixbuf::from_file_at_scale(
        path,
        (f64::from(width) * scale).ceil().max(1.) as i32,
        (f64::from(height) * scale).ceil().max(1.) as i32,
        true,
    )
    .ok()?
    .apply_embedded_orientation()?;
    let crop = image.width().min(image.height());
    image
        .new_subpixbuf(
            (image.width() - crop) / 2,
            (image.height() - crop) / 2,
            crop,
            crop,
        )
        .scale_simple(side, side, gtk::gdk_pixbuf::InterpType::Bilinear)
}

fn fetch(uri: &str, destination: &Path) -> bool {
    Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--location",
            "--max-redirs",
            "3",
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "--connect-timeout",
            "3",
            "--max-time",
            "12",
            "--max-filesize",
            "8388608",
            "--output",
        ])
        .arg(destination)
        .arg(uri)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn prepare(job: &Job) -> Option<PathBuf> {
    if image_size(&job.destination) == Some((job.side, job.side))
        && Pixbuf::from_file(&job.destination).is_ok()
    {
        return Some(job.destination.clone());
    }
    fs::create_dir_all(job.destination.parent()?).ok()?;
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let temporary = job.destination.with_extension(format!(
        "{}.{}.part",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed),
    ));
    let result = (|| {
        let source = if let Some(source) = &job.source {
            source
        } else if fetch(&job.uri, &temporary) {
            &temporary
        } else {
            return None;
        };
        if !job.active.load(Ordering::Relaxed) {
            return None;
        }
        let image = thumbnail(source, job.side)?;
        image.savev(&temporary, "png", &[]).ok()?;
        fs::rename(&temporary, &job.destination).ok()?;
        Some(job.destination.clone())
    })();
    let _ = fs::remove_file(temporary);
    result
}

fn prune(directory: &Path) {
    let mut files: Vec<_> = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_str()?;
            if path.extension()?.to_str()? != "png"
                || name.len() != 64
                || !name.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then(|| (metadata.modified().ok(), metadata.len(), path))
        })
        .collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.0));
    let mut bytes = 0;
    for (index, (_, size, path)) in files.into_iter().enumerate() {
        bytes += size;
        if index >= MAX_DISK_FILES || bytes > MAX_DISK_BYTES {
            let _ = fs::remove_file(path);
        }
    }
}

impl ArtworkCache {
    pub fn new() -> Self {
        let directory = env::var_os("XDG_CACHE_HOME")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".cache")
            })
            .join("astal-shell/artwork/thumbnails-v1");
        let (jobs, incoming) = mpsc::sync_channel::<Job>(4);
        let (results, completed) = mpsc::sync_channel(4);
        let cache_directory = directory.clone();
        thread::spawn(move || {
            prune(&cache_directory);
            for job in incoming {
                if !job.active.load(Ordering::Relaxed) {
                    continue;
                }
                let result = prepare(&job);
                if result.is_some() {
                    prune(&cache_directory);
                }
                if results.send((job, result)).is_err() {
                    return;
                }
            }
        });
        Self {
            directory,
            entries: HashMap::new(),
            jobs,
            completed,
        }
    }

    pub fn begin_update(&mut self) {
        for (job, path) in self.completed.try_iter() {
            if let Some(entry) = self.entries.get_mut(&job.key)
                && Arc::ptr_eq(&entry.active, &job.active)
            {
                entry.path = path;
                entry.pending = false;
                entry.retry = Instant::now() + Duration::from_secs(60);
            }
        }
        for entry in self.entries.values_mut() {
            entry.used = false;
        }
    }

    pub fn end_update(&mut self) {
        self.entries.retain(|_, entry| {
            entry.active.store(entry.used, Ordering::Relaxed);
            entry.used
        });
    }

    pub fn resolve(&mut self, uri: &str, side: i32) -> Option<PathBuf> {
        let source = if Path::new(uri).is_absolute() {
            Some(PathBuf::from(uri))
        } else if uri.starts_with("file://") {
            Some(gio::File::for_uri(uri).path()?)
        } else if uri.starts_with("http://") || uri.starts_with("https://") {
            None
        } else {
            return None;
        };
        let stamp = if let Some(path) = &source {
            let metadata = fs::metadata(path).ok()?;
            if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
                return None;
            }
            Some((metadata.len(), metadata.modified().ok()))
        } else {
            None
        };
        let side = side.clamp(36, 288);
        let key = filename(&format!("{uri}\n{side}\n{stamp:?}"));
        if !self.entries.contains_key(&key) && self.entries.len() >= MAX_ENTRIES {
            return None;
        }
        let entry = self.entries.entry(key.clone()).or_insert_with(|| Entry {
            path: None,
            retry: Instant::now(),
            pending: false,
            used: true,
            active: Arc::new(AtomicBool::new(true)),
        });
        entry.used = true;
        if entry.path.as_ref().is_some_and(|path| !path.is_file()) {
            entry.path = None;
            entry.retry = Instant::now();
        }
        if entry.path.is_none() && !entry.pending && entry.retry <= Instant::now() {
            entry.pending = self
                .jobs
                .try_send(Job {
                    destination: self.directory.join(format!("{key}.png")),
                    key,
                    uri: uri.into(),
                    source,
                    side,
                    active: entry.active.clone(),
                })
                .is_ok();
        }
        entry.path.clone()
    }
}

impl Drop for ArtworkCache {
    fn drop(&mut self) {
        for entry in self.entries.values() {
            entry.active.store(false, Ordering::Relaxed);
        }
    }
}
