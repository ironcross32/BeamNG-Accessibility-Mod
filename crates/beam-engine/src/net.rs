//! Owner: EA. The UDP plumbing every listener and command sender in beamtel.py shares,
//! and the port map (`beamtel.py` ~738-739 and 6728-6798).
//!
//! # The listener shape
//!
//! Every Python listener is the same loop:
//!
//! ```text
//! sock = socket(AF_INET, SOCK_DGRAM)          # no SO_REUSEADDR anywhere in beamtel.py
//! sock.bind(("127.0.0.1", PORT))              # telemetry_loop alone binds 0.0.0.0
//! sock.settimeout(0.2)                        # 0.05 for nodegrab, 0.25 for telemetry
//! while not stop_event.is_set():
//!     try: data, addr = sock.recvfrom(BUFSIZE)
//!     except socket.timeout: continue
//!     except OSError:
//!         if stop_event.is_set(): break
//!         logger.error("<Name> listener socket error.")
//!     text = data.decode(<encoding>, errors=<mode>).strip()
//! ```
//!
//! [`UdpListener`] is that socket, [`UdpListener::recv`] classifies the outcome the same
//! way, and [`decode`] reproduces each `bytes.decode` variant the listeners use. A
//! listener whose bind fails raises out of its thread in Python (the thread ends; the
//! traceback goes to a stderr nobody sees in the windowed build): the Rust listener
//! returns, logging the error.
//!
//! # The command senders
//!
//! All `_send_*_cmd` functions are one throwaway socket, one `sendto` to 127.0.0.1, close,
//! and on any exception `logger.error(f"Failed to send <what> via UDP: {e}")`. Some return
//! True/False. [`send_cmd`] is that function; each wave-2 module keeps its own named
//! wrapper (`send_scanner_cmd`, ...) so the Python names still grep.

use std::io;
use std::net::{SocketAddr, UdpSocket};

use crate::timer::secs;

// ---- Port map (names exactly as in beamtel.py) ------------------------------------------
pub const TELEMETRY_PORT: u16 = 4444;
pub const UI_PORT: u16 = 4579;
pub const SCANNER_LISTEN_PORT: u16 = 4445;
pub const DUMP_CMD_PORT: u16 = 4446;
/// `DUMP_RESP_PORT` inside the F9 dump closures, and the induction report's receive port.
pub const DUMP_RESP_PORT: u16 = 4447;
pub const SCANNER_CMD_PORT: u16 = 4448;
pub const AI_CMD_PORT: u16 = 4449;
pub const CAMERA_LISTEN_PORT: u16 = 4450;
pub const CAMERA_CMD_PORT: u16 = 4451;
pub const OBSTACLE_LISTEN_PORT: u16 = 4452;
pub const OBSTACLE_CMD_PORT: u16 = 4453;
pub const NODEGRAB_LISTEN_PORT: u16 = 4454;
pub const NODEGRAB_CMD_PORT: u16 = 4455;
pub const CLICKSPOT_LISTEN_PORT: u16 = 4456;
pub const CLICKSPOT_CMD_PORT: u16 = 4457;
pub const SLOT_LISTEN_PORT: u16 = 4458;
pub const SLOT_CMD_PORT_OUT: u16 = 4459;
pub const ROAD_LISTEN_PORT: u16 = 4462;
pub const ROAD_CMD_PORT: u16 = 4463;
pub const UI_TOGGLE_CMD_PORT: u16 = 4464;
pub const CONSOLE_CMD_PORT: u16 = 4465;
pub const CONSOLE_RESP_PORT: u16 = 4466;
pub const VEHICLE_BINDINGS_LISTEN_PORT: u16 = 4467;
pub const VEHICLE_BINDINGS_CMD_PORT: u16 = 4468;
pub const IMPLEMENT_LISTEN_PORT: u16 = 4469;
pub const IMPLEMENT_CMD_PORT: u16 = 4470;
pub const TERRAIN_SCAN_LISTEN_PORT: u16 = 4471;
pub const TERRAIN_SCAN_CMD_PORT: u16 = 4472;
pub const CANNON_SHOT_LISTEN_PORT: u16 = 4473;
pub const ENV_LISTEN_PORT: u16 = 4474;
pub const ENV_CMD_PORT: u16 = 4475;
pub const TRAILER_ANGLE_LISTEN_PORT: u16 = 4476;
pub const VEHICLE_INFO_LISTEN_PORT: u16 = 4477;
pub const VEHICLE_INFO_CMD_PORT: u16 = 4478;
pub const BINDING_LEARN_LISTEN_PORT: u16 = 4479;
pub const BINDING_LEARN_CMD_PORT: u16 = 4480;
pub const ROUTE_BEACON_LISTEN_PORT: u16 = 4482;
pub const AUDIO_DEBUG_LISTEN_PORT: u16 = 4483;
pub const AUDIO_DEBUG_CMD_PORT: u16 = 4484;
pub const WHEEL_REPORT_LISTEN_PORT: u16 = 4485;
pub const FUEL_LISTEN_PORT: u16 = 4486;
pub const FUEL_CMD_PORT: u16 = 4488;
pub const CAMERA_LINEUP_LISTEN_PORT: u16 = 4489;

/// The loopback host every listener and sender uses.
pub const LOCALHOST: &str = "127.0.0.1";

/// How a listener turns its datagram into text (`bytes.decode(encoding, errors)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decode {
    /// `decode("utf-8")`: a bad byte raises `UnicodeDecodeError` -> `None`.
    Utf8Strict,
    /// `decode("utf-8", errors="ignore")`: bad bytes are dropped.
    Utf8Ignore,
    /// `decode("utf-8", errors="replace")`: bad sequences become U+FFFD.
    Utf8Replace,
    /// `decode("ascii")`: any byte >= 0x80 raises -> `None`.
    AsciiStrict,
    /// `decode("ascii", errors="replace")`: each byte >= 0x80 becomes U+FFFD.
    AsciiReplace,
}

/// `bytes.decode(...)` for each mode the listeners use. `None` where Python raises.
pub fn decode(data: &[u8], mode: Decode) -> Option<String> {
    match mode {
        Decode::Utf8Strict => std::str::from_utf8(data).ok().map(str::to_string),
        Decode::Utf8Replace => Some(String::from_utf8_lossy(data).into_owned()),
        Decode::Utf8Ignore => {
            let mut out = String::with_capacity(data.len());
            for chunk in data.utf8_chunks() {
                out.push_str(chunk.valid());
            }
            Some(out)
        }
        Decode::AsciiStrict => {
            if data.is_ascii() {
                Some(String::from_utf8_lossy(data).into_owned())
            } else {
                None
            }
        }
        Decode::AsciiReplace => Some(
            data.iter()
                .map(|&b| if b < 0x80 { b as char } else { '\u{FFFD}' })
                .collect(),
        ),
    }
}

/// What one `recvfrom` produced.
#[derive(Debug)]
pub enum Recv<'a> {
    Data(&'a [u8], SocketAddr),
    /// `socket.timeout`.
    Timeout,
    /// Any other `OSError` (including a datagram larger than the buffer, which Windows
    /// reports as WSAEMSGSIZE in both Python and Rust).
    Error(io::Error),
}

/// A bound UDP socket with a read timeout and a receive buffer of a fixed size.
pub struct UdpListener {
    sock: UdpSocket,
    buf: Vec<u8>,
}

impl UdpListener {
    /// `bind(("127.0.0.1", port))` + `settimeout(timeout_s)`, reading `bufsize` bytes.
    pub fn bind(port: u16, timeout_s: f64, bufsize: usize) -> io::Result<UdpListener> {
        Self::bind_on(LOCALHOST, port, timeout_s, bufsize)
    }

    /// As [`UdpListener::bind`] on another host (`telemetry_loop` binds `0.0.0.0`).
    pub fn bind_on(
        host: &str,
        port: u16,
        timeout_s: f64,
        bufsize: usize,
    ) -> io::Result<UdpListener> {
        let sock = UdpSocket::bind((host, port))?;
        sock.set_read_timeout(Some(
            secs(timeout_s).max(std::time::Duration::from_millis(1)),
        ))?;
        Ok(UdpListener {
            sock,
            buf: vec![0u8; bufsize.max(1)],
        })
    }

    /// `setsockopt(SOL_SOCKET, SO_RCVBUF, size)`. `console_listener` sets 1 MiB BEFORE
    /// binding and ignores a failure; call [`set_rcvbuf_before_bind`] for that order.
    pub fn socket(&self) -> &UdpSocket {
        &self.sock
    }

    pub fn recv(&mut self) -> Recv<'_> {
        match self.sock.recv_from(&mut self.buf) {
            Ok((n, addr)) => Recv::Data(&self.buf[..n], addr),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Recv::Timeout
            }
            Err(e) => Recv::Error(e),
        }
    }
}

/// `console_listener`'s order: SO_RCVBUF on the unbound socket (failure ignored), then
/// bind and timeout. std cannot set an option before bind, so the option is applied
/// immediately after binding; for a fresh UDP socket with nothing queued the effect is
/// the same.
pub fn bind_with_rcvbuf(
    port: u16,
    timeout_s: f64,
    bufsize: usize,
    rcvbuf: i32,
) -> io::Result<UdpListener> {
    let l = UdpListener::bind(port, timeout_s, bufsize)?;
    set_rcvbuf_before_bind(&l.sock, rcvbuf);
    Ok(l)
}

/// Best-effort `SO_RCVBUF`; a failure is ignored, as the Python's `except OSError: pass`.
pub fn set_rcvbuf_before_bind(sock: &UdpSocket, size: i32) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows::Win32::Networking::WinSock::{SO_RCVBUF, SOCKET, SOL_SOCKET, setsockopt};
        let raw = SOCKET(sock.as_raw_socket() as usize);
        let bytes = size.to_ne_bytes();
        // SAFETY: `raw` is a live socket owned by `sock` for the duration of the call, and
        // the option value is a 4-byte int as SO_RCVBUF requires.
        let _ = unsafe { setsockopt(raw, SOL_SOCKET, SO_RCVBUF, Some(&bytes)) };
    }
    #[cfg(not(windows))]
    {
        let _ = (sock, size);
    }
}

/// One datagram to 127.0.0.1:`port` from a throwaway socket (bound to an ephemeral port).
pub fn send_udp(port: u16, payload: &[u8]) -> io::Result<()> {
    let s = UdpSocket::bind((LOCALHOST, 0))?;
    s.send_to(payload, (LOCALHOST, port))?;
    Ok(())
}

/// The shape of every `_send_*_cmd`: send `command` (UTF-8) to `port`; on failure log
/// `Failed to send {what} via UDP: {e}` and return false.
///
/// Python logs `str(e)` of the OSError (`[WinError 10051] ...`); Rust logs the io::Error
/// display (`... (os error 10051)`). Log text only.
pub fn send_cmd(port: u16, command: &str, what: &str) -> bool {
    match send_udp(port, command.as_bytes()) {
        Ok(()) => true,
        Err(e) => {
            log::error!("Failed to send {what} via UDP: {e}");
            false
        }
    }
}

/// A sender whose failure Python swallows silently (`except Exception: pass`).
pub fn send_quiet(port: u16, payload: &[u8]) {
    let _ = send_udp(port, payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_modes_match_python() {
        let bad = b"ab\xffc\xe2\x82";
        assert_eq!(decode(b"abc", Decode::Utf8Strict).as_deref(), Some("abc"));
        assert_eq!(decode(bad, Decode::Utf8Strict), None);
        assert_eq!(decode(bad, Decode::Utf8Ignore).as_deref(), Some("abc"));
        assert_eq!(
            decode(bad, Decode::Utf8Replace).as_deref(),
            Some("ab\u{FFFD}c\u{FFFD}")
        );
        assert_eq!(decode(b"a\xe9", Decode::AsciiStrict), None);
        assert_eq!(
            decode(b"a\xe9\xe9", Decode::AsciiReplace).as_deref(),
            Some("a\u{FFFD}\u{FFFD}")
        );
    }

    #[test]
    fn listener_receives_and_times_out() {
        let mut l = UdpListener::bind(0, 0.05, 64).expect("bind");
        let port = l.socket().local_addr().unwrap().port();
        assert!(matches!(l.recv(), Recv::Timeout));
        assert!(send_cmd(port, "PING", "test command"));
        match l.recv() {
            Recv::Data(d, _) => assert_eq!(d, b"PING"),
            other => panic!("expected data, got {other:?}"),
        }
    }
}
