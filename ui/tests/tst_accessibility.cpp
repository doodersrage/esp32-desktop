// Every control in the main window has a name a screen reader can say: buttons their
// text or an accessible name, and fields, lists and canvases an accessible name or a
// label that names them (a buddy).

#include "CoreConnection.h"
#include "MainWindow.h"

#include <QAbstractButton>
#include <QAbstractItemView>
#include <QAbstractScrollArea>
#include <QComboBox>
#include <QLabel>
#include <QLineEdit>
#include <QStandardPaths>
#include <QTabBar>
#include <QTest>

using namespace Qt::StringLiterals;

class TestAccessibility : public QObject {
    Q_OBJECT

    // Qt's own parts of composite widgets (a line edit's clear button, a tab bar's
    // scroll arrows, a dock's title buttons), which Qt names itself.
    static bool qtInternal(const QWidget *w)
    {
        if (w->objectName().startsWith(u"qt_"_s))
            return true;
        const QWidget *parent = w->parentWidget();
        return parent
               && (qobject_cast<const QLineEdit *>(parent) || qobject_cast<const QTabBar *>(parent)
                   || qobject_cast<const QComboBox *>(parent) || qobject_cast<const QAbstractScrollArea *>(parent)
                   || parent->inherits("QAbstractSpinBox") || parent->inherits("QScrollBar")
                   || parent->inherits("QComboBoxPrivateContainer"));
    }

private slots:
    void initTestCase() { QStandardPaths::setTestModeEnabled(true); }

    void everyControlHasAName()
    {
        CoreConnection core;
        MainWindow window(&core);
        QSet<const QWidget *> labelled;
        for (const QLabel *l : window.findChildren<QLabel *>()) {
            if (l->buddy())
                labelled.insert(l->buddy());
        }
        QStringList unnamed;
        for (QWidget *w : window.findChildren<QWidget *>()) {
            if (qtInternal(w) || !w->accessibleName().isEmpty() || labelled.contains(w))
                continue;
            // Its class, then its parents' up to a named panel, to find it by.
            QString where = QString::fromLatin1(w->metaObject()->className());
            for (const QWidget *p = w->parentWidget(); p; p = p->parentWidget()) {
                where += u" in "_s + QString::fromLatin1(p->metaObject()->className());
                if (QByteArray(p->metaObject()->className()) != "QWidget"
                    && QByteArray(p->metaObject()->className()) != "QSplitter")
                    break;
            }
            if (auto *b = qobject_cast<QAbstractButton *>(w)) {
                if (b->text().remove(u'&').trimmed().isEmpty())
                    unnamed << u"button %1 (%2)"_s.arg(where, b->toolTip());
            } else if (qobject_cast<QLineEdit *>(w) || qobject_cast<QComboBox *>(w)
                       || qobject_cast<QAbstractItemView *>(w) || w->inherits("QPlainTextEdit")
                       || w->inherits("QGraphicsView") || w->inherits("PlotCanvas") || w->inherits("TraceCanvas")) {
                unnamed << where;
            }
        }
        QVERIFY2(unnamed.isEmpty(), qPrintable(u"\n"_s + unnamed.join(u'\n')));
    }
};

QTEST_MAIN(TestAccessibility)
#include "tst_accessibility.moc"
