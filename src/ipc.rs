use crate::model::Output;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

#[derive(Debug)]
pub enum Message {
    Event(Value),
    Outputs(BTreeMap<String, Output>),
    Connection(bool),
}

fn socket_path() -> io::Result<PathBuf> {
    env::var_os("NIRI_SOCKET")
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "NIRI_SOCKET is not set; run inside a Niri session",
            )
        })
}

pub fn request(value: &Value) -> io::Result<Value> {
    request_at(&socket_path()?, value)
}

fn request_at(path: &std::path::Path, value: &Value) -> io::Result<Value> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    writeln!(stream, "{value}")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let response: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
    if let Some(error) = response.get("Err") {
        return Err(io::Error::other(error.to_string()));
    }
    response
        .get("Ok")
        .cloned()
        .ok_or_else(|| io::Error::other("invalid Niri response"))
}

pub fn action(value: Value) {
    thread::spawn(move || {
        if let Err(e) = request(&json!({"Action": value})) {
            eprintln!("Niri action failed: {e}");
        }
    });
}

fn event_stream_at(path: &std::path::Path, timeout: Duration) -> io::Result<BufReader<UnixStream>> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(stream, "\"EventStream\"")?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let response: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
    if response.get("Ok").is_none() {
        return Err(io::Error::other(line.trim().to_owned()));
    }
    // An established event stream may legitimately be idle indefinitely.
    reader.get_ref().set_read_timeout(None)?;
    Ok(reader)
}

pub fn start() -> (Receiver<Message>, Sender<()>) {
    let (tx, rx) = mpsc::channel();
    let (refresh_tx, refresh_rx) = mpsc::channel();
    let event_tx = tx.clone();
    thread::spawn(move || {
        let mut last_error = String::new();
        let mut delay = 250;
        loop {
            let result = (|| -> io::Result<()> {
                let mut reader = event_stream_at(&socket_path()?, Duration::from_secs(2))?;
                let mut line = String::new();
                if event_tx.send(Message::Connection(true)).is_err() {
                    return Ok(());
                }
                delay = 250;
                last_error.clear();
                loop {
                    line.clear();
                    if reader.read_line(&mut line)? == 0 {
                        return Err(io::Error::other("Niri disconnected"));
                    }
                    match serde_json::from_str(&line) {
                        Ok(event) => {
                            if event_tx.send(Message::Event(event)).is_err() {
                                return Ok(());
                            }
                        }
                        Err(e) => eprintln!("Ignoring malformed Niri event: {e}"),
                    }
                }
            })();
            if let Err(error) = result {
                let error = error.to_string();
                if error != last_error {
                    eprintln!("{error}");
                    last_error = error;
                }
            } else {
                return;
            }
            if event_tx.send(Message::Connection(false)).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(delay));
            delay = (delay * 2).min(5000);
        }
    });
    thread::spawn(move || {
        loop {
            if let Ok(response) = request(&json!("Outputs"))
                && let Some(outputs) = response.get("Outputs")
            {
                match serde_json::from_value(outputs.clone()) {
                    Ok(outputs) => {
                        if tx.send(Message::Outputs(outputs)).is_err() {
                            return;
                        }
                    }
                    Err(e) => eprintln!("Invalid Niri outputs: {e}"),
                }
            }
            match refresh_rx.recv_timeout(Duration::from_secs(2)) {
                Err(RecvTimeoutError::Disconnected) => return,
                _ => while refresh_rx.try_recv().is_ok() {},
            }
        }
    });
    (rx, refresh_tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn direct_socket_request_checks_reply() {
        let path = env::temp_dir().join(format!("astal-ipc-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap(),
                json!("Outputs")
            );
            writeln!(stream, "{}", json!({"Ok":{"Outputs":{}}})).unwrap();
        });
        assert_eq!(
            request_at(&path, &json!("Outputs")).unwrap(),
            json!({"Outputs":{}})
        );
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn event_stream_handshake_times_out_when_server_does_not_reply() {
        let path = env::temp_dir().join(format!("astal-ipc-stall-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.trim(), "\"EventStream\"");
            line.clear();
            assert_eq!(reader.read_line(&mut line).unwrap(), 0);
        });
        let started = std::time::Instant::now();
        let error = event_stream_at(&path, Duration::from_millis(100)).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn event_stream_preserves_initial_events_and_allows_idle_connections() {
        let path = env::temp_dir().join(format!("astal-ipc-events-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let event = json!({"OverviewOpenedOrClosed":{"is_open":true}});
        let sent = event.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            write!(stream, "{}\n{sent}\n", json!({"Ok":"Handled"})).unwrap();
        });
        let mut reader = event_stream_at(&path, Duration::from_secs(2)).unwrap();
        assert_eq!(reader.get_ref().read_timeout().unwrap(), None);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), event);
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
