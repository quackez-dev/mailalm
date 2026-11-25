//! Folders as `.mbox` files: messages one after the other, each after a `From_` line.
//!
//! Written as **mboxrd**: a line of a message that starts with `From `, after any number of `>`,
//! gets one more `>`, so reading takes exactly one away again and the message comes back as it
//! was. Read as mboxrd too, which also reads mboxo (Thunderbird's, eM Client's): there `>From ` was
//! `From `, and a line that was `>From ` already is the one thing the format can't tell apart.
//!
//! A message starts at a line beginning with `From `, at the start of the file or after an empty
//! line; the empty line before the next one is the separator, not part of the message.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use mailalm_model::Flags;
use mailalm_store::Store;

use crate::{Cancel, Item, MAX_MESSAGE, Report, Run, Sink, Skip, walk};

/// The most of a line read at once: a longer line is read in pieces.
const PIECE: usize = 64 << 10;
/// The most of a message's header looked at for its marks.
const MAX_MARKS_HEADER: usize = 64 << 10;

/// Writes messages as an mbox (mboxrd) to `out`.
pub struct Writer<W: Write> {
    out: W,
}

impl<W: Write> Writer<W> {
    pub fn new(out: W) -> Self {
        Writer { out }
    }

    /// Adds a message: its `From_` line (with `sender`, an address, and `date`, seconds since
    /// 1970), its lines escaped, a line end if it has none at its end, and the empty line that
    /// separates it from the next.
    pub fn add(&mut self, raw: &[u8], sender: &str, date: i64) -> io::Result<()> {
        writeln!(self.out, "From {} {}", envelope_sender(sender), asctime(date))?;
        for line in raw.split_inclusive(|b| *b == b'\n') {
            if is_from_line(line) {
                self.out.write_all(b">")?;
            }
            self.out.write_all(line)?;
        }
        if !raw.ends_with(b"\n") {
            self.out.write_all(b"\n")?;
        }
        self.out.write_all(b"\n")
    }

    /// Flushes what is written, and gives the output back.
    pub fn finish(mut self) -> io::Result<W> {
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Whether a line is one mboxrd escapes: `From `, after any number of `>`.
fn is_from_line(line: &[u8]) -> bool {
    let start = line.iter().position(|b| *b != b'>').unwrap_or(line.len());
    line.get(start..).is_some_and(|rest| rest.starts_with(b"From "))
}

/// The sender as a `From_` line takes it: one word of visible ASCII, at most 254 characters;
/// `MAILER-DAEMON` when there is none.
fn envelope_sender(sender: &str) -> String {
    let s: String = sender.chars().filter(|c| c.is_ascii_graphic()).take(254).collect();
    if s.is_empty() { "MAILER-DAEMON".to_owned() } else { s }
}

/// `date` (seconds since 1970, UTC) as `asctime` writes it: `Thu Nov 24 18:22:48 1986`.
pub(crate) fn asctime(date: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    // From 1970 to the end of 9999.
    let t = date.clamp(0, 253_402_300_799);
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let (year, month, day) = civil(days);
    let weekday = usize::try_from((days + 4).rem_euclid(7)).unwrap_or(0);
    let month_name = usize::try_from(month - 1).ok().and_then(|m| MONTHS.get(m)).copied().unwrap_or("Jan");
    format!(
        "{} {} {:>2} {:02}:{:02}:{:02} {}",
        DAYS.get(weekday).copied().unwrap_or("Thu"),
        month_name,
        day,
        secs / 3600,
        secs % 3600 / 60,
        secs % 60,
        year
    )
}

/// The year, month and day of a day counted from 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// What an export did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Exported {
    /// Messages written.
    pub written: usize,
    /// Messages of the folder not in the cache (never opened, or too large to keep): not
    /// written.
    pub missing: usize,
    /// Stopped by [`Cancel`]: no file was written.
    pub cancelled: bool,
}

/// Writes the messages of `folder` that the cache holds to the mbox file `to`, oldest first.
/// `progress` is told the messages done and their number. The file is written beside `to`, then
/// renamed over it: a cancelled or failed export leaves no half file.
pub fn export(store: &Store, folder: &str, to: &Path, cancel: &Cancel, progress: &mut dyn FnMut(usize, usize)) -> io::Result<Exported> {
    let summaries = store.summaries(folder);
    let mut tmp = to.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut done = Exported::default();
    let file = File::create(&tmp)?;
    let mut writer = Writer::new(BufWriter::new(file));
    let total = summaries.len();
    let written = (|| -> io::Result<()> {
        for (n, s) in summaries.iter().rev().enumerate() {
            if cancel.is_cancelled() {
                done.cancelled = true;
                return Ok(());
            }
            match store.message(folder, s.uid) {
                Some(raw) => {
                    writer.add(&raw, &s.from.address, s.date)?;
                    done.written += 1;
                }
                None => done.missing += 1,
            }
            progress(n + 1, total);
        }
        Ok(())
    })();
    let finished = written.and_then(|()| writer.finish()).and_then(|out| out.into_inner().map_err(|e| e.into_error())).and_then(|f| f.sync_all());
    if let Err(e) = finished {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if done.cancelled {
        let _ = fs::remove_file(&tmp);
        done.written = 0;
        return Ok(done);
    }
    if let Err(e) = fs::rename(&tmp, to) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(done)
}

/// A message's marks, from the headers mail programs keep them in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Marks {
    /// `None` when the message has none of those headers.
    pub(crate) flags: Option<Flags>,
    /// Thunderbird deleted it (`X-Mozilla-Status` 0x0008) and kept it in the file until the
    /// folder is compacted.
    pub(crate) deleted: bool,
}

/// The marks `X-Mozilla-Status` (Thunderbird), or `Status` and `X-Status` (other mbox programs)
/// keep, read from the first [`MAX_MARKS_HEADER`] bytes of the header.
pub(crate) fn marks(raw: &[u8]) -> Marks {
    let head = raw.get(..raw.len().min(MAX_MARKS_HEADER)).unwrap_or_default();
    let (mut mozilla, mut status, mut x_status): (Option<u16>, Option<String>, Option<String>) = (None, None, None);
    for line in head.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            break;
        }
        let Some(colon) = line.iter().position(|b| *b == b':') else { continue };
        let name = String::from_utf8_lossy(line.get(..colon).unwrap_or_default()).to_ascii_lowercase();
        let value = String::from_utf8_lossy(line.get(colon + 1..).unwrap_or_default()).trim().to_owned();
        match name.as_str() {
            "x-mozilla-status" if mozilla.is_none() => mozilla = u16::from_str_radix(&value, 16).ok(),
            "status" if status.is_none() => status = Some(value),
            "x-status" if x_status.is_none() => x_status = Some(value),
            _ => {}
        }
    }
    if let Some(bits) = mozilla {
        let flags = Flags { seen: bits & 0x0001 != 0, answered: bits & 0x0002 != 0, flagged: bits & 0x0004 != 0, draft: false };
        return Marks { flags: Some(flags), deleted: bits & 0x0008 != 0 };
    }
    if status.is_none() && x_status.is_none() {
        return Marks::default();
    }
    let has = |v: &Option<String>, c: char| v.as_deref().is_some_and(|v| v.contains(c));
    let flags = Flags { seen: has(&status, 'R'), answered: has(&x_status, 'A'), flagged: has(&x_status, 'F'), draft: has(&x_status, 'T') };
    Marks { flags: Some(flags), deleted: false }
}

/// Imports one mbox file into `folder`. An error when it is not an mbox file at all.
pub fn import_file(path: &Path, folder: &[String], sink: &mut dyn Sink, cancel: &Cancel) -> Result<Report, String> {
    let file = File::open(path).map_err(|e| format!("This file can't be read: {e}"))?;
    let mut run = Run::new(sink, cancel);
    run.progress.total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let was_mbox = read(&mut run, BufReader::new(file), folder)?;
    if !was_mbox {
        return Err("This file is not an mbox file.".to_owned());
    }
    run.report.folders = 1;
    Ok(run.finish())
}

/// Imports a folder tree of mbox files: a Thunderbird profile's mail folder (`Inbox`,
/// `Inbox.sbd/Work`…; the `.msf` index files and other files are passed over), or a folder of
/// `.mbox` files (eM Client's export). Each file is a folder named after it, without `.mbox`;
/// a `.sbd` folder holds the folders inside the one of the same name.
pub fn import_tree(root: &Path, sink: &mut dyn Sink, cancel: &Cancel) -> Result<Report, String> {
    let sbd = |name: &str| name.strip_suffix(".sbd").unwrap_or(name).to_owned();
    let found = walk::walk(root, &sbd).map_err(|e| format!("This folder can't be read: {e}"))?;
    let mut run = Run::new(sink, cancel);
    run.report.left_out = found.left_out;
    run.progress.total = found.files.iter().map(|f| f.len).fold(0u64, u64::saturating_add);
    for f in &found.files {
        if run.stopped() {
            break;
        }
        let start = run.progress.done;
        let lower = f.name.to_ascii_lowercase();
        let skip_name = [".msf", ".dat", ".html", ".json", ".sqlite", ".js", ".eml"].iter().any(|ext| lower.ends_with(ext));
        let Ok(file) = File::open(&f.path) else {
            run.report.left_out += 1;
            continue;
        };
        let stem = f.name.strip_suffix(".mbox").or_else(|| f.name.strip_suffix(".MBOX")).unwrap_or(&f.name);
        let mut folder = f.dirs.clone();
        folder.push(crate::folder_name(stem));
        let mut reader = BufReader::new(file);
        // Only a file that starts as an mbox does (an empty one is an empty folder).
        let starts = reader.fill_buf().map(|b| b.starts_with(b"From ")).unwrap_or(false);
        if skip_name || !starts {
            run.progress.done = start.saturating_add(f.len);
            continue;
        }
        if read(&mut run, reader, &folder)? {
            run.report.folders += 1;
        }
        run.progress.done = start.saturating_add(f.len);
    }
    Ok(run.finish())
}

/// Reads up to `cap` bytes of `r` into `out`, up to and with the next line end. The bytes read (0
/// at the end of the file), and whether they end a line.
fn read_piece(r: &mut impl BufRead, out: &mut Vec<u8>, cap: usize) -> io::Result<(usize, bool)> {
    let mut n = 0;
    loop {
        let buf = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.is_empty() {
            return Ok((n, false));
        }
        let room = cap.saturating_sub(n);
        let window = buf.get(..buf.len().min(room)).unwrap_or_default();
        let (used, ended) = match window.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (window.len(), false),
        };
        out.extend_from_slice(window.get(..used).unwrap_or_default());
        r.consume(used);
        n += used;
        if ended || n >= cap {
            return Ok((n, ended));
        }
    }
}

/// A message being gathered.
struct Gathering {
    raw: Vec<u8>,
    too_large: bool,
}

/// Reads the messages of an mbox from `r` into `folder`. False when it doesn't start as an mbox
/// does (its first line isn't a `From ` line).
fn read(run: &mut Run<'_, '_>, mut r: impl BufRead, folder: &[String]) -> Result<bool, String> {
    let fail = |e: io::Error| format!("The file could not be read to its end: {e}");
    let mut line = Vec::with_capacity(PIECE);
    let mut current: Option<Gathering> = None;
    let (mut line_start, mut after_empty, mut first) = (true, true, true);
    let start = run.progress.done;
    let mut consumed = 0u64;
    loop {
        if run.stopped() {
            return Ok(true);
        }
        line.clear();
        let (n, ended) = read_piece(&mut r, &mut line, PIECE).map_err(fail)?;
        if n == 0 {
            break;
        }
        consumed += n as u64;
        run.progress.done = start.saturating_add(consumed);
        let at_start = line_start;
        line_start = ended;
        if first {
            first = false;
            if !line.starts_with(b"From ") {
                return Ok(false);
            }
        }
        if at_start && after_empty && line.starts_with(b"From ") {
            if let Some(done) = current.take() {
                hand_over(run, done, folder)?;
            }
            current = Some(Gathering { raw: Vec::new(), too_large: false });
            after_empty = false;
            // The rest of a `From_` line longer than a piece.
            let mut ends = ended;
            while !ends {
                line.clear();
                let (n, e) = read_piece(&mut r, &mut line, PIECE).map_err(fail)?;
                if n == 0 {
                    break;
                }
                consumed += n as u64;
                ends = e;
            }
            line_start = true;
            continue;
        }
        after_empty = at_start && ended && (line == b"\n" || line == b"\r\n");
        let Some(message) = current.as_mut() else { continue };
        if message.too_large {
            continue;
        }
        // mboxrd: one `>` less on a `From ` line that had some.
        let piece: &[u8] = if at_start && line.first() == Some(&b'>') && is_from_line(&line) { line.get(1..).unwrap_or_default() } else { &line };
        if message.raw.len().saturating_add(piece.len()) > MAX_MESSAGE.saturating_add(2) {
            message.too_large = true;
            message.raw = Vec::new();
        } else {
            message.raw.extend_from_slice(piece);
        }
    }
    if let Some(done) = current.take() {
        hand_over(run, done, folder)?;
    }
    // (An empty file is an empty mbox.)
    Ok(true)
}

/// A message gathered, without the empty line that separated it from the next.
fn hand_over(run: &mut Run<'_, '_>, mut m: Gathering, folder: &[String]) -> Result<(), String> {
    if m.too_large {
        run.skip(Skip::TooLarge);
        return Ok(());
    }
    if m.raw.ends_with(b"\r\n\r\n") {
        m.raw.truncate(m.raw.len() - 2);
    } else if m.raw.ends_with(b"\n\n") {
        m.raw.truncate(m.raw.len() - 1);
    }
    if m.raw.len() > MAX_MESSAGE {
        run.skip(Skip::TooLarge);
        return Ok(());
    }
    if m.raw.iter().all(|b| b.is_ascii_whitespace()) {
        run.skip(Skip::NotAMessage);
        return Ok(());
    }
    let marks = marks(&m.raw);
    if marks.deleted {
        run.skip(Skip::Deleted);
        return Ok(());
    }
    let flags = marks.flags.unwrap_or(Flags { seen: true, ..Flags::default() });
    run.message(Item { folder: folder.to_vec(), raw: m.raw, flags })
}

/// Reads an mbox held in memory into `folder` (for the tests and the app's own small files).
pub fn import_bytes(bytes: &[u8], folder: &[String], sink: &mut dyn Sink, cancel: &Cancel) -> Result<Report, String> {
    let mut run = Run::new(sink, cancel);
    run.progress.total = bytes.len() as u64;
    if !read(&mut run, io::Cursor::new(bytes), folder)? {
        return Err("This is not an mbox file.".to_owned());
    }
    run.report.folders = 1;
    Ok(run.finish())
}

