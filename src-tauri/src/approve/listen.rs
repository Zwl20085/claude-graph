//! The loopback listener of the approval relay. One connection carries one
//! request line `{token, run_id, node_id, reg, arguments}` and gets one reply
//! line `{decision}`. A wrong token, a malformed line or a silent peer is
//! closed without a reply. When the asker closes its end before the user
//! answered, the prompt is withdrawn.

use std::net::SocketAddr;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::contract::{AppError, AppResult};

use super::hub::{Approvals, Inner, Submitted};
use super::wire::{deny, RelayReply, RelayRequest};
use super::CANCELLED;

/// Longest request line (a Write call carries the whole file content).
const REQUEST_MAX: u64 = 32 * 1024 * 1024;
/// How long a fresh connection may take to send its request line.
const REQUEST_WAIT: Duration = Duration::from_secs(30);

/// Bind `127.0.0.1:0` and serve on the async runtime; returns the address.
pub fn start(approvals: &Approvals) -> AppResult<SocketAddr> {
    let std_listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| AppError::new("io", format!("approval listener: {e}")))?;
    std_listener
        .set_nonblocking(true)
        .map_err(|e| AppError::new("io", format!("approval listener: {e}")))?;
    let addr = std_listener
        .local_addr()
        .map_err(|e| AppError::new("io", format!("approval listener: {e}")))?;
    let weak = Arc::downgrade(&approvals.inner);
    tauri::async_runtime::spawn(async move {
        match TcpListener::from_std(std_listener) {
            Ok(listener) => accept_loop(listener, weak).await,
            Err(e) => log::error!("approval listener could not start: {e}"),
        }
    });
    log::info!("approval listener on {addr}");
    Ok(addr)
}

async fn accept_loop(listener: TcpListener, weak: Weak<Inner>) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => {
                log::warn!("approval listener accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Some(inner) = weak.upgrade() else { return };
        let approvals = Approvals { inner };
        tokio::spawn(connection(stream, approvals));
    }
}

async fn read_request(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    let mut limited = (&mut *reader).take(REQUEST_MAX);
    let read = limited.read_until(b'\n', &mut buf);
    match tokio::time::timeout(REQUEST_WAIT, read).await {
        Ok(Ok(n)) if n > 0 && buf.ends_with(b"\n") => Some(buf),
        _ => None,
    }
}

/// Serve one relay connection.
pub async fn connection(stream: TcpStream, approvals: Approvals) {
    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd);
    let Some(line) = read_request(&mut reader).await else {
        log::warn!("approval connection closed: no request line");
        return;
    };
    let Ok(request) = serde_json::from_slice::<RelayRequest>(&line) else {
        log::warn!("approval connection closed: malformed request");
        return;
    };
    if !approvals.token_matches(&request.token) {
        log::warn!("approval connection closed: wrong token");
        return;
    }
    let decision = match approvals.submit(&request) {
        Submitted::Now(decision) => decision,
        Submitted::Wait(id, rx) => {
            let mut sink = Vec::new();
            let mut limited = (&mut reader).take(1024);
            let gone = limited.read_until(b'\n', &mut sink);
            tokio::select! {
                answer = rx => answer.unwrap_or_else(|_| deny(CANCELLED)),
                _ = gone => {
                    approvals.abandon(&id);
                    return;
                }
            }
        }
    };
    let reply = RelayReply { decision };
    let line = match serde_json::to_string(&reply) {
        Ok(l) => l,
        Err(e) => {
            log::error!("approval reply could not be encoded: {e}");
            return;
        }
    };
    if let Err(e) = wr.write_all(format!("{line}\n").as_bytes()).await {
        log::warn!("approval reply could not be sent: {e}");
    }
    if let Err(e) = wr.shutdown().await {
        log::debug!("approval connection shutdown: {e}");
    }
}
