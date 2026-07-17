use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::manifest::{LoadedFixture, load_for_runtime};
use crate::native_path;
use crate::runtime::{CommandSpec, RuntimePlan, create_plan};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub supervisor: PathBuf,
    pub runtime: String,
    pub runtime_path: Option<PathBuf>,
    pub report: Option<PathBuf>,
    pub project_root: PathBuf,
    pub help: bool,
}

impl RunOptions {
    pub fn help() -> Self {
        Self {
            supervisor: PathBuf::new(),
            runtime: String::new(),
            runtime_path: None,
            report: None,
            project_root: PathBuf::new(),
            help: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Check {
    id: String,
    status: String,
    expected: Value,
    actual: Value,
    detail: String,
}

pub fn run(options: RunOptions) -> Result<u8, String> {
    if options.help {
        return Ok(0);
    }
    let project_root = native_path::normalize(
        options
            .project_root
            .canonicalize()
            .map_err(|error| format!("resolve {}: {error}", options.project_root.display()))?,
    );
    let fixture = load_for_runtime(&project_root, &options.runtime)?;
    let started_at = rfc3339_now();
    let run_id = format!(
        "{}-{}",
        started_at
            .replace(['-', ':'], "")
            .replace('.', "")
            .replace("+00:00", "Z"),
        std::process::id()
    );
    let run_directory = project_root.join(".artifacts/runs").join(&run_id);
    fs::create_dir_all(&run_directory)
        .map_err(|error| format!("create {}: {error}", run_directory.display()))?;
    let report_path = options.report.clone().unwrap_or_else(|| {
        project_root
            .join(".artifacts/reports")
            .join(format!("{run_id}.json"))
    });
    let mut execution = Execution::new(
        options,
        project_root,
        fixture,
        started_at,
        run_id,
        run_directory,
        report_path,
    );
    execution.execute()
}

struct Execution {
    options: RunOptions,
    project_root: PathBuf,
    fixture: LoadedFixture,
    started_at: String,
    run_id: String,
    run_directory: PathBuf,
    report_path: PathBuf,
    config_path: PathBuf,
    checks: Vec<Check>,
    evidence: Map<String, Value>,
    supervisor_path: Option<PathBuf>,
    supervisor_version: Option<String>,
    supervisor_hash: Option<String>,
    runtime_plan: Option<RuntimePlan>,
    service_port: Option<u16>,
    service_started: bool,
    service_stopped: bool,
}

impl Execution {
    #[allow(clippy::too_many_arguments)]
    fn new(
        options: RunOptions,
        project_root: PathBuf,
        fixture: LoadedFixture,
        started_at: String,
        run_id: String,
        run_directory: PathBuf,
        report_path: PathBuf,
    ) -> Self {
        let config_path = run_directory.join("services.json");
        Self {
            options,
            project_root,
            fixture,
            started_at,
            run_id,
            run_directory,
            report_path,
            config_path,
            checks: Vec::new(),
            evidence: Map::new(),
            supervisor_path: None,
            supervisor_version: None,
            supervisor_hash: None,
            runtime_plan: None,
            service_port: None,
            service_started: false,
            service_stopped: false,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn execute(&mut self) -> Result<u8, String> {
        if !cfg!(windows)
            || !self
                .fixture
                .manifest
                .supported_platforms
                .iter()
                .any(|item| item == "windows")
        {
            self.add_check(
                "platform_supported",
                "skipped",
                json!(self.fixture.manifest.supported_platforms),
                json!(std::env::consts::OS),
                "the selected fixture has no native contract for this host",
            );
            self.write_report("skipped", 2)?;
            return Ok(2);
        }
        if !self.options.supervisor.is_file() {
            self.add_check(
                "supervisor_available",
                "failed",
                json!("existing executable"),
                json!(self.options.supervisor),
                "the supplied AkuSupervisor executable does not exist",
            );
            self.write_report("failed", 1)?;
            return Ok(1);
        }
        let supervisor = native_path::normalize(
            self.options
                .supervisor
                .canonicalize()
                .map_err(|error| format!("resolve supervisor: {error}"))?,
        );
        self.supervisor_version = Some(capture_simple(&supervisor, &["--version"])?);
        self.supervisor_hash = Some(sha256_file(&supervisor)?);
        self.supervisor_path = Some(supervisor);

        let control_port = free_port()?;
        let service_port = loop {
            let candidate = free_port()?;
            if candidate != control_port {
                break candidate;
            }
        };
        self.service_port = Some(service_port);
        let plan = match create_plan(
            &self.options.runtime,
            &self.fixture,
            self.options.runtime_path.as_deref(),
            &self.run_directory,
            service_port,
        ) {
            Ok(plan) => plan,
            Err(error) if self.options.runtime_path.is_none() => {
                self.add_check(
                    "runtime_available",
                    "skipped",
                    json!(format!(
                        "{} >= {}",
                        self.fixture.manifest.runtime.id,
                        self.fixture.manifest.runtime.minimum_version
                    )),
                    Value::Null,
                    &error,
                );
                self.write_report("skipped", 2)?;
                return Ok(2);
            }
            Err(error) => {
                self.add_check(
                    "runtime_available",
                    "failed",
                    json!("valid explicit runtime executable"),
                    json!(self.options.runtime_path),
                    &error,
                );
                self.write_report("failed", 1)?;
                return Ok(1);
            }
        };
        let compatible = version_at_least(
            &plan.normalized_version,
            &self.fixture.manifest.runtime.minimum_version,
        )?;
        self.add_check(
            "runtime_version",
            if compatible { "passed" } else { "skipped" },
            json!(format!(
                ">= {}",
                self.fixture.manifest.runtime.minimum_version
            )),
            json!(plan.runtime_version),
            if compatible {
                "the selected runtime satisfies this opt-in fixture"
            } else {
                "the selected runtime is below the fixture minimum"
            },
        );
        self.runtime_plan = Some(plan);
        if !compatible {
            self.write_report("skipped", 2)?;
            return Ok(2);
        }

        let result = self.execute_native(control_port);
        if let Err(error) = result {
            self.add_check(
                "runner_completed",
                "failed",
                json!("completed"),
                json!(error),
                "the Rust native conformance runner encountered an error",
            );
        }
        let failed = self.checks.iter().any(|check| check.status == "failed");
        let (status, exit_code) = if failed { ("failed", 1) } else { ("passed", 0) };
        self.write_report(status, exit_code)?;
        Ok(exit_code)
    }

    #[allow(clippy::too_many_lines)]
    fn execute_native(&mut self, control_port: u16) -> Result<(), String> {
        let plan = self
            .runtime_plan
            .clone()
            .ok_or_else(|| "runtime plan missing".to_owned())?;
        let deterministic = run_command(&plan.deterministic, &self.run_directory)?;
        self.evidence.insert(
            "deterministicTestLog".to_owned(),
            json!(deterministic.log_path),
        );
        self.add_check(
            "deterministic_application_test",
            if deterministic.success { "passed" } else { "failed" },
            json!(0),
            json!(deterministic.exit_code),
            "idempotency, active-request drain, resource cleanup, and listener release were exercised",
        );
        if !deterministic.success {
            return Err("deterministic fixture test failed".to_owned());
        }
        for command in &plan.prepare {
            let output = run_command(command, &self.run_directory)?;
            self.evidence
                .insert(format!("{}Log", command.id), json!(output.log_path));
            self.add_check(
                command.id,
                if output.success { "passed" } else { "failed" },
                json!(0),
                json!(output.exit_code),
                "the runtime-specific direct executable was prepared",
            );
            if !output.success {
                return Err(format!("fixture preparation {} failed", command.id));
            }
        }
        if !plan.service.command.is_file() {
            return Err(format!(
                "prepared service executable is missing: {}",
                plan.service.command.display()
            ));
        }
        self.evidence
            .insert("serviceExecutable".to_owned(), json!(plan.service.command));
        self.write_config(control_port, &plan)?;

        let mut sentinel =
            Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
                .arg("sentinel")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| format!("start unrelated sentinel: {error}"))?;
        self.evidence
            .insert("unrelatedSentinelPid".to_owned(), json!(sentinel.id()));
        let mut supervisor = self.start_supervisor()?;
        self.evidence
            .insert("supervisorPid".to_owned(), json!(supervisor.id()));

        let native_result = self.run_lifecycle(&mut supervisor, &mut sentinel, control_port);
        self.cleanup(&mut supervisor, &mut sentinel);
        native_result
    }

    #[allow(clippy::too_many_lines)]
    fn run_lifecycle(
        &mut self,
        supervisor: &mut Child,
        sentinel: &mut Child,
        control_port: u16,
    ) -> Result<(), String> {
        self.wait_supervisor_ready(supervisor)?;
        self.add_check(
            "isolated_supervisor_ready",
            "passed",
            json!(control_port),
            json!(control_port),
            "isolated control API is ready on a run-specific configuration",
        );
        let service_id = self.fixture.manifest.id.clone();
        let start_reason = format!(
            "AkuSupervisorConformance native {} start {}",
            self.options.runtime, self.run_id
        );
        self.service_started = true;
        let start = self.supervisor_json(&[
            "start",
            &service_id,
            "--actor",
            "codex",
            "--reason",
            &start_reason,
            "--request-id",
            &format!("conformance-start-{}", self.run_id),
        ])?;
        self.evidence
            .insert("startResponse".to_owned(), start["response"].clone());
        let outcome = start["response"]["outcome"].as_str().unwrap_or("missing");
        self.add_check(
            "supervised_start",
            if outcome == "started" {
                "passed"
            } else {
                "failed"
            },
            json!("started"),
            json!(outcome),
            "AkuSupervisor owns the fixture's direct executable",
        );
        let status = self.supervisor_json(&["status"])?;
        let service = status["response"]["services"]
            .as_array()
            .and_then(|services| services.iter().find(|service| service["id"] == service_id))
            .cloned()
            .ok_or_else(|| "fixture service is absent from status".to_owned())?;
        let health = service["health"]["status"].as_str().unwrap_or("missing");
        self.add_check(
            "supervisor_process_health",
            if health == "healthy" {
                "passed"
            } else {
                "failed"
            },
            json!("healthy"),
            json!(health),
            "the shutdown suite isolates generic ownership from HTTP-adapter conformance",
        );
        let app_status = wait_http_status(
            self.service_port
                .ok_or_else(|| "service port missing".to_owned())?,
            &self.fixture.manifest.service.health_path,
            STARTUP_TIMEOUT,
        )?;
        self.add_check(
            "application_health_reached",
            if app_status == 200 {
                "passed"
            } else {
                "failed"
            },
            json!(200),
            json!(app_status),
            "the Rust runner independently reached the fixture readiness endpoint",
        );

        let stop_reason = format!(
            "AkuSupervisorConformance native {} stop {}",
            self.options.runtime, self.run_id
        );
        let stop = self.supervisor_json(&[
            "stop",
            &service_id,
            "--actor",
            "codex",
            "--reason",
            &stop_reason,
            "--request-id",
            &format!("conformance-stop-{}", self.run_id),
        ])?;
        self.service_stopped = true;
        self.evidence
            .insert("stopResponse".to_owned(), stop["response"].clone());
        let shutdown = &stop["response"]["shutdown"];
        let signal_sent = shutdown["gracefulSignalSent"].as_bool() == Some(true);
        self.add_check(
            "graceful_signal_sent",
            pass(signal_sent),
            json!(true),
            shutdown["gracefulSignalSent"].clone(),
            "AkuSupervisor reported targeted native signal delivery",
        );
        let not_forced = shutdown["forced"].as_bool() == Some(false);
        self.add_check(
            "no_forced_fallback",
            pass(not_forced),
            json!(false),
            shutdown["forced"].clone(),
            "the application exited before the bounded fallback deadline",
        );
        let owned_after = shutdown["ownedPidsAfter"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        self.add_check(
            "owned_tree_empty",
            pass(owned_after.is_empty()),
            json!([]),
            Value::Array(owned_after),
            "no managed descendant survived the stop",
        );

        let logs =
            self.supervisor_json(&["logs", &service_id, "--stream", "stdout", "--tail", "200"])?;
        let lines = logs["response"]["log"]["lines"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        self.evidence
            .insert("applicationStdout".to_owned(), Value::Array(lines.clone()));
        let records = lines
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect::<Vec<_>>();
        let observed_events = records
            .iter()
            .filter_map(|record| record["event"].as_str())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let events_complete = self
            .fixture
            .manifest
            .expected_evidence
            .application_events
            .iter()
            .all(|event| observed_events.contains(event));
        self.add_check(
            "application_cleanup_events",
            pass(events_complete),
            json!(self.fixture.manifest.expected_evidence.application_events),
            json!(observed_events),
            "every required application cleanup event must be logged",
        );
        let observed_signal = records
            .iter()
            .rev()
            .find(|record| record["event"] == "shutdown_started")
            .and_then(|record| record["signal"].as_str());
        let expected_signal = self
            .fixture
            .manifest
            .expected_evidence
            .windows_signal
            .clone();
        self.add_check(
            "application_observed_native_signal",
            pass(observed_signal == Some(expected_signal.as_str())),
            json!(expected_signal),
            json!(observed_signal),
            "the application handler recorded the exact Windows signal contract",
        );

        let events = self.supervisor_json(&["events", "--limit", "200"])?;
        let journal_record = events["response"]["events"]
            .as_array()
            .and_then(|events| {
                events.iter().rev().find(|event| {
                    event["serviceId"] == service_id
                        && event["action"] == "stop"
                        && event["reason"] == stop_reason
                })
            })
            .cloned();
        let journal_matches = journal_record.as_ref().is_some_and(|record| {
            record["shutdown"]["gracefulSignalSent"] == true
                && record["shutdown"]["forced"] == false
                && record["shutdown"]["ownedPidsAfter"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
        });
        self.evidence.insert(
            "lifecycleRecord".to_owned(),
            journal_record.clone().unwrap_or(Value::Null),
        );
        self.add_check(
            "lifecycle_journal_matches",
            pass(journal_matches),
            json!("graceful=true, forced=false, ownedPidsAfter=[]"),
            journal_record.map_or(Value::Null, |record| record["shutdown"].clone()),
            "the canonical journal carries identical shutdown evidence",
        );
        thread::sleep(Duration::from_millis(100));
        let released = !tcp_open(
            self.service_port.unwrap_or_default(),
            Duration::from_millis(400),
        );
        self.add_check(
            "listener_port_released",
            pass(released),
            json!("closed"),
            json!(if released { "closed" } else { "open" }),
            "the declared fixture listener is no longer reachable",
        );
        let sentinel_alive = sentinel
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none();
        self.add_check(
            "unrelated_process_preserved",
            pass(sentinel_alive),
            json!("running"),
            json!(if sentinel_alive { "running" } else { "exited" }),
            "a process outside the Supervisor ownership boundary was not affected",
        );
        Ok(())
    }

    fn write_config(&self, control_port: u16, plan: &RuntimePlan) -> Result<(), String> {
        let service_port = self
            .service_port
            .ok_or_else(|| "service port missing".to_owned())?;
        let mut services = BTreeMap::new();
        services.insert(
            self.fixture.manifest.id.clone(),
            json!({
                "label": format!("{} application-owned conformance fixture", self.options.runtime),
                "cwd": self.fixture.root,
                "command": plan.service.command,
                "args": plan.service.arguments,
                "environment": {},
                "health": { "type": "process" },
                "ports": [service_port],
                "restartPolicy": "manual",
                "shutdownGraceMs": self.fixture.manifest.service.supervisor_shutdown_grace_ms
            }),
        );
        let config = json!({
            "version": 1,
            "control": {
                "host": "127.0.0.1",
                "port": control_port,
                "tokenFile": format!(".runtime\\{}\\control-token", self.run_id),
                "mcp": { "enabled": false, "allowedOrigins": [] }
            },
            "observability": { "consoleEvents": "verbose" },
            "services": services
        });
        write_json(&self.config_path, &config)
    }

    fn start_supervisor(&self) -> Result<Child, String> {
        Command::new(
            self.supervisor_path
                .as_ref()
                .ok_or_else(|| "supervisor missing".to_owned())?,
        )
        .arg("run")
        .arg("--config")
        .arg(&self.config_path)
        .current_dir(&self.run_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("start isolated AkuSupervisor: {error}"))
    }

    fn wait_supervisor_ready(&self, child: &mut Child) -> Result<(), String> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                return Err(format!("AkuSupervisor exited before readiness: {status}"));
            }
            if self.supervisor_json(&["status"]).is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(
                    "AkuSupervisor control API did not become ready within 10 seconds".to_owned(),
                );
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn supervisor_json(&self, arguments: &[&str]) -> Result<Value, String> {
        let mut command = Command::new(
            self.supervisor_path
                .as_ref()
                .ok_or_else(|| "supervisor missing".to_owned())?,
        );
        command
            .args(arguments)
            .arg("--json")
            .arg("--config")
            .arg(&self.config_path);
        let output = command
            .output()
            .map_err(|error| format!("run supervisor CLI: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "AkuSupervisor command failed ({}): {}{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        serde_json::from_slice(&output.stdout).map_err(|error| {
            format!(
                "AkuSupervisor returned invalid JSON: {error}: {}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }

    fn cleanup(&mut self, supervisor: &mut Child, sentinel: &mut Child) {
        if self.service_started
            && !self.service_stopped
            && supervisor.try_wait().ok().flatten().is_none()
        {
            let service_id = self.fixture.manifest.id.clone();
            let request_id = format!("conformance-cleanup-{}", self.run_id);
            let _ = self.supervisor_json(&[
                "stop",
                &service_id,
                "--actor",
                "codex",
                "--reason",
                "AkuSupervisorConformance failure cleanup",
                "--request-id",
                &request_id,
            ]);
        }
        if supervisor.try_wait().ok().flatten().is_none() {
            if let Some(stdin) = supervisor.stdin.as_mut() {
                let _ = writeln!(stdin, "quit");
                let _ = stdin.flush();
            }
            wait_or_kill(supervisor, Duration::from_secs(5));
        }
        if sentinel.try_wait().ok().flatten().is_none() {
            let _ = sentinel.kill();
            let _ = sentinel.wait();
        }
    }

    fn add_check(&mut self, id: &str, status: &str, expected: Value, actual: Value, detail: &str) {
        let color = match status {
            "passed" => "\x1b[32m",
            "failed" => "\x1b[31m",
            _ => "\x1b[33m",
        };
        println!("{color}[{status}] {id}: {detail}\x1b[0m");
        self.checks.push(Check {
            id: id.to_owned(),
            status: status.to_owned(),
            expected,
            actual,
            detail: detail.to_owned(),
        });
    }

    fn write_report(&self, status: &str, exit_code: u8) -> Result<(), String> {
        let conformance_version = fs::read_to_string(self.project_root.join("VERSION"))
            .map_err(|error| format!("read VERSION: {error}"))?
            .trim()
            .to_owned();
        let runtime = self.runtime_plan.as_ref();
        let report = json!({
            "schemaVersion": 2,
            "conformanceVersion": conformance_version,
            "runId": self.run_id,
            "startedAtUtc": self.started_at,
            "completedAtUtc": rfc3339_now(),
            "status": status,
            "exitCode": exit_code,
            "supervisor": {
                "path": self.supervisor_path.as_ref().unwrap_or(&self.options.supervisor),
                "version": self.supervisor_version,
                "sha256": self.supervisor_hash
            },
            "host": {
                "os": if cfg!(windows) { "windows" } else { std::env::consts::OS },
                "architecture": std::env::consts::ARCH,
                "runner": "rust"
            },
            "runtime": {
                "id": self.options.runtime,
                "path": runtime.map(|plan| &plan.runtime_path),
                "version": runtime.map(|plan| &plan.runtime_version)
            },
            "fixture": {
                "id": self.fixture.manifest.id,
                "contractVersion": self.fixture.manifest.contract_version,
                "manifestPath": self.fixture.path
            },
            "checks": self.checks,
            "evidence": self.evidence
        });
        write_json(&self.report_path, &report)?;
        println!("Conformance report: {}", self.report_path.display());
        Ok(())
    }
}

#[derive(Debug)]
struct CapturedCommand {
    success: bool,
    exit_code: i32,
    log_path: PathBuf,
}

fn run_command(spec: &CommandSpec, run_directory: &Path) -> Result<CapturedCommand, String> {
    let output = Command::new(&spec.program)
        .args(&spec.arguments)
        .current_dir(&spec.working_directory)
        .output()
        .map_err(|error| format!("run {}: {error}", spec.program.display()))?;
    let log_path = run_directory.join(format!("{}.log", spec.id));
    let mut log = File::create(&log_path)
        .map_err(|error| format!("create {}: {error}", log_path.display()))?;
    log.write_all(&output.stdout)
        .map_err(|error| error.to_string())?;
    log.write_all(&output.stderr)
        .map_err(|error| error.to_string())?;
    Ok(CapturedCommand {
        success: output.status.success(),
        exit_code: output.status.code().unwrap_or(1),
        log_path,
    })
}

fn capture_simple(program: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("run {}: {error}", program.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} failed with {}",
            program.display(),
            output.status
        ));
    }
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .trim()
    .to_owned())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn free_port() -> Result<u16, String> {
    let seed = u16::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
            % 1000,
    )
    .unwrap_or_default();
    for offset in 0..1000_u16 {
        let port = 48_000 + (seed + offset) % 1000;
        if let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            drop(listener);
            return Ok(port);
        }
    }
    Err("no free port in conformance range 48000-48999".to_owned())
}

fn wait_http_status(port: u16, path: &str, timeout: Duration) -> Result<u16, String> {
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();
    while Instant::now() < deadline {
        match http_status(port, path) {
            Ok(status) => return Ok(status),
            Err(error) => last_error = error,
        }
        thread::sleep(POLL_INTERVAL);
    }
    Err(format!("fixture health did not become ready: {last_error}"))
}

fn http_status(port: u16, path: &str) -> Result<u16, String> {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .map_err(|error| error.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| error.to_string())?;
    response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .ok_or_else(|| "invalid HTTP status line".to_owned())
}

fn tcp_open(port: u16, timeout: Duration) -> bool {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    TcpStream::connect_timeout(&address, timeout).is_ok()
}

fn wait_or_kill(child: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| format!("write {}: {error}", path.display()))
}

fn pass(value: bool) -> &'static str {
    if value { "passed" } else { "failed" }
}

fn version_at_least(actual: &str, minimum: &str) -> Result<bool, String> {
    fn parse(value: &str) -> Result<Vec<u64>, String> {
        value
            .split('.')
            .map(|part| {
                part.parse::<u64>()
                    .map_err(|_| format!("invalid version {value}"))
            })
            .collect()
    }
    let mut actual = parse(actual)?;
    let mut minimum = parse(minimum)?;
    let width = actual.len().max(minimum.len());
    actual.resize(width, 0);
    minimum.resize(width, 0);
    Ok(actual >= minimum)
}

fn rfc3339_now() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        duration.subsec_millis()
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, version_at_least};

    #[test]
    fn version_comparison_is_numeric_and_width_independent() {
        assert!(version_at_least("1.21.4", "1.21.0").unwrap());
        assert!(version_at_least("24.16.0", "20.0.0").unwrap());
        assert!(!version_at_least("1.20.9", "1.21.0").unwrap());
    }

    #[test]
    fn utc_day_conversion_keeps_epoch_and_known_date() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_651), (2026, 7, 17));
    }
}
