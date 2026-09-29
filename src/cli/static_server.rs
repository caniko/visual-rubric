use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use percent_encoding::percent_decode_str;

pub(super) struct StaticServer {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    port: u16,
}

impl StaticServer {
    pub(super) fn start(root: PathBuf, port: u16) -> Result<Self> {
        let listener =
            TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("bind port {port}"))?;
        listener
            .set_nonblocking(true)
            .context("set static server nonblocking")?;
        let port = listener
            .local_addr()
            .context("read static server addr")?
            .port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = serve_static_request(stream, &root);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            stop,
            handle: Some(handle),
            port,
        })
    }

    pub(super) fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    pub(super) fn wait_forever(mut self) -> Result<()> {
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| anyhow!("static server thread panicked"))?;
        }
        Ok(())
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve_static_request(
    mut stream: impl std::io::Read + std::io::Write,
    root: &Path,
) -> Result<()> {
    let mut buf = [0; 2048];
    let mut n = 0;
    // A TCP read can end anywhere in the request line, including inside the
    // path. Keep the original byte bound while waiting for the complete line.
    while n < buf.len() && !buf[..n].contains(&b'\n') {
        let count = stream.read(&mut buf[n..]).context("read request")?;
        if count == 0 {
            break;
        }
        n += count;
    }
    if !buf[..n].contains(&b'\n') {
        return write_http_response(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            b"invalid request line",
            false,
        );
    }
    let request = String::from_utf8_lossy(&buf[..n]);
    let mut request_parts = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = request_parts.next().unwrap_or_default();
    let request_path = request_parts.next().map_or("/", |path| path);
    if method != "GET" && method != "HEAD" {
        return write_http_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            b"method not allowed",
            method == "HEAD",
        );
    }
    let file = resolve_static_path(root, request_path);
    if let Ok(bytes) = fs::read(&file) {
        write_http_response(
            &mut stream,
            "200 OK",
            content_type(&file),
            &bytes,
            method == "HEAD",
        )?;
    } else {
        write_http_response(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found",
            method == "HEAD",
        )?;
    }
    Ok(())
}

pub(super) fn resolve_static_path(root: &Path, request_path: &str) -> PathBuf {
    let request_path_without_query = request_path
        .split_once('?')
        .map_or(request_path, |(path, _query)| path);
    let Some(clean) = percent_decode_path(request_path_without_query) else {
        return root.join("__invalid__");
    };
    let clean = clean.trim_start_matches('/');
    if clean.is_empty() {
        return root.join("index.html");
    }
    if clean
        .split('/')
        .any(|component| component == "." || component == "..")
    {
        return root.join("__invalid__");
    }
    let path = root.join(clean);
    if request_path_without_query.ends_with('/') {
        path.join("index.html")
    } else {
        path
    }
}

pub(super) fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("gif") => "image/gif",
        Some("html") => "text/html; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("txt") => "text/plain; charset=utf-8",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

fn write_http_response(
    stream: &mut impl std::io::Write,
    status: &str,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    if !head_only {
        stream.write_all(body)?;
    }
    Ok(())
}

fn percent_decode_path(path: &str) -> Option<String> {
    if !has_valid_percent_escapes(path) {
        return None;
    }
    percent_decode_str(path)
        .decode_utf8()
        .ok()
        .map(std::borrow::Cow::into_owned)
}

fn has_valid_percent_escapes(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(hi) = bytes.get(index + 1) else {
                return false;
            };
            let Some(lo) = bytes.get(index + 2) else {
                return false;
            };
            if !hi.is_ascii_hexdigit() || !lo.is_ascii_hexdigit() {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor, Read, Write};

    struct FragmentedRequest {
        input: Cursor<Vec<u8>>,
        response: Vec<u8>,
    }

    impl Read for FragmentedRequest {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let len = buf.len().min(6);
            self.input.read(&mut buf[..len])
        }
    }

    impl Write for FragmentedRequest {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.response.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn fragmented_request_line_is_read_before_resolving_path() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("install.html"), "Install").unwrap();
        let mut stream = FragmentedRequest {
            input: Cursor::new(b"GET /install.html HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec()),
            response: Vec::new(),
        };
        serve_static_request(&mut stream, temp.path()).unwrap();
        let response = String::from_utf8(stream.response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.ends_with("\r\n\r\nInstall"), "{response}");
    }

    #[test]
    fn incomplete_or_oversized_request_lines_are_rejected() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("index.html"), "Index").unwrap();
        for request in [
            "GET /".to_owned(),
            format!("GET /{} HTTP/1.1\r\n", "x".repeat(2048)),
        ] {
            let mut stream = FragmentedRequest {
                input: Cursor::new(request.into_bytes()),
                response: Vec::new(),
            };
            serve_static_request(&mut stream, temp.path()).unwrap();
            let response = String::from_utf8(stream.response).unwrap();
            assert!(
                response.starts_with("HTTP/1.1 400 Bad Request"),
                "{response}"
            );
            assert!(stream.input.position() <= 2048);
        }
    }
}
