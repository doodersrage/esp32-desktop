---
paths:
  - "ui/**"
---

# Qt UI rules

- Qt 6 Widgets, C++20, CMake with Ninja. No QML, no Qt Quick, no Qt 5 compatibility code.
- The UI holds no business logic. It calls the core through the JSON-RPC client class and renders what comes back. If a change needs parsing, file access or process control, it belongs in `core/`.
- Never block the GUI thread. All RPC calls are asynchronous and report through signals.
- The UI must start and stay usable when the core is not running: show the connection state and offer to start it.
- Every user-visible string goes through `tr()`. No string concatenation to build sentences; use `arg()`. New or changed strings need translating in `ui/translations/` (all four languages) before `tst_translations` passes.
- Use theme icons (`QIcon::fromTheme`) and the system palette so the app follows the Plasma theme, including dark mode. No hard-coded colours outside the canvas.
- Panels are `QDockWidget`s, and layout is saved and restored with `QSettings`.
- Ownership follows Qt parent-child rules. No raw `new` without a parent, and no manual `delete` of parented objects.
- Test on Plasma Wayland first, then X11.
