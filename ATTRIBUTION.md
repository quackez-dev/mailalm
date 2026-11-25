# Attribution

MailALM includes code adapted from **MailCraft** (<https://github.com/ZIDAN44/mailcraft>), at commit
`c3510d134ebc3f2532a1922354fcad0d542c931e` (v0.41.0), dual-licensed under the MIT licence or the Apache
License 2.0, at your option (the same terms as MailALM).

MailCraft's copyright notice (from its `LICENSE-MIT`):

> Copyright (c) 2026 Zinadin Zidan and the MailCraft contributors

The MIT and Apache 2.0 licence texts are in MailALM's `LICENSE-MIT` and `LICENSE-APACHE`. Each adapted
file starts with a comment naming its origin and what changed. The copy was read and audited before use:
`audits/mailalm/reprise-net.md` in CraftTools.

## Files

| MailALM file | MailCraft origin (commit `c3510d13`) | Licence |
|---|---|---|
| `crates/net/src/lib.rs` | `crates/net/src/lib.rs` | MIT OR Apache-2.0 |
| `crates/net/src/stream.rs` | `crates/net/src/stream.rs` | MIT OR Apache-2.0 |
| `crates/net/src/stream/trust.rs` | `crates/net/src/stream/trust.rs` | MIT OR Apache-2.0 |
| `crates/net/src/stream/trust/test_certs.rs` | `crates/net/src/stream/trust/test_certs.rs` | MIT OR Apache-2.0 |
| `crates/net/src/stream/trust/tests.rs` | `crates/net/src/stream/trust/tests.rs` | MIT OR Apache-2.0 |
| `crates/net/src/imap.rs` | `crates/net/src/protocols/imap.rs`, with `quote` from `crates/net/src/protocols.rs` and `days_from_civil` from `crates/model/src/dates.rs` | MIT OR Apache-2.0 |
| `crates/net/src/imap/conn.rs` | `crates/net/src/protocols/imap/conn.rs` | MIT OR Apache-2.0 |
| `crates/net/src/imap/quotes.rs` | `crates/net/src/protocols/imap/quotes.rs` | MIT OR Apache-2.0 |
| `crates/net/src/imap/sign_in.rs` | `crates/net/src/protocols/imap/sign_in.rs` | MIT OR Apache-2.0 |
| `crates/net/src/imap/folders.rs` | `crates/net/src/protocols/imap/folders.rs`, with `FolderRole::from_attribute` from `crates/model/src/folder.rs` | MIT OR Apache-2.0 |
| `crates/net/src/imap/messages.rs` | `crates/net/src/protocols/imap/messages.rs` | MIT OR Apache-2.0 |
| `crates/net/src/imap/utf7.rs` | `crates/model/src/encoding/utf7.rs` | MIT OR Apache-2.0 |
| `crates/net/src/imap/tests.rs` | `crates/net/src/protocols/imap/tests.rs` | MIT OR Apache-2.0 |
| `crates/net/src/smtp.rs` | `crates/net/src/protocols/smtp.rs` | MIT OR Apache-2.0 |
| `crates/net/src/base64.rs` | `encode` from `crates/model/src/encoding/base64.rs`, `b64url` from `crates/net/src/oauth.rs` | MIT OR Apache-2.0 |
| `crates/net/src/http.rs` | `crates/net/src/http.rs` | MIT OR Apache-2.0 |
| `crates/net/src/http/body.rs` | `crates/net/src/http/body.rs` | MIT OR Apache-2.0 |
| `crates/net/src/http/tests.rs` | `crates/net/src/http/tests.rs` | MIT OR Apache-2.0 |
| `crates/net/src/oauth.rs` | `crates/net/src/oauth.rs` | MIT OR Apache-2.0 |
| `crates/net/src/oauth/tests.rs` | the tests in `crates/net/src/oauth.rs` | MIT OR Apache-2.0 |
| `crates/net/src/graph.rs` | `crates/net/src/exchange/graph.rs` | MIT OR Apache-2.0 |
| `crates/net/src/graph/convert.rs` | `flags` from `crates/net/src/exchange/graph/flags.rs`, `address` from `crates/net/src/exchange/graph.rs` | MIT OR Apache-2.0 |
| `crates/net/src/graph/pim/convert.rs` | `rrule_of` and `recurrence_of` from `crates/net/src/exchange/graph_pim.rs` (the rest is MailALM's own) | MIT OR Apache-2.0 |
| `crates/model/src/pim/recurrence.rs` | `crates/model/src/pim/ical/recurrence.rs` | MIT OR Apache-2.0 |
| `crates/model/src/pim/ical.rs` | content lines, escaping, folding, dates and durations from `crates/model/src/pim/ical.rs`; a `VTIMEZONE`'s offsets from `crates/model/src/pim/ical/zones.rs` | MIT OR Apache-2.0 |

The phase 6 copies (calendars, recurrences, iCalendar) were audited in `audits/mailalm/reprise-pim.md`.

The test certificates and keys in `crates/net/src/stream/trust/test_certs.rs` were made up for
MailCraft's tests; they are compiled into tests only and are used nowhere else.
