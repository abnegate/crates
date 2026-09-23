//! Resumable downloads of large model files.
//!
//! [`download_gguf`] writes into a sibling `.part` file and renames it onto
//! the target only once the transfer is complete and, when the caller has
//! one, its SHA-256 matches. A resume is only ever spliced onto the bytes of
//! the same upstream file: the `.part` file's URL and its `ETag` or
//! `Last-Modified` are kept beside it, the validator is sent as `If-Range`,
//! and a range that does not start where the `.part` file ends is thrown
//! away rather than appended.

mod checksum;
mod content_range;
mod error;
mod lock;
mod progress;
mod validator;
mod writer;

use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use reqwest::StatusCode;
use reqwest::header::CONTENT_LENGTH;
use reqwest::header::HeaderMap;
use reqwest::header::IF_RANGE;
use reqwest::header::RANGE;
use sha2::Digest;
use sha2::Sha256;
use tokio::fs;

pub use crate::download::checksum::Checksum;
pub use crate::download::error::DownloadError;
pub use crate::download::progress::DownloadProgress;

use crate::download::content_range::ContentRange;
use crate::download::lock::TransferLock;
use crate::download::validator::Validator;
use crate::download::writer::Writer;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest the server may go without sending a byte. A whole-transfer
/// deadline would cut off a multi-gigabyte file on a slow link.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const HASH_BUFFER_BYTES: usize = 1 << 20;
/// A resume, then one fresh start if the resume could not be trusted.
const ATTEMPTS: usize = 2;

/// Download a GGUF file, resuming a partial download when one is present.
///
/// Bytes land in a sibling `.part` file that is renamed onto `target` only once
/// the transfer completes, so an interrupted download never looks finished.
/// When `expected` is given, a completed file whose SHA-256 differs is deleted
/// and reported instead of installed.
///
/// One transfer to a target runs at a time, in this process or any other: a
/// second is refused with [`DownloadError::InProgress`] rather than left to
/// write into the same `.part` file. A transfer that fails or is dropped keeps
/// the target until the bytes it had already received are on disk.
pub async fn download_gguf(
    url: &str,
    target: &Path,
    expected: Option<Checksum>,
    progress: Arc<DownloadProgress>,
) -> Result<(), DownloadError> {
    match transfer(url, target, expected, &progress).await {
        Ok(()) => {
            progress.completed.store(true, Ordering::Relaxed);
            Ok(())
        }
        Err(error) => {
            progress.fail(error.to_string());
            Err(error)
        }
    }
}

async fn transfer(
    url: &str,
    target: &Path,
    expected: Option<Checksum>,
    progress: &DownloadProgress,
) -> Result<(), DownloadError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).await?;
    }

    let part = part_path(target);
    let lock = Arc::new(TransferLock::acquire(&part).await?);
    let client = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()?;

    for _ in 0..ATTEMPTS {
        if fetch(&client, url, &part, &lock, progress).await? {
            return finish(&part, target, expected).await;
        }
        discard(&part).await?;
    }
    Err(DownloadError::Inconsistent)
}

/// Bring the `.part` file up to the whole upstream file.
///
/// `false` means what is on disk cannot be continued and must be discarded
/// before the next attempt starts from the first byte: the server would not
/// resume it, or the `.part` file is not the length this transfer wrote, so
/// its end is no longer the offset of the next upstream byte. A body that
/// fails partway leaves the `.part` file for the next call to resume, unless
/// its length is wrong too, when it is discarded before the error returns.
async fn fetch(
    client: &Client,
    url: &str,
    part: &Path,
    lock: &Arc<TransferLock>,
    progress: &DownloadProgress,
) -> Result<bool, DownloadError> {
    let existing = fs::metadata(part).await.map(|file| file.len()).unwrap_or(0);
    let validator = match existing {
        0 => None,
        _ => Validator::load(part, url).await,
    };
    let guard = validator
        .as_ref()
        .and_then(Validator::if_range)
        .map(str::to_string);

    let mut request = client.get(url);
    if let Some(guard) = &guard {
        request = request
            .header(RANGE, format!("bytes={existing}-"))
            .header(IF_RANGE, guard);
    }
    let response = request.send().await?;
    let status = response.status();

    if status == StatusCode::RANGE_NOT_SATISFIABLE && guard.is_some() {
        let complete = remote_length(client, url).await == Some(existing);
        if complete {
            progress.total_bytes.store(existing, Ordering::Relaxed);
            progress.downloaded_bytes.store(existing, Ordering::Relaxed);
        }
        return Ok(complete);
    }
    if !status.is_success() {
        return Err(DownloadError::Status(status.as_u16()));
    }

    let (offset, total) = if status == StatusCode::PARTIAL_CONTENT {
        match ContentRange::from_headers(response.headers()) {
            Some(range) if guard.is_some() && range.start == existing => {
                (existing, range.file_length(response.content_length())?)
            }
            _ => return Ok(false),
        }
    } else {
        (0, response.content_length())
    };

    progress
        .total_bytes
        .store(total.unwrap_or_default(), Ordering::Relaxed);
    progress.downloaded_bytes.store(offset, Ordering::Relaxed);

    let file = if offset > 0 {
        open(part, OpenOptions::new().append(true), lock).await?
    } else {
        restart(url, part, response.headers(), lock).await?
    };
    let writer = Writer::spawn(file, lock);
    let (piped, streamed) = pipe(response.bytes_stream(), &writer, progress).await;
    writer.finish().await?;
    let received = offset + piped;
    if fs::metadata(part).await?.len() != received {
        return match streamed {
            Ok(()) => Ok(false),
            Err(error) => {
                discard(part).await?;
                Err(error)
            }
        };
    }
    streamed?;

    match total {
        Some(expected) if expected != received => {
            Err(DownloadError::Incomplete { expected, received })
        }
        _ => Ok(true),
    }
}

/// Queue a body's chunks on `writer`, and count the bytes queued, even
/// when the body fails partway.
///
/// Stops early once the writer has stopped, whose error [`Writer::finish`]
/// reports.
async fn pipe<Chunk>(
    chunks: impl Stream<Item = reqwest::Result<Chunk>>,
    writer: &Writer<Chunk>,
    progress: &DownloadProgress,
) -> (u64, Result<(), DownloadError>)
where
    Chunk: AsRef<[u8]> + Send + 'static,
{
    let mut chunks = std::pin::pin!(chunks);
    let mut piped = 0;
    while let Some(chunk) = chunks.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => return (piped, Err(error.into())),
        };
        let length = chunk.as_ref().len() as u64;
        if !writer.write(chunk).await {
            break;
        }
        piped += length;
        progress
            .downloaded_bytes
            .fetch_add(length, Ordering::Relaxed);
    }
    (piped, Ok(()))
}

/// Empty the `.part` file for a whole new body, and only then record which
/// file that body is.
///
/// The other order leaves the old bytes beside the new file's validator when
/// the open fails or the call is dropped between the two, and the next call
/// would resume the old bytes as though they were the start of the new file.
async fn restart(
    url: &str,
    part: &Path,
    headers: &HeaderMap,
    lock: &Arc<TransferLock>,
) -> Result<File, DownloadError> {
    let file = open(
        part,
        OpenOptions::new().create(true).write(true).truncate(true),
        lock,
    )
    .await?;
    Validator::from_headers(url, headers).store(part).await?;
    Ok(file)
}

/// Open the `.part` file under the lock, so an open that truncates it never
/// lands after the transfer that asked for it has let go.
async fn open(
    part: &Path,
    options: &OpenOptions,
    lock: &Arc<TransferLock>,
) -> std::io::Result<File> {
    let (part, options) = (part.to_path_buf(), options.clone());
    lock.hold(move || options.open(part))
        .await
        .map_err(std::io::Error::other)?
}

/// The length the server reports for the whole file, read from the header
/// because a `HEAD` response has no body to measure.
async fn remote_length(client: &Client, url: &str) -> Option<u64> {
    let response = client.head(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .headers()
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

async fn finish(
    part: &Path,
    target: &Path,
    expected: Option<Checksum>,
) -> Result<(), DownloadError> {
    if let Some(expected) = expected {
        let actual = digest(part).await?;
        if actual != expected {
            discard(part).await?;
            return Err(DownloadError::Checksum {
                expected: expected.to_string(),
                actual: actual.to_string(),
            });
        }
    }
    fs::rename(part, target).await?;
    remove(&Validator::path(part)).await?;
    Ok(())
}

async fn digest(path: &Path) -> Result<Checksum, DownloadError> {
    let path = path.to_path_buf();
    let digest = tokio::task::spawn_blocking(move || -> std::io::Result<Checksum> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(Checksum::new(hasher.finalize().into()))
    })
    .await
    .map_err(std::io::Error::other)??;
    Ok(digest)
}

async fn discard(part: &Path) -> Result<(), DownloadError> {
    remove(part).await?;
    remove(&Validator::path(part)).await
}

async fn remove(path: &Path) -> Result<(), DownloadError> {
    match fs::remove_file(path).await {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

fn part_path(target: &Path) -> PathBuf {
    target.with_extension(
        target
            .extension()
            .map(|extension| format!("{}.part", extension.to_string_lossy()))
            .unwrap_or_else(|| "part".to_string()),
    )
}

#[cfg(test)]
mod tests;
