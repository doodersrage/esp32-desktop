// Display names for ids the core sends. Presentation only.

#pragma once

#include <QDateTime>
#include <QString>

namespace Labels {
// esp32bench.dev chip id ("esp32s3") to its product name ("ESP32-S3").
QString chipName(const QString &id);
// The link that opens a library project (on a board, if given) in the app:
// esp32bench://project/<id>[?board=<board>], as link.open reads it.
QString projectLink(const QString &id, const QString &board = {});
// When something happened, as a person says it: "just now", "5 min ago", "3 h ago",
// or, from more than twelve hours ago, the date and time.
QString ago(const QDateTime &when, const QDateTime &now);
// Text for a forum post or an issue: a line naming the app, its version and
// `context` (what the text is from), then the text as a fenced code block.
QString forumPost(const QString &context, const QString &text);
// What circuit.pinNotes' kind means for a pin, in a sentence: a part on GPIO 12, a
// strapping pin, can stop the board starting.
QString pinNoteText(const QString &kind, int gpio, const QString &why = {});
// How long something took: "12.3 s", or "2 min 05 s".
QString duration(double seconds);
}
