use anyhow::Context;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

const HF_REPO: &str = "reazon-research/reazonspeech-k2-v2";
const HF_REVISION: &str = "a454b3fe1e63f4189ae3994248aeb3d31b6682f4";

const ENCODER_FP32_SHA256: &str =
    "ecdb0b771e16104aaf8e579cb3c1e32fbd589eb641c5946d82b615bd366c5f96";
pub const ENCODER_INT8_SHA256: &str =
    "2c7bd08a8a99f9ddd0d9e458456577b1f6279214e51426f114f9eced44c54e1d";
const DECODER_SHA256: &str = "58b18211ae06265466bfa17172dab574df94f76c8bcb61a3640c28ba860e4124";
const JOINER_SHA256: &str = "d38a81d1191c9ed6de6a1719503692e07e3e973e2364adde0abae5eaaded1174";
const TOKENS_SHA256: &str = "2c3ac659818a48a0c04010e0593bbc4d7c8a24a054340b01131499c05fd52def";

struct ModelFile {
    filename: &'static str,
    expected_sha256: &'static [&'static str],
}

const MODEL_FILES: &[ModelFile] = &[
    ModelFile {
        filename: "encoder-epoch-99-avg-1.onnx",
        // The active encoder path may contain either the original FP32 model
        // or the official INT8 model after `download` / `serve` quantization.
        expected_sha256: &[ENCODER_FP32_SHA256, ENCODER_INT8_SHA256],
    },
    ModelFile {
        filename: "decoder-epoch-99-avg-1.onnx",
        expected_sha256: &[DECODER_SHA256],
    },
    ModelFile {
        filename: "joiner-epoch-99-avg-1.onnx",
        expected_sha256: &[JOINER_SHA256],
    },
    ModelFile {
        filename: "tokens.txt",
        expected_sha256: &[TOKENS_SHA256],
    },
];

const VAD_URL: &str = concat!(
    "https://raw.githubusercontent.com/snakers4/silero-vad/",
    "7e30209a3e901f9842f81b225f3e93d8199902b1/",
    "src/silero_vad/data/silero_vad.onnx"
);
const VAD_FILENAME: &str = "silero_vad.onnx";
const VAD_SHA256: &str = "1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3";

// WeSpeaker ResNet34 ONNX (Apache 2.0) — used as the default speaker-embedding
// model for diarization when ecapa_tdnn.onnx is not present.
// Mirrored on HuggingFace for reliability (original: WenET WeSpeaker CDN).
const WESPEAKER_URL: &str = concat!(
    "https://huggingface.co/hbredin/wespeaker-voxceleb-resnet34-LM/resolve/",
    "0ae88dcaf48cacdf741275d6d1a8101f45eee220/",
    "speaker-embedding.onnx"
);
const WESPEAKER_FILENAME: &str = "wespeaker_resnet34.onnx";
const WESPEAKER_SHA256: &str = "7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068";

/// Default model directory: `~/.nihostt/models`
pub fn default_model_dir() -> String {
    home_dir()
        .map(|p| {
            p.join(".nihostt")
                .join("models")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_else(|| "./models".to_string())
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| {
                let drive = std::env::var_os("HOMEDRIVE")?;
                let path = std::env::var_os("HOMEPATH")?;
                let mut home = PathBuf::from(drive);
                home.push(PathBuf::from(path));
                Some(home)
            })
    }

    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Ensure all model files exist, downloading from HuggingFace if necessary.
pub async fn ensure_model(model_dir: &str) -> anyhow::Result<()> {
    let dir = Path::new(model_dir);
    tokio::fs::create_dir_all(dir).await?;

    let client = reqwest::Client::new();

    for model_file in MODEL_FILES {
        let path = dir.join(model_file.filename);
        if path.exists() {
            match verify_sha256_any(&path, model_file.expected_sha256).await {
                Ok(()) => {
                    tracing::info!(file = %model_file.filename, "model file already exists");
                    continue;
                }
                Err(e) => {
                    tracing::warn!(
                        file = %model_file.filename,
                        error = %e,
                        "model file SHA-256 mismatch, removing corrupt file"
                    );
                    let _ = tokio::fs::remove_file(&path).await;
                }
            }
        }

        let url = format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            HF_REPO, HF_REVISION, model_file.filename
        );
        tracing::info!(file = %model_file.filename, "downloading model from HuggingFace…");
        download_with_progress(&client, &url, &path).await?;
        if let Err(e) = verify_sha256_any(&path, model_file.expected_sha256).await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(e).with_context(|| {
                format!(
                    "downloaded model file {} failed SHA-256 verification",
                    model_file.filename
                )
            });
        }
    }

    // Ensure VAD model
    let vad_path = dir.join(VAD_FILENAME);
    if !vad_path.exists() {
        tracing::info!(file = %VAD_FILENAME, "downloading VAD model…");
        download_with_progress(&client, VAD_URL, &vad_path).await?;
    }
    if let Err(e) = verify_file_sha256(&vad_path, VAD_SHA256).await {
        tracing::warn!(
            error = %e,
            path = %vad_path.display(),
            "VAD model SHA-256 mismatch, removing corrupt file"
        );
        let _ = tokio::fs::remove_file(&vad_path).await;
        tracing::info!(file = %VAD_FILENAME, "re-downloading VAD model…");
        download_with_progress(&client, VAD_URL, &vad_path).await?;
        verify_file_sha256(&vad_path, VAD_SHA256)
            .await
            .with_context(|| "re-downloaded VAD model still has bad SHA-256")?;
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
    if let Err(e) = verify_file_sha256(&speaker_path, WESPEAKER_SHA256).await {
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
        verify_file_sha256(&speaker_path, WESPEAKER_SHA256)
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

    if !response.status().is_success() {
        return Err(download_status_error(response.status(), url));
    }

    let mut file = tokio::fs::File::create(&partial)
        .await
        .with_context(|| format!("failed to create {}", partial.display()))?;

    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk)
            .await
            .with_context(|| format!("failed to write to {}", partial.display()))?;
    }
    file.flush()
        .await
        .with_context(|| format!("failed to flush {}", partial.display()))?;
    drop(file);

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

fn download_status_error(status: reqwest::StatusCode, url: &str) -> anyhow::Error {
    anyhow::anyhow!("download failed with HTTP {status} for {url}")
}

/// Verify the SHA-256 hash of a file against an expected hex string.
pub async fn verify_file_sha256(path: &Path, expected: &str) -> anyhow::Result<()> {
    verify_sha256_any(path, &[expected]).await
}

async fn verify_sha256_any(path: &Path, expected: &[&str]) -> anyhow::Result<()> {
    use sha2::Digest;
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read {} for SHA-256 check", path.display()))?;
    let hash = sha2::Sha256::digest(&bytes);
    let got = hex::encode(hash);
    if expected.iter().any(|sha| got.eq_ignore_ascii_case(sha)) {
        Ok(())
    } else {
        anyhow::bail!(
            "SHA-256 mismatch for {}: expected one of [{}], got {got}",
            path.display(),
            expected.join(", ")
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

    #[test]
    fn download_status_error_mentions_status_and_url() {
        let err = download_status_error(
            reqwest::StatusCode::NOT_FOUND,
            "https://example.test/model.onnx",
        );
        assert!(
            err.to_string().contains("HTTP 404"),
            "error should mention HTTP status, got {err:#}"
        );
        assert!(
            err.to_string().contains("https://example.test/model.onnx"),
            "error should mention URL, got {err:#}"
        );
    }

    #[tokio::test]
    async fn verify_sha256_any_accepts_known_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("tokens.txt");
        tokio::fs::write(&file, b"fixture")
            .await
            .expect("write fixture");

        verify_sha256_any(
            &file,
            &[
                "0000000000000000000000000000000000000000000000000000000000000000",
                "f16d05ec6b29248d2c61adb1e9263f78e4f7bace1b955014a2d17872cfe4064d",
            ],
        )
        .await
        .expect("one matching hash should be accepted");
    }
}
