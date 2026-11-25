//! Project-only configuration mapping. Native discovery and trust policies still apply.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item, Table};

mod mapping;
mod writes;
pub use writes::{apply, history, restore, ConfigChange};

const SOURCE: &str = ".agents/mcp_config.json";
const TARGET: &str = ".codex/config.toml";
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfigPlan {
    pub plan_id: String,
    pub root: PathBuf,
    pub source: String,
    pub target: String,
    pub changes: Vec<String>,
    pub conflicts: Vec<String>,
    pub warnings: Vec<String>,
    pub shared_instructions: Vec<String>,
    pub shared_skills: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub native_only_servers: Vec<String>,
    pub replace: Vec<String>,
    pub status: String,
    pub verification: String,
}

struct Prepared {
    plan: ProjectConfigPlan,
    original: Option<String>,
    desired: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpDocument {
    #[serde(rename = "mcpServers", deserialize_with = "unique_servers")]
    servers: serde_json::Map<String, Value>,
}

fn unique_servers<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<serde_json::Map<String, Value>, D::Error> {
    struct Servers;
    impl<'de> serde::de::Visitor<'de> for Servers {
        type Value = serde_json::Map<String, Value>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a map of unique MCP server names")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut values = serde_json::Map::new();
            while let Some((key, value)) = map.next_entry::<String, Value>()? {
                if values.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("Duplicate MCP server name"));
                }
            }
            Ok(values)
        }
    }
    deserializer.deserialize_map(Servers)
}

/// Read-only: does not start providers, create backups, or change project files.
pub fn plan(root: &Path, replace: &[String]) -> Result<ProjectConfigPlan> {
    Ok(prepare(root, replace)?.plan)
}

fn prepare(root: &Path, replace: &[String]) -> Result<Prepared> {
    let root = root
        .canonicalize()
        .context("Project directory unavailable")?;
    ensure!(root.is_dir(), "Project path must be a directory");
    safe_target(&root)?;
    let source = read_optional(&root.join(SOURCE))?;
    let original = read_optional(&root.join(TARGET))?;
    let mut document = original
        .as_deref()
        .unwrap_or("")
        .parse::<DocumentMut>()
        .map_err(|_| {
            anyhow::anyhow!("Invalid TOML in .codex/config.toml; inspect the file locally")
        })?;
    if document.contains_key("mcp_servers") {
        ensure!(
            document["mcp_servers"].is_table(),
            "Codex mcp_servers must be a TOML table"
        );
    }
    let native_values = original
        .as_deref()
        .unwrap_or("")
        .parse::<toml::Value>()
        .map_err(|_| anyhow::anyhow!("Invalid Codex project TOML"))?;
    let mut replacements = replace.iter().cloned().collect::<BTreeSet<_>>();
    ensure!(
        replacements.len() == replace.len(),
        "Duplicate replacement name"
    );
    let mut changes = vec![];
    let mut conflicts = vec![];
    let mut warnings = vec!["Project-root inventory only; native/global instructions, trust, managed policies and skill enablement still apply. File presence is not execution verification.".into()];
    warnings.push("Codex can ignore project MCP configuration in untrusted projects. Open native Codex in the project and review its trust decision; Baize never grants trust.".into());
    let mut names = BTreeSet::new();
    if let Some(source) = &source {
        let value: McpDocument = serde_json::from_str(source).map_err(|error| anyhow::anyhow!("Invalid/unsupported MCP JSON or duplicate server name at line {}, column {}; inspect the source locally",error.line(),error.column()))?;
        let servers = &value.servers;
        ensure!(
            servers.len() <= 128,
            "At most 128 project MCP servers are supported"
        );
        for (name, config) in servers {
            mapping::validate_name(name)?;
            names.insert(name.clone());
            let replace_authorized = replacements.remove(name);
            let mapped = match mapping::codex_server(config, &root) {
                Ok(value) => value,
                Err(error) => {
                    conflicts.push(format!("{name}: {error}"));
                    continue;
                }
            };
            let native = native_values
                .get("mcp_servers")
                .and_then(|table| table.get(name));
            let mapped = match mapping::with_native_policies(mapped, native, &root) {
                Ok(value) => value,
                Err(error) => {
                    conflicts.push(format!("{name}: {error}"));
                    continue;
                }
            };
            let existing = document
                .get("mcp_servers")
                .and_then(Item::as_table)
                .and_then(|table| table.get(name));
            let same = native.is_some_and(|value| value == &mapped);
            if same {
                continue;
            }
            if existing.is_some() && !replace_authorized {
                conflicts.push(format!("{name}: existing Codex definition differs; preview with --replace {name} to authorize this exact replacement"));
                continue;
            }
            let entry = toml::to_string(&mapped)?
                .parse::<DocumentMut>()?
                .into_table();
            if !document.contains_key("mcp_servers") {
                document["mcp_servers"] = Item::Table(Table::new());
            }
            document["mcp_servers"][name] = Item::Table(entry);
            let fields = mapped
                .as_table()
                .context("Invalid mapped MCP table")?
                .iter()
                .filter(|(key, value)| native.and_then(|old| old.get(key.as_str())) != Some(*value))
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            changes.push(format!("Map {name}: update {fields}; retain native tool policies. Review values in {SOURCE}; values are omitted from this report."));
        }
    } else {
        warnings.push(format!("No {SOURCE}; no MCP mapping will be written"));
    }
    ensure!(
        replacements.is_empty(),
        "Replacement names must refer to a shared MCP server"
    );
    let native_only_servers = document
        .get("mcp_servers")
        .and_then(Item::as_table)
        .map(|table| {
            table
                .iter()
                .filter(|(name, _)| !names.contains(*name))
                .map(|(name, _)| name.to_string())
                .collect()
        })
        .unwrap_or_default();
    let shared_instructions = ["AGENTS.md"]
        .iter()
        .filter_map(|name| match read_optional(&root.join(name)) {
            Ok(Some(_)) => Some(Ok(name.to_string())),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>>>()?;
    if root.join("AGENTS.override.md").exists() {
        warnings.push("Codex AGENTS.override.md can mask the shared AGENTS.md; review native instruction precedence".into());
    }
    let mut shared_skills = vec![];
    let skills = root.join(".agents/skills");
    if skills.exists() {
        for entry in fs::read_dir(&skills).context("Cannot inspect .agents/skills")? {
            let entry = entry?;
            ensure!(
                shared_skills.len() < 256,
                "At most 256 project skills are supported"
            );
            if entry.path().is_dir() {
                if read_optional(&entry.path().join("SKILL.md"))?.is_some() {
                    shared_skills.push(format!(
                        ".agents/skills/{}/SKILL.md",
                        entry.file_name().to_string_lossy()
                    ));
                } else {
                    warnings.push(format!(
                        "Skill directory {} has no SKILL.md",
                        entry.file_name().to_string_lossy()
                    ));
                }
            }
        }
    }
    shared_skills.sort();
    let mut replace = replace.to_vec();
    replace.sort();
    let identity = serde_json::to_vec(&(
        &root,
        &source,
        &original,
        &replace,
        file_mode(&root.join(TARGET))?,
    ))?;
    let plan = ProjectConfigPlan {
        plan_id: digest(&identity),
        root,
        source: SOURCE.into(),
        target: TARGET.into(),
        changes,
        conflicts,
        warnings,
        shared_instructions,
        shared_skills,
        mcp_servers: names.into_iter().collect(),
        native_only_servers,
        replace,
        status: String::new(),
        verification: "configured_only_not_runtime_verified".into(),
    };
    let desired = if plan.changes.is_empty() {
        original.clone().unwrap_or_default()
    } else {
        document.to_string()
    };
    ensure!(
        desired.len() as u64 <= MAX_BYTES,
        "Mapped configuration exceeds the size limit"
    );
    let mut plan = plan;
    plan.status = if !plan.conflicts.is_empty() {
        "blocked"
    } else if plan.changes.is_empty() {
        "in_sync"
    } else {
        "ready"
    }
    .into();
    Ok(Prepared {
        plan,
        original,
        desired,
    })
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    read_with_limit(path, MAX_BYTES)
}

fn read_with_limit(path: &Path, limit: u64) -> Result<Option<String>> {
    match fs::metadata(path) {
        Ok(metadata) => ensure!(
            metadata.is_file(),
            "Project configuration must be a regular file"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Cannot inspect project configuration"),
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Cannot read project configuration"),
    };
    ensure!(
        file.metadata()?.is_file(),
        "Project configuration must be a regular file"
    );
    let mut text = String::new();
    (&mut file)
        .take(limit + 1)
        .read_to_string(&mut text)
        .context("Project configuration must be readable UTF-8")?;
    ensure!(
        text.len() as u64 <= limit,
        "Project configuration exceeds the size limit"
    );
    Ok(Some(text))
}

fn safe_target(root: &Path) -> Result<()> {
    for name in [".codex", TARGET, ".baize", ".baize/project-config.lock"] {
        match fs::symlink_metadata(root.join(name)) {
            Ok(metadata) => ensure!(
                !metadata.file_type().is_symlink(),
                "Refusing a symlinked configuration target or control directory: {name}"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Cannot inspect configuration target"),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn file_mode(path: &Path) -> Result<Option<u32>> {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.permissions().mode() & 0o777)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
#[cfg(not(unix))]
fn file_mode(_: &Path) -> Result<Option<u32>> {
    Ok(None)
}

#[cfg(test)]
mod tests;
