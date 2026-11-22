#include "Preferences.h"

#include <QCheckBox>
#include <QComboBox>
#include <QLineEdit>
#include <algorithm>
#include <QCoreApplication>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QGuiApplication>
#include <QStyleHints>
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
QString theme()
{
    return QSettings().value(u"ui/theme"_s).toString();
}

bool canChooseTheme()
{
#if QT_VERSION >= QT_VERSION_CHECK(6, 8, 0)
    return true;
#else
    return false;
#endif
}

void applyTheme()
{
#if QT_VERSION >= QT_VERSION_CHECK(6, 8, 0)
    const QString t = theme();
    QGuiApplication::styleHints()->setColorScheme(t == u"light"   ? Qt::ColorScheme::Light
                                                  : t == u"dark" ? Qt::ColorScheme::Dark
                                                                 : Qt::ColorScheme::Unknown);
#endif
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
QString editorCommand()
{
    return QSettings().value(u"editor/command"_s).toString().trimmed();
}

int interfaceSize()
{
    const int size = QSettings().value(u"ui/size"_s, 100).toInt();
    return std::clamp(size, 50, 300);
}

void applyInterfaceSize()
{
    // Settings are the app's own: read them as the app will once it has its names.
    QCoreApplication::setApplicationName(u"esp32bench"_s);
    QCoreApplication::setOrganizationName(u"esp32bench"_s);
    const int size = interfaceSize();
    if (size != 100 && qEnvironmentVariableIsEmpty("QT_SCALE_FACTOR"))
        qputenv("QT_SCALE_FACTOR", QByteArray::number(size / 100.0));
}
} // namespace Preferences

PreferencesDialog::PreferencesDialog(QWidget *parent)
    : QDialog(parent)
    , m_language(new QComboBox(this))
    , m_theme(new QComboBox(this))
    , m_size(new QComboBox(this))
    , m_editor(new QComboBox(this))
    , m_editorTemplate(new QLineEdit(this))
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
    m_theme->addItem(tr("The system's"), QString());
    m_theme->addItem(tr("Light"), u"light"_s);
    m_theme->addItem(tr("Dark"), u"dark"_s);
    m_theme->setToolTip(tr("Light or dark colours, or the system's"));
    for (const int size : {100, 125, 150, 175, 200})
        m_size->addItem(tr("%1%").arg(size), size);
    m_size->setToolTip(tr("Everything bigger than the desktop makes it, for a large or very sharp screen"));
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
    if (Preferences::canChooseTheme()) {
        auto *theme = new QLabel(tr("&Colours:"), this);
        theme->setBuddy(m_theme);
        form->addRow(theme, m_theme);
    } else {
        m_theme->hide();
    }
    form->addRow(size, m_textSize);
    // The editor a problem's file opens in, at its line.
    m_editor->addItem(tr("The system's default"), QString());
    m_editor->addItem(u"Kate"_s, u"kate"_s);
    m_editor->addItem(u"Visual Studio Code"_s, u"code"_s);
    m_editor->addItem(tr("Another, with this command"), u"custom"_s);
    m_editor->setToolTip(tr("Where a build problem's file opens, at its line. The system's default opens the "
                            "file without going to the line."));
    m_editorTemplate->setPlaceholderText(u"kate -l {line} -c {col} {file}"_s);
    m_editorTemplate->setToolTip(tr("The editor's command: {file} is the file, {line} and {col} where in it"));
    m_editorTemplate->setAccessibleName(tr("Editor command"));
    connect(m_editor, &QComboBox::currentIndexChanged, this,
            [this] { m_editorTemplate->setEnabled(m_editor->currentData().toString() == u"custom"); });
    auto *editorLabel = new QLabel(tr("&Editor:"), this);
    editorLabel->setBuddy(m_editor);
    form->addRow(editorLabel, m_editor);
    form->addRow(QString(), m_editorTemplate);
    auto *interfaceSize = new QLabel(tr("&Interface size:"), this);
    interfaceSize->setBuddy(m_size);
    form->addRow(interfaceSize, m_size);
    auto *sizeNote = new QLabel(tr("A new size is used from the next start."), this);
    sizeNote->setWordWrap(true);
    sizeNote->setForegroundRole(QPalette::PlaceholderText);
    form->addRow(QString(), sizeNote);
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
    const int t = m_theme->findData(Preferences::theme());
    m_theme->setCurrentIndex(t < 0 ? 0 : t);
    {
        const QSignalBlocker block(m_textSize);
        m_textSize->setValue(s.value(u"console/zoom"_s, 0).toInt());
    }
    {
        const QString editor = Preferences::editorCommand();
        int e = m_editor->findData(editor);
        if (e < 0) {
            e = m_editor->findData(u"custom"_s);
            m_editorTemplate->setText(editor);
        }
        m_editor->setCurrentIndex(e);
        m_editorTemplate->setEnabled(m_editor->currentData().toString() == u"custom");
    }
    const int z = m_size->findData(Preferences::interfaceSize());
    m_size->setCurrentIndex(z < 0 ? 0 : z);
    m_notices->setChecked(Preferences::releaseNotices());
    m_reopenProject->setChecked(Preferences::reopenProject());
    m_reconnect->setChecked(Preferences::serialReconnect());
    m_reopenSerial->setChecked(Preferences::reopenSerial());
}

void PreferencesDialog::save()
{
    QSettings s;
    s.setValue(u"ui/language"_s, m_language->currentData().toString());
    s.setValue(u"ui/theme"_s, m_theme->currentData().toString());
    s.setValue(u"ui/size"_s, m_size->currentData().toInt());
    const QString editor = m_editor->currentData().toString();
    s.setValue(u"editor/command"_s, editor == u"custom"_s ? m_editorTemplate->text().trimmed() : editor);
    Preferences::applyTheme(); // at once
    s.setValue(u"console/zoom"_s, m_textSize->value());
    s.setValue(u"update/notices"_s, m_notices->isChecked());
    s.setValue(u"project/reopen"_s, m_reopenProject->isChecked());
    s.setValue(u"serial/reconnect"_s, m_reconnect->isChecked());
    s.setValue(u"serial/reopen"_s, m_reopenSerial->isChecked());
}
