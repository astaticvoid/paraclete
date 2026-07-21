//! rtkit integration — request realtime scheduling via D-Bus system bus.
//!
//! On Linux desktops with PipeWire/PulseAudio installed, rtkit
//! (`org.freedesktop.RealtimeKit1`) grants `SCHED_FIFO` to any process
//! without requiring `CAP_SYS_NICE` or `limits.conf` configuration.
//!
//! This module speaks the raw D-Bus wire protocol — deps-free, Linux-only.
//! Auth is a single EXTERNAL handshake; the method call is a fixed-size
//! binary message built by hand.  ~100 lines, zero dependencies.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

extern "C" {
    fn gettid() -> i32;
    fn getuid() -> u32;
}

fn unix_uid() -> u32 {
    unsafe { getuid() }
}

/// Try to acquire realtime scheduling from rtkit over D-Bus.
///
/// Returns `true` if rtkit granted `SCHED_FIFO` on the calling thread.
/// On any failure (no system bus, rtkit not running, auth failure,
/// method call error) returns `false` — caller falls back to the raw
/// `pthread_setschedparam` path.
pub fn try_acquire_realtime(priority: i32) -> bool {
    let mut sock = match connect_system_bus() {
        Some(s) => s,
        None => {
            log::warn!("realtime priority: rtkit not reachable (no system bus)");
            return false;
        }
    };

    if !dbus_auth(&mut sock) {
        log::warn!("realtime priority: rtkit auth failed");
        return false;
    }

    let tid = unsafe { gettid() } as u64;
    if make_thread_realtime(&mut sock, tid, priority as u32) {
        log::info!(
            "realtime priority: rtkit granted SCHED_FIFO prio={} on thread {}",
            priority,
            tid,
        );
        true
    } else {
        log::warn!(
            "realtime priority: rtkit method call failed — trying raw SCHED_FIFO",
        );
        false
    }
}

fn connect_system_bus() -> Option<UnixStream> {
    // DBUS_SYSTEM_BUS_ADDRESS env var, or the standard socket path.
    let addr = std::env::var("DBUS_SYSTEM_BUS_ADDRESS").unwrap_or_else(|_| {
        "unix:path=/var/run/dbus/system_bus_socket".into()
    });
    let path = addr.strip_prefix("unix:path=")?;
    UnixStream::connect(path).ok()
}

fn dbus_auth(sock: &mut UnixStream) -> bool {
    // SASL EXTERNAL auth: send UID as hex string.
    let uid = unix_uid();
    let auth_line = format!("\0AUTH EXTERNAL {:x}\r\n", uid);
    if sock.write_all(auth_line.as_bytes()).is_err() {
        return false;
    }

    let mut buf = [0u8; 256];
    let n = sock.read(&mut buf).unwrap_or(0);
    let reply = std::str::from_utf8(&buf[..n]).unwrap_or("");
    if !reply.starts_with("OK ") {
        return false;
    }

    // BEGIN the binary protocol.
    sock.write_all(b"BEGIN\r\n").is_ok()
}

fn make_thread_realtime(sock: &mut UnixStream, tid: u64, priority: u32) -> bool {
    let msg = build_method_call(tid, priority);
    if sock.write_all(&msg).is_err() {
        return false;
    }

    // Read the reply — we only need to know whether it's a method
    // return (type byte 0x02 at message offset 1) or an error.
    let mut buf = [0u8; 128];
    let n = sock.read(&mut buf).unwrap_or(0);
    if n < 2 {
        return false;
    }

    // D-Bus message: byte 0 = endian (matching the call — 'l' / 0x6C),
    // byte 1 = message type (2 = METHOD_RETURN, 3 = ERROR).
    matches!(buf.get(1), Some(&2))
}

fn build_method_call(tid: u64, priority: u32) -> Vec<u8> {
    // D-Bus method call: org.freedesktop.RealtimeKit1.MakeThreadRealtime
    //
    // Fixed header (16 bytes) + 4 header fields + 8-byte pad gap +
    // body (u64 tid + u32 priority).

    let path = b"/org/freedesktop/RealtimeKit1\0";   // 34 bytes
    let member = b"MakeThreadRealtime\0";              // 20 bytes
    let iface = b"org.freedesktop.RealtimeKit1\0";    // 34 bytes
    let dest = b"org.freedesktop.RealtimeKit1\0";     // 34 bytes

    // Each header field: 1(code) + 1(sig) + 2(pad) + 4(str_len u32) + str + pad_to_4
    let fields: Vec<u8> = {
        let mut f = Vec::with_capacity(200);
        // PATH (code=1, sig='o')
        f.extend_from_slice(&[0x01, 0x6F, 0x00, 0x00]);
        f.extend_from_slice(&(path.len() as u32).to_le_bytes());
        f.extend_from_slice(path);
        pad_to(&mut f, 4);
        // MEMBER (code=3, sig='s')
        f.extend_from_slice(&[0x03, 0x73, 0x00, 0x00]);
        f.extend_from_slice(&(member.len() as u32).to_le_bytes());
        f.extend_from_slice(member);
        pad_to(&mut f, 4);
        // INTERFACE (code=2, sig='s')
        f.extend_from_slice(&[0x02, 0x73, 0x00, 0x00]);
        f.extend_from_slice(&(iface.len() as u32).to_le_bytes());
        f.extend_from_slice(iface);
        pad_to(&mut f, 4);
        // DESTINATION (code=6, sig='s')
        f.extend_from_slice(&[0x06, 0x73, 0x00, 0x00]);
        f.extend_from_slice(&(dest.len() as u32).to_le_bytes());
        f.extend_from_slice(dest);
        pad_to(&mut f, 4);
        f
    };

    let body_len: u32 = 8 + 4; // u64 + u32
    let header = build_header(body_len, 1, fields.len() as u32);
    let mut msg = Vec::with_capacity(header.len() + fields.len() + 12 + 8);
    msg.extend_from_slice(&header);
    msg.extend_from_slice(&fields);

    // Pad to 8-byte boundary for body start.
    pad_to(&mut msg, 8);

    // Body: thread_id (u64) + priority (u32).
    msg.extend_from_slice(&tid.to_le_bytes());
    msg.extend_from_slice(&priority.to_le_bytes());
    msg
}

fn build_header(body_len: u32, serial: u32, field_array_len: u32) -> [u8; 16] {
    let mut h = [0u8; 16];
    h[0] = 0x6C; // 'l' — little-endian
    h[1] = 0x01; // message type: METHOD_CALL
    h[2] = 0x00; // flags
    h[3] = 0x01; // major protocol version
    h[4..8].copy_from_slice(&body_len.to_le_bytes());
    h[8..12].copy_from_slice(&serial.to_le_bytes());
    h[12..16].copy_from_slice(&field_array_len.to_le_bytes());
    h
}

fn pad_to(buf: &mut Vec<u8>, align: usize) {
    let rem = buf.len() % align;
    if rem != 0 {
        buf.resize(buf.len() + align - rem, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_call_message_structure() {
        let msg = build_method_call(12345, 50);

        assert_eq!(msg[0], 0x6C, "endian must be 'l'");
        assert_eq!(msg[1], 0x01, "message type must be METHOD_CALL");
        assert_eq!(msg[3], 0x01, "protocol version 1");

        let header_len = 16u32;
        let field_array_len = u32::from_le_bytes(msg[12..16].try_into().unwrap());
        assert!(field_array_len > 0, "must have header fields");

        // Body must start at an 8-byte-aligned offset after the field array.
        let body_start = (header_len as usize + field_array_len as usize + 7) & !7;
        assert_eq!(body_start % 8, 0, "body must be 8-byte aligned");

        // Body: tid (u64 LE) + priority (u32 LE).
        let tid_bytes = &msg[body_start..body_start + 8];
        let tid = u64::from_le_bytes(tid_bytes.try_into().unwrap());
        assert_eq!(tid, 12345);

        let pri_bytes = &msg[body_start + 8..body_start + 12];
        let pri = u32::from_le_bytes(pri_bytes.try_into().unwrap());
        assert_eq!(pri, 50);

        // Total message must not extend past the computed body.
        assert_eq!(msg.len(), body_start + 12, "message length must match header + fields + body");
    }

    #[test]
    fn body_alignment_for_various_field_sizes() {
        // Pick two different tids — the message structure should be independent
        // of the actual values (only the body content changes, not layout).
        let a = build_method_call(0, 99);
        let b = build_method_call(u64::MAX, 1);

        assert_eq!(a.len(), b.len(), "message length must be independent of body values");
        // Verify the header fields are identical (first 185 bytes or so, up to body).
        let body_off = a.len() - 12;
        assert_eq!(&a[..body_off], &b[..body_off], "header+fields must be identical");
    }

    #[test]
    fn try_acquire_fails_gracefully_when_no_rtkit() {
        // This box may or may not have rtkit.  The function must never crash;
        // it returns a bool — whichever answer, it's a clean return.
        let result = std::panic::catch_unwind(|| try_acquire_realtime(50));
        assert!(result.is_ok(), "try_acquire_realtime must never panic");
    }
}
