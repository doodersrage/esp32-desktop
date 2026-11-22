// Loads the app's translations (built from ui/translations/*.ts into the binary)
// and Qt's own, for the user's language.

#pragma once

#include <QLocale>
#include <QStringList>

class QCoreApplication;

namespace Translations {
// The languages the app is translated into, besides English: "es", "de", "fr", "pt_BR".
QStringList languages();
// Install the translators for `locale` on `app`. True if the app's own strings are
// translated for it (false for English and for languages it has no translation of).
bool install(QCoreApplication &app, const QLocale &locale = QLocale());
// Whether install() put the app's own strings in another language (false: English).
bool translated();
}
