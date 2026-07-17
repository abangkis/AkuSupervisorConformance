use std::process::ExitCode;
use std::time::Duration;

use rust_application_owned::Application;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let arguments = Arguments::parse()?;
    let (application, _) = Application::start(&arguments.host, arguments.port)?;
    let signal = native_signal::wait()?;
    application.shutdown(signal, Duration::from_millis(arguments.shutdown_ms))
}

#[derive(Debug)]
struct Arguments {
    host: String,
    port: u16,
    shutdown_ms: u64,
}

impl Arguments {
    fn parse() -> Result<Self, String> {
        let mut host = "127.0.0.1".to_owned();
        let mut port = 8093;
        let mut shutdown_ms = 4_000;
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| format!("missing value for {argument}"))?;
            match argument.as_str() {
                "--host" => host = value,
                "--port" => port = value.parse().map_err(|_| "invalid --port".to_owned())?,
                "--shutdown-ms" => {
                    shutdown_ms = value
                        .parse()
                        .map_err(|_| "invalid --shutdown-ms".to_owned())?;
                }
                _ => return Err(format!("unknown argument {argument}")),
            }
        }
        if port == 0 || shutdown_ms == 0 {
            return Err("port and shutdown deadline must be non-zero".to_owned());
        }
        Ok(Self {
            host,
            port,
            shutdown_ms,
        })
    }
}

#[cfg(windows)]
mod native_signal {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread;
    use std::time::Duration;

    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    static RECEIVED: AtomicU32 = AtomicU32::new(u32::MAX);

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    unsafe extern "system" fn handler(code: u32) -> i32 {
        if matches!(code, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
            RECEIVED.store(code, Ordering::Release);
            1
        } else {
            0
        }
    }

    pub fn wait() -> Result<&'static str, String> {
        let installed = unsafe { SetConsoleCtrlHandler(Some(handler), 1) };
        if installed == 0 {
            return Err(format!(
                "SetConsoleCtrlHandler failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        loop {
            match RECEIVED.load(Ordering::Acquire) {
                CTRL_C_EVENT => return Ok("CTRL_C_EVENT"),
                CTRL_BREAK_EVENT => return Ok("CTRL_BREAK_EVENT"),
                _ => thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

#[cfg(not(windows))]
mod native_signal {
    pub fn wait() -> Result<&'static str, String> {
        Err("the Rust fixture currently certifies Windows only".to_owned())
    }
}
