//! Shared in-process file download used by config URL imports and plugin
//! installs.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// Downloads `url` into `dest` with a hard timeout and an optional size cap.
pub fn download_to_file(
    url: &str,
    dest: &Path,
    timeout: Duration,
    max_bytes: Option<u64>,
) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("request {url}"))?;
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
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
}
