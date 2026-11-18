use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use base64::Engine as _;
use kumo_core::chat::{Attachment, MessageRow};

use crate::kumo::Kumo;

pub const MAX_INLINE_IMAGE_BYTES: usize = 3_700_000;
const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;
const MAX_UPLOAD_BYTES: u64 = 25 * 1024 * 1024;
const INLINE_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

pub struct Downloaded {
    pub path: PathBuf,
    pub mime_type: String,
    pub size: usize,
    pub inline_image: Option<String>,
}

pub async fn download(kumo: &Kumo, message: &MessageRow) -> anyhow::Result<Downloaded> {
    if message.file_url.is_empty() {
        bail!("message {} has no file", message.id);
    }
    let url = kumo.engine.authorized_file_url(&message.file_url);
    let mut response = kumo
        .http
        .get(&url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("could not download the file of message {}", message.id))?;
    if response
        .content_length()
        .is_some_and(|len| len > MAX_DOWNLOAD_BYTES)
    {
        bail!(
            "the file of message {} is larger than {} MB",
            message.id,
            MAX_DOWNLOAD_BYTES / 1024 / 1024
        );
    }
    let header_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).trim().to_string());
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if (bytes.len() + chunk.len()) as u64 > MAX_DOWNLOAD_BYTES {
            bail!(
                "the file of message {} is larger than {} MB",
                message.id,
                MAX_DOWNLOAD_BYTES / 1024 / 1024
            );
        }
        bytes.extend_from_slice(&chunk);
    }
    let mime_type = if message.file_type.is_empty() {
        header_type.unwrap_or_else(|| mime_from_name(&message.file_name).to_string())
    } else {
        message.file_type.clone()
    };
    let dir = kumo.data_dir.join("attachments");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}-{}", message.id, safe_name(&message.file_name)));
    std::fs::write(&path, &bytes)?;
    let inline_image = (INLINE_IMAGE_TYPES.contains(&mime_type.as_str())
        && bytes.len() <= MAX_INLINE_IMAGE_BYTES)
        .then(|| base64::engine::general_purpose::STANDARD.encode(&bytes));
    Ok(Downloaded {
        path,
        mime_type,
        size: bytes.len(),
        inline_image,
    })
}

pub fn is_image(message: &MessageRow) -> bool {
    message.kind == "FILE"
        && (message.file_type.starts_with("image/")
            || mime_from_name(&message.file_name).starts_with("image/"))
}

pub fn load_local(path: &str) -> anyhow::Result<Attachment> {
    let path = expand_home(path);
    let meta =
        std::fs::metadata(&path).with_context(|| format!("cannot read {}", path.display()))?;
    if !meta.is_file() {
        bail!("{} is not a file", path.display());
    }
    if meta.len() > MAX_UPLOAD_BYTES {
        bail!(
            "{} is {} MB; the limit is {} MB",
            path.display(),
            meta.len() / 1024 / 1024,
            MAX_UPLOAD_BYTES / 1024 / 1024
        );
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    Ok(Attachment {
        mime_type: mime_from_name(&name).to_string(),
        data: std::fs::read(&path)?,
        name,
    })
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::home_dir().unwrap_or_default().join(rest),
        None => Path::new(path).to_path_buf(),
    }
}

fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches('.');
    if trimmed.is_empty() {
        "file".into()
    } else {
        trimmed.to_string()
    }
}

pub fn mime_from_name(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guesses_types_and_sanitizes_names() {
        assert_eq!(mime_from_name("Shot.PNG"), "image/png");
        assert_eq!(mime_from_name("noext"), "application/octet-stream");
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name(""), "file");
    }
}
