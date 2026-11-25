# Attribution

MailALM includes code adapted from **MailCraft** (<https://github.com/ZIDAN44/mailcraft>), at commit
`c3510d134ebc3f2532a1922354fcad0d542c931e` (v0.41.0), dual-licensed under the MIT licence or the Apache
License 2.0, at your option (the same terms as MailALM).

MailCraft's copyright notice (from its `LICENSE-MIT`):

> Copyright (c) 2026 Zinadin Zidan and the MailCraft contributors

The MIT and Apache 2.0 licence texts are in MailALM's `LICENSE-MIT` and `LICENSE-APACHE`. Each adapted
file starts with a comment naming its origin and what changed. The copy was read and audited before use:
`audits/mailalm/reprise-net.md`, `reprise-oauth-graph.md` and `reprise-apercus.md` in CraftTools.

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
| `apps/mailalm/src/preview.rs` | `decode_pixels` from `crates/ui-egui/src/reading/attachments.rs`, `pdf_page` and `is_bold` from `crates/ui-egui/src/look/thumbs.rs` | MIT OR Apache-2.0 |
| `crates/net/src/graph/pim/convert.rs` | `rrule_of` and `recurrence_of` from `crates/net/src/exchange/graph_pim.rs` (the rest is MailALM's own) | MIT OR Apache-2.0 |
| `crates/model/src/pim/recurrence.rs` | `crates/model/src/pim/ical/recurrence.rs` | MIT OR Apache-2.0 |
| `crates/model/src/pim/ical.rs` | content lines, escaping, folding, dates and durations from `crates/model/src/pim/ical.rs`; a `VTIMEZONE`'s offsets from `crates/model/src/pim/ical/zones.rs` | MIT OR Apache-2.0 |

The phase 6 copies (calendars, recurrences, iCalendar) were audited in `audits/mailalm/reprise-pim.md`.

The test certificates and keys in `crates/net/src/stream/trust/test_certs.rs` were made up for
MailCraft's tests; they are compiled into tests only and are used nowhere else.

## Druide's antidote-api-python

MailALM's Antidote bridge (`crates/antidote`) ports to Rust the protocol of **antidote-api-python** 1.1.0, the
integration API Druide informatique sent for Antidote (<https://github.com/DruideInformatiqueInc/antidote-api-python>),
under the MIT licence. No Python code is included: the messages, their fields and the conversation were rewritten in
Rust from it, and every check on what Antidote sends is MailALM's. The package was read and audited before use:
`audits/mailalm/reprise-antidote.md` in CraftTools.

Druide's copyright notice (from its `LICENSE`):

> © 2026 Druide informatique inc.

Its permission notice is the MIT licence text in MailALM's `LICENSE-MIT`. Each ported file starts with a comment
naming its origin.

| MailALM file | antidote-api-python 1.1.0 origin | Licence |
|---|---|---|
| `crates/antidote/src/protocol.rs` | `messages.py`, `requete_sortante.py`, `requete_entrante.py`, `params.py`, `reponses.py`, `configuration_texteur.py`, `zone_de_texte.py`, `info_style.py`, `style_texte.py`, `type_document.py`, `type_identifiant_cache.py`, the dispatch of `agent_connectix.py` | MIT |
| `crates/antidote/src/session.rs` | `agent_connectix.py`, `agent_texteur.py` | MIT |
| `crates/antidote/src/locate.rs` | `recupere_port_websocket.py`, `windows.py` | MIT |

## WordCraft

MailALM's MCP channel (`crates/control`) and its MCP server (`apps/mailalm-mcp`) include code adapted from
**WordCraft** (<https://github.com/storytold/wordcraft>), at commit `ca7f262a597c75da4a157b730c68aa2db8d12734`,
dual-licensed under the MIT licence or the Apache License 2.0, at your option (the same terms as MailALM).

WordCraft's copyright notice (from its `LICENSE-MIT`):

> Copyright (c) 2026 ArtCraft Team and the WordCraft contributors

Each adapted file starts with a comment naming its origin and what changed. The copy was read and audited
before use: `audits/mailalm/reprise-mcp.md` in CraftTools.

| MailALM file | WordCraft origin (commit `ca7f262a`) | Licence |
|---|---|---|
| `crates/control/src/key.rs` | `crates/control-key/src/lib.rs`; `random_key`, `write_key_file` and `keys_match` from `apps/wordcraft/src/control_server.rs` | MIT OR Apache-2.0 |
| `crates/control/src/wire.rs` | `read_bounded_line` from `crates/mcp/src/server.rs`; `discard_rest_of_line` from `apps/wordcraft/src/control_server.rs` | MIT OR Apache-2.0 |
| `crates/control/src/server.rs` | `apps/wordcraft/src/control_server.rs`; `ControlRequest` and its waiter from `crates/ui-egui/src/control.rs` | MIT OR Apache-2.0 |
| `crates/control/src/server/tests.rs` | some tests of `apps/wordcraft/src/control_server.rs` | MIT OR Apache-2.0 |
| `crates/control/src/client.rs` | `Remote` from `crates/mcp/src/backend.rs` | MIT OR Apache-2.0 |
| `apps/mailalm-mcp/src/rpc.rs` | `crates/mcp/src/server.rs` | MIT OR Apache-2.0 |
| `apps/mailalm-mcp/src/tools.rs` | `ToolResult` and the tool entry from `crates/mcp/src/tools.rs` | MIT OR Apache-2.0 |

The tool catalog, the arguments and answers, the app-side adapter, the journal and the tests of `apps/mailalm-mcp`
are MailALM's own.
