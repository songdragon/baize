use super::*;

pub(super) fn get_json(path: &str) -> Result<Value> {
    request_json_with_timeout(
        "GET",
        path,
        None,
        Duration::from_secs(DAEMON_REQUEST_TIMEOUT_SECONDS),
    )
}

pub(super) fn post_json(path: &str, body: Value) -> Result<Value> {
    post_json_with_timeout(
        path,
        body,
        Duration::from_secs(DAEMON_REQUEST_TIMEOUT_SECONDS),
    )
}

pub(super) fn post_json_with_timeout(
    path: &str,
    body: Value,
    read_timeout: Duration,
) -> Result<Value> {
    request_json_with_timeout("POST", path, Some(body), read_timeout)
}

pub(super) fn request_json_with_timeout(
    method: &str,
    path: &str,
    body: Option<Value>,
    read_timeout: Duration,
) -> Result<Value> {
    let body = body.map(|body| body.to_string()).unwrap_or_default();
    let address = daemon_address();
    let mut stream =
        TcpStream::connect_timeout(&address, Duration::from_millis(DAEMON_CONNECT_TIMEOUT_MS))
            .with_context(|| format!("connect to baize daemon at {address}"))?;
    stream
        .set_read_timeout(Some(read_timeout))
        .context("set daemon read timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(DAEMON_REQUEST_TIMEOUT_SECONDS)))
        .context("set daemon write timeout")?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .context("send daemon request")?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .context("read daemon response")?;
    parse_http_json_bytes(&response)
}

#[cfg(test)]
pub(super) fn parse_http_json_response(response: &str) -> Result<Value> {
    parse_http_json_bytes(response.as_bytes())
}

fn parse_http_json_bytes(response: &[u8]) -> Result<Value> {
    let boundary = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("invalid daemon response")?;
    let head = std::str::from_utf8(&response[..boundary]).context("invalid HTTP headers")?;
    let status_line = head.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .context("invalid HTTP status")?;
    let raw = &response[boundary + 4..];
    let chunked = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .any(|(key, value)| {
            key.eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
        });
    let body = if chunked {
        decode_chunks(raw)?
    } else {
        if let Some(length) = head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
            .map(|(_, v)| v.trim().parse::<usize>())
            .transpose()
            .context("invalid content length")?
        {
            if raw.len() != length {
                anyhow::bail!(
                    "incomplete daemon response: expected {length} bytes, received {}",
                    raw.len()
                );
            }
        }
        raw.to_vec()
    };
    if body.is_empty() {
        anyhow::bail!("daemon returned {status_line} with empty body");
    }
    let value: Value = serde_json::from_slice(&body).context("parse daemon JSON response")?;
    if !(200..300).contains(&status) {
        if value["not_submitted"] == true
            || matches!(value["status"].as_str(), Some("failed" | "canceled"))
        {
            return Ok(value);
        }
        if let Some(error) = value["error"].as_str() {
            anyhow::bail!("daemon returned {status_line}: {error}");
        }
        anyhow::bail!("daemon returned {status_line}");
    }
    if let Some(error) = value["error"].as_str() {
        anyhow::bail!("{error}");
    }
    Ok(value)
}

fn decode_chunks(mut input: &[u8]) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let end = input
            .windows(2)
            .position(|w| w == b"\r\n")
            .context("missing HTTP chunk size")?;
        let size = std::str::from_utf8(&input[..end])?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size, 16).context("invalid HTTP chunk size")?;
        input = &input[end + 2..];
        if size == 0 {
            return Ok(body);
        }
        let total = size.checked_add(2).context("HTTP chunk size overflow")?;
        if input.len() < total || &input[size..total] != b"\r\n" {
            anyhow::bail!("incomplete HTTP chunk");
        }
        body.extend_from_slice(&input[..size]);
        input = &input[total..];
    }
}

pub(super) fn response_error(field: &str, response: &Value) -> anyhow::Error {
    anyhow!("daemon response missing {field}: {response}")
}

pub(super) fn provider_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.as_object()?.get("0")?.as_str())
}

pub(super) fn permission_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.as_object()?.get("0")?.as_str())
}

pub(super) fn task_session_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.as_object()?.get("0")?.as_str())
}

pub(super) fn workspace_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.as_object()?.get("0")?.as_str())
}

pub(super) fn workspace_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string())
}

pub(super) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn provider_error_hint(response: &Value) -> Option<String> {
    if let Some(kind) = response
        .get("limit_inference")
        .and_then(|limit| limit.get("kind"))
        .and_then(Value::as_str)
    {
        return match kind {
            "RateLimit" => Some(
                "provider hint: rate limit detected; switch provider or wait for the quota window"
                    .to_string(),
            ),
            "QuotaExceeded" => Some(
                "provider hint: quota exhausted; switch provider or wait for quota reset"
                    .to_string(),
            ),
            _ => None,
        };
    }

    let detail = [
        response.get("error").and_then(Value::as_str),
        response.get("stderr").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    if detail.trim().is_empty() {
        return None;
    }

    let detail = detail.to_ascii_lowercase();
    if contains_any(
        &detail,
        &[
            "timed out",
            "timeout",
            "deadline",
            "interactive prompt",
            "would block",
        ],
    ) {
        return Some(
            "provider hint: command timed out; check CLI login/interactivity or increase timeout"
                .to_string(),
        );
    }
    if contains_any(
        &detail,
        &[
            "auth",
            "login",
            "oauth",
            "api key",
            "credential",
            "permission denied",
            "unauthorized",
            "not authenticated",
        ],
    ) {
        return Some(
            "provider hint: authentication/setup required; run the provider CLI login flow"
                .to_string(),
        );
    }

    None
}

pub(super) fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

pub(super) fn short_text(text: &str, max_chars: usize) -> String {
    let text = one_line(text);
    if text.chars().count() <= max_chars {
        return text;
    }

    let keep = max_chars.saturating_sub(3);
    format!("{}...", text.chars().take(keep).collect::<String>())
}

pub(super) fn transcript_detail(text: &str) -> String {
    short_text(text, TRANSCRIPT_DETAIL_MAX_CHARS)
}

pub(super) fn daemon_address() -> SocketAddr {
    DAEMON_ADDRESS
        .get()
        .copied()
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 7878)))
}

pub(super) fn configured_daemon_address(config: &baize_config::BaizeConfig) -> Result<SocketAddr> {
    let host: std::net::IpAddr = config
        .daemon
        .host
        .parse()
        .context("daemon host must be an IP address")?;
    let host = match host {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => std::net::Ipv4Addr::LOCALHOST.into(),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => std::net::Ipv6Addr::LOCALHOST.into(),
        other => other,
    };
    Ok(SocketAddr::new(host, config.daemon.port))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_chunked_unicode_and_rejects_truncated_bodies() {
        let json = "{\"text\":\"中\"}".as_bytes();
        let split = 10;
        let mut response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for chunk in [&json[..split], &json[split..]] {
            response.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            response.extend_from_slice(chunk);
            response.extend_from_slice(b"\r\n");
        }
        response.extend_from_slice(b"0\r\n\r\n");
        assert_eq!(parse_http_json_bytes(&response).unwrap()["text"], "中");
        assert!(
            parse_http_json_bytes(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{}").is_err()
        );
        assert!(decode_chunks(b"ffff\r\nx").is_err());
    }

    #[test]
    fn daemon_connection_honors_config_and_maps_wildcard_to_loopback() {
        let mut config = baize_config::BaizeConfig::default();
        config.daemon.host = "0.0.0.0".into();
        config.daemon.port = 17878;
        assert_eq!(
            configured_daemon_address(&config).unwrap().to_string(),
            "127.0.0.1:17878"
        );
        config.daemon.host = "::".into();
        assert_eq!(
            configured_daemon_address(&config).unwrap().to_string(),
            "[::1]:17878"
        );
        config.daemon.host = "not an IP".into();
        assert!(configured_daemon_address(&config).is_err());
    }
}
