//! HTTP download with progress reporting.
//!
//! Streams the response body chunk-by-chunk to a destination file, emitting
//! [`FetcherProgress`] callbacks throttled to every ~100KB or ~100ms (whichever
//! comes first). A final `Done` callback fires once the body is fully written.

use std::path::Path;
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::error::FetcherError;
use crate::{FetcherPhase, FetcherProgress};

/// Bytes between forced progress emissions (in addition to time throttling).
const PROGRESS_BYTES_INTERVAL: u64 = 100 * 1024;
/// Wall-clock interval between forced progress emissions.
const PROGRESS_TIME_INTERVAL: Duration = Duration::from_millis(100);

/// Streams `url` to `dest_path`, emitting [`FetcherProgress`] callbacks
/// during the download and a final `Done` phase callback on success.
///
/// The callback is throttled — it fires at most once per
/// [`PROGRESS_BYTES_INTERVAL`] bytes *or* once per
/// [`PROGRESS_TIME_INTERVAL`], whichever happens first, with the
/// final emission always fired regardless of throttle.
pub(crate) async fn download(
    url: &str,
    dest_path: &Path,
    progress_cb: Option<&(dyn Fn(FetcherProgress) + Send + Sync)>,
) -> Result<(), FetcherError> {
    let resp = reqwest::get(url).await?.error_for_status()?;
    let total = resp.content_length();

    let mut file = File::create(dest_path).await?;
    let mut stream = resp.bytes_stream();

    let mut downloaded: u64 = 0;
    let mut last_emit_bytes: u64 = 0;
    let mut last_emit_time = Instant::now();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        if let Some(cb) = progress_cb {
            let bytes_since = downloaded - last_emit_bytes;
            let time_since = last_emit_time.elapsed();
            if bytes_since >= PROGRESS_BYTES_INTERVAL || time_since >= PROGRESS_TIME_INTERVAL {
                cb(FetcherProgress {
                    downloaded,
                    total,
                    phase: FetcherPhase::Downloading,
                });
                last_emit_bytes = downloaded;
                last_emit_time = Instant::now();
            }
        }
    }

    file.flush().await?;
    file.sync_all().await?;

    if let Some(cb) = progress_cb {
        cb(FetcherProgress {
            downloaded,
            total,
            phase: FetcherPhase::Done,
        });
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn downloads_file_and_invokes_progress_callback() {
        let server = MockServer::start().await;
        let body = b"hello chrome zip payload".to_vec();

        Mock::given(method("GET"))
            .and(path("/chrome.zip"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", body.len().to_string().as_str())
                    .set_body_bytes(body.clone()),
            )
            .mount(&server)
            .await;

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let dest = tmp.path().to_path_buf();
        let url = format!("{}/chrome.zip", server.uri());

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_cb = Arc::clone(&calls);
        let cb = move |_p: FetcherProgress| {
            calls_cb.fetch_add(1, Ordering::SeqCst);
        };

        download(&url, &dest, Some(&cb)).await.unwrap();

        let written = tokio::fs::read(&dest).await.unwrap();
        assert_eq!(written, body);
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "progress callback should fire at least once (got {})",
            calls.load(Ordering::SeqCst)
        );
    }
}
