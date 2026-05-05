use anyhow::Context;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

const HF_REPO: &str = "reazon-research/reazonspeech-k2-v2";
const HF_REVISION: &str = "main";

const MODEL_FILES: &[(&str, &str)] = &[
    ("encoder-epoch-99-avg-1.onnx", "sha256-placeholder"),
    ("decoder-epoch-99-avg-1.onnx", "sha256-placeholder"),
    ("joiner-epoch-99-avg-1.onnx", "sha256-placeholder"),
    ("tokens.txt", "sha256-placeholder"),
];

const VAD_URL: &str =
    "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx";
const VAD_FILENAME: &str = "silero_vad.onnx";

// WeSpeaker ResNet34 ONNX (Apache 2.0) — used as the default speaker-embedding
// model for diarization when ecapa_tdnn.onnx is not present.
// Mirrored on HuggingFace for reliability (original: WenET WeSpeaker CDN).
const WESPEAKER_URL: &str = "https://huggingface.co/hbredin/wespeaker-voxceleb-resnet34-LM/resolve/main/speaker-embedding.onnx";
const WESPEAKER_FILENAME: &str = "wespeaker_resnet34.onnx";
const WESPEAKER_SHA256: &str = "7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068";

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

    // Ensure speaker-embedding model for diarization
    let speaker_path = dir.join(WESPEAKER_FILENAME);
    if !speaker_path.exists() {
        tracing::info!(
            file = %WESPEAKER_FILENAME,
            "downloading speaker embedding model (WeSpeaker ResNet34)…"
        );
        download_with_progress(&client, WESPEAKER_URL, &speaker_path).await?;
    }
    // Verify SHA-256 even if the file was already present (corrupted download guard).
    if let Err(e) = verify_sha256(&speaker_path, WESPEAKER_SHA256).await {
        tracing::warn!(
            error = %e,
            path = %speaker_path.display(),
            "speaker model SHA-256 mismatch, removing corrupt file"
        );
        let _ = tokio::fs::remove_file(&speaker_path).await;
        tracing::info!(
            file = %WESPEAKER_FILENAME,
            "re-downloading speaker embedding model…"
        );
        download_with_progress(&client, WESPEAKER_URL, &speaker_path).await?;
        verify_sha256(&speaker_path, WESPEAKER_SHA256)
            .await
            .with_context(|| "re-downloaded speaker model still has bad SHA-256")?;
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

/// Verify the SHA-256 hash of a file against an expected hex string.
async fn verify_sha256(path: &Path, expected: &str) -> anyhow::Result<()> {
    use sha2::Digest;
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read {} for SHA-256 check", path.display()))?;
    let hash = sha2::Sha256::digest(&bytes);
    let got = hex::encode(hash);
    if got.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        anyhow::bail!(
            "SHA-256 mismatch for {}: expected {expected}, got {got}",
            path.display()
        )
    }
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
