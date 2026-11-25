use super::*;

pub(super) fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "MCP names must be 1..64 ASCII letters, numbers, underscore or hyphen"
    );
    Ok(())
}

pub(super) fn codex_server(value: &Value, root: &Path) -> Result<toml::Value> {
    let object = value
        .as_object()
        .context("Server definition must be an object")?;
    let allowed = [
        "command",
        "args",
        "env",
        "cwd",
        "serverUrl",
        "url",
        "headers",
        "disabled",
        "enabledTools",
        "disabledTools",
        "timeoutSeconds",
    ];
    ensure!(object.keys().all(|key| allowed.contains(&key.as_str())), "Unsupported server fields; native authentication/per-tool policies are not silently mapped");
    let string = |key: &str| -> Result<Option<String>> {
        object
            .get(key)
            .map(|v| {
                v.as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .context("Expected a nonempty string field")
            })
            .transpose()
    };
    let command = string("command")?;
    let endpoint = string("serverUrl")?.or(string("url")?);
    ensure!(
        !(object.contains_key("serverUrl") && object.contains_key("url")),
        "Specify one endpoint key"
    );
    ensure!(
        command.is_some() ^ endpoint.is_some(),
        "Specify exactly one transport: command or serverUrl/url"
    );
    let mut output = toml::map::Map::new();
    if let Some(command) = command {
        ensure!(
            !object.contains_key("headers"),
            "HTTP headers cannot be attached to a stdio server"
        );
        output.insert("command".into(), toml::Value::String(command));
        output.insert("args".into(), toml::Value::Array(vec![]));
        output.insert("env".into(), toml::Value::Table(toml::map::Map::new()));
        output.insert(
            "cwd".into(),
            toml::Value::String(
                root.join(string("cwd")?.unwrap_or_else(|| ".".into()))
                    .to_string_lossy()
                    .into_owned(),
            ),
        );
        if let Some(args) = object.get("args") {
            output.insert("args".into(), strings(args)?);
        }
        if let Some(env) = object.get("env") {
            output.insert("env".into(), string_map(env)?);
        }
    } else if let Some(endpoint) = endpoint {
        ensure!(
            !["args", "env", "cwd"]
                .iter()
                .any(|key| object.contains_key(*key)),
            "stdio settings cannot be attached to an HTTP server"
        );
        ensure!(
            (endpoint.starts_with("https://") || endpoint.starts_with("http://"))
                && !endpoint.contains('@')
                && !endpoint.chars().any(char::is_whitespace),
            "Expected an HTTP endpoint without embedded credentials"
        );
        output.insert("url".into(), toml::Value::String(endpoint));
        output.insert(
            "http_headers".into(),
            toml::Value::Table(toml::map::Map::new()),
        );
        if let Some(headers) = object.get("headers") {
            output.insert("http_headers".into(), string_map(headers)?);
        }
    }
    if let Some(disabled) = object.get("disabled") {
        output.insert(
            "enabled".into(),
            toml::Value::Boolean(!disabled.as_bool().context("disabled must be boolean")?),
        );
    }
    for (source, target) in [
        ("enabledTools", "enabled_tools"),
        ("disabledTools", "disabled_tools"),
    ] {
        if let Some(value) = object.get(source) {
            let list = strings(value)?;
            ensure!(
                source != "enabledTools" || !list.as_array().is_some_and(Vec::is_empty),
                "Empty enabledTools is ambiguous across providers"
            );
            output.insert(target.into(), list);
        }
    }
    if let Some(seconds) = object.get("timeoutSeconds") {
        let seconds = seconds
            .as_i64()
            .context("timeoutSeconds must be a positive integer")?;
        ensure!((1..=3600).contains(&seconds), "timeoutSeconds must be 1..3600 for the common mapping; native unlimited/default differences are not discarded");
        output.insert("startup_timeout_sec".into(), toml::Value::Integer(seconds));
        output.insert("tool_timeout_sec".into(), toml::Value::Integer(seconds));
    }
    Ok(toml::Value::Table(output))
}

fn strings(value: &Value) -> Result<toml::Value> {
    Ok(toml::Value::Array(
        value
            .as_array()
            .context("Expected a string list")?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| toml::Value::String(s.into()))
                    .context("Expected a string list")
            })
            .collect::<Result<_>>()?,
    ))
}

fn string_map(value: &Value) -> Result<toml::Value> {
    Ok(toml::Value::Table(
        value
            .as_object()
            .context("Expected a string map")?
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.clone(),
                    toml::Value::String(
                        value.as_str().context("Expected string map values")?.into(),
                    ),
                ))
            })
            .collect::<Result<_>>()?,
    ))
}

pub(super) fn with_native_policies(
    mut mapped: toml::Value,
    native: Option<&toml::Value>,
    root: &Path,
) -> Result<toml::Value> {
    let Some(native) = native else {
        return Ok(mapped);
    };
    let original = native
        .as_table()
        .context("Native MCP definition must be a table")?;
    let desired = mapped
        .as_table_mut()
        .context("Mapped MCP definition must be a table")?;
    if desired.iter().all(|(key, value)| {
        original.get(key) == Some(value)
            || (original.get(key).is_none()
                && match key.as_str() {
                    "args" => value.as_array().is_some_and(Vec::is_empty),
                    "env" | "http_headers" => {
                        value.as_table().is_some_and(toml::map::Map::is_empty)
                    }
                    "cwd" => value.as_str().is_some_and(|cwd| Path::new(cwd) == root),
                    _ => false,
                })
    }) {
        return Ok(native.clone());
    }
    ensure!(
        !(original.contains_key("url") && desired.contains_key("command")
            || original.contains_key("command") && desired.contains_key("url")),
        "Native transport differs; migrate it explicitly in native configuration"
    );
    let common = ["command", "args", "env", "cwd", "url", "http_headers"];
    let policies = [
        "tools",
        "default_tools_approval_mode",
        "required",
        "enabled",
        "enabled_tools",
        "disabled_tools",
        "startup_timeout_sec",
        "tool_timeout_sec",
    ];
    ensure!(original.keys().all(|key|common.contains(&key.as_str()) || policies.contains(&key.as_str())),"Native-only authentication/connection fields require explicit native migration; replacement is blocked");
    ensure!(
        !(original.get("enabled").and_then(toml::Value::as_bool) == Some(false)
            && desired.get("enabled").and_then(toml::Value::as_bool) == Some(true)),
        "Mapping would re-enable a natively disabled server"
    );
    for (key, allowlist) in [("enabled_tools", true), ("disabled_tools", false)] {
        if let (Some(old), Some(new)) = (original.get(key), desired.get(key)) {
            let old = old.as_array().context("Invalid native tool policy")?;
            let new = new.as_array().context("Invalid mapped tool policy")?;
            ensure!(
                if allowlist {
                    new.iter().all(|item| old.contains(item))
                } else {
                    old.iter().all(|item| new.contains(item))
                },
                "Mapping would broaden native tool access; review native policy first"
            );
        }
    }
    for key in policies {
        if !desired.contains_key(key) {
            if let Some(value) = original.get(key) {
                desired.insert(key.into(), value.clone());
            }
        }
    }
    Ok(mapped)
}
