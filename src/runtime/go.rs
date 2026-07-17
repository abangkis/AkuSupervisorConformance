use std::fs;
use std::path::Path;

use super::{
    CommandSpec, RuntimePlan, ServiceLaunch, capture_version, os_argument, resolve_executable,
};
use crate::manifest::LoadedFixture;

pub fn create_plan(
    fixture: &LoadedFixture,
    explicit_runtime: Option<&Path>,
    run_directory: &Path,
    service_port: u16,
) -> Result<RuntimePlan, String> {
    let runtime_path =
        resolve_executable(explicit_runtime, &fixture.manifest.runtime.executable_hint)?;
    let runtime_version = capture_version(&runtime_path, &["version"])?;
    let normalized_version = runtime_version
        .split_whitespace()
        .find_map(|word| {
            word.strip_prefix("go")
                .filter(|version| version.starts_with(|character: char| character.is_ascii_digit()))
        })
        .ok_or_else(|| format!("cannot parse Go runtime version from {runtime_version:?}"))?
        .to_owned();
    let binary_directory = run_directory.join("bin");
    fs::create_dir_all(&binary_directory)
        .map_err(|error| format!("create {}: {error}", binary_directory.display()))?;
    let executable = binary_directory.join(if cfg!(windows) {
        "go-application-owned.exe"
    } else {
        "go-application-owned"
    });
    Ok(RuntimePlan {
        runtime_path: runtime_path.clone(),
        runtime_version,
        normalized_version,
        deterministic: CommandSpec {
            id: "go_test",
            program: runtime_path.clone(),
            arguments: vec![os_argument("test"), os_argument("./...")],
            working_directory: fixture.root.clone(),
        },
        prepare: vec![CommandSpec {
            id: "go_build",
            program: runtime_path,
            arguments: vec![
                os_argument("build"),
                os_argument("-o"),
                os_argument(&executable),
                os_argument("./cmd/server"),
            ],
            working_directory: fixture.root.clone(),
        }],
        service: ServiceLaunch {
            command: executable,
            arguments: vec![
                "--host".to_owned(),
                "127.0.0.1".to_owned(),
                "--port".to_owned(),
                service_port.to_string(),
                "--shutdown-ms".to_owned(),
                fixture.manifest.service.application_shutdown_ms.to_string(),
            ],
        },
    })
}
