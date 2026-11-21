// Toolchain setup: what builds and the simulator need, what is missing, how big it
// is, and installing what the core can install. Renders toolchain.status only.

#pragma once

#include <QDialog>
#include <QElapsedTimer>
#include <QList>

#include <utility>
#include <QJsonArray>

class CoreConnection;
class QLabel;
class QPushButton;
class QPlainTextEdit;
class QProgressBar;
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

    QPushButton *installAllButton() const { return m_installAll; }

    // Render toolchain.download (public for tests), and what the bar says.
    void showDownload(qint64 bytes, qint64 total);
    QString downloadText() const;

private:
    void install(const QString &component, const QString &name);
    // Start the next queued install (Install everything needed).
    void installNext();

    CoreConnection *m_core;
    QLabel *m_summary;
    QTreeWidget *m_tree;
    QPlainTextEdit *m_log;
    QProgressBar *m_download;
    QElapsedTimer m_clock;
    QList<std::pair<qint64, qint64>> m_samples; // (ms, bytes) of the last half minute
    QPushButton *m_installAll = nullptr;
    // Components the app can install that are missing (id, name), and those still to
    // install in this round.
    QList<std::pair<QString, QString>> m_missing;
    QList<std::pair<QString, QString>> m_queue;
};
