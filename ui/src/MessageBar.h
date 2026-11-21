// A message across the top of the main window that stays until it is closed or a new
// attempt starts: build, flash and link errors, which the status bar would show only
// for a few seconds. Theme colours and icons only.

#pragma once

#include <QFrame>

#include <functional>

class QLabel;
class QPushButton;
class QToolButton;

class MessageBar : public QFrame {
    Q_OBJECT

public:
    explicit MessageBar(QWidget *parent = nullptr);

    // A problem; with `url`, a link to it (such as the file a build failed in).
    void showError(const QString &text, const QString &url = {}, const QString &linkText = {});
    // An explanation of something the board or a tool printed, with a link to the
    // site's page about it when there is one.
    void showExplanation(const QString &text, const QString &url);
    // News rather than a problem (a newer release), with a link. Closing it emits
    // dismissed(key), so the window can stop showing it.
    void showNotice(const QString &text, const QString &url, const QString &linkText, const QString &key);
    // Something just done, with a button that takes it back (Undo).
    void showUndo(const QString &text, const QString &button, std::function<void()> undo);
    QString text() const;
    // Close it as its close button does (Esc in the main window).
    void dismiss();
    QPushButton *actionButton() const { return m_action; }

signals:
    void dismissed(const QString &key);

private:
    QLabel *m_icon;
    QLabel *m_text;
    QToolButton *m_close;
    QPushButton *m_action;
    std::function<void()> m_undo;
    void setIcon(bool notice);

    QString m_plain; // an explanation's text without its link
    QString m_key;   // the notice shown, if one is
};
