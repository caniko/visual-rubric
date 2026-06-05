use std::fs;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};

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

fn serve_static_request(mut stream: TcpStream, root: &Path) -> Result<()> {
    let mut buf = [0; 2048];
    let n = stream.read(&mut buf).context("read request")?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let mut request_parts = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = request_parts.next().unwrap_or_default();
    let request_path = request_parts.next().unwrap_or("/");
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
    let request_path_without_query = request_path.split('?').next().unwrap_or("/");
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
    stream: &mut TcpStream,
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
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = *bytes.get(i + 1)?;
            let lo = *bytes.get(i + 2)?;
            decoded.push(hex_value(hi)? * 16 + hex_value(lo)?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
