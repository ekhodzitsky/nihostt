use anyhow::Context;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

const HF_REPO: &str = "reazon-research/reazonspeech-k2-v2";
const HF_REVISION: &str = "main";

const MODEL_FILES: &[(&str, &str)] = &[
    ("encoder.onnx", "sha256-placeholder"),
    ("decoder.onnx", "sha256-placeholder"),
    ("joiner.onnx", "sha256-placeholder"),
    ("tokens.txt", "sha256-placeholder"),
];

const VAD_URL: &str =
    "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx";
const VAD_FILENAME: &str = "silero_vad.onnx";

/// Default model directory: `~/.nihostt/models`
pub fn default_model_dir() -> String {
    dirs::home_dir()
        .map(|p| {
            p.join(".nihostt")
                .join("models")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_else(|| "./models".to_string())
}

/// Ensure all model files exist, downloading from HuggingFace if necessary.
pub async fn ensure_model(model_dir: &str) -> anyhow::Result<()> {
    let dir = Path::new(model_dir);
    tokio::fs::create_dir_all(dir).await?;

    let client = reqwest::Client::new();

    for (filename, _expected_sha) in MODEL_FILES {
        let path = dir.join(filename);
        if path.exists() {
            tracing::info!(file = %filename, "model file already exists");
            continue;
        }

        let url = format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            HF_REPO, HF_REVISION, filename
        );
        tracing::info!(file = %filename, "downloading model from HuggingFace…");
        download_with_progress(&client, &url, &path).await?;
    }

    // Ensure VAD model
    let vad_path = dir.join(VAD_FILENAME);
    if !vad_path.exists() {
        tracing::info!(file = %VAD_FILENAME, "downloading VAD model…");
        download_with_progress(&client, VAD_URL, &vad_path).await?;
    }

    Ok(())
}

async fn download_with_progress(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
) -> anyhow::Result<()> {
    let partial = dest.with_extension("partial");
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?;

    let mut file = tokio::fs::File::create(&partial)
        .await
        .with_context(|| format!("failed to create {}", partial.display()))?;

    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk)
            .await
            .with_context(|| format!("failed to write to {}", partial.display()))?;
    }

    tokio::fs::rename(&partial, dest).await.with_context(|| {
        format!(
            "failed to rename {} to {}",
            partial.display(),
            dest.display()
        )
    })?;

    tracing::info!(path = %dest.display(), "download complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_model_dir_is_not_empty() {
        let dir = default_model_dir();
        assert!(!dir.is_empty());
        assert!(dir.contains("nihostt"));
    }
}
