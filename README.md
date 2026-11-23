# MailALM

A mail and calendar client for Windows, written in Rust with egui: Outlook.com/Hotmail, Gmail and
classic IMAP accounts side by side, each kept clearly apart.

**Status: phase 2.** IMAP accounts (Vidéotron, Bell and others): reading, sending in plain text,
folders, moving, flags, a local cache, search in the cache, and changes made while offline. Outlook
and Gmail come in phases 3 and 4; attachments can be listed but not opened yet (phase 5).

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
crates/model/     accounts, servers, folders, summaries, search queries, workspace rules (pure, tested)
crates/mime/      reading messages and writing outgoing mail (mail-parser, mail-builder, html2text)
crates/net/       IMAP, TLS and SMTP, adapted from MailCraft (see ATTRIBUTION.md)
crates/store/     the local cache, one folder per account
crates/engine/    one worker thread per account: cache first, then the server; offline changes
docs/             the domains MailALM contacts
```

Passwords are kept in the Windows Credential Manager, never in a file. The settings are in
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
