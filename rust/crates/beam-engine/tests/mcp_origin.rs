//! python-bugs.md 66: the MCP server's DNS-rebinding guard. The Python accepted any
//! `Origin` that began with `http://127.0.0.1` or `http://localhost`; the Rust requires
//! the host exactly, with an optional numeric port.

use beam_engine::mcp_server::origin_allowed;

#[test]
fn loopback_origins_are_allowed() {
    for o in [
        "",
        "http://127.0.0.1",
        "http://127.0.0.1:4481",
        "http://localhost",
        "http://localhost:3000",
        "http://LOCALHOST:8080",
    ] {
        assert!(origin_allowed(o), "{o:?} should be allowed");
    }
}

#[test]
fn look_alikes_and_other_origins_are_refused() {
    for o in [
        // What the Python's prefix match let through.
        "http://localhost.example.com",
        "http://localhost.example.com:4481",
        "http://127.0.0.1.nip.io",
        "http://127.0.0.10",
        "http://localhost@evil.example",
        "http://127.0.0.1:4481@evil.example",
        "http://localhost:4481.evil.example",
        // Refused by the Python too.
        "https://localhost",
        "null",
        "file://",
        "http://example.com",
        "http://localhost:",
        "http://localhost:abc",
        "http://localhost:123456",
        "http://[::1]",
    ] {
        assert!(!origin_allowed(o), "{o:?} should be refused");
    }
}
