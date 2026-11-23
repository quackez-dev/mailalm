# MailALM

A mail and calendar client for Windows, written in Rust with egui: Outlook.com/Hotmail, Gmail and
classic IMAP accounts side by side, each kept clearly apart.

**Status: phase 1.** The window and its layout over a sample mailbox. No account can be connected
yet and the app makes no network connection.

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
crates/model/     accounts, providers and the workspace rules (pure Rust, tested)
```

## Building

MailALM is built and tested on GitHub Actions only ([build.yml](.github/workflows/build.yml)):

- dependencies are pinned in `Cargo.lock` and audited before they are used;
- egui and winit (storytold's forks) come from private mirrors at the audited commits;
- tests and build run offline.

A manual run or a `crafttools-v*` tag produces an unsigned portable zip with its SHA-256 sums.

Run the sample with a single account: `mailalm --one-account`.
