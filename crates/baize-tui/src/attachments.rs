use super::*;

pub(super) fn handle_input_command(state: &mut TuiState, input: &str) -> bool {
    let input = input.trim();
    if input == "/unimage" {
        state.image_paths.clear();
        return true;
    }
    if let Some(rest) = input.strip_prefix("/image ") {
        attach_images_from_text(state, rest);
        return true;
    }
    if matches!(
        input,
        "/image-clipboard" | "/clipboard-image" | "/clip-image"
    ) {
        attach_clipboard_image(state);
        return true;
    }
    false
}

pub(super) fn handle_paste(state: &mut TuiState, text: &str) {
    let image_paths = image_paths_from_text(text);
    if image_paths.is_empty() {
        append_pasted_text(state, text);
        return;
    }

    for path in image_paths {
        attach_image_path(state, path);
    }
}

pub(super) fn append_pasted_text(state: &mut TuiState, text: &str) {
    insert_text(state, text);
}

pub(super) fn attach_images_from_text(state: &mut TuiState, text: &str) {
    let image_paths = image_paths_from_text(text);
    if image_paths.is_empty() {
        state.push_message("image attach failed: provide a PNG/JPG/GIF/WebP path".to_string());
        return;
    }

    for path in image_paths {
        attach_image_path(state, path);
    }
}

pub(super) fn attach_image_path(state: &mut TuiState, path: PathBuf) {
    let path = resolve_user_path(path);
    if !is_supported_image_path(&path) {
        state.push_message(format!(
            "image attach failed: {} is not a supported image file",
            path.display()
        ));
        return;
    }
    let path = match path.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            state.push_message(format!(
                "image attach failed: {} is not readable ({error})",
                path.display()
            ));
            return;
        }
    };
    if state.image_paths.iter().any(|existing| existing == &path) {
        state.push_message(format!("image already attached: {}", path.display()));
        return;
    }

    state.image_paths.push(path.clone());
    state.push_message(format!("image attached: {}", path.display()));
}

pub(super) fn attach_clipboard_image(state: &mut TuiState) {
    match save_clipboard_image_to_temp_file() {
        Ok(path) => attach_image_path(state, path),
        Err(error) => state.push_message(format!("clipboard image failed: {error:#}")),
    }
}

pub(super) fn image_paths_from_text(text: &str) -> Vec<PathBuf> {
    shell_like_words(text)
        .into_iter()
        .filter_map(|word| image_path_from_token(&word))
        .collect()
}

pub(super) fn image_path_from_token(token: &str) -> Option<PathBuf> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let path = if let Some(path) = token.strip_prefix("file://") {
        percent_decode_file_url_path(path)
    } else {
        token.to_string()
    };
    let path = PathBuf::from(path);
    is_supported_image_path(&path).then_some(path)
}

pub(super) fn shell_like_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if matches!(ch, '"' | '\'') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch.is_whitespace() && quote.is_none() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

pub(super) fn percent_decode_file_url_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                decoded.push((hi << 4) | lo);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).to_string()
}

pub(super) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) fn resolve_user_path(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return std::env::var_os("HOME").map(PathBuf::from).unwrap_or(path);
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or(path)
    }
}

pub(super) fn is_supported_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "heic"
            )
        })
        .unwrap_or(false)
}

pub(super) fn save_clipboard_image_to_temp_file() -> Result<PathBuf> {
    save_macos_clipboard_image_to_temp_file()
}

#[cfg(target_os = "macos")]
pub(super) fn save_macos_clipboard_image_to_temp_file() -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "baize-clipboard-{}.png",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    let output = Command::new("osascript")
        .args(macos_clipboard_image_osascript_args(&path))
        .output()
        .context("run osascript to read macOS clipboard image")?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        anyhow::bail!(
            "macOS clipboard does not contain a PNG image{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(" ({detail})")
            }
        );
    }
    Ok(path)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn save_macos_clipboard_image_to_temp_file() -> Result<PathBuf> {
    anyhow::bail!("clipboard image capture is only implemented on macOS")
}

#[cfg(target_os = "macos")]
pub(super) fn macos_clipboard_image_osascript_args(path: &Path) -> Vec<String> {
    vec![
        "-e".to_string(),
        format!(
            "set outPath to POSIX file {}",
            applescript_string_literal(&path.to_string_lossy())
        ),
        "-e".to_string(),
        "try".to_string(),
        "-e".to_string(),
        "set imageData to the clipboard as «class PNGf»".to_string(),
        "-e".to_string(),
        "set outFile to open for access outPath with write permission".to_string(),
        "-e".to_string(),
        "set eof of outFile to 0".to_string(),
        "-e".to_string(),
        "write imageData to outFile".to_string(),
        "-e".to_string(),
        "close access outFile".to_string(),
        "-e".to_string(),
        "on error errMsg number errNum".to_string(),
        "-e".to_string(),
        "try".to_string(),
        "-e".to_string(),
        "close access outPath".to_string(),
        "-e".to_string(),
        "end try".to_string(),
        "-e".to_string(),
        "error errMsg number errNum".to_string(),
        "-e".to_string(),
        "end try".to_string(),
    ]
}

#[cfg(target_os = "macos")]
pub(super) fn applescript_string_literal(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
