#include "Preferences.h"

#include <QCheckBox>
#include <QComboBox>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QLabel>
#include <QSettings>
#include <QSpinBox>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace Preferences {
QString language()
{
    return QSettings().value(u"ui/language"_s).toString();
}
bool releaseNotices()
{
    return QSettings().value(u"update/notices"_s, true).toBool();
}
bool reopenProject()
{
    return QSettings().value(u"project/reopen"_s, true).toBool();
}
bool serialReconnect()
{
    return QSettings().value(u"serial/reconnect"_s, true).toBool();
}
bool reopenSerial()
{
    return QSettings().value(u"serial/reopen"_s, true).toBool();
}
} // namespace Preferences

PreferencesDialog::PreferencesDialog(QWidget *parent)
    : QDialog(parent)
    , m_language(new QComboBox(this))
    , m_textSize(new QSpinBox(this))
    , m_notices(new QCheckBox(tr("Say when a newer release is out"), this))
    , m_reopenProject(new QCheckBox(tr("Reopen the last project at start"), this))
    , m_reconnect(new QCheckBox(tr("Wait for a board that goes away, such as one that resets over its own USB"), this))
    , m_reopenSerial(new QCheckBox(tr("Reopen the serial monitor at start if it was open"), this))
{
    setWindowTitle(tr("Preferences"));
    // Each language in its own name, so anyone can find theirs.
    m_language->addItem(tr("The desktop's language"), QString());
    m_language->addItem(u"English"_s, u"en"_s);
    m_language->addItem(u"Español"_s, u"es"_s);
    m_language->addItem(u"Deutsch"_s, u"de"_s);
    m_language->addItem(u"Français"_s, u"fr"_s);
    m_language->addItem(u"Português (Brasil)"_s, u"pt_BR"_s);
    m_textSize->setRange(-4, 24);
    m_textSize->setToolTip(tr("Steps bigger or smaller than the system's fixed-width font, as Ctrl+= and Ctrl+- set"));
    connect(m_textSize, &QSpinBox::valueChanged, this, &PreferencesDialog::textSizeChanged);

    auto *language = new QLabel(tr("&Language:"), this);
    language->setBuddy(m_language);
    auto *size = new QLabel(tr("Console &text size (0 is the usual size):"), this);
    size->setBuddy(m_textSize);
    auto *note = new QLabel(tr("A new language is used from the next start."), this);
    note->setWordWrap(true);
    note->setForegroundRole(QPalette::PlaceholderText);

    auto *form = new QFormLayout;
    form->addRow(language, m_language);
    form->addRow(QString(), note);
    form->addRow(size, m_textSize);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, this);
    connect(buttons, &QDialogButtonBox::accepted, this, [this] {
        save();
        accept();
    });
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(form);
    layout->addWidget(m_notices);
    layout->addWidget(m_reopenProject);
    layout->addWidget(m_reconnect);
    layout->addWidget(m_reopenSerial);
    layout->addStretch(1);
    layout->addWidget(buttons);
    load();
}

void PreferencesDialog::load()
{
    QSettings s;
    const int i = m_language->findData(Preferences::language());
    m_language->setCurrentIndex(i < 0 ? 0 : i);
    {
        const QSignalBlocker block(m_textSize);
        m_textSize->setValue(s.value(u"console/zoom"_s, 0).toInt());
    }
    m_notices->setChecked(Preferences::releaseNotices());
    m_reopenProject->setChecked(Preferences::reopenProject());
    m_reconnect->setChecked(Preferences::serialReconnect());
    m_reopenSerial->setChecked(Preferences::reopenSerial());
}

void PreferencesDialog::save()
{
    QSettings s;
    s.setValue(u"ui/language"_s, m_language->currentData().toString());
    s.setValue(u"console/zoom"_s, m_textSize->value());
    s.setValue(u"update/notices"_s, m_notices->isChecked());
    s.setValue(u"project/reopen"_s, m_reopenProject->isChecked());
    s.setValue(u"serial/reconnect"_s, m_reconnect->isChecked());
    s.setValue(u"serial/reopen"_s, m_reopenSerial->isChecked());
}
