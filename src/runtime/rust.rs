use std::path::Path;

use super::{
    CommandSpec, RuntimePlan, ServiceLaunch, capture_version, os_argument, parse_version_after,
    resolve_executable,
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
    let runtime_version = capture_version(&runtime_path, &["--version"])?;
    let normalized_version = parse_version_after(&runtime_version, "cargo")?;
    let manifest_path = fixture.root.join("Cargo.toml");
    let target_directory = run_directory.join("rust-target");
    let executable = target_directory.join("debug").join(if cfg!(windows) {
        "rust-application-owned.exe"
    } else {
        "rust-application-owned"
    });
    let common_arguments = vec![
        os_argument("--manifest-path"),
        os_argument(&manifest_path),
        os_argument("--target-dir"),
        os_argument(&target_directory),
    ];
    let mut test_arguments = vec![os_argument("test")];
    test_arguments.extend(common_arguments.clone());
    let mut build_arguments = vec![os_argument("build")];
    build_arguments.extend(common_arguments);
    Ok(RuntimePlan {
        runtime_path: runtime_path.clone(),
        runtime_version,
        normalized_version,
        deterministic: CommandSpec {
            id: "rust_test",
            program: runtime_path.clone(),
            arguments: test_arguments,
            working_directory: fixture.root.clone(),
        },
        prepare: vec![CommandSpec {
            id: "rust_build",
            program: runtime_path,
            arguments: build_arguments,
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
