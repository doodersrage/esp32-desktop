// Ctrl+K: type to find an action, a project or a panel, and Enter to use it. The
// main window says what there is; this only filters and runs.

#pragma once

#include <QDialog>

#include <functional>

class QLineEdit;
class QListWidget;

class CommandPalette : public QDialog {
    Q_OBJECT

public:
    struct Entry {
        QString text;   // what it does, or the project's name
        QString detail; // its shortcut, or what kind of thing it is
        std::function<void()> run;
    };

    explicit CommandPalette(QWidget *parent = nullptr);
    void setEntries(QList<Entry> entries);
    QLineEdit *field() const { return m_field; }
    // The entries the typed words match, in order (for tests).
    QStringList shown() const;
    // Run the selected entry and close.
    void runSelected();

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    void filter();

    QLineEdit *m_field;
    QListWidget *m_list;
    QList<Entry> m_entries;
};
