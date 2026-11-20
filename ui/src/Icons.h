// Icons where the system has no icon theme (macOS, Windows): the bundled fallback theme.

#pragma once

namespace Icons {
// Add ui/icons/esp32bench-fallback (compiled in) as Qt's fallback icon theme, so a name
// the system theme lacks still has an icon. Call once, after the QApplication.
void install();
// The fallback theme's name.
inline constexpr const char *FallbackTheme = "esp32bench-fallback";
}
