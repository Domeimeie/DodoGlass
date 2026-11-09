//! A small HTTP endpoint on 127.0.0.1 that ffmpeg writes its output to.
//!
//! ffmpeg's HLS and DASH muxers can upload every file they produce with an
//! HTTP PUT instead of writing it to disk (`-method PUT`). But S3 requires
//! every request to be signed, which ffmpeg cannot do. So ffmpeg PUTs to this
//! endpoint, and each file is passed on to the bucket, signed, as soon as it is
//! complete:
//!
//! ```text
//! ffmpeg ──PUT http://127.0.0.1:<port>/<token>/<task>/hls/720p/seg_003.ts──► here ──signed PUT──► s3://final/<task>/hls/720p/seg_003.ts
//! ```
//!
//! Nothing touches the disk. Memory use is one segment per running upload
//! (a few MB), however long the video is: the reply to ffmpeg is only sent
//! once the segment is stored, so ffmpeg cannot run ahead of the uploads.
//!
//! The random `<token>` in the path keeps other local programs from writing
//! into the bucket through this endpoint.

use std::collections::HashMap;
use std::convert::Infallible;
use std::io::Read;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tracing::{debug, warn};

use crate::storage::Storage;

/// Largest single file accepted. Segments are a few MB; this only guards
/// against something going badly wrong.
const MAX_FILE_BYTES: usize = 512 * 1024 * 1024;

/// What was uploaded below one prefix (`<task>/<format>`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stats {
    pub files: u64,
    pub bytes: u64,
    /// Uploads that failed; ffmpeg does not always stop on a failed PUT, so
    /// the service checks this after each job.
    pub failed: u64,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct UploadServer {
    base_url: String,
    bucket: String,
    stats: Arc<Mutex<HashMap<String, Stats>>>,
}

struct Shared {
    storage: Arc<Storage>,
    bucket: String,
    prefix: String,
    stats: Arc<Mutex<HashMap<String, Stats>>>,
}

impl UploadServer {
    /// Starts listening on a free port on 127.0.0.1.
    pub async fn start(storage: Arc<Storage>, bucket: String) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("cannot open the local upload endpoint")?;
        let port = listener.local_addr()?.port();
        let token = random_token()?;
        let stats = Arc::new(Mutex::new(HashMap::new()));
        let shared = Arc::new(Shared {
            storage,
            bucket: bucket.clone(),
            prefix: format!("/{token}/"),
            stats: stats.clone(),
        });

        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(e) => {
                        warn!(error = %e, "upload endpoint: accept failed");
                        continue;
                    }
                };
                let shared = shared.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req| handle(req, shared.clone()));
                    // ffmpeg keeps the connection open between files (-http_persistent).
                    if let Err(e) = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                    {
                        debug!(error = %e, "upload connection ended");
                    }
                });
            }
        });

        Ok(UploadServer {
            base_url: format!("http://127.0.0.1:{port}/{token}"),
            bucket,
            stats,
        })
    }

    /// The URL ffmpeg writes `key` to.
    pub fn url_for(&self, key: &str) -> String {
        format!("{}/{key}", self.base_url)
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Takes (and resets) the statistics for everything below `prefix`.
    pub fn take_stats(&self, prefix: &str) -> Stats {
        self.stats
            .lock()
            .expect("stats lock poisoned")
            .remove(prefix.trim_end_matches('/'))
            .unwrap_or_default()
    }
}

type Reply = Response<Full<Bytes>>;

async fn handle(req: Request<Incoming>, shared: Arc<Shared>) -> Result<Reply, Infallible> {
    let Some(key) = req
        .uri()
        .path()
        .strip_prefix(&shared.prefix)
        .map(str::to_owned)
    else {
        return Ok(reply(StatusCode::NOT_FOUND, "unknown path"));
    };
    if key.is_empty() || key.split('/').any(|part| part.is_empty() || part == "..") {
        return Ok(reply(StatusCode::BAD_REQUEST, "invalid key"));
    }
    let group = stats_group(&key);

    match *req.method() {
        Method::PUT | Method::POST => {
            // ffmpeg sends the file with chunked encoding (length unknown in
            // advance), and S3 wants the length up front: so collect this one
            // file, upload it, and only then answer ffmpeg.
            let body = match Limited::new(req.into_body(), MAX_FILE_BYTES)
                .collect()
                .await
            {
                Ok(body) => body.to_bytes(),
                Err(e) => {
                    record(&shared, &group, 0, Some(format!("receiving {key}: {e}")));
                    return Ok(reply(StatusCode::BAD_REQUEST, "could not read body"));
                }
            };
            let size = body.len() as u64;
            match shared.storage.put(&shared.bucket, &key, body).await {
                Ok(()) => {
                    debug!(%key, size, "uploaded");
                    record(&shared, &group, size, None);
                    Ok(reply(StatusCode::CREATED, ""))
                }
                Err(e) => {
                    let e = format!("{e:#}");
                    warn!(%key, error = %e, "upload failed");
                    record(&shared, &group, 0, Some(e));
                    Ok(reply(StatusCode::BAD_GATEWAY, "upload to storage failed"))
                }
            }
        }
        // The DASH muxer may remove segments it no longer lists.
        Method::DELETE => match shared.storage.delete(&shared.bucket, &key).await {
            Ok(()) => {
                debug!(%key, "deleted");
                Ok(reply(StatusCode::NO_CONTENT, ""))
            }
            Err(e) => {
                warn!(%key, error = format!("{e:#}"), "delete failed");
                Ok(reply(StatusCode::BAD_GATEWAY, "delete failed"))
            }
        },
        _ => Ok(reply(StatusCode::METHOD_NOT_ALLOWED, "")),
    }
}

/// "task/hls/720p/seg_001.ts" -> "task/hls": statistics are kept per job.
fn stats_group(key: &str) -> String {
    key.splitn(3, '/').take(2).collect::<Vec<_>>().join("/")
}

fn record(shared: &Shared, group: &str, bytes: u64, error: Option<String>) {
    let mut stats = shared.stats.lock().expect("stats lock poisoned");
    let entry = stats.entry(group.to_string()).or_default();
    match error {
        None => {
            entry.files += 1;
            entry.bytes += bytes;
        }
        Some(e) => {
            entry.failed += 1;
            entry.last_error = Some(e);
        }
    }
}

fn reply(status: StatusCode, body: &'static str) -> Reply {
    let mut response = Response::new(Full::new(Bytes::from_static(body.as_bytes())));
    *response.status_mut() = status;
    response
}

/// 128 random bits as hex, from the kernel.
fn random_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .context("cannot read /dev/urandom")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_stats_per_job() {
        assert_eq!(stats_group("movie/hls/720p/seg_001.ts"), "movie/hls");
        assert_eq!(stats_group("movie/dash/manifest.mpd"), "movie/dash");
        assert_eq!(stats_group("movie"), "movie");
    }

    #[test]
    fn tokens_are_random() {
        let (a, b) = (random_token().unwrap(), random_token().unwrap());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
