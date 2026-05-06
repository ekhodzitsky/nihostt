use crate::inference::PooledSession;
use anyhow::Context;
use ort::value::TensorRef;

/// Greedy RNN-T decode.
///
/// `mel` shape: `[n_frames, N_MELS]`
/// Returns the decoded text and the average token confidence (`None` if no
/// non-blank tokens were emitted).
pub fn greedy_decode(
    mel: &[Vec<f32>],
    session: &mut PooledSession,
    tokens: &[String],
) -> anyhow::Result<(String, Option<f32>)> {
    if mel.is_empty() {
        return Ok((String::new(), None));
    }

    let n_frames = mel.len();
    let n_mels = mel[0].len();

    // Build encoder input: [1, n_frames, n_mels]
    let mut encoder_input = vec![0.0f32; n_frames * n_mels];
    for (t, frame) in mel.iter().enumerate() {
        for (m, &val) in frame.iter().enumerate() {
            encoder_input[t * n_mels + m] = val;
        }
    }

    let encoder_tensor =
        TensorRef::from_array_view(([1_usize, n_frames, n_mels], encoder_input.as_slice()))?;
    let x_lens = [n_frames as i64];
    let x_lens_tensor = TensorRef::from_array_view(([1_usize], x_lens.as_slice()))?;

    // Run encoder
    let encoder_outputs = session
        .encoder
        .run(ort::inputs![encoder_tensor, x_lens_tensor])?;
    let (_enc_shape, enc_data) = encoder_outputs[0].try_extract_tensor::<f32>()?;

    // Copy shape and data so we can release the borrow
    let enc_shape_owned: Vec<i64> = _enc_shape.iter().copied().collect();
    let enc_data_owned: Vec<f32> = enc_data.to_vec();
    drop(encoder_outputs);

    // enc_data_owned layout: [batch=1, T_enc, encoder_dim] flat
    let t_enc = enc_shape_owned[1] as usize;
    let enc_dim = enc_shape_owned[2] as usize;

    // Decoder initial state: prev_token = blank (usually last token index or 0)
    let blank_id = 0;
    let mut prev_token = blank_id as i64;
    let mut prev_prev_token = blank_id as i64;
    let mut result_tokens: Vec<usize> = Vec::new();
    let mut token_confidences: Vec<f32> = Vec::new();

    let mut dec_data_buf: Vec<f32> = Vec::new();
    let mut logits_buf: Vec<f32> = Vec::new();

    for t in 0..t_enc {
        let enc_offset = t * enc_dim;
        let enc_frame = &enc_data_owned[enc_offset..enc_offset + enc_dim];

        // Run decoder with [prev_prev_token, prev_token]
        let target_data = [prev_prev_token, prev_token];
        let target_tensor = TensorRef::from_array_view(([1_usize, 2], target_data.as_slice()))?;

        let decoder_outputs = session.decoder.run(ort::inputs![target_tensor])?;
        let (_dec_shape, dec_data) = decoder_outputs[0].try_extract_tensor::<f32>()?;

        let dec_shape_owned: Vec<i64> = _dec_shape.iter().copied().collect();
        dec_data_buf.clear();
        dec_data_buf.extend_from_slice(dec_data);
        drop(decoder_outputs);

        // dec_out shape: [1, 1, decoder_dim] or [1, decoder_dim]
        let dec_frame = if dec_shape_owned.len() == 3 {
            let dec_dim = dec_shape_owned[2] as usize;
            &dec_data_buf[..dec_dim]
        } else {
            &dec_data_buf[..]
        };

        // Joiner: combine encoder frame + decoder frame
        let enc_tensor = TensorRef::from_array_view(([1_usize, enc_dim], enc_frame))?;
        let dec_tensor = TensorRef::from_array_view(([1_usize, dec_frame.len()], dec_frame))?;

        let joiner_outputs = session.joiner.run(ort::inputs![enc_tensor, dec_tensor])?;
        let (_logits_shape, logits_data) = joiner_outputs[0].try_extract_tensor::<f32>()?;

        let logits_shape_owned: Vec<i64> = _logits_shape.iter().copied().collect();
        logits_buf.clear();
        if logits_shape_owned.len() == 4 {
            // [1, 1, 1, vocab_size]
            let vocab_size = logits_shape_owned[3] as usize;
            logits_buf.extend_from_slice(&logits_data[..vocab_size]);
        } else {
            logits_buf.extend_from_slice(logits_data);
        }
        drop(joiner_outputs);

        // Argmax + softmax for confidence
        let mut best_token = blank_id;
        let mut best_logit = f32::NEG_INFINITY;
        for (i, &logit) in logits_buf.iter().enumerate() {
            if logit > best_logit {
                best_logit = logit;
                best_token = i;
            }
        }

        if best_token != blank_id {
            result_tokens.push(best_token);
            prev_prev_token = prev_token;
            prev_token = best_token as i64;
            // numerically-stable softmax over the logits
            let max_logit = logits_buf.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let exp_sum: f32 = logits_buf.iter().map(|&l| (l - max_logit).exp()).sum();
            let prob = (best_logit - max_logit).exp() / exp_sum;
            token_confidences.push(prob);
        }
    }

    // Decode tokens to string
    let mut text = String::new();
    for &tid in &result_tokens {
        if tid < tokens.len() {
            text.push_str(&tokens[tid]);
        }
    }

    let avg_confidence = if token_confidences.is_empty() {
        None
    } else {
        Some(token_confidences.iter().sum::<f32>() / token_confidences.len() as f32)
    };

    Ok((text, avg_confidence))
}
