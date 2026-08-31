//! Shared in-process file download used by config URL imports and plugin
//! installs.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpValidators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadOutcome {
    Updated(HttpValidators),
    NotModified(HttpValidators),
}

/// Downloads `url` into `dest` with a hard timeout and an optional size cap.
pub fn download_to_file(
    url: &str,
    dest: &Path,
    timeout: Duration,
    max_bytes: Option<u64>,
) -> Result<()> {
    match download_to_file_conditional(url, dest, timeout, max_bytes, None)? {
        DownloadOutcome::Updated(_) => Ok(()),
        DownloadOutcome::NotModified(_) => {
            bail!("download {url} returned not modified without validators")
        }
    }
}

pub fn download_to_file_conditional(
    url: &str,
    dest: &Path,
    timeout: Duration,
    max_bytes: Option<u64>,
    validators: Option<&HttpValidators>,
) -> Result<DownloadOutcome> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .into();
    let mut request = agent.get(url);
    if let Some(etag) = validators.and_then(|validators| validators.etag.as_deref()) {
        request = request.header("If-None-Match", etag);
    }
    if let Some(last_modified) =
        validators.and_then(|validators| validators.last_modified.as_deref())
    {
        request = request.header("If-Modified-Since", last_modified);
    }
    let mut response = request.call().with_context(|| format!("request {url}"))?;
    let response_validators = HttpValidators {
        etag: response
            .headers()
            .get("ETag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        last_modified: response
            .headers()
            .get("Last-Modified")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    };
    if response.status().as_u16() == 304 {
        return Ok(DownloadOutcome::NotModified(HttpValidators {
            etag: response_validators
                .etag
                .or_else(|| validators.and_then(|validators| validators.etag.clone())),
            last_modified: response_validators
                .last_modified
                .or_else(|| validators.and_then(|validators| validators.last_modified.clone())),
        }));
    }
    let mut source = response.body_mut().as_reader();
    let mut destination =
        File::create(dest).with_context(|| format!("create download {}", dest.display()))?;

    let copied = match max_bytes {
        Some(limit) => io::copy(&mut source.take(limit.saturating_add(1)), &mut destination),
        None => io::copy(&mut source, &mut destination),
    }
    .with_context(|| format!("write download {}", dest.display()))?;

    if let Some(limit) = max_bytes {
        if copied > limit {
            bail!("download {url} exceeded the {limit}-byte limit");
        }
    }
    Ok(DownloadOutcome::Updated(response_validators))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    fn serve_once(body: Vec<u8>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });
        (format!("http://{address}/download"), server)
    }

    #[test]
    fn downloads_without_an_external_process() {
        let (url, server) = serve_once(b"downloaded".to_vec());
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("download");

        download_to_file(&url, &destination, Duration::from_secs(5), None).unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), b"downloaded");
        server.join().unwrap();
    }

    #[test]
    fn stops_after_the_configured_size_limit() {
        let (url, server) = serve_once(b"too large".to_vec());
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("download");

        let error =
            download_to_file(&url, &destination, Duration::from_secs(5), Some(3)).unwrap_err();

        assert!(format!("{error:#}").contains("exceeded the 3-byte limit"));
        assert_eq!(std::fs::metadata(destination).unwrap().len(), 4);
        server.join().unwrap();
    }

    #[test]
    fn conditional_download_sends_validators_and_accepts_not_modified() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            assert!(request.contains("if-none-match: \"rules-v1\""));
            assert!(request.contains("if-modified-since: wed, 29 jul 2026 12:00:00 gmt"));
            write!(
                stream,
                "HTTP/1.1 304 Not Modified\r\nETag: \"rules-v1\"\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("download");
        let validators = HttpValidators {
            etag: Some("\"rules-v1\"".to_string()),
            last_modified: Some("Wed, 29 Jul 2026 12:00:00 GMT".to_string()),
        };

        let outcome = download_to_file_conditional(
            &format!("http://{address}/download"),
            &destination,
            Duration::from_secs(5),
            None,
            Some(&validators),
        )
        .unwrap();

        assert_eq!(
            outcome,
            DownloadOutcome::NotModified(HttpValidators {
                etag: Some("\"rules-v1\"".to_string()),
                last_modified: Some("Wed, 29 Jul 2026 12:00:00 GMT".to_string()),
            })
        );
        assert!(!destination.exists());
        server.join().unwrap();
    }
}
