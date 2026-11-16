// Display names for ids the core sends. Presentation only.

#pragma once

#include <QString>

namespace Labels {
// esp32bench.dev chip id ("esp32s3") to its product name ("ESP32-S3").
QString chipName(const QString &id);
}
