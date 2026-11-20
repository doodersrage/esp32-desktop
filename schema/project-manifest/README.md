# Project manifest

The project library the esp32bench.dev site publishes for the desktop app, at `https://esp32bench.dev/desktop-manifest.json`. The site generates it from `src/data/projects.ts` in `src/pages/desktop-manifest.json.ts`.

- `v1.schema.json`: the format, version 1.x (JSON Schema 2020-12).
- `example.json`: a valid manifest with the real board list and two real projects, trimmed to two boards each. Sketch code is replaced by placeholders, and `blink` lists one extra library (Adafruit NeoPixel) so that `install` is exercised. The rest of the text comes from esp32bench.dev and is CC BY 4.0.

## Versioning

The file carries `format` and `version`. Readers check that `format` is `esp32bench-project-manifest` and that the major version is one they know, and ignore fields they do not know.

- Adding an optional field is a minor bump. Add it to `v1.schema.json` in the same change.
- Removing or renaming a field, making one required, or changing what one means is a major bump, needs the owner's sign-off, and gets a new `v2.schema.json`. The core keeps reading v1, because cached manifests exist on users' machines.

## Versions

- 1.1 (2026-11-20): a build of prebuilt firmware may carry `image` (an https URL, its SHA-256 and size), the merged image the browser flasher also uses. The app downloads it, after asking, checks it, and runs it in the simulator or flashes it.

## Tests

- `core/crates/library/tests/manifest.rs` validates `example.json` against the schema and checks the reader. Set `ESP32BENCH_TEST_MANIFEST` to a site build's `dist/desktop-manifest.json` to validate the real file too.
- The site's `tests/desktop-manifest.test.ts` checks the invariants the app relies on (unique ids, known boards, code for every sketch build, no unfilled `%VAR%`).
