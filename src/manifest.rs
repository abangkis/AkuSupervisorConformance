use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FixtureManifest {
    #[serde(rename = "$schema")]
    pub _schema: Option<String>,
    pub schema_version: u32,
    pub id: String,
    pub contract_version: u32,
    pub runtime: RuntimeManifest,
    pub supported_platforms: Vec<String>,
    pub service: ServiceManifest,
    pub expected_evidence: ExpectedEvidence,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeManifest {
    pub id: String,
    pub minimum_version: String,
    pub executable_hint: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceManifest {
    pub entrypoint: String,
    pub health_path: String,
    pub application_shutdown_ms: u64,
    pub supervisor_shutdown_grace_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpectedEvidence {
    pub windows_signal: String,
    pub application_events: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedFixture {
    pub path: PathBuf,
    pub root: PathBuf,
    pub manifest: FixtureManifest,
}

pub fn load_for_runtime(project_root: &Path, runtime_id: &str) -> Result<LoadedFixture, String> {
    let fixtures_root = project_root.join("fixtures");
    let entries = fs::read_dir(&fixtures_root)
        .map_err(|error| format!("read {}: {error}", fixtures_root.display()))?;
    let mut matches = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read fixture entry: {error}"))?;
        let root = entry.path();
        if !root.is_dir() {
            continue;
        }
        let path = root.join("fixture.json");
        if !path.is_file() {
            continue;
        }
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let manifest: FixtureManifest = serde_json::from_str(&source)
            .map_err(|error| format!("parse {}: {error}", path.display()))?;
        validate(&manifest, &path)?;
        if manifest.runtime.id == runtime_id {
            matches.push(LoadedFixture {
                path,
                root,
                manifest,
            });
        }
    }
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(format!("no fixture declares runtime {runtime_id}")),
        count => Err(format!("{count} fixtures declare runtime {runtime_id}")),
    }
}

fn validate(manifest: &FixtureManifest, path: &Path) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err(format!(
            "{} uses unsupported schemaVersion {}",
            path.display(),
            manifest.schema_version
        ));
    }
    if manifest.id.is_empty()
        || !manifest
            .id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(format!("{} has an invalid fixture id", path.display()));
    }
    if manifest.contract_version == 0 {
        return Err(format!("{} has contractVersion 0", path.display()));
    }
    if manifest.runtime.id.is_empty()
        || manifest.runtime.minimum_version.is_empty()
        || manifest.runtime.executable_hint.is_empty()
    {
        return Err(format!(
            "{} has an incomplete runtime contract",
            path.display()
        ));
    }
    if manifest.supported_platforms.is_empty()
        || manifest.service.health_path.is_empty()
        || !manifest.service.health_path.starts_with('/')
        || manifest.service.application_shutdown_ms == 0
        || manifest.service.supervisor_shutdown_grace_ms <= manifest.service.application_shutdown_ms
        || manifest.expected_evidence.windows_signal.is_empty()
        || manifest.expected_evidence.application_events.is_empty()
    {
        return Err(format!(
            "{} has an invalid service/evidence contract",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::load_for_runtime;

    #[test]
    fn every_maintained_runtime_has_one_strict_manifest() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for runtime in ["node", "go", "rust"] {
            let fixture = load_for_runtime(root, runtime).expect("load runtime fixture");
            assert_eq!(fixture.manifest.runtime.id, runtime);
        }
    }
}
