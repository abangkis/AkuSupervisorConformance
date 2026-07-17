use std::path::Path;

use super::{
    CommandSpec, RuntimePlan, ServiceLaunch, capture_version, os_argument, parse_version_after,
    resolve_executable,
};
use crate::manifest::LoadedFixture;

pub fn create_plan(
    fixture: &LoadedFixture,
    explicit_runtime: Option<&Path>,
    service_port: u16,
) -> Result<RuntimePlan, String> {
    let runtime_path =
        resolve_executable(explicit_runtime, &fixture.manifest.runtime.executable_hint)?;
    let runtime_version = capture_version(&runtime_path, &["--version"])?;
    let normalized_version = parse_version_after(&runtime_version, "node")?;
    let entrypoint = fixture.root.join(&fixture.manifest.service.entrypoint);
    let deterministic = CommandSpec {
        id: "node_test",
        program: runtime_path.clone(),
        arguments: vec![os_argument(fixture.root.join("test/application.test.mjs"))],
        working_directory: fixture.root.clone(),
    };
    Ok(RuntimePlan {
        runtime_path: runtime_path.clone(),
        runtime_version,
        normalized_version,
        deterministic,
        prepare: Vec::new(),
        service: ServiceLaunch {
            command: runtime_path,
            arguments: vec![
                entrypoint.display().to_string(),
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
