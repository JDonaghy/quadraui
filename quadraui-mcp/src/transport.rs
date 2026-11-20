//! How an MCP client reaches this crate's server: this process's own
//! stdio, or a socket it can attach to from outside.
//!
//! A GUI or headless app can share its own stdin/stdout with an MCP
//! client. A TUI app cannot — it owns the terminal's stdio for rendering
//! — so [`Transport::Unix`]/[`Transport::Tcp`] let an agent attach from a
//! second process instead. Every transport speaks the same line-
//! delimited JSON-RPC framing (one request or response per line, no
//! embedded newlines — the MCP spec's own stdio framing, reused here for
//! the socket transports too rather than inventing a second framing).
//!
//! No async runtime: each connection (the single stdio session, or one
//! per accepted socket connection) is serviced by a blocking read loop on
//! its own OS thread via [`std::thread::spawn`], so arbitrarily many MCP
//! clients can attach to a socket transport at once without pulling in
//! `tokio`.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
#[cfg(unix)]
use std::path::PathBuf;

use quadraui::control::ControlHandle;

/// Where [`crate::serve`] listens for an MCP client.
///
/// `#[non_exhaustive]` so a future variant (e.g. a Windows named pipe)
/// does not break a consumer's `match`.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum Transport {
    /// This process's own stdin/stdout.
    Stdio,
    /// A Unix-domain socket at this path, created fresh on
    /// [`crate::serve`] (an existing file at the same path is removed
    /// first — the stale remnant of a prior run that didn't shut down
    /// cleanly, same convention most Unix-socket servers use). Only
    /// available on Unix targets.
    #[cfg(unix)]
    Unix(PathBuf),
    /// A loopback TCP listener. Works on every target this crate builds
    /// for, so it is also this crate's story for Windows — the issue
    /// that added this module allows a named pipe there too, but a
    /// loopback socket needs nothing platform-specific and already
    /// satisfies "attach from outside the process that owns stdio" on
    /// every platform.
    Tcp(SocketAddr),
}

/// Parses `QUADRAUI_MCP`-style transport strings: `"stdio"`, `"unix:
/// <path>"`, `"tcp:<host>:<port>"`. Returns `None` on anything else —
/// callers that want a different error shape should match the env var
/// themselves instead of going through this helper.
impl Transport {
    pub fn parse(spec: &str) -> Option<Self> {
        if spec == "stdio" {
            return Some(Transport::Stdio);
        }
        #[cfg(unix)]
        if let Some(path) = spec.strip_prefix("unix:") {
            return Some(Transport::Unix(PathBuf::from(path)));
        }
        if let Some(addr) = spec.strip_prefix("tcp:") {
            return addr.parse().ok().map(Transport::Tcp);
        }
        None
    }
}

/// Runs `transport` until its listener (or, for [`Transport::Stdio`],
/// standard input) closes. Blocks the calling thread — see
/// [`crate::serve`]'s doc for how a caller that also owns a TUI event
/// loop runs this on a background thread instead.
pub fn serve(handle: ControlHandle, transport: Transport) -> io::Result<()> {
    match transport {
        Transport::Stdio => serve_stdio(handle),
        #[cfg(unix)]
        Transport::Unix(path) => serve_unix(handle, &path),
        Transport::Tcp(addr) => serve_tcp(handle, addr),
    }
}

fn serve_stdio(handle: ControlHandle) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve_session(&handle, stdin.lock(), stdout.lock())
}

#[cfg(unix)]
fn serve_unix(handle: ControlHandle, path: &std::path::Path) -> io::Result<()> {
    // A prior run's socket file that never got cleaned up (process
    // killed, not a graceful shutdown) — remove it so `bind` doesn't fail
    // with "address already in use" against a dead listener.
    let _ = std::fs::remove_file(path);
    let listener = std::os::unix::net::UnixListener::bind(path)?;
    for stream in listener.incoming() {
        let stream = stream?;
        let handle = handle.clone();
        std::thread::spawn(move || {
            let reader = stream.try_clone().expect("clone a connected UnixStream");
            let _ = serve_session(&handle, BufReader::new(reader), stream);
        });
    }
    Ok(())
}

fn serve_tcp(handle: ControlHandle, addr: SocketAddr) -> io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    for stream in listener.incoming() {
        let stream = stream?;
        let handle = handle.clone();
        std::thread::spawn(move || {
            let reader = stream.try_clone().expect("clone a connected TcpStream");
            let _ = serve_session(&handle, BufReader::new(reader), stream);
        });
    }
    Ok(())
}

/// One connection's read/dispatch/write loop, shared by every transport:
/// read a line, hand it to [`rpc::handle_line`], write back whatever
/// reply it returns (nothing, for a notification). Returns once `reader`
/// hits EOF (the client disconnected, or stdin closed).
fn serve_session<R: BufRead, W: Write>(
    handle: &ControlHandle,
    mut reader: R,
    mut writer: W,
) -> io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(response) = crate::handle_line(handle, trimmed) {
            writer.write_all(response.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_recognises_stdio_and_tcp() {
        assert!(matches!(Transport::parse("stdio"), Some(Transport::Stdio)));
        assert!(matches!(
            Transport::parse("tcp:127.0.0.1:9999"),
            Some(Transport::Tcp(_))
        ));
        assert!(Transport::parse("bogus").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn parse_recognises_unix() {
        assert!(matches!(
            Transport::parse("unix:/tmp/app.sock"),
            Some(Transport::Unix(_))
        ));
    }
}
