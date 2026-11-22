// Display names for ids the core sends. Presentation only.

#pragma once

#include <QString>

namespace Labels {
// esp32bench.dev chip id ("esp32s3") to its product name ("ESP32-S3").
QString chipName(const QString &id);
// The link that opens a library project (on a board, if given) in the app:
// esp32bench://project/<id>[?board=<board>], as link.open reads it.
QString projectLink(const QString &id, const QString &board = {});
}
