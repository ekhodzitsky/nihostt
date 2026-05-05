//! WER benchmark for nihostt on real Japanese speech clips.
//!
//! Uses Tatoeba Japanese audio recordings (native speaker voice samples)
//! stored in `tests/fixtures/tatoeba/`.
//! For Japanese, "word" = character, so this computes character error rate (CER).

use std::path::PathBuf;

fn main() {
    let model_dir = nihostt::model::default_model_dir();
    let encoder = PathBuf::from(&model_dir).join("encoder-epoch-99-avg-1.onnx");
    assert!(
        encoder.exists(),
        "Model not found at {}. Run `cargo run -- download` first.",
        model_dir
    );

    let engine = nihostt::inference::Engine::load_with_pool_size(&model_dir, 1)
        .expect("failed to load engine");

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let pool = engine.pool.as_ref();

    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("tatoeba");

    // Load clips metadata from Tatoeba
    let meta_path = fixtures_dir.join("meta.txt");
    let meta_content = std::fs::read_to_string(&meta_path)
        .expect("failed to read tatoeba meta.txt; run the download script first");

    let mut clips = Vec::new();
    for line in meta_content.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let wav_file = parts[0].replace(".mp3", ".wav");
            let reference = parts[1].to_string();
            clips.push((wav_file, reference));
        }
    }

    assert!(
        !clips.is_empty(),
        "No Tatoeba clips found. Download them first: see tests/fixtures/tatoeba/"
    );

    rt.block_on(async {
        let mut session = pool.checkout().await.expect("pool checkout failed");

        let mut total_ref_chars = 0_usize;
        let mut total_errors = 0_usize;

        for (file, reference) in &clips {
            let path = fixtures_dir.join(file);
            if !path.exists() {
                eprintln!("Skip missing file: {}", path.display());
                continue;
            }
            let result = engine
                .transcribe_file(path.to_str().unwrap(), &mut session)
                .expect("transcription failed");

            let ref_norm = normalize(reference);
            let hyp_norm = normalize(&result.text);

            let errors = levenshtein_chars(&ref_norm, &hyp_norm);
            let ref_len = ref_norm.chars().count();
            let cer = if ref_len > 0 {
                errors as f64 / ref_len as f64
            } else {
                0.0
            };

            println!(
                "{file}: ref=\"{ref_norm}\" hyp=\"{hyp_norm}\" errors={errors} len={ref_len} CER={:.2}%",
                cer * 100.0
            );

            total_ref_chars += ref_len;
            total_errors += errors;
        }

        let overall_cer = if total_ref_chars > 0 {
            total_errors as f64 / total_ref_chars as f64
        } else {
            0.0
        };

        println!("\n=== BENCHMARK RESULT ===");
        println!(
            "Overall CER = {:.2}% ({}/{} chars)",
            overall_cer * 100.0,
            total_errors,
            total_ref_chars
        );

        // Explicitly drop the session guard inside the runtime so the spawned
        // return-to-pool task has a reactor available.
        drop(session);
    });
}

/// Remove whitespace (ASCII + full-width) so comparison is character-based.
fn normalize(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Levenshtein distance computed on Unicode scalar values (characters).
fn levenshtein_chars(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let n = a_chars.len();
    let m = b_chars.len();

    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }

    let mut prev = vec![0; m + 1];
    let mut curr = vec![0; m + 1];

    for (j, item) in prev.iter_mut().enumerate().take(m + 1) {
        *item = j;
    }

    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[m]
}
