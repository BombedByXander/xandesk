# Validation — November 19, 2026

Windows x64, Rust 1.100.0 GNU toolchain, release build, local Chrome.

Passed:

- `cargo check --workspace --locked`.
- `cargo test --release --workspace --locked`: four ordinary tests passed. The real-desktop browser test is ignored by default and was run separately.
- Encoder submitted 240 synthetic frames with 2 ms timestamp spacing; every frame produced encoded output. This tests absence of the 60/120 FPS gate, not full-resolution throughput.
- Read-only real Windows capture → OpenH264 → WebRTC → Chrome test: final run decoded 8 frames at **1920 × 1080**, received 216,707 bytes, peer state `connected`, control data channel `open`. The test does not inject input or save desktop screenshots.
- Relay integration: HTTP serving and security headers, invalid-secret rejection, room ownership, approval gate, single-viewer limit, SDP routing, rejection, viewer cleanup and host shutdown.
- Browser UI: pairing fragments removed from URL, no secret written to localStorage, invalid-host errors recover, insecure remote HTTP rejected, no horizontal overflow at 390/768/1440 px, no uncaught page errors.
- Packaged native host starts and stays running. The startup check did not enable sharing.
- Browser JavaScript syntax check.

Not verified: separate-network connectivity, TURN fallback, real gaming performance, glass-to-glass latency, input injection into arbitrary games/elevated applications, long-running stability, or the full native UI approval workflow. No quality, latency, or game-FPS guarantee is made.

The build emits a future Rust compatibility notice for the transitive `block 0.1.6` dependency. OpenH264 also notes that bitrate cannot be strictly enforced with frame skipping disabled; the UI therefore labels bitrate as a target. Frame skipping is disabled deliberately.

This is a working preview with the feature gaps listed in README.md, not a claim of full DextraDesk parity or production readiness.

