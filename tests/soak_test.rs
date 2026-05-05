//! Soak test for the nihostt WebSocket server.
//!
//! Cycles connect→stream→stop→disconnect repeatedly for a configurable duration
//! to surface memory leaks, resource exhaustion, or connection handling bugs.
//!
//! Marked `#[ignore]` — run locally only:
//! ```
//! NIHOSTT_SOAK_DURATION_SECS=60 cargo test --test soak_test -- --ignored
//! ```

mod common;

use futures_util::{SinkExt, StreamExt};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
#[ignore]
async fn test_soak_ws_continuous() {
    let soak_duration_secs: u64 = std::env::var("NIHOSTT_SOAK_DURATION_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let soak_duration = Duration::from_secs(soak_duration_secs);

    let port = common::start_server().await;

    let silence = common::generate_pcm16_silence(2.0, 48_000);

    let mut iteration: u64 = 0;
    let mut error_count: u64 = 0;
    let start = Instant::now();

    println!("[soak] starting — port={port}, duration={soak_duration_secs}s");

    while start.elapsed() < soak_duration {
        iteration += 1;
        let iter_start = Instant::now();

        let result: Result<(), String> = async {
            let (mut sink, mut stream, _ready) =
                tokio::time::timeout(Duration::from_secs(10), common::ws_connect(port))
                    .await
                    .map_err(|_| format!("iter {iteration}: timeout connecting"))?;

            for chunk in silence.chunks(9_600) {
                tokio::time::timeout(
                    Duration::from_secs(10),
                    sink.send(Message::Binary(chunk.to_vec().into())),
                )
                .await
                .map_err(|_| format!("iter {iteration}: timeout sending audio chunk"))?
                .map_err(|e| format!("iter {iteration}: send audio error: {e}"))?;
            }

            tokio::time::timeout(
                Duration::from_secs(10),
                sink.send(Message::Text(
                    serde_json::to_string(&serde_json::json!({"type": "stop"}))
                        .unwrap()
                        .into(),
                )),
            )
            .await
            .map_err(|_| format!("iter {iteration}: timeout sending Stop"))?
            .map_err(|e| format!("iter {iteration}: send Stop error: {e}"))?;

            loop {
                let msg = tokio::time::timeout(Duration::from_secs(10), stream.next())
                    .await
                    .map_err(|_| format!("iter {iteration}: timeout waiting for Final"))?
                    .ok_or_else(|| format!("iter {iteration}: stream ended before Final"))?
                    .map_err(|e| format!("iter {iteration}: ws error waiting for Final: {e}"))?;

                let text = msg
                    .into_text()
                    .map_err(|e| format!("iter {iteration}: non-text message: {e}"))?;
                let v: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|e| format!("iter {iteration}: invalid JSON: {e}"))?;

                match v["type"].as_str().unwrap_or("") {
                    "partial" => continue,
                    "final" => break,
                    other => {
                        return Err(format!(
                            "iter {iteration}: unexpected message type: {other}"
                        ));
                    }
                }
            }

            drop(sink);
            drop(stream);

            Ok(())
        }
        .await;

        if let Err(msg) = result {
            eprintln!("[soak] ERROR — {msg}");
            error_count += 1;
        }

        let iter_ms = iter_start.elapsed().as_millis();
        if iteration.is_multiple_of(10) {
            println!(
                "[soak] iter={iteration} errors={error_count} elapsed={:.1}s iter_ms={iter_ms}",
                start.elapsed().as_secs_f64()
            );
        }
    }

    let total_secs = start.elapsed().as_secs_f64();

    println!(
        "[soak] done — iterations={iteration} errors={error_count} total_duration={total_secs:.1}s"
    );

    assert_eq!(
        error_count, 0,
        "soak test completed {iteration} iterations in {total_secs:.1}s with {error_count} error(s)"
    );
}
