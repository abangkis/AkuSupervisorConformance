mod manifest;
mod native_path;
mod runner;
mod runtime;

use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use runner::RunOptions;

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "sentinel")
    {
        loop {
            thread::sleep(Duration::from_mins(1));
        }
    }
    match parse(arguments).and_then(runner::run) {
        Ok(exit_code) => ExitCode::from(exit_code),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse(mut arguments: Vec<String>) -> Result<RunOptions, String> {
    if arguments.first().is_some_and(|argument| argument == "run") {
        arguments.remove(0);
    }
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        println!(
            "aku-supervisor-conformance run --supervisor <path> --runtime <node|go|rust> \
             [--runtime-path <path>] [--report <path>] [--project-root <path>]"
        );
        return Ok(RunOptions::help());
    }
    let mut supervisor = None;
    let mut runtime = None;
    let mut runtime_path = None;
    let mut report = None;
    let mut project_root = None;
    let mut index = 0;
    while index < arguments.len() {
        let option = &arguments[index];
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {option}"))?;
        match option.as_str() {
            "--supervisor" => supervisor = Some(PathBuf::from(value)),
            "--runtime" => runtime = Some(value.to_owned()),
            "--runtime-path" => runtime_path = Some(PathBuf::from(value)),
            "--report" => report = Some(PathBuf::from(value)),
            "--project-root" => project_root = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option {option}")),
        }
        index += 2;
    }
    let runtime = runtime.ok_or_else(|| "--runtime is required".to_owned())?;
    if !matches!(runtime.as_str(), "node" | "go" | "rust") {
        return Err(format!("unsupported runtime {runtime}"));
    }
    Ok(RunOptions {
        supervisor: supervisor.ok_or_else(|| "--supervisor is required".to_owned())?,
        runtime,
        runtime_path,
        report,
        project_root: project_root.unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
        help: false,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::parse;

    #[test]
    fn run_cli_keeps_runtime_and_binary_paths_explicit() {
        let options = parse(vec![
            "run".to_owned(),
            "--supervisor".to_owned(),
            "C:\\bin\\aku-supervisor.exe".to_owned(),
            "--runtime".to_owned(),
            "go".to_owned(),
            "--runtime-path".to_owned(),
            "C:\\Go\\bin\\go.exe".to_owned(),
        ])
        .expect("valid command");
        assert_eq!(options.runtime, "go");
        assert!(options.runtime_path.is_some());
    }

    #[test]
    fn powershell_remains_a_thin_rust_launcher() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let launcher = fs::read_to_string(root.join("scripts/conformance.ps1"))
            .expect("read Windows launcher");
        assert!(launcher.contains("CargoPath build"));
        assert!(launcher.contains("aku-supervisor-conformance.exe"));
        for forbidden in [
            "function Add-Check",
            "function Write-Report",
            "ConvertTo-Json",
        ] {
            assert!(
                !launcher.contains(forbidden),
                "PowerShell launcher contains conformance-engine responsibility: {forbidden}"
            );
        }

        let schema: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(root.join("schemas/conformance-report.schema.json"))
                .expect("read report schema"),
        )
        .expect("report schema is valid JSON");
        assert_eq!(
            schema["properties"]["host"]["properties"]["runner"]["const"],
            "rust"
        );
    }
}
