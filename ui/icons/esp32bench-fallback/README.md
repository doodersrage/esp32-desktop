# Fallback icons

The app asks for freedesktop icon names (`media-playback-start`, `run-build`, ...).
Linux desktops have a theme that answers them; macOS and Windows do not. These SVGs,
from [Lucide](https://lucide.dev) (ISC licence; the icons Lucide took from Feather are
MIT, see `LICENSE-lucide`), are installed as Qt's fallback theme by `Icons::install()`,
so they are used only where the system theme has no icon of that name. Each file is
named for the icon it stands for; its stroke is a mid grey, which reads on light and
dark backgrounds, because Qt's SVG renderer does not take `currentColor` from the
palette. `dialog-warning` is Lucide's triangle with an exclamation mark added.

`tst_icons` fails if the UI asks for a name this folder lacks.
