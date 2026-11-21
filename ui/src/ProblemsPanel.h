// Every explanation of the session (crashes, resets, upload errors, a build's first
// error), newest last, with when and where it came from and the site's page about
// it. The message bar shows only the latest; this keeps them all.

#pragma once

#include <QJsonArray>
#include <QWidget>

class CoreConnection;
class QPushButton;
class QTreeWidget;

class ProblemsPanel : public QWidget {
    Q_OBJECT

public:
    enum Column { TimeColumn, SourceColumn, TextColumn, ColumnCount };
    static constexpr int UrlRole = Qt::UserRole + 1;

    explicit ProblemsPanel(CoreConnection *core, QWidget *parent = nullptr);
    // Add findings as outputExplained has them; `source` is sim, serial or build.
    void add(const QString &source, const QJsonArray &findings);
    QTreeWidget *tree() const { return m_tree; }

signals:
    // The first problem (not a note) since the list was cleared.
    void firstProblem();

private:
    QTreeWidget *m_tree;
    QPushButton *m_clear;
    bool m_hasProblem = false;
};
