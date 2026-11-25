# MailALM privacy policy

MailALM is a mail and calendar client that runs on your own Windows computer. It has no server of its own and no
account system. This page says what it does with your data, including the data it reads from Google and Microsoft
accounts.

## What MailALM reads

When you add an account, MailALM reads, through the provider's own interface (Gmail API, Microsoft Graph, IMAP and
SMTP):

- your messages, folders or labels, and their read, flagged and junk states;
- your account's address and display name;
- on Gmail, your filters and automatic reply settings, when you manage them from MailALM.

It does so only to show you your mail and to carry out what you ask: send, move, mark, delete, report as junk,
block or allow a sender.

## Where your data goes

- **It stays on your computer.** Messages are cached in your Windows user profile, for reading offline and
  searching. Sign-in tokens and passwords are kept in the Windows Credential Manager, never in MailALM's files.
- **MailALM connects only to your account's provider**: Google for Gmail, Microsoft for Outlook.com, your mail
  provider's servers for IMAP. The full list of addresses is in `docs/domaines.md`.
- **Nothing is sent to the authors of MailALM or to anyone else.** There is no telemetry, no analytics, no
  advertising, no automatic update and no crash reporting.
- Your data is not sold, shared, transferred to third parties, used for advertising, or used to train AI models.

## Google user data

MailALM's use and transfer of information received from Google APIs adheres to the
[Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy),
including the Limited Use requirements. No human reads your Google data: it is processed only by MailALM on your
own computer.

## Removing your data

- Removing an account in MailALM deletes its local cache and its stored sign-in.
- You can also withdraw MailALM's access at any time: for Google at `https://myaccount.google.com/permissions`, for
  Microsoft at `https://account.live.com/consent/Manage`.

## Changes and contact

Changes to this policy are made in this file, and its history shows every version. MailALM is maintained by the
owner of the GitHub account `quackez-dev`.
