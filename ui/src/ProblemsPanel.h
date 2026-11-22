// Every explanation of the session (crashes, resets, upload errors, a build's first
// error), newest last, with when and where it came from and the site's page about
// it. The message bar shows only the latest; this keeps them all.

#pragma once

#include <QJsonArray>
#include <QUrl>
#include <QWidget>

class CoreConnection;
class QComboBox;
class QPushButton;
class QTreeWidget;

class ProblemsPanel : public QWidget {
    Q_OBJECT

public:
    enum Column { TimeColumn, SourceColumn, TextColumn, ColumnCount };
    static constexpr int UrlRole = Qt::UserRole + 1;
    // A crash's line, as ConsoleView::frameActivated gives it.
    static constexpr int LocationRole = Qt::UserRole + 2;
    // What to look up on esp32bench.dev's error pages: the error's own words.
    static constexpr int SearchRole = Qt::UserRole + 3;
    // Where it came from: build, sim or serial.
    static constexpr int SourceRole = Qt::UserRole + 4;
    // Only the problems from `source` (build, sim, serial), or all for none; Clear then
    // clears just those.
    void showOnly(const QString &source);
    QPushButton *clearButton() const { return m_clear; }
    // esp32bench.dev's search for an error's words (nothing else is sent).
    static QUrl searchUrl(const QString &words);
    // The words to search for: a crash's exception name (LoadProhibited), or a
    // compiler message without the names it quotes, at most six words.
    static QString searchTerms(const QString &text);

    explicit ProblemsPanel(CoreConnection *core, QWidget *parent = nullptr);
    // Add findings as outputExplained has them; `source` is sim, serial or build.
    void add(const QString &source, const QJsonArray &findings);
    // A crash whose backtrace the core decoded (sim.backtrace, serial.backtrace): where
    // it was, the sketch's own line first; double-click to see it.
    void addCrash(const QString &source, const QJsonArray &frames);
    // A build's errors and warnings (build.finished's diagnostics), in place of the last
    // build's: each opens at its line on a double-click.
    void setBuildDiagnostics(const QJsonArray &diagnostics);
    // A build diagnostic's row (replaced by the next build's).
    static constexpr int DiagnosticRole = Qt::UserRole + 5;
    QTreeWidget *tree() const { return m_tree; }

signals:
    // The first problem (not a note) since the list was cleared.
    void firstProblem();
    // A crash's entry was activated: {file, line, path}.
    void locationActivated(const QJsonObject &at);
    // How many entries the list has now.
    void countChanged(int count);

private:
    QTreeWidget *m_tree;
    QPushButton *m_clear;
    QComboBox *m_filter = nullptr;
    QString m_only;
    bool m_hasProblem = false;
};
