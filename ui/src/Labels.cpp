#include "Labels.h"

using namespace Qt::StringLiterals;

QString Labels::chipName(const QString &id)
{
    // Product names; not translated.
    if (id == u"esp32") return u"ESP32"_s;
    if (id == u"esp32s2") return u"ESP32-S2"_s;
    if (id == u"esp32s3") return u"ESP32-S3"_s;
    if (id == u"esp32c3") return u"ESP32-C3"_s;
    if (id == u"esp32c6") return u"ESP32-C6"_s;
    return id;
}
