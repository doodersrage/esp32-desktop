# Core RPC protocol

The UI talks to the core daemon (`esp32benchd`) with JSON-RPC 2.0 over a Unix socket. This note fixes the wire format, errors, events and how methods are defined. Method definitions live in `schema/rpc/`; the `add-rpc-method` skill has the steps for adding one.

## Transport

- **Socket:** `$XDG_RUNTIME_DIR/esp32bench/core.sock`, mode 0600, in a directory with mode 0700. Only the user's own processes can connect.
- **One daemon per user.** The daemon holds an exclusive lock on `core.lock` next to the socket. A second daemon exits with an error instead of taking over the socket.
- **Framing:** one JSON object per line, UTF-8, ending in `\n`. JSON escapes any newline inside strings, so a line is always one message. Qt reads it with `QLocalSocket::readLine`. Messages over 16 MiB are rejected and the connection is closed.
- **No batches.** JSON-RPC batch arrays are answered with an `Invalid Request` error.

## Messages

- **Request:** `{"jsonrpc":"2.0","id":<int|string>,"method":"namespace.verb","params":{...}}`. Params are always an object; omitted params mean `{}`.
- **Response:** `{"jsonrpc":"2.0","id":...,"result":...}` or `{"jsonrpc":"2.0","id":...,"error":{"code":...,"message":"...","data":{...}}}`.
- **Notification (core to UI):** `{"jsonrpc":"2.0","method":"namespace.event","params":{"subscription":<id>,...}}`. Events carry the subscription id they belong to.
- Requests are handled concurrently. Responses can arrive in a different order from the requests; match them by `id`.

## Errors

Standard JSON-RPC codes for protocol problems, and application codes from -32000 down. Every error has a `message` written for a person; `data` may add machine-readable detail.

| Code | Name | When |
| --- | --- | --- |
| -32700 | Parse error | The line is not JSON |
| -32600 | Invalid request | Not a JSON-RPC 2.0 request object |
| -32601 | Method not found | Unknown method |
| -32602 | Invalid params | Params fail the method's schema |
| -32603 | Internal error | A bug in the core |
| -32000 | Not found | The named thing (session, port, project) does not exist |
| -32001 | Busy | The resource is in use, e.g. a serial port another program holds |
| -32002 | Consent required | The project is untrusted and no valid confirmation token was given (rule 4) |
| -32003 | Unavailable | A required tool is missing, e.g. QEMU or `arduino-cli` |
| -32004 | Cancelled | The job was cancelled |
| -32005 | Failed | The operation ran and failed; `data` says how |

## Events and jobs

- **Subscriptions:** a method that starts a stream returns `{"subscription": <id>}`. Events for it are notifications that carry that id. `core.unsubscribe {"subscription": id}` ends it.
- **Ownership:** subscriptions and jobs belong to the connection that created them. When the connection closes, the core unsubscribes them and stops work nobody else is watching. This is how a crashed UI does not leave QEMU or a build running.
- **Jobs:** work that can take longer than a second returns `{"job": <id>}` at once, reports progress as events, and accepts `job.cancel {"job": id}`.

## Versioning

`schema/rpc/index.json` carries the protocol version (semver). `core.hello` returns it, so the UI can refuse a core it does not understand.

- Adding a method, an event or an optional field is a minor bump.
- Removing or renaming anything, or changing a field's meaning, is a major bump and needs the owner's sign-off.
- Before 1.0.0 the protocol is internal to this repo and only the UI depends on it.

## Method definitions

Each method is one file, `schema/rpc/<method>.json`, holding a JSON Schema (draft 2020-12) for `params` and `result`, the error codes it can return, and an example request and response. A test in the core checks every example against its schema, and every method the daemon registers against the files. The UI client is written against the same files.
