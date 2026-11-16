---
paths:
  - "core/**"
---

# Rust core rules

- The core is a Cargo workspace. Keep the daemon, the CLI and the reusable logic in separate crates so the logic can be tested without a socket.
- Async runtime is Tokio. Do not add a second runtime.
- Errors: `thiserror` for library crates, `anyhow` only in the binaries. No `unwrap()` or `expect()` outside tests, except on values proven at compile time.
- Child processes (QEMU, `idf.py`, `arduino-cli`, GDB) are always spawned with an explicit argument list. Never build a shell string from project data or user input.
- Every child process is owned by a supervisor that kills it when the core exits or the session ends. No orphaned QEMU processes.
- Paths from projects and manifests are untrusted. Resolve them and reject anything that escapes the project directory.
- Serial and QEMU output is bytes, not UTF-8. Decode lossily at the edge and keep the raw bytes available.
- Every JSON-RPC method has a definition in `schema/rpc/` and a test that round-trips a request and response. Long-running work reports progress as events and can be cancelled.
- Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` before finishing a change.
