# MailALM

A mail and calendar client for Windows, written in Rust with egui: Outlook.com/Hotmail, Gmail and
classic IMAP accounts side by side, each kept clearly apart.

**Status: phases 8 to 10, interface written, not yet tested.** IMAP accounts (Vidéotron, Bell and others), Hotmail/Outlook.com accounts
(Microsoft Graph) and Gmail accounts (Gmail API), the last two signed in on the provider's page in
the browser: reading, sending, folders and labels, moving, flags, a local cache,
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

**Phase 6** adds the calendar, reminders, invitations, tasks and contacts, offline too (changes
wait for the network). Outlook accounts use Microsoft Graph (calendars, Microsoft To Do,
contacts; a Teams meeting when Microsoft offers one), Gmail accounts Google Calendar (with Meet
links), Google Tasks and Google contacts (read only); an IMAP account has a calendar and a task
list kept on this computer, and sends and answers invitations by mail. The calendar shows every
account's calendars in their colours (day, work week, week, month, list), each can be hidden, and
the time taken in all of them can be drawn over the days; events are planned in their own window,
which shows what they clash with. An invitation received shows in the message, with its clashes;
it enters the calendar only when answered. Reminders ring in their own window (snooze, dismiss).
The recipient fields offer the contacts of every account and the people seen in mail.

**Phase 7** brings writing and reading. Messages are written in rich text (or plain text), with
each account's signature (above a reply's quoted text, switched with the sender), templates and
quick replies; replies and forwards quote the original with its formatting. Drafts are kept on
the account's server. Before sending, the window shows the sender in its colour, the recipients
outside its domain, a missing attachment and the size; Antidote's corrector, dictionaries and
guides (and Anti-Oups!, as an option) work on the text when Antidote is installed. A message sent
waits a few seconds with an Undo button, or until the date and time it is scheduled for. HTML
messages are shown formatted (converted by MailALM, no web engine): links show their real domain
and warn about look-alike names before they open; pictures from the internet stay blocked unless
loaded for a message or approved for a sender, and tracking pictures never load; a sender failing
DMARC is flagged; a mailing list can be left after a confirmation that says what will happen.

**Phases 8 to 10 (interface)**: the search box has its scope (the current account, accounts
ticked, or all; the current folder, with its subfolders, or the whole account), can ask the
servers too (their results come below, "from the server") and keeps searches as search folders.
Settings, Rules lists each account's rules (on the server for Outlook and Gmail, in MailALM for
IMAP), with an editor, a preview of the messages a rule would touch, and an explicit confirmation
for a forwarding. Messages can be snoozed and followed up ("remind me if no reply"). MCP access
(off by default) answers an agent's calls from the cache, asks before sending, deleting,
blocking, answering an invitation or changing a rule, shows a triage proposal as a list to tick,
and keeps a journal. Messages export as .eml or PDF, folders as .mbox; .eml, .mbox (Thunderbird,
eM Client's export) and Outlook .pst/.ost files import into "On this PC", a folder tree kept on
this computer only. The theme can be dark, light or as Windows is set; the language setting is
there, its French texts are not yet.

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
apps/mailalm-mcp/ the MCP server an agent starts (stdio); it forwards each tool call to the running
                  MailALM over 127.0.0.1 with a random key, only while MCP access is on (off by default)
crates/control/   that channel (key and port files, server and client), the tool catalog, the app-side
                  adapter (confirmations, accounts hidden from the MCP) and the call journal; partly
                  adapted from WordCraft (see ATTRIBUTION.md)
crates/model/     accounts, servers, folders, summaries, search queries, workspace rules; calendars,
                  events, recurrences, tasks, contacts, iCalendar invitations (pure, tested)
crates/mime/      reading messages and writing outgoing mail (mail-parser, mail-builder, html2text)
crates/compose/   the rich-text editor: document model and edits (pure, tested), egui widget and toolbar,
                  HTML sent with its plain-text alternative, import of untrusted HTML (html5ever's tokenizer)
crates/net/       IMAP, TLS, SMTP, Microsoft and Google sign-in, Graph, Gmail, Google Calendar, Tasks
                  and People, partly adapted from MailCraft (see ATTRIBUTION.md)
crates/store/     the local cache, one folder per account (mail, and calendars, tasks, contacts)
crates/safety/    attachments: real type from the content, warnings, what may be done, SPF/DKIM/DMARC, Mark-of-the-Web (pure, tested)
crates/engine/    one worker thread per account (IMAP, Outlook or Gmail): cache first, then the server; offline changes;
                  calendars, tasks and contacts synced, invitations by mail for IMAP; reminders and clashes (agenda)
crates/porting/   import and export: a message as .eml or PDF, a folder as .mbox; import of .eml, .mbox
                  (Thunderbird, eM Client's export) and Outlook .pst/.ost files, capped as received mail
crates/pst/       reading Outlook data files (.pst, .ost), read only; adapted from MailCraft
crates/print/     a message's header and text as a PDF; adapted from MailCraft
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
