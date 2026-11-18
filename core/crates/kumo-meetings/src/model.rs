use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    name: String,
}

impl Model {
    pub fn named(name: &str) -> anyhow::Result<Self> {
        let name = name.trim();
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_');
        if !valid {
            bail!("\"{name}\" is not a Whisper model name, for example large-v3-turbo-q5_0");
        }
        Ok(Self { name: name.into() })
    }

    pub fn file_name(&self) -> String {
        format!("ggml-{}.bin", self.name)
    }

    pub fn url(&self) -> String {
        format!("{BASE_URL}/{}", self.file_name())
    }

    pub fn path_in(&self, dir: &Path) -> PathBuf {
        dir.join(self.file_name())
    }
}

pub fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("better-kumo/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

pub async fn fetch(url: &str, partial: &Path, progress: impl Fn(u8)) -> anyhow::Result<String> {
    let mut response = client()?
        .get(url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("cannot download {url}"))?;
    let total = response.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(partial).await?;
    let mut hasher = Sha256::new();
    let mut done: u64 = 0;
    let mut shown = u8::MAX;
    while let Some(bytes) = response.chunk().await? {
        file.write_all(&bytes).await?;
        hasher.update(&bytes);
        done += bytes.len() as u64;
        let percent = (done * 100).checked_div(total).unwrap_or(0).min(100) as u8;
        if percent != shown {
            shown = percent;
            progress(percent);
        }
    }
    file.flush().await?;
    drop(file);
    if total > 0 && done != total {
        let _ = tokio::fs::remove_file(partial).await;
        bail!("the download stopped at {done} of {total} bytes");
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

pub async fn download(model: &Model, dir: &Path, progress: impl Fn(u8)) -> anyhow::Result<PathBuf> {
    let target = model.path_in(dir);
    if target.exists() {
        return Ok(target);
    }
    tokio::fs::create_dir_all(dir).await?;
    let partial = dir.join(format!("{}.part", model.file_name()));
    fetch(&model.url(), &partial, progress)
        .await
        .with_context(|| format!("speech model {}", model.file_name()))?;
    tokio::fs::rename(&partial, &target).await?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_whisper_cpp_files() {
        let model = Model::named("large-v3-turbo-q5_0").unwrap();
        assert_eq!(model.file_name(), "ggml-large-v3-turbo-q5_0.bin");
        assert_eq!(
            model.url(),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin"
        );
    }

    #[test]
    fn rejects_names_that_are_not_file_names() {
        assert!(Model::named("../etc").is_err());
        assert!(Model::named("").is_err());
        assert!(Model::named("a b").is_err());
    }
}
