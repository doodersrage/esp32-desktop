---
paths:
  - "schema/**"
---

# Shared schema rules

- Files here are a contract with the esp32bench.dev site and with the UI. Treat every change as a public API change.
- Each schema carries a version. Adding an optional field is a minor bump. Removing, renaming or changing the meaning of a field is a major bump and needs the owner's sign-off.
- The core must keep reading the previous major version of the project manifest, because cached manifests exist on users' machines.
- Pin numbers are GPIO numbers, matching the site.
- A board entry must say whether it is simulated or workbench-only. Only ESP32, ESP32-S3 and ESP32-C3 may be marked as simulated.
- Add or update an example file and a validation test with every schema change.
