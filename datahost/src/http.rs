//! A deliberately small HTTP/1.1 server on `std` alone: `GET`/`HEAD` with `Range`, one thread per
//! connection, `Connection: close` after each response. The clients are a few phones downloading a
//! dozen files, not the internet.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

/// A parsed request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The path, percent-decoded, without the query.
    pub path: String,
    pub query: Vec<(String, String)>,
    /// Header names lower-cased.
    pub headers: Vec<(String, String)>,
    /// The peer, for logs.
    pub peer: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn query(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The bearer token, from `Authorization: Bearer …` or `?token=…`.
    pub fn token(&self) -> Option<&str> {
        self.header("authorization")
            .and_then(|h| h.strip_prefix("Bearer "))
            .or_else(|| self.query("token"))
    }
}

/// Read one request's head off `r`. A body, if any, is not read: nothing here accepts one.
pub fn read_request(r: &mut impl BufRead) -> io::Result<Request> {
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no request"));
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), parse_query(q)),
        None => (target, Vec::new()),
    };
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
        if headers.len() > 100 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many headers",
            ));
        }
    }
    Ok(Request {
        method,
        path: percent_decode(&path),
        query,
        headers,
        peer: String::new(),
    })
}

pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(p), String::new()),
        })
        .collect()
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).ok();
                match h.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(v) => {
                        out.push(v);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
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

/// `Range: bytes=a-b` against a file of `size` bytes → the inclusive range to send. `None` for
/// a multi-range or unsatisfiable header (the caller answers 416).
pub fn parse_range(header: &str, size: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || size == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        // `bytes=-N`: the last N bytes.
        let n: u64 = b.parse().ok()?;
        (size.saturating_sub(n), size - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() {
            size - 1
        } else {
            b.parse::<u64>().ok()?.min(size - 1)
        };
        (start, end)
    };
    (start <= end && start < size).then_some((start, end))
}

/// A response's head.
pub struct Head<'a> {
    pub status: u16,
    pub content_type: &'a str,
    pub length: Option<u64>,
    pub extra: &'a [(&'a str, String)],
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        _ => "",
    }
}

pub fn write_head(w: &mut impl Write, head: &Head) -> io::Result<()> {
    write!(w, "HTTP/1.1 {} {}\r\n", head.status, reason(head.status))?;
    write!(w, "Content-Type: {}\r\n", head.content_type)?;
    if let Some(n) = head.length {
        write!(w, "Content-Length: {n}\r\n")?;
    }
    for (k, v) in head.extra {
        write!(w, "{k}: {v}\r\n")?;
    }
    write!(w, "Connection: close\r\nCache-Control: no-store\r\n\r\n")
}

/// A whole response in one go; `head_only` sends the head a `GET` would have had.
pub fn respond(
    w: &mut impl Write,
    status: u16,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> io::Result<()> {
    write_head(
        w,
        &Head {
            status,
            content_type,
            length: Some(body.len() as u64),
            extra: &[],
        },
    )?;
    if !head_only {
        w.write_all(body)?;
    }
    w.flush()
}

pub fn respond_text(
    w: &mut impl Write,
    status: u16,
    text: &str,
    head_only: bool,
) -> io::Result<()> {
    respond(
        w,
        status,
        "text/plain; charset=utf-8",
        text.as_bytes(),
        head_only,
    )
}

/// Serve `listener` forever, one thread per connection, handing each parsed request to
/// `handler` together with the stream to answer on.
pub fn serve<H>(listener: TcpListener, handler: H)
where
    H: Fn(Request, &mut TcpStream) + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        let handler = Arc::clone(&handler);
        let _ = std::thread::Builder::new()
            .name("inilla-http".into())
            .spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                let peer = stream
                    .peer_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_default();
                let Ok(read_half) = stream.try_clone() else {
                    return;
                };
                let mut reader = BufReader::new(read_half);
                match read_request(&mut reader) {
                    Ok(mut req) => {
                        req.peer = peer;
                        handler(req, &mut stream);
                    }
                    Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                        let _ = respond_text(&mut stream, 400, &e.to_string(), false);
                    }
                    Err(_) => {}
                }
            });
    }
}

/// Stream `len` bytes of `src` to `w` in chunks; returns how many were sent.
pub fn copy_n(src: &mut impl Read, w: &mut impl Write, len: u64) -> io::Result<u64> {
    let mut left = len;
    let mut buf = vec![0u8; 256 * 1024];
    while left > 0 {
        let want = buf.len().min(left as usize);
        let n = src.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
        left -= n as u64;
    }
    Ok(len - left)
}

/// Constant-time token comparison.
pub fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_parses_with_query_and_headers() {
        let raw = b"GET /files/a%20b?x=1&token=t%2Bu HTTP/1.1\r\nHost: h\r\nAuthorization: Bearer tok\r\n\r\n";
        let req = read_request(&mut &raw[..]).unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/files/a b");
        assert_eq!(req.query("token"), Some("t+u"));
        assert_eq!(req.header("host"), Some("h"));
        assert_eq!(req.token(), Some("tok"), "the header wins over the query");
    }

    #[test]
    fn ranges_parse_and_clamp() {
        assert_eq!(parse_range("bytes=0-", 10), Some((0, 9)));
        assert_eq!(parse_range("bytes=4-6", 10), Some((4, 6)));
        assert_eq!(parse_range("bytes=4-100", 10), Some((4, 9)));
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=10-", 10), None);
        assert_eq!(parse_range("bytes=0-1,4-5", 10), None);
        assert_eq!(parse_range("items=0-1", 10), None);
    }

    #[test]
    fn percent_decoding_survives_malformed_input() {
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("a+b"), "a b");
    }

    #[test]
    fn tokens_compare_exactly() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "ab"));
    }
}
