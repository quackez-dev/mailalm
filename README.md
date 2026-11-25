# MailALM

A mail and calendar client for Windows, written in Rust with egui: Outlook.com/Hotmail, Gmail and
classic IMAP accounts side by side, each kept clearly apart.

**Status: phase 5.** IMAP accounts (Vidéotron, Bell and others), Hotmail/Outlook.com accounts
(Microsoft Graph) and Gmail accounts (Gmail API), the last two signed in on the provider's page in
the browser: reading, sending in plain text, folders and labels, moving, flags, a local cache,
search in the cache, and changes made while offline. Outlook accounts report junk to Outlook.com;
Gmail accounts report spam to Gmail and block or allow senders with Gmail filters.

Attachments stay locked until they are reviewed one by one: the review shows the file's real
type (found from its content, compared with its name), its size, the sender and the message's
SPF, DKIM and DMARC results, and the warnings (double extension, program, script, macros,
archive, shortcut, disk image…). MailALM can then preview it itself (pictures, the first pages
of a PDF, text), save it (with Windows' "from the internet" mark), or, for PDFs, pictures, .txt
files and Office documents without macros only, open a marked copy with Windows after a second
confirmation. Messages are sent with attachments (Attach… or dropped on the window), with a
warning when the text mentions one that is missing; an Outlook account sends at most 3 MB for
now (no upload session yet).

## Principles

- **Security first.** Nothing opens, loads or runs without an explicit action: attachments are
  locked until they are reviewed one by one, remote pictures are blocked, links show their real
  domain. No telemetry, no automatic update.
- **Separate accounts.** The focused account is expanded and the others are collapsed; "New mail"
  writes from the focused account, and the unified inbox has no default sender.
- **The server is the reference.** Blocking a sender, rules and the calendar are done on the
  account's server whenever the provider allows it.

The full plan, in French: [PLAN.md](PLAN.md). Ideas kept for later: [IDEES.md](IDEES.md).

## Layout

```
apps/mailalm/     the desktop app (egui)
crates/model/     accounts, servers, folders, summaries, search queries, workspace rules; calendars,
                  events, recurrences, tasks, contacts, iCalendar invitations (pure, tested)
crates/mime/      reading messages and writing outgoing mail (mail-parser, mail-builder, html2text)
crates/net/       IMAP, TLS, SMTP, Microsoft and Google sign-in, Graph, Gmail, Google Calendar, Tasks
                  and People, partly adapted from MailCraft (see ATTRIBUTION.md)
crates/store/     the local cache, one folder per account
crates/safety/    attachments: real type from the content, warnings, what may be done, SPF/DKIM/DMARC, Mark-of-the-Web (pure, tested)
crates/engine/    one worker thread per account (IMAP, Outlook or Gmail): cache first, then the server; offline changes
docs/             the domains MailALM contacts; registering MailALM with Microsoft
```

Passwords and Microsoft sign-ins (refresh tokens) are kept in the Windows Credential Manager, never
in a file. The settings are in
`%APPDATA%\MailALM`, the cache in `%LOCALAPPDATA%\MailALM\cache` (not encrypted: BitLocker and the
Windows session protect it).

## Building

MailALM is built and tested on GitHub Actions only ([build.yml](.github/workflows/build.yml)):

- dependencies are pinned in `Cargo.lock` and audited before they are used;
- egui and winit (storytold's forks) come from private mirrors at the audited commits;
- tests and build run offline.

A manual run or a `crafttools-v*` tag produces an unsigned portable zip with its SHA-256 sums.

## License

MailALM is dual-licensed under the [MIT license](LICENSE-MIT) or the [Apache License 2.0](LICENSE-APACHE), at
your option. Code taken from other MIT or Apache-2.0 projects keeps its origin and notices, listed in
[ATTRIBUTION.md](ATTRIBUTION.md).
