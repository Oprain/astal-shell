use gtk::{gio, glib, prelude::*};
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

struct Entry {
    path: Option<PathBuf>,
    retry: Instant,
    pending: bool,
}

pub struct ArtworkCache {
    directory: PathBuf,
    entries: HashMap<String, Entry>,
    jobs: Sender<(String, PathBuf)>,
    completed: Receiver<(String, Option<PathBuf>)>,
}

fn filename(uri: &str) -> String {
    let mut hash = glib::Checksum::new(glib::ChecksumType::Sha256).unwrap();
    hash.update(uri.as_bytes());
    hash.string().unwrap()
}

fn valid_image(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.len() > 0 && m.len() <= 8 * 1024 * 1024)
        && gtk::gdk_pixbuf::Pixbuf::from_file(path).is_ok()
}

pub fn download(uri: &str, destination: &Path) -> Option<PathBuf> {
    if !uri.starts_with("http://") && !uri.starts_with("https://") {
        return None;
    }
    fs::create_dir_all(destination.parent()?).ok()?;
    let temporary = destination.with_extension(format!("{}.part", std::process::id()));
    let result = Command::new("curl")
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
        .arg(&temporary)
        .arg(uri)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !result.is_ok_and(|s| s.success()) || !valid_image(&temporary) {
        let _ = fs::remove_file(temporary);
        return None;
    }
    if fs::rename(&temporary, destination).is_err() {
        let _ = fs::remove_file(temporary);
        return None;
    }
    Some(destination.into())
}

impl ArtworkCache {
    pub fn new() -> Self {
        let directory = env::var_os("XDG_CACHE_HOME")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".cache")
            })
            .join("astal-shell/artwork");
        let (jobs, incoming) = mpsc::channel::<(String, PathBuf)>();
        let (results, completed) = mpsc::channel();
        thread::spawn(move || {
            for (uri, path) in incoming {
                let result = download(&uri, &path);
                if results.send((uri, result)).is_err() {
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

    pub fn resolve(&mut self, uri: &str) -> Option<PathBuf> {
        for (uri, path) in self.completed.try_iter() {
            if let Some(entry) = self.entries.get_mut(&uri) {
                entry.path = path;
                entry.pending = false;
                entry.retry = Instant::now() + Duration::from_secs(60);
            }
        }
        if uri.starts_with("file://") || Path::new(uri).is_absolute() {
            let path = if Path::new(uri).is_absolute() {
                PathBuf::from(uri)
            } else {
                gio::File::for_uri(uri).path()?
            };
            return path.is_file().then_some(path);
        }
        if !uri.starts_with("http://") && !uri.starts_with("https://") {
            return None;
        }
        let path = self.directory.join(filename(uri));
        let entry = self.entries.entry(uri.into()).or_insert_with(|| Entry {
            path: valid_image(&path).then(|| path.clone()),
            retry: Instant::now(),
            pending: false,
        });
        if entry.path.is_none() && !entry.pending && entry.retry <= Instant::now() {
            entry.pending = self.jobs.send((uri.into(), path)).is_ok();
        }
        entry.path.clone()
    }
}
