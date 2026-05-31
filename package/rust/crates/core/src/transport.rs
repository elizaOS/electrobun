//! Encrypted WebSocket host↔webview transport.
//!
//! This is the high-throughput channel the shipped webview JS speaks (see
//! `bun/core/Socket.ts` + the preload globals built in [`crate::preload`]). It
//! is a faithful hand-rolled port of the Zig `std.net`/`std.http` server so the
//! wire bytes stay identical to what the unchanged webview client expects:
//!
//! * TCP listener on `127.0.0.1`, scanning ports 50000..=65535 (or a single
//!   requested port), `SO_REUSEADDR`.
//! * One detached accept thread; one detached thread per connection.
//! * RFC6455: client→server frames MUST be masked and FIN; opcodes handled are
//!   0x1 (text), 0x8 (close), 0x9 (ping→0xA pong). Frames > 500MB are rejected.
//!   Server→client frames are unmasked.
//! * SHA-1 handshake (`key + magic`, base64) for the `Sec-WebSocket-Accept`.
//! * Each text frame payload is a JSON object `{encryptedData, iv, tag}` with
//!   base64 fields; AES-256-GCM with a per-webview key, 12-byte nonce, 16-byte
//!   tag, empty AAD.
//!
//! Decrypted inbound messages are pushed onto the host queue
//! ([`crate::host_queue`]); the first decryptable frame marks the webview's
//! transport "ready" so the host may start sending.
//!
//! Hand-rolled deliberately (no tokio/tungstenite) to avoid any wire drift.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::Mutex;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use once_cell::sync::Lazy;
use sha1::{Digest, Sha1};

use crate::error::set_last_error;
use crate::registry::{WebviewSecretKey, REGISTRIES};

const WEBSOCKET_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const PAYLOAD_LIMIT: usize = 1024 * 1024 * 500;
const PORT_RANGE_START: u16 = 50000;
const PORT_RANGE_END: u16 = 65535;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

struct HostTransportState {
    started: bool,
    port: u32,
}

static HOST_TRANSPORT: Lazy<Mutex<HostTransportState>> = Lazy::new(|| {
    Mutex::new(HostTransportState {
        started: false,
        port: 0,
    })
});

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Start the transport server (idempotent). On success sets the runtime
/// rpc_port to the bound port. `requested_port == 0` means "scan the range".
/// Mirrors `startHostTransportServer`.
pub fn start_host_transport_server(requested_port: u32) -> bool {
    let mut state = lock(&HOST_TRANSPORT);
    if state.started {
        crate::runtime::set_rpc_port(state.port);
        return true;
    }

    let (mut current_port, port_limit): (u16, u16) = if requested_port == 0 {
        (PORT_RANGE_START, PORT_RANGE_END)
    } else if let Ok(p) = u16::try_from(requested_port) {
        (p, p)
    } else {
        set_last_error(format!(
            "Requested websocket port is out of range: {requested_port}"
        ));
        return false;
    };

    while current_port <= port_limit {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, current_port);
        match bind_reuse(addr) {
            Ok(listener) => {
                let actual_port = match listener.local_addr() {
                    Ok(a) => a.port(),
                    Err(err) => {
                        set_last_error(format!("Failed to read websocket listen address: {err}"));
                        return false;
                    }
                };
                std::thread::spawn(move || accept_loop(listener));
                state.started = true;
                state.port = u32::from(actual_port);
                crate::runtime::set_rpc_port(u32::from(actual_port));
                return true;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                if current_port == port_limit {
                    break;
                }
                current_port += 1;
                continue;
            }
            Err(err) => {
                set_last_error(format!("Failed to start websocket server: {err}"));
                return false;
            }
        }
    }

    set_last_error("Unable to find an available websocket port");
    false
}

/// Bind a TCP listener with `SO_REUSEADDR`, matching the Zig
/// `listen(.{ .reuse_address = true })`.
fn bind_reuse(addr: SocketAddrV4) -> std::io::Result<TcpListener> {
    #[cfg(unix)]
    {
        use std::net::SocketAddr;
        use std::os::fd::FromRawFd;

        // SAFETY: standard socket(2)/setsockopt(2)/bind(2)/listen(2) sequence;
        // the fd is wrapped into an owning TcpListener on success and closed on
        // any early-return error path.
        unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let enable: libc::c_int = 1;
            if libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_REUSEADDR,
                &enable as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            ) != 0
            {
                let err = std::io::Error::last_os_error();
                libc::close(fd);
                return Err(err);
            }
            let sockaddr = SocketAddr::V4(addr);
            let mut storage: libc::sockaddr_in = std::mem::zeroed();
            storage.sin_family = libc::AF_INET as libc::sa_family_t;
            storage.sin_port = addr.port().to_be();
            storage.sin_addr.s_addr = u32::from_ne_bytes(addr.ip().octets());
            let _ = sockaddr;
            if libc::bind(
                fd,
                &storage as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            ) != 0
            {
                let err = std::io::Error::last_os_error();
                libc::close(fd);
                return Err(err);
            }
            if libc::listen(fd, 128) != 0 {
                let err = std::io::Error::last_os_error();
                libc::close(fd);
                return Err(err);
            }
            Ok(TcpListener::from_raw_fd(fd))
        }
    }
    #[cfg(not(unix))]
    {
        // Windows binds without explicit SO_REUSEADDR; AddrInUse still drives
        // the port scan. Functional parity for the localhost scan loop.
        TcpListener::bind(addr)
    }
}

fn accept_loop(listener: TcpListener) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                std::thread::spawn(move || handle_connection(stream));
            }
            Err(_) => break,
        }
    }
    let mut state = lock(&HOST_TRANSPORT);
    state.started = false;
    state.port = 0;
}

/// Read the HTTP request head (up to and including the terminating CRLFCRLF),
/// returning the head bytes plus any already-read overflow that belongs to the
/// first WebSocket frame.
fn read_http_head(stream: &mut TcpStream) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_double_crlf(&buf) {
            let head_end = pos + 4;
            let overflow = buf[head_end..].to_vec();
            buf.truncate(head_end);
            return Ok((buf, overflow));
        }
        if buf.len() > 64 * 1024 {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
        }
    }
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Extract a header value (case-insensitive name match), skipping the request
/// line. Mirrors `parseRequestHeaderValue`.
fn parse_request_header_value<'a>(headers: &'a str, header_name: &str) -> Option<&'a str> {
    let mut lines = headers.split("\r\n");
    lines.next(); // request line
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some(sep) = line.find(':') else { continue };
        let name = &line[..sep];
        if !name.eq_ignore_ascii_case(header_name) {
            continue;
        }
        return Some(line[sep + 1..].trim_matches([' ', '\t']));
    }
    None
}

/// Parse the `webviewId` query param from a `/socket?webviewId=N` target.
/// Mirrors `parseWebviewIdFromTarget`.
fn parse_webview_id_from_target(target: &str) -> Option<u32> {
    let query_index = target.find('?')?;
    let path = &target[..query_index];
    if path != "/socket" {
        return None;
    }
    let query = &target[query_index + 1..];
    for pair in query.split('&') {
        let Some(sep) = pair.find('=') else { continue };
        if &pair[..sep] != "webviewId" {
            continue;
        }
        return pair[sep + 1..].parse::<u32>().ok();
    }
    None
}

/// Request target (method line second token).
fn parse_request_target(headers: &str) -> Option<&str> {
    let request_line = headers.split("\r\n").next()?;
    request_line.split(' ').nth(1)
}

fn write_simple_http_response(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn write_websocket_handshake(stream: &mut TcpStream, websocket_key: &str) -> std::io::Result<()> {
    let mut hasher = Sha1::new();
    hasher.update(websocket_key.as_bytes());
    hasher.update(WEBSOCKET_MAGIC.as_bytes());
    let digest = hasher.finalize();
    let accept_value = BASE64.encode(digest);
    write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept_value}\r\n\r\n"
    )
}

/// Buffered reader that serves the post-head overflow before falling back to
/// the socket. Mirrors the Zig `PendingStreamReader`.
struct PendingStreamReader<'a> {
    stream: &'a mut TcpStream,
    pending: Vec<u8>,
    index: usize,
}

impl<'a> PendingStreamReader<'a> {
    fn read_exact(&mut self, dest: &mut [u8]) -> std::io::Result<()> {
        let mut written = 0;
        if self.index < self.pending.len() {
            let available = self.pending.len() - self.index;
            let to_copy = dest.len().min(available);
            dest[..to_copy].copy_from_slice(&self.pending[self.index..self.index + to_copy]);
            self.index += to_copy;
            written += to_copy;
        }
        if written == dest.len() {
            return Ok(());
        }
        self.stream.read_exact(&mut dest[written..])
    }
}

struct WebSocketFrame {
    opcode: u8,
    payload: Vec<u8>,
}

#[derive(Debug)]
enum FrameError {
    ConnectionClosed,
    Invalid,
    TooLarge,
    Io,
}

fn read_websocket_frame(reader: &mut PendingStreamReader) -> Result<WebSocketFrame, FrameError> {
    let mut header = [0u8; 2];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(FrameError::ConnectionClosed)
        }
        Err(_) => return Err(FrameError::Io),
    }

    let fin = (header[0] & 0x80) != 0;
    let opcode = header[0] & 0x0F;
    let masked = (header[1] & 0x80) != 0;
    let mut payload_len: usize = (header[1] & 0x7F) as usize;

    // Zig treats non-FIN as unsupported and unmasked as invalid → both break
    // the loop. We surface as Invalid (caller breaks).
    if !fin || !masked {
        return Err(FrameError::Invalid);
    }

    if payload_len == 126 {
        let mut ext = [0u8; 2];
        reader.read_exact(&mut ext).map_err(|_| FrameError::Io)?;
        payload_len = ((ext[0] as usize) << 8) | (ext[1] as usize);
    } else if payload_len == 127 {
        let mut ext = [0u8; 8];
        reader.read_exact(&mut ext).map_err(|_| FrameError::Io)?;
        let mut parsed: u64 = 0;
        for byte in ext {
            parsed = (parsed << 8) | (byte as u64);
        }
        payload_len = usize::try_from(parsed).map_err(|_| FrameError::TooLarge)?;
    }

    if payload_len > PAYLOAD_LIMIT {
        return Err(FrameError::TooLarge);
    }

    let mut mask = [0u8; 4];
    reader.read_exact(&mut mask).map_err(|_| FrameError::Io)?;

    let mut payload = vec![0u8; payload_len];
    reader
        .read_exact(&mut payload)
        .map_err(|_| FrameError::Io)?;
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[index % mask.len()];
    }

    Ok(WebSocketFrame { opcode, payload })
}

/// Write a server→client (unmasked) frame. Mirrors `writeWebSocketFrame`.
fn write_websocket_frame(
    stream: &mut TcpStream,
    opcode: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    let mut header = [0u8; 10];
    let mut header_len = 0;
    header[header_len] = 0x80 | (opcode & 0x0F);
    header_len += 1;

    if payload.len() <= 125 {
        header[header_len] = payload.len() as u8;
        header_len += 1;
    } else if payload.len() <= u16::MAX as usize {
        header[header_len] = 126;
        header_len += 1;
        header[header_len] = ((payload.len() >> 8) & 0xFF) as u8;
        header[header_len + 1] = (payload.len() & 0xFF) as u8;
        header_len += 2;
    } else {
        header[header_len] = 127;
        header_len += 1;
        let len = payload.len() as u64;
        let mut shift: i32 = 56;
        loop {
            header[header_len] = ((len >> shift) & 0xFF) as u8;
            header_len += 1;
            if shift == 0 {
                break;
            }
            shift -= 8;
        }
    }

    stream.write_all(&header[..header_len])?;
    if !payload.is_empty() {
        stream.write_all(payload)?;
    }
    Ok(())
}

fn raw_handle(stream: &TcpStream) -> i32 {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        stream.as_raw_fd()
    }
    #[cfg(not(unix))]
    {
        use std::os::windows::io::AsRawSocket;
        stream.as_raw_socket() as i32
    }
}

fn handle_connection(mut stream: TcpStream) {
    let (head, overflow) = match read_http_head(&mut stream) {
        Ok(parts) => parts,
        Err(_) => return,
    };
    let head_str = String::from_utf8_lossy(&head);

    let Some(websocket_key) = parse_request_header_value(&head_str, "Sec-WebSocket-Key") else {
        write_simple_http_response(&mut stream, "400 Bad Request", "Missing Sec-WebSocket-Key");
        return;
    };
    let websocket_key = websocket_key.to_string();

    let webview_id = match parse_request_target(&head_str).and_then(parse_webview_id_from_target) {
        Some(id) => id,
        None => {
            write_simple_http_response(&mut stream, "400 Bad Request", "Missing webviewId");
            return;
        }
    };

    if REGISTRIES.webview_state(webview_id).is_none() {
        write_simple_http_response(&mut stream, "404 Not Found", "Unknown webviewId");
        return;
    }

    if write_websocket_handshake(&mut stream, &websocket_key).is_err() {
        return;
    }

    let handle = raw_handle(&stream);
    match REGISTRIES.attach_webview_socket(webview_id, handle) {
        Ok(Some(previous)) => close_raw_socket(previous),
        Ok(None) => {}
        Err(()) => return,
    }

    let mut reader = PendingStreamReader {
        stream: &mut stream,
        pending: overflow,
        index: 0,
    };

    while let Ok(frame) = read_websocket_frame(&mut reader) {
        match frame.opcode {
            0x1 => dispatch_host_transport_message(webview_id, &frame.payload),
            0x8 => {
                let _ = write_websocket_frame(reader.stream, 0x8, b"");
                break;
            }
            0x9 => {
                if write_websocket_frame(reader.stream, 0xA, &frame.payload).is_err() {
                    break;
                }
            }
            _ => {}
        }
    }

    REGISTRIES.clear_webview_socket_if_current(webview_id, handle);
}

/// AES-256-GCM encrypt + wrap as the `{encryptedData, iv, tag}` JSON the client
/// decrypts. Mirrors `encryptHostTransportPacket`.
pub fn encrypt_host_transport_packet(
    message_json: &str,
    secret_key: &WebviewSecretKey,
) -> Result<String, String> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    fill_random(&mut nonce_bytes)?;

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(secret_key));
    let nonce = Nonce::from_slice(&nonce_bytes);
    let combined = cipher
        .encrypt(
            nonce,
            Payload {
                msg: message_json.as_bytes(),
                aad: b"",
            },
        )
        .map_err(|_| "AesGcmEncryptFailed".to_string())?;

    // aes-gcm appends the 16-byte tag to the ciphertext; split it to match the
    // Zig's separate ciphertext + tag fields.
    if combined.len() < TAG_LEN {
        return Err("AesGcmEncryptFailed".to_string());
    }
    let (ciphertext, tag) = combined.split_at(combined.len() - TAG_LEN);

    let payload = serde_json::json!({
        "encryptedData": BASE64.encode(ciphertext),
        "iv": BASE64.encode(nonce_bytes),
        "tag": BASE64.encode(tag),
    });
    Ok(payload.to_string())
}

/// Decrypt a `{encryptedData, iv, tag}` packet. Mirrors
/// `decryptHostTransportPacket`.
fn decrypt_host_transport_packet(
    message_json: &[u8],
    secret_key: &WebviewSecretKey,
) -> Result<Vec<u8>, ()> {
    let value: serde_json::Value = serde_json::from_slice(message_json).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    let encrypted_data = object
        .get("encryptedData")
        .and_then(|v| v.as_str())
        .ok_or(())?;
    let iv = object.get("iv").and_then(|v| v.as_str()).ok_or(())?;
    let tag = object.get("tag").and_then(|v| v.as_str()).ok_or(())?;

    let ciphertext = BASE64.decode(encrypted_data).map_err(|_| ())?;
    let nonce_bytes = BASE64.decode(iv).map_err(|_| ())?;
    let tag_bytes = BASE64.decode(tag).map_err(|_| ())?;

    if nonce_bytes.len() != NONCE_LEN || tag_bytes.len() != TAG_LEN {
        return Err(());
    }

    // Recombine ciphertext||tag for aes-gcm's combined API.
    let mut combined = ciphertext;
    combined.extend_from_slice(&tag_bytes);

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(secret_key));
    let nonce = Nonce::from_slice(&nonce_bytes);
    cipher
        .decrypt(
            nonce,
            Payload {
                msg: &combined,
                aad: b"",
            },
        )
        .map_err(|_| ())
}

/// Decrypt an inbound frame, mark the transport ready, and enqueue the
/// plaintext for the worker. Mirrors `dispatchHostTransportMessage`.
fn dispatch_host_transport_message(webview_id: u32, encrypted_packet: &[u8]) {
    let Some(context) = REGISTRIES.webview_transport_context(webview_id) else {
        return;
    };
    let Ok(plaintext) = decrypt_host_transport_packet(encrypted_packet, &context.secret_key) else {
        return;
    };
    if let Some(handle) = context.socket_handle {
        REGISTRIES.mark_webview_transport_ready(webview_id, handle);
    }
    // The Zig re-duplicates as a sentinel string then enqueues; we enqueue the
    // UTF-8 plaintext directly (host messages are JSON).
    let Ok(message) = std::str::from_utf8(&plaintext) else {
        return;
    };
    crate::host_queue::enqueue(webview_id, message);
}

/// Send a host→webview message over the webview's live socket, encrypting it
/// first. Returns false (and clears the socket on write failure) exactly like
/// `sendHostMessageToWebviewViaTransport`.
pub fn send_via_transport(webview_id: u32, message_json: &str) -> bool {
    let Some(context) = REGISTRIES.webview_transport_context(webview_id) else {
        return false;
    };
    if !context.transport_ready {
        return false;
    }
    let Some(handle) = context.socket_handle else {
        return false;
    };

    let encrypted_packet = match encrypt_host_transport_packet(message_json, &context.secret_key) {
        Ok(packet) => packet,
        Err(err) => {
            set_last_error(format!("Failed to encrypt host transport packet: {err}"));
            return false;
        }
    };

    let mut stream = match stream_from_handle(handle) {
        Some(stream) => stream,
        None => {
            REGISTRIES.clear_webview_socket_if_current(webview_id, handle);
            return false;
        }
    };

    let result = write_websocket_frame(&mut stream, 0x1, encrypted_packet.as_bytes());
    // Do not let the borrowed stream close the fd; the connection thread owns
    // it. `forget` here mirrors the Zig wrapping a handle in a transient
    // `std.net.Stream` without taking ownership.
    forget_stream(stream);

    if result.is_err() {
        REGISTRIES.clear_webview_socket_if_current(webview_id, handle);
        return false;
    }
    true
}

/// Wrap a raw fd/socket in a transient `TcpStream` for a single write, without
/// owning it (the connection thread retains ownership).
fn stream_from_handle(handle: i32) -> Option<TcpStream> {
    #[cfg(unix)]
    {
        use std::os::fd::FromRawFd;
        // SAFETY: handle is a live socket fd owned by the connection thread.
        // We use it transiently and `forget` the wrapper so the fd is not
        // closed here.
        Some(unsafe { TcpStream::from_raw_fd(handle) })
    }
    #[cfg(not(unix))]
    {
        use std::os::windows::io::FromRawSocket;
        // SAFETY: handle is a live socket owned by the connection thread.
        Some(unsafe { TcpStream::from_raw_socket(handle as u64) })
    }
}

fn forget_stream(stream: TcpStream) {
    std::mem::forget(stream);
}

fn close_raw_socket(handle: i32) {
    #[cfg(unix)]
    {
        // SAFETY: closing a socket fd we are responsible for retiring.
        unsafe {
            libc::close(handle);
        }
    }
    #[cfg(not(unix))]
    {
        use std::os::windows::io::FromRawSocket;
        // SAFETY: take ownership of the socket and let TcpStream's Drop close
        // it.
        drop(unsafe { TcpStream::from_raw_socket(handle as u64) });
    }
}

/// Close-and-clear a webview's socket (backs `clearWebviewHostTransport`).
pub fn close_and_clear_webview_socket(webview_id: u32) {
    if let Some(handle) = REGISTRIES.take_webview_socket(webview_id) {
        close_raw_socket(handle);
    }
}

/// Close a socket handle taken from a removed webview (used by
/// `webviewRemove`).
pub fn close_socket_handle(handle: i32) {
    close_raw_socket(handle);
}

fn fill_random(buf: &mut [u8]) -> Result<(), String> {
    use aes_gcm::aead::rand_core::RngCore;
    aes_gcm::aead::OsRng
        .try_fill_bytes(buf)
        .map_err(|_| "RandomFailed".to_string())
}
