use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub event: String,
    pub signal: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    pub ready: bool,
    pub active_requests: usize,
    pub cleanup_runs: usize,
    pub worker_active: bool,
    pub listening: bool,
}

#[derive(Debug, Clone)]
enum ShutdownState {
    Running,
    InProgress,
    Complete(Result<(), String>),
}

#[derive(Debug)]
struct Shared {
    ready: AtomicBool,
    stopping: AtomicBool,
    listening: AtomicBool,
    worker_active: AtomicBool,
    active_requests: AtomicUsize,
    cleanup_runs: AtomicUsize,
    handlers: Mutex<Vec<JoinHandle<()>>>,
    events: Mutex<Vec<Event>>,
}

#[derive(Debug)]
pub struct Application {
    shared: Arc<Shared>,
    accept_thread: Mutex<Option<JoinHandle<()>>>,
    worker_thread: Mutex<Option<JoinHandle<()>>>,
    shutdown: (Mutex<ShutdownState>, Condvar),
}

impl Application {
    pub fn start(host: &str, port: u16) -> Result<(Arc<Self>, SocketAddr), String> {
        let listener = TcpListener::bind((host, port)).map_err(|error| error.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        let shared = Arc::new(Shared {
            ready: AtomicBool::new(true),
            stopping: AtomicBool::new(false),
            listening: AtomicBool::new(true),
            worker_active: AtomicBool::new(true),
            active_requests: AtomicUsize::new(0),
            cleanup_runs: AtomicUsize::new(0),
            handlers: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
        });
        let worker_shared = Arc::clone(&shared);
        let worker_thread = thread::spawn(move || {
            while !worker_shared.stopping.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(20));
            }
            worker_shared.worker_active.store(false, Ordering::Release);
        });
        let accept_shared = Arc::clone(&shared);
        let accept_thread = thread::spawn(move || {
            while !accept_shared.stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let handler_shared = Arc::clone(&accept_shared);
                        let handler =
                            thread::spawn(move || handle_connection(stream, &handler_shared));
                        accept_shared
                            .handlers
                            .lock()
                            .expect("handler registry poisoned")
                            .push(handler);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            accept_shared.listening.store(false, Ordering::Release);
        });
        let application = Arc::new(Self {
            shared,
            accept_thread: Mutex::new(Some(accept_thread)),
            worker_thread: Mutex::new(Some(worker_thread)),
            shutdown: (Mutex::new(ShutdownState::Running), Condvar::new()),
        });
        application.record("server_ready", None);
        Ok((application, address))
    }

    pub fn shutdown(&self, signal: &str, timeout: Duration) -> Result<(), String> {
        let (state_lock, state_changed) = &self.shutdown;
        let mut state = state_lock
            .lock()
            .map_err(|_| "shutdown lock poisoned".to_owned())?;
        loop {
            match &*state {
                ShutdownState::Running => {
                    *state = ShutdownState::InProgress;
                    break;
                }
                ShutdownState::InProgress => {
                    state = state_changed
                        .wait(state)
                        .map_err(|_| "shutdown wait poisoned".to_owned())?;
                }
                ShutdownState::Complete(result) => return result.clone(),
            }
        }
        drop(state);

        let result = self.perform_shutdown(signal, timeout);
        let mut state = state_lock
            .lock()
            .map_err(|_| "shutdown lock poisoned".to_owned())?;
        *state = ShutdownState::Complete(result.clone());
        state_changed.notify_all();
        result
    }

    pub fn metrics(&self) -> Metrics {
        Metrics {
            ready: self.shared.ready.load(Ordering::Acquire),
            active_requests: self.shared.active_requests.load(Ordering::Acquire),
            cleanup_runs: self.shared.cleanup_runs.load(Ordering::Acquire),
            worker_active: self.shared.worker_active.load(Ordering::Acquire),
            listening: self.shared.listening.load(Ordering::Acquire),
        }
    }

    pub fn events(&self) -> Vec<Event> {
        self.shared
            .events
            .lock()
            .expect("event log poisoned")
            .clone()
    }

    fn perform_shutdown(&self, signal: &str, timeout: Duration) -> Result<(), String> {
        let started = Instant::now();
        self.shared.ready.store(false, Ordering::Release);
        self.record("shutdown_started", Some(signal));
        self.shared.stopping.store(true, Ordering::Release);

        if let Some(thread) = self
            .accept_thread
            .lock()
            .map_err(|_| "accept thread lock poisoned".to_owned())?
            .take()
        {
            thread
                .join()
                .map_err(|_| "accept thread panicked".to_owned())?;
        }
        let handlers = self
            .shared
            .handlers
            .lock()
            .map_err(|_| "handler registry poisoned".to_owned())?
            .drain(..)
            .collect::<Vec<_>>();
        for handler in handlers {
            handler
                .join()
                .map_err(|_| "request handler panicked".to_owned())?;
        }
        if let Some(thread) = self
            .worker_thread
            .lock()
            .map_err(|_| "worker thread lock poisoned".to_owned())?
            .take()
        {
            thread
                .join()
                .map_err(|_| "worker thread panicked".to_owned())?;
        }
        if started.elapsed() > timeout {
            self.record("shutdown_failed", Some(signal));
            return Err("application shutdown deadline exceeded".to_owned());
        }
        self.shared.cleanup_runs.fetch_add(1, Ordering::AcqRel);
        self.record("resource_cleanup_completed", Some(signal));
        self.record("shutdown_completed", Some(signal));
        Ok(())
    }

    fn record(&self, event: &str, signal: Option<&str>) {
        self.shared
            .events
            .lock()
            .expect("event log poisoned")
            .push(Event {
                event: event.to_owned(),
                signal: signal.map(str::to_owned),
            });
        match signal {
            Some(signal) => println!(r#"{{"event":"{event}","signal":"{signal}"}}"#),
            None => println!(r#"{{"event":"{event}"}}"#),
        }
    }
}

fn handle_connection(mut stream: TcpStream, shared: &Shared) {
    shared.active_requests.fetch_add(1, Ordering::AcqRel);
    let _guard = ActiveRequestGuard(shared);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut bytes = [0_u8; 4096];
    let count = stream.read(&mut bytes).unwrap_or(0);
    let request = String::from_utf8_lossy(&bytes[..count]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let (status, body) = if target.starts_with("/health") {
        if shared.ready.load(Ordering::Acquire) {
            ("200 OK", r#"{"status":"ready"}"#)
        } else {
            ("503 Service Unavailable", r#"{"status":"stopping"}"#)
        }
    } else if target.starts_with("/hold") {
        let milliseconds = target
            .split_once("ms=")
            .and_then(|(_, value)| value.split('&').next())
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (1..=2000).contains(value))
            .unwrap_or(150);
        thread::sleep(Duration::from_millis(milliseconds));
        ("200 OK", "held")
    } else {
        ("200 OK", "ok")
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

struct ActiveRequestGuard<'a>(&'a Shared);

impl Drop for ActiveRequestGuard<'_> {
    fn drop(&mut self) {
        self.0.active_requests.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::Application;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn shutdown_is_idempotent_drains_request_and_releases_resources() {
        let (application, address) = Application::start("127.0.0.1", 0).expect("start fixture");
        assert!(request(address, "/health").contains("ready"));
        let held = thread::spawn(move || request(address, "/hold?ms=150"));
        let deadline = Instant::now() + Duration::from_secs(2);
        while application.metrics().active_requests != 1 {
            assert!(
                Instant::now() < deadline,
                "hold request never became active"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let first = Arc::clone(&application);
        let second = Arc::clone(&application);
        let first_stop = thread::spawn(move || first.shutdown("TEST", Duration::from_secs(1)));
        let second_stop = thread::spawn(move || second.shutdown("TEST", Duration::from_secs(1)));
        assert_eq!(held.join().expect("hold thread"), "held");
        first_stop
            .join()
            .expect("first stop")
            .expect("first shutdown");
        second_stop
            .join()
            .expect("second stop")
            .expect("second shutdown");
        assert_eq!(
            application.metrics(),
            super::Metrics {
                ready: false,
                active_requests: 0,
                cleanup_runs: 1,
                worker_active: false,
                listening: false,
            }
        );
        assert!(TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err());
        assert_eq!(
            application
                .events()
                .into_iter()
                .map(|event| event.event)
                .collect::<Vec<_>>(),
            [
                "server_ready",
                "shutdown_started",
                "resource_cleanup_completed",
                "shutdown_completed",
            ]
        );
    }

    fn request(address: std::net::SocketAddr, path: &str) -> String {
        let mut stream = TcpStream::connect(address).expect("connect fixture");
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        )
        .expect("write request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or(&response)
            .to_owned()
    }
}
