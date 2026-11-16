---
name: add-rpc-method
description: Add or change a JSON-RPC method between the Rust core and the Qt UI. Use when a feature needs the UI to ask the core for something, when adding a core event stream, or when changing a request or response shape.
---

# Adding a JSON-RPC method

Every feature crosses the UI-to-core boundary through a JSON-RPC method. Follow these steps in order. Do not start at the UI.

## Steps

1. **Define it in `schema/rpc/`.** Add the method name, params, result and errors. Names are `namespace.verb`, for example `serial.open`, `build.start`, `sim.reset`. Long-running work returns a job id at once and reports through events.
2. **Implement the handler in the core.** Put the logic in a library crate and keep the handler thin: validate params, call the library, map errors to JSON-RPC error codes.
3. **Write the core test.** Round-trip a request and response through the real socket transport. Cover one success and one failure.
4. **Expose it in the CLI.** Add a subcommand that calls the same library function. The CLI must reach every feature the UI can.
5. **Add the client call in the UI.** Extend the RPC client class with an asynchronous method that emits a signal on completion. No blocking calls.
6. **Wire the widget.** The widget calls the client and renders the result. It holds no logic of its own.
7. **Update the docs.** If the method changes what the app can do, update `docs/PLAN.md` or the relevant design note.

## Rules

- **Events:** streams (serial output, build logs, device hotplug, simulator state) are notifications with a subscription id. The UI must be able to unsubscribe, and the core must stop the work when the last subscriber leaves.
- **Cancellation:** every job that can run longer than a second accepts a cancel request.
- **Untrusted input:** params that name files or projects are validated in the core, never trusted because the UI sent them.
- **Consent:** methods that build or flash take an explicit confirmation token for projects that came from a link or a download. Do not add a bypass.
- **Compatibility:** changing an existing method's params or result is a breaking change. Add a new method or an optional field instead.

## Done when

- Schema, handler, test, CLI subcommand and UI client all exist for the method.
- `cargo test` and `ctest` pass.
- `cargo clippy --all-targets -- -D warnings` is clean.
