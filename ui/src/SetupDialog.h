// Toolchain setup: what builds and the simulator need, what is missing, how big it
// is, and installing what the core can install. Renders toolchain.status only.

#pragma once

#include <QDialog>
#include <QJsonArray>

class CoreConnection;
class QLabel;
class QPlainTextEdit;
class QTreeWidget;

class SetupDialog : public QDialog {
    Q_OBJECT

public:
    enum Column { NameColumn, NeededForColumn, StateColumn, VersionColumn, SizeColumn, ActionColumn, ColumnCount };

    explicit SetupDialog(CoreConnection *core, QWidget *parent = nullptr);

    // Render a toolchain.status result. Public so tests can feed fixed data.
    void showStatus(const QJsonArray &components);
    QTreeWidget *tree() const { return m_tree; }
    QString logText() const;

private:
    void install(const QString &component, const QString &name);

    CoreConnection *m_core;
    QLabel *m_summary;
    QTreeWidget *m_tree;
    QPlainTextEdit *m_log;
};
