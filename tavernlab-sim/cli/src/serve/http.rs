//! Just enough HTTP to serve one local app.
//!
//! There is no framework here for the same reason there is no serde: this
//! workspace carries no third-party dependencies. What that costs is written
//! down honestly — this speaks HTTP/1.1 with `Connection: close`, no
//! keep-alive, no chunked bodies, no TLS, and it binds to loopback only. What
//! it buys is that the whole request path is a few hundred lines you can read.
//!
//! The server is not on a network and must not behave as if that makes it
//! safe. Three things are checked rather than assumed:
//!
//! * the listener binds `127.0.0.1`, so nothing off the machine can reach it;
//! * the `Host` header must name loopback, which is what stops a page on the
//!   internet from pointing a DNS name at 127.0.0.1 and driving this API
//!   through the user's own browser;
//! * request lines, headers and bodies are all capped, so a stuck or hostile
//!   client cannot make the process allocate without bound.

use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Longest request line or header line accepted.
const MAX_LINE: usize = 8 * 1024;
/// Most headers accepted.
const MAX_HEADERS: usize = 64;
/// Largest request body accepted. Deck codes are a few hundred bytes; this
/// is generous by three orders of magnitude and still bounded.
const MAX_BODY: usize = 1024 * 1024;
/// Connections handled at once. A browser opens a handful; anything past
/// this is answered with 503 rather than spawning threads without end.
const MAX_CONNECTIONS: usize = 64;

pub struct Request {
    pub method: String,
    /// Percent-decoded path, without the query string.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The first value for a query parameter.
    pub fn param(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn body_str(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap_or("")
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// A `Cache-Control` value, for the art that is immutable by card id.
    pub cache: Option<&'static str>,
}

impl Response {
    pub fn json(status: u16, body: String) -> Response {
        Response {
            status,
            content_type: "application/json; charset=utf-8",
            body: body.into_bytes(),
            cache: None,
        }
    }

    pub fn text(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status,
            content_type,
            body: body.into(),
            cache: None,
        }
    }

    /// A JSON error in the shape the front end reads: `{"error": "..."}`.
    pub fn error(status: u16, msg: &str) -> Response {
        Response::json(
            status,
            tavernlab_json::to_string(|o| o.obj(|o| o.str_field("error", msg))),
        )
    }
}

/// Serve until the process is stopped. `handle` runs on a worker thread per
/// connection, so it must be `Send + Sync`.
pub fn serve(
    listener: TcpListener,
    handle: impl Fn(&Request) -> Response + Send + Sync + 'static,
) -> std::io::Result<()> {
    let handle = Arc::new(handle);
    let live = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if live.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
            let _ = write_response(
                &stream,
                &Response::error(503, "server busy: too many open connections"),
            );
            continue;
        }
        live.fetch_add(1, Ordering::Relaxed);
        let handle = Arc::clone(&handle);
        let live = Arc::clone(&live);
        // A panic in one request must not take the server down with it; a
        // detached thread contains it, and the client sees a dropped
        // connection rather than a dead app.
        let _ = std::thread::Builder::new()
            .name("tavernlab-http".into())
            .spawn(move || {
                serve_one(stream, handle.as_ref());
                live.fetch_sub(1, Ordering::Relaxed);
            });
    }
    Ok(())
}

fn serve_one(stream: TcpStream, handle: &(impl Fn(&Request) -> Response + ?Sized)) {
    // A client that opens a connection and says nothing must not hold a
    // thread forever.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let response = match read_request(&stream) {
        Ok(req) => handle(&req),
        Err(e) => Response::error(e.status, &e.msg),
    };
    let _ = write_response(&stream, &response);
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

pub struct ReadError {
    pub status: u16,
    pub msg: String,
}

fn bad(status: u16, msg: &str) -> ReadError {
    ReadError {
        status,
        msg: msg.to_string(),
    }
}

fn read_request(stream: &TcpStream) -> Result<Request, ReadError> {
    let mut reader = BufReader::new(stream);
    let line = read_line(&mut reader)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("");
    if method.is_empty() || target.is_empty() {
        return Err(bad(400, "malformed request line"));
    }

    let (raw_path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let path = percent_decode(raw_path);
    let query = parse_query(raw_query);

    let mut length = 0usize;
    let mut host = String::new();
    let mut headers_ended = false;
    for _ in 0..MAX_HEADERS {
        let line = read_line(&mut reader)?;
        if line.is_empty() {
            headers_ended = true;
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(bad(400, "malformed header"));
        };
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => {
                length = value.parse().map_err(|_| bad(400, "bad Content-Length"))?;
                if length > MAX_BODY {
                    return Err(bad(413, "request body too large"));
                }
            }
            "host" => host = value.to_string(),
            // Nothing here streams, and a body arriving in chunks would be
            // silently truncated if it were accepted quietly.
            "transfer-encoding" if value.eq_ignore_ascii_case("chunked") => {
                return Err(bad(411, "chunked request bodies are not supported"));
            }
            _ => {}
        }
    }

    if !headers_ended {
        // Past the cap the blank line separating headers from body has not
        // been seen, so whatever comes next is not the body: reading it as
        // one would silently mis-parse the request.
        return Err(bad(431, "too many request headers"));
    }
    if !host_is_loopback(&host) {
        // DNS rebinding: a page on the internet resolves its own name to
        // 127.0.0.1 and then talks to this API with the user's browser. The
        // Host header is the one thing that still names the attacker.
        return Err(bad(403, "this API answers only to a loopback Host"));
    }

    let mut body = vec![0u8; length];
    if length > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|_| bad(400, "request body ended early"))?;
    }
    Ok(Request {
        method,
        path,
        query,
        body,
    })
}

fn read_line(reader: &mut BufReader<&TcpStream>) -> Result<String, ReadError> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => return Err(bad(400, "connection closed mid-request")),
            Ok(_) => {}
            Err(_) => return Err(bad(408, "timed out reading the request")),
        }
        if byte[0] == b'\n' {
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return String::from_utf8(buf).map_err(|_| bad(400, "request line is not UTF-8"));
        }
        buf.push(byte[0]);
        if buf.len() > MAX_LINE {
            return Err(bad(431, "request header too long"));
        }
    }
}

/// Whether a `Host` header names this machine.
fn host_is_loopback(host: &str) -> bool {
    // An IPv6 literal is bracketed and its own colons are not a port
    // separator, so it cannot be split the same way as a name.
    let name = if let Some(rest) = host.strip_prefix('[') {
        match rest.split_once(']') {
            Some((h, _)) => h,
            None => return false,
        }
    } else {
        host.split(':').next().unwrap_or("")
    };
    matches!(name, "localhost" | "::1") || name.starts_with("127.")
}

fn write_response(mut stream: &TcpStream, res: &Response) -> std::io::Result<()> {
    let reason = match res.status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        res.status,
        res.content_type,
        res.body.len()
    );
    // Everything without an explicit policy is generated per request and
    // must not be reused: the front end polls a job every 700 ms and would
    // otherwise read a cached "still running" forever.
    let cache = res.cache.unwrap_or("no-store");
    head.push_str(&format!("Cache-Control: {cache}\r\n"));
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&res.body)?;
    stream.flush()
}

fn parse_query(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match hex(b[i + 1]).zip(hex(b[i + 2])) {
                Some((h, l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                }
                None => {
                    out.push(b[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_percent_encoded_path_comes_back_decoded() {
        assert_eq!(
            percent_decode("/api/art/tile/CS2_029"),
            "/api/art/tile/CS2_029"
        );
        assert_eq!(percent_decode("Quest%20Hunter"), "Quest Hunter");
        assert_eq!(
            percent_decode("%D0%9A%D0%BE%D0%BB%D0%BE%D0%B4%D0%B0"),
            "Колода"
        );
        // A stray percent is data, not a parse error: the alternative is
        // dropping a request over a character in a deck name.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a+b"), "a b");
    }

    #[test]
    fn a_query_string_splits_into_pairs() {
        let q = parse_query("format=wild&limit=10&flag&name=Quest%20Hunter");
        assert_eq!(q[0], ("format".into(), "wild".into()));
        assert_eq!(q[2], ("flag".into(), String::new()));
        assert_eq!(q[3].1, "Quest Hunter");
        assert!(parse_query("").is_empty());
    }

    #[test]
    fn only_a_loopback_host_is_answered() {
        assert!(host_is_loopback("127.0.0.1:8765"));
        assert!(host_is_loopback("localhost:8765"));
        assert!(host_is_loopback("localhost"));
        assert!(host_is_loopback("[::1]:8765"));
        assert!(host_is_loopback("127.0.0.2"));
        // The rebinding case: a real name that currently resolves here.
        assert!(!host_is_loopback("evil.example.com"));
        assert!(!host_is_loopback("192.168.1.10:8765"));
        assert!(!host_is_loopback(""));
    }
}
