use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::manifest::LoadedFixture;
use crate::native_path;

mod go;
mod node;
mod rust;

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub id: &'static str,
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ServiceLaunch {
    pub command: PathBuf,
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimePlan {
    pub runtime_path: PathBuf,
    pub runtime_version: String,
    pub normalized_version: String,
    pub deterministic: CommandSpec,
    pub prepare: Vec<CommandSpec>,
    pub service: ServiceLaunch,
}

pub fn create_plan(
    runtime_id: &str,
    fixture: &LoadedFixture,
    explicit_runtime: Option<&Path>,
    run_directory: &Path,
    service_port: u16,
) -> Result<RuntimePlan, String> {
    match runtime_id {
        "node" => node::create_plan(fixture, explicit_runtime, service_port),
        "go" => go::create_plan(fixture, explicit_runtime, run_directory, service_port),
        "rust" => rust::create_plan(fixture, explicit_runtime, run_directory, service_port),
        other => Err(format!("no Rust runtime adapter is registered for {other}")),
    }
}

pub fn resolve_executable(explicit: Option<&Path>, hint: &str) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        if path.is_file() {
            return path
                .canonicalize()
                .map(native_path::normalize)
                .map_err(|error| format!("resolve {}: {error}", path.display()));
        }
        return Err(format!(
            "runtime executable does not exist: {}",
            path.display()
        ));
    }
    let path = env::var_os("PATH").ok_or_else(|| "PATH is not set".to_owned())?;
    let extensions = if cfg!(windows) {
        env::var_os("PATHEXT")
            .unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"))
            .to_string_lossy()
            .split(';')
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        vec![String::new()]
    };
    for directory in env::split_paths(&path) {
        for extension in &extensions {
            let candidate = if extension.is_empty() {
                directory.join(hint)
            } else {
                directory.join(format!("{hint}{extension}"))
            };
            if candidate.is_file() {
                return candidate
                    .canonicalize()
                    .map(native_path::normalize)
                    .map_err(|error| format!("resolve {}: {error}", candidate.display()));
            }
        }
    }
    Err(format!("runtime executable {hint} was not found on PATH"))
}

pub fn capture_version(program: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("run {}: {error}", program.display()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .trim()
    .to_owned();
    if !output.status.success() {
        return Err(format!(
            "{} version probe failed: {text}",
            program.display()
        ));
    }
    Ok(text)
}

pub fn parse_version_after(text: &str, marker: &str) -> Result<String, String> {
    let remainder = text
        .split_once(marker)
        .map_or(text, |(_, remainder)| remainder)
        .trim_start_matches(['v', ' ']);
    let version = remainder
        .split(|character: char| !(character.is_ascii_digit() || character == '.'))
        .next()
        .unwrap_or_default();
    if version.split('.').count() < 2 || version.is_empty() {
        Err(format!("cannot parse runtime version from {text:?}"))
    } else {
        Ok(version.to_owned())
    }
}

pub fn os_argument(value: impl Into<OsString>) -> OsString {
    value.into()
}
