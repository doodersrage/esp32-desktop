#include "Translations.h"

#include <QCoreApplication>
#include <QLibraryInfo>
#include <QTranslator>

#include <memory>

using namespace Qt::StringLiterals;

namespace Translations {

QStringList languages()
{
    return {u"es"_s, u"de"_s, u"fr"_s, u"pt_BR"_s};
}

namespace {
bool s_translated = false; // set by install()
}

bool translated()
{
    return s_translated;
}

bool install(QCoreApplication &app, const QLocale &locale)
{
    // Qt's own strings (standard buttons, file dialogs), where the system has them.
    // A translator is handed to `app` only once it has loaded.
    auto add = [&app](std::unique_ptr<QTranslator> t) {
        QCoreApplication::installTranslator(t.get());
        t.release()->setParent(&app);
    };
    auto qt = std::make_unique<QTranslator>();
    if (qt->load(locale, u"qtbase"_s, u"_"_s, QLibraryInfo::path(QLibraryInfo::TranslationsPath)))
        add(std::move(qt));

    auto own = std::make_unique<QTranslator>();
    if (!own->load(locale, u"esp32bench"_s, u"_"_s, u":/i18n"_s))
        return false;
    add(std::move(own));
    s_translated = true;
    return true;
}

} // namespace Translations
