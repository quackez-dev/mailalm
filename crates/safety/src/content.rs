//! The real type of a file, from its content: magic numbers, and a cheap look inside a few
//! containers (a ZIP's central directory, an OLE file's directory entries, a PDF's name tokens,
//! an RTF's control words). Nothing is decompressed or decoded.
//!
//! Every read goes through `get` and checked arithmetic, and every loop is capped: the bytes come
//! from strangers.

use crate::names::{self, ExtClass};

/// The most text looked at to tell text from binary, and a script from plain text.
const TEXT_SAMPLE: usize = 64 << 10;
/// The most entries read from a ZIP's central directory.
const MAX_ZIP_ENTRIES: usize = 65_536;
/// The most names of programs inside an archive kept for the warning.
const MAX_LISTED: usize = 5;
/// The longest name kept from an archive.
const MAX_LISTED_NAME: usize = 80;
/// The most directory entries kept from an OLE file.
const MAX_OLE_NAMES: usize = 100_000;
/// The longest PDF name token compared.
const MAX_PDF_NAME: usize = 32;

/// A picture's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Webp,
    Tiff,
}

impl ImageFormat {
    /// Whether MailALM decodes it for a preview (TIFF is not decoded).
    pub fn previewable(self) -> bool {
        !matches!(self, ImageFormat::Tiff)
    }

    fn name(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Gif => "GIF",
            ImageFormat::Bmp => "BMP",
            ImageFormat::Webp => "WebP",
            ImageFormat::Tiff => "TIFF",
        }
    }
}

/// How an Office document is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficeFormat {
    /// Office Open XML (`.docx`, `.xlsx`, `.pptx` and their macro-enabled kin): a ZIP.
    Ooxml,
    /// OpenDocument (`.odt`, `.ods`, `.odp`): a ZIP.
    OpenDocument,
    /// The old binary formats (`.doc`, `.xls`, `.ppt`): an OLE compound file.
    Ole,
    /// Rich Text Format.
    Rtf,
}

/// Which Office program the document belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    Word,
    Excel,
    PowerPoint,
    Other,
}

/// Whether a document has macros.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Macros {
    None,
    Present,
    /// Its format can hold macros MailALM cannot find cheaply (old Excel and PowerPoint files).
    Unknown,
}

/// An Office document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OfficeInfo {
    pub format: OfficeFormat,
    pub app: App,
    pub macros: Macros,
    /// Embedded objects (OLE packages, ActiveX controls).
    pub embedded: bool,
    /// An Equation Editor object.
    pub equation: bool,
}

/// An archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveInfo {
    /// `ZIP`, `RAR`, `7-Zip`…
    pub format: &'static str,
    /// Some entries are encrypted (ZIP only).
    pub encrypted: bool,
    /// The first names inside that are programs, scripts, shortcuts or disk images (ZIP only:
    /// the other formats are not listed).
    pub programs: Vec<String>,
}

/// A program's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramKind {
    Windows,
    WindowsLibrary,
    Installer,
    Java,
    Linux,
    Mac,
}

/// What a text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind {
    Plain,
    /// HTML or SVG.
    Web,
    /// Text that reads like a script; the clue found.
    Script(&'static str),
}

/// What the content really is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RealType {
    Empty,
    /// A PDF; the active names found in it (`JavaScript`, `Launch`…).
    Pdf { active: Vec<&'static str> },
    Image(ImageFormat),
    Office(OfficeInfo),
    Archive(ArchiveInfo),
    Program(ProgramKind),
    /// A Windows shortcut (`.lnk`), or an internet shortcut (`.url`).
    Shortcut,
    /// A disk image; its kind (`ISO`, `VHD`…).
    DiskImage(&'static str),
    Text(TextKind),
    Unknown,
}

/// The broad kinds an extension and a content are compared by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    Pdf,
    Image,
    Office,
    Archive,
    Program,
    Shortcut,
    DiskImage,
    Text,
}

impl RealType {
    /// What it is, as a sentence ends: "a PDF document", "a Windows program"…
    pub fn label(&self) -> String {
        match self {
            RealType::Empty => "an empty file".to_owned(),
            RealType::Pdf { .. } => "a PDF document".to_owned(),
            RealType::Image(f) => format!("a {} picture", f.name()),
            RealType::Office(o) => {
                let what = match (o.format, o.app) {
                    (OfficeFormat::Rtf, _) => "an RTF document",
                    (OfficeFormat::OpenDocument, App::Word) => "an OpenDocument text",
                    (OfficeFormat::OpenDocument, App::Excel) => "an OpenDocument spreadsheet",
                    (OfficeFormat::OpenDocument, App::PowerPoint) => "an OpenDocument presentation",
                    (OfficeFormat::OpenDocument, App::Other) => "an OpenDocument file",
                    (OfficeFormat::Ooxml, App::Word) => "a Word document",
                    (OfficeFormat::Ooxml, App::Excel) => "an Excel workbook",
                    (OfficeFormat::Ooxml, App::PowerPoint) => "a PowerPoint presentation",
                    (OfficeFormat::Ooxml, App::Other) => "an Office Open XML file",
                    (OfficeFormat::Ole, App::Word) => "a Word document (old format)",
                    (OfficeFormat::Ole, App::Excel) => "an Excel workbook (old format)",
                    (OfficeFormat::Ole, App::PowerPoint) => "a PowerPoint presentation (old format)",
                    (OfficeFormat::Ole, App::Other) => "an OLE compound file (old Office format)",
                };
                match o.macros {
                    Macros::Present => format!("{what} with macros"),
                    _ => what.to_owned(),
                }
            }
            RealType::Archive(a) => format!("a {} archive", a.format),
            RealType::Program(p) => match p {
                ProgramKind::Windows => "a Windows program",
                ProgramKind::WindowsLibrary => "a Windows library (DLL)",
                ProgramKind::Installer => "a Windows installer",
                ProgramKind::Java => "a Java program",
                ProgramKind::Linux => "a Linux program",
                ProgramKind::Mac => "a macOS program",
            }
            .to_owned(),
            RealType::Shortcut => "a shortcut".to_owned(),
            RealType::DiskImage(k) => format!("a disk image ({k})"),
            RealType::Text(TextKind::Plain) => "plain text".to_owned(),
            RealType::Text(TextKind::Web) => "a web page (HTML or SVG)".to_owned(),
            RealType::Text(TextKind::Script(_)) => "text that reads like a script".to_owned(),
            RealType::Unknown => "of a kind MailALM does not recognise".to_owned(),
        }
    }

    /// Its broad kind; `None` when nothing is known (empty, unknown).
    pub(crate) fn family(&self) -> Option<Family> {
        match self {
            RealType::Empty | RealType::Unknown => None,
            RealType::Pdf { .. } => Some(Family::Pdf),
            RealType::Image(_) => Some(Family::Image),
            RealType::Office(_) => Some(Family::Office),
            RealType::Archive(_) => Some(Family::Archive),
            RealType::Program(_) => Some(Family::Program),
            RealType::Shortcut => Some(Family::Shortcut),
            RealType::DiskImage(_) => Some(Family::DiskImage),
            RealType::Text(_) => Some(Family::Text),
        }
    }
}

// ---- reading helpers (never panic) ----

fn at(b: &[u8], from: usize, len: usize) -> Option<&[u8]> {
    b.get(from..from.checked_add(len)?)
}

fn has_at(b: &[u8], from: usize, magic: &[u8]) -> bool {
    at(b, from, magic.len()) == Some(magic)
}

fn u16le(b: &[u8], from: usize) -> Option<u16> {
    Some(u16::from_le_bytes(at(b, from, 2)?.try_into().ok()?))
}

fn u32le(b: &[u8], from: usize) -> Option<u32> {
    Some(u32::from_le_bytes(at(b, from, 4)?.try_into().ok()?))
}

fn u64le(b: &[u8], from: usize) -> Option<u64> {
    Some(u64::from_le_bytes(at(b, from, 8)?.try_into().ok()?))
}

fn usize_of(n: impl TryInto<usize>) -> Option<usize> {
    n.try_into().ok()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

fn contains_ignore_case(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

// ---- detection ----

/// The real type of `b`.
pub fn detect(b: &[u8]) -> RealType {
    if b.is_empty() {
        return RealType::Empty;
    }
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return RealType::Image(ImageFormat::Png);
    }
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return RealType::Image(ImageFormat::Jpeg);
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return RealType::Image(ImageFormat::Gif);
    }
    if b.starts_with(b"RIFF") && has_at(b, 8, b"WEBP") {
        return RealType::Image(ImageFormat::Webp);
    }
    if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        return RealType::Image(ImageFormat::Tiff);
    }
    // "BM" alone is too common a start: the size of the header that follows must be one BMP uses.
    if b.starts_with(b"BM") && u32le(b, 14).is_some_and(|h| matches!(h, 12 | 40 | 52 | 56 | 64 | 108 | 124)) {
        return RealType::Image(ImageFormat::Bmp);
    }
    if b.starts_with(b"%PDF-") {
        return RealType::Pdf { active: pdf_active(b) };
    }
    if b.starts_with(b"PK\x03\x04") || b.starts_with(b"PK\x05\x06") || b.starts_with(b"PK\x07\x08") {
        return zip_type(b);
    }
    if b.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return ole_type(b);
    }
    for (magic, format) in [
        (&b"Rar!\x1a\x07"[..], "RAR"),
        (&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C][..], "7-Zip"),
        (&[0x1F, 0x8B][..], "gzip"),
        (&[0xFD, b'7', b'z', b'X', b'Z', 0x00][..], "xz"),
        (&[0x28, 0xB5, 0x2F, 0xFD][..], "Zstandard"),
        (&b"MSCF\0\0\0\0"[..], "cabinet"),
    ] {
        if b.starts_with(magic) {
            return RealType::Archive(ArchiveInfo { format, encrypted: false, programs: Vec::new() });
        }
    }
    if b.starts_with(b"BZh") && b.get(3).is_some_and(|d| (b'1'..=b'9').contains(d)) {
        return RealType::Archive(ArchiveInfo { format: "bzip2", encrypted: false, programs: Vec::new() });
    }
    if has_at(b, 257, b"ustar") {
        return RealType::Archive(ArchiveInfo { format: "tar", encrypted: false, programs: Vec::new() });
    }
    if b.starts_with(b"MZ") {
        return RealType::Program(pe_kind(b));
    }
    if b.starts_with(b"\x7fELF") {
        return RealType::Program(ProgramKind::Linux);
    }
    if [b"\xFE\xED\xFA\xCE", b"\xFE\xED\xFA\xCF", b"\xCE\xFA\xED\xFE", b"\xCF\xFA\xED\xFE"].iter().any(|m| b.starts_with(*m)) {
        return RealType::Program(ProgramKind::Mac);
    }
    if b.starts_with(&[0xCA, 0xFE, 0xBA, 0xBE]) {
        return RealType::Program(ProgramKind::Java);
    }
    if b.starts_with(&[0x4C, 0, 0, 0, 0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46]) {
        return RealType::Shortcut;
    }
    if b.starts_with(b"vhdxfile") {
        return RealType::DiskImage("VHDX");
    }
    if b.starts_with(b"conectix") {
        return RealType::DiskImage("VHD");
    }
    if b.starts_with(b"{\\rtf") {
        return rtf_type(b);
    }
    for offset in [0x8001, 0x8801, 0x9001] {
        if has_at(b, offset, b"CD001") {
            return RealType::DiskImage("ISO");
        }
        if has_at(b, offset, b"BEA01") || has_at(b, offset, b"NSR02") || has_at(b, offset, b"NSR03") {
            return RealType::DiskImage("UDF");
        }
    }
    if let Some(footer) = b.len().checked_sub(512) {
        if has_at(b, footer, b"conectix") {
            return RealType::DiskImage("VHD");
        }
        if has_at(b, footer, b"koly") {
            return RealType::DiskImage("DMG");
        }
    }
    // PDF readers accept a PDF whose header comes after some junk.
    if b.get(..1024).unwrap_or(b).windows(5).any(|w| w == b"%PDF-") {
        return RealType::Pdf { active: pdf_active(b) };
    }
    text_type(b).unwrap_or(RealType::Unknown)
}

/// An `MZ` file: a library when its PE header says so, otherwise a program.
fn pe_kind(b: &[u8]) -> ProgramKind {
    let library = u32le(b, 0x3C)
        .and_then(usize_of)
        .filter(|pe| has_at(b, *pe, b"PE\0\0"))
        .and_then(|pe| u16le(b, pe.checked_add(4 + 18)?))
        .is_some_and(|characteristics| characteristics & 0x2000 != 0);
    if library { ProgramKind::WindowsLibrary } else { ProgramKind::Windows }
}

// ---- ZIP ----

struct ZipListing {
    names: Vec<String>,
    encrypted: bool,
}

/// The names in a ZIP's central directory, read from its end record (ZIP64 too). `None` when
/// there is no end record.
fn zip_listing(b: &[u8]) -> Option<ZipListing> {
    const EOCD: &[u8] = b"PK\x05\x06";
    let min = b.len().saturating_sub(22 + 0xFFFF);
    let tail = b.get(min..)?;
    let end = min.checked_add(tail.windows(4).rposition(|w| w == EOCD)?)?;
    let mut count = usize_of(u16le(b, end.checked_add(10)?)?)?;
    let mut size = usize_of(u32le(b, end.checked_add(12)?)?)?;
    let mut offset = usize_of(u32le(b, end.checked_add(16)?)?)?;
    let mut record = end;
    // ZIP64: the real values are in the ZIP64 end record, which its locator (just before the
    // end record) points to.
    if (count == 0xFFFF || size == 0xFFFF_FFFF || offset == 0xFFFF_FFFF)
        && let Some(locator) = end.checked_sub(20)
        && has_at(b, locator, b"PK\x06\x07")
        && let Some(z) = u64le(b, locator + 8).and_then(usize_of)
        && has_at(b, z, b"PK\x06\x06")
    {
        count = u64le(b, z.checked_add(32)?).and_then(usize_of)?;
        size = u64le(b, z.checked_add(40)?).and_then(usize_of)?;
        offset = u64le(b, z.checked_add(48)?).and_then(usize_of)?;
        record = z;
    }
    const ENTRY: &[u8] = b"PK\x01\x02";
    // A ZIP with something before it (a self-extracting program): its offsets are off by that
    // much, and the directory ends where the end record starts.
    let start = if has_at(b, offset, ENTRY) { offset } else { record.checked_sub(size)? };
    let mut names = Vec::new();
    let mut encrypted = false;
    let mut pos = start;
    while names.len() < count.min(MAX_ZIP_ENTRIES) && has_at(b, pos, ENTRY) {
        let (Some(flags), Some(name_len), Some(extra_len), Some(comment_len)) =
            (u16le(b, pos + 8), u16le(b, pos + 28), u16le(b, pos + 30), u16le(b, pos + 32))
        else {
            break;
        };
        let name_at = pos + 46;
        let Some(name) = at(b, name_at, usize::from(name_len)) else { break };
        encrypted |= flags & 1 != 0;
        names.push(String::from_utf8_lossy(name).into_owned());
        let Some(next) = name_at.checked_add(usize::from(name_len) + usize::from(extra_len) + usize::from(comment_len)) else { break };
        pos = next;
    }
    Some(ZipListing { names, encrypted })
}

fn zip_type(b: &[u8]) -> RealType {
    let Some(list) = zip_listing(b) else {
        return RealType::Archive(ArchiveInfo { format: "ZIP", encrypted: false, programs: Vec::new() });
    };
    let lower: Vec<String> = list.names.iter().map(|n| n.to_ascii_lowercase()).collect();
    let has = |name: &str| lower.iter().any(|n| n == name);
    let starts = |prefix: &str| lower.iter().any(|n| n.starts_with(prefix));

    if has("[content_types].xml") {
        let app = if starts("word/") {
            App::Word
        } else if starts("xl/") {
            App::Excel
        } else if starts("ppt/") {
            App::PowerPoint
        } else {
            App::Other
        };
        // VBA projects, and Excel 4 macro sheets.
        let macros = lower.iter().any(|n| n.ends_with("vbaproject.bin") || n.ends_with("vbadata.xml") || n.starts_with("xl/macrosheets/"));
        let embedded = lower.iter().any(|n| n.contains("/embeddings/") || n.contains("/activex/"));
        return RealType::Office(OfficeInfo {
            format: OfficeFormat::Ooxml,
            app,
            macros: if macros { Macros::Present } else { Macros::None },
            embedded,
            equation: false,
        });
    }
    if has("mimetype") && (has("content.xml") || has("meta-inf/manifest.xml")) {
        // The `mimetype` entry is stored first and uncompressed: its value is near the start.
        let head = b.get(..512).unwrap_or(b);
        let app = if contains(head, b"opendocument.text") {
            App::Word
        } else if contains(head, b"opendocument.spreadsheet") {
            App::Excel
        } else if contains(head, b"opendocument.presentation") {
            App::PowerPoint
        } else {
            App::Other
        };
        let macros = starts("basic/") || starts("scripts/");
        return RealType::Office(OfficeInfo {
            format: OfficeFormat::OpenDocument,
            app,
            macros: if macros { Macros::Present } else { Macros::None },
            embedded: false,
            equation: false,
        });
    }
    if has("meta-inf/manifest.mf") && lower.iter().any(|n| n.ends_with(".class")) {
        return RealType::Program(ProgramKind::Java);
    }
    let programs = list
        .names
        .iter()
        .filter(|n| runnable(n))
        .take(MAX_LISTED)
        .map(|n| {
            let leaf = n.rsplit(['/', '\\']).next().unwrap_or(n);
            names::visible(leaf).chars().take(MAX_LISTED_NAME).collect()
        })
        .collect();
    RealType::Archive(ArchiveInfo { format: "ZIP", encrypted: list.encrypted, programs })
}

/// Whether a name inside an archive is a program, a script, a shortcut or a disk image.
fn runnable(name: &str) -> bool {
    names::extension(name).is_some_and(|e| matches!(names::ext_class(&e), ExtClass::Program | ExtClass::Script | ExtClass::Shortcut | ExtClass::DiskImage))
}

// ---- OLE compound files (old Office, MSI) ----

/// An OLE directory entry at `e` (128 bytes): its name (without the leading control character
/// some names have), its type (1 storage, 2 stream, 5 root) and its class id. `None` when the
/// bytes are not one.
fn ole_entry(e: &[u8]) -> Option<(String, u8, [u8; 16])> {
    let name_len = usize::from(u16le(e, 64)?);
    let kind = *e.get(66)?;
    let color = *e.get(67)?;
    if !(2..=64).contains(&name_len) || name_len % 2 != 0 || !matches!(kind, 1 | 2 | 5) || color > 1 {
        return None;
    }
    let units = name_len / 2 - 1;
    if at(e, units * 2, 2)? != b"\0\0" {
        return None;
    }
    let raw = at(e, 0, units * 2)?;
    let utf16 = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c.first().copied().unwrap_or(0), c.get(1).copied().unwrap_or(0)]));
    let name: String = char::decode_utf16(utf16).collect::<Result<String, _>>().ok()?;
    let name = name.trim_start_matches(|c: char| ('\u{1}'..='\u{6}').contains(&c));
    if name.is_empty() || name.chars().any(char::is_control) {
        return None;
    }
    let clsid: [u8; 16] = at(e, 80, 16)?.try_into().ok()?;
    Some((name.to_owned(), kind, clsid))
}

/// Root class ids of Windows Installer files: MSI, MSP (patch), MST (transform).
const INSTALLER_IDS: [u8; 3] = [0x84, 0x86, 0x82];
const INSTALLER_REST: [u8; 15] = [0x10, 0x0C, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46];

fn ole_type(b: &[u8]) -> RealType {
    // Directory entries are 128 bytes, at 128-byte steps in every sector after the header
    // (512 bytes, or 4096 when the sectors are). They are found by their shape rather than by
    // following the file's allocation chains: cheaper, and a damaged chain hides nothing.
    let sector: usize = if u16le(b, 0x1E) == Some(12) { 4096 } else { 512 };
    let mut names: Vec<String> = Vec::new();
    let mut root: Option<[u8; 16]> = None;
    let mut pos = sector;
    while names.len() < MAX_OLE_NAMES {
        let Some(e) = at(b, pos, 128) else { break };
        if let Some((name, kind, clsid)) = ole_entry(e) {
            if kind == 5 && root.is_none() {
                root = Some(clsid);
            }
            names.push(name);
        }
        pos += 128;
    }
    if let Some(id) = root
        && id.first().is_some_and(|f| INSTALLER_IDS.contains(f))
        && id.get(1..) == Some(&INSTALLER_REST[..])
    {
        return RealType::Program(ProgramKind::Installer);
    }
    let has = |n: &str| names.iter().any(|x| x.eq_ignore_ascii_case(n));
    let app = if has("WordDocument") {
        App::Word
    } else if has("Workbook") || has("Book") {
        App::Excel
    } else if has("PowerPoint Document") {
        App::PowerPoint
    } else {
        App::Other
    };
    let vba = has("_VBA_PROJECT_CUR") || has("_VBA_PROJECT") || has("VBA") || has("Macros");
    let macros = match (vba, app) {
        (true, _) => Macros::Present,
        // Word keeps its macros in those storages; Excel 4 macro sheets and PowerPoint's macros
        // are inside streams MailALM does not read.
        (false, App::Word | App::Other) => Macros::None,
        (false, App::Excel | App::PowerPoint) => Macros::Unknown,
    };
    let embedded = has("ObjectPool") || has("Ole10Native") || names.iter().any(|n| n.len() == 11 && n.starts_with("MBD"));
    RealType::Office(OfficeInfo { format: OfficeFormat::Ole, app, macros, embedded, equation: has("Equation Native") })
}

// ---- RTF ----

fn rtf_type(b: &[u8]) -> RealType {
    let embedded = contains(b, b"\\object") || contains(b, b"\\objdata") || contains(b, b"\\objemb");
    // "Equation.3", as text or in the hexadecimal an object's data is written in.
    let equation = contains_ignore_case(b, b"equation.3") || contains_ignore_case(b, b"4571756174696f6e2e33");
    RealType::Office(OfficeInfo { format: OfficeFormat::Rtf, app: App::Word, macros: Macros::None, embedded, equation })
}

// ---- PDF ----

/// The names a PDF uses for what runs or carries other files.
const PDF_ACTIVE: [&str; 7] = ["JavaScript", "JS", "Launch", "EmbeddedFile", "EmbeddedFiles", "RichMedia", "XFA"];

/// The active names in a PDF, read as name tokens (`/JavaScript`, also written `/J#61vaScript`).
/// Names inside compressed object streams are not seen.
fn pdf_active(b: &[u8]) -> Vec<&'static str> {
    let mut found: Vec<&'static str> = Vec::new();
    let mut i = 0;
    while found.len() < PDF_ACTIVE.len() {
        let Some(slash) = b.get(i..).and_then(|r| r.iter().position(|&c| c == b'/')) else { break };
        i += slash + 1;
        let mut name = [0u8; MAX_PDF_NAME];
        let mut n = 0usize;
        let mut j = i;
        while let Some(&c) = b.get(j) {
            if c <= b' ' || c >= 0x7F || matches!(c, b'/' | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'%') {
                break;
            }
            let byte = if c == b'#' {
                match (b.get(j + 1).and_then(|h| (*h as char).to_digit(16)), b.get(j + 2).and_then(|h| (*h as char).to_digit(16))) {
                    (Some(hi), Some(lo)) => {
                        j += 2;
                        u8::try_from(hi * 16 + lo).unwrap_or(0)
                    }
                    _ => c,
                }
            } else {
                c
            };
            if let Some(slot) = name.get_mut(n) {
                *slot = byte;
            }
            n += 1;
            j += 1;
        }
        if let Some(token) = name.get(..n)
            && let Some(hit) = PDF_ACTIVE.iter().find(|a| a.as_bytes() == token)
            && !found.contains(hit)
        {
            found.push(*hit);
        }
        i = j.max(i);
    }
    found
}

// ---- text ----

/// What gives a script away (lowercase), most telling first.
const SCRIPT_CLUES: [&str; 30] = [
    "<hta:application",
    "wscript.shell",
    "wscript.createobject",
    "shell.application",
    "activexobject",
    "createobject(",
    "powershell",
    "invoke-expression",
    "frombase64string",
    "downloadstring(",
    "downloadfile(",
    "net.webclient",
    "start-process",
    "set-executionpolicy",
    "@echo off",
    "cmd.exe",
    "cmd /c",
    "mshta",
    "rundll32",
    "regsvr32",
    "certutil",
    "bitsadmin",
    "on error resume next",
    "<job",
    "<package>",
    "#!/",
    "eval(",
    "document.write(",
    "unescape(",
    "[scriptfile]",
];

/// What gives a web page away (lowercase).
const WEB_CLUES: [&str; 7] = ["<!doctype html", "<html", "<svg", "<script", "<iframe", "<body", "<object"];

/// The type of `b` if it is text: plain, a web page, a script, or an internet shortcut.
fn text_type(b: &[u8]) -> Option<RealType> {
    let sample = b.get(..TEXT_SAMPLE).unwrap_or(b);
    let text: String = if let Some(rest) = sample.strip_prefix(b"\xFF\xFE") {
        char::decode_utf16(rest.chunks_exact(2).map(|c| u16::from_le_bytes([c.first().copied().unwrap_or(0), c.get(1).copied().unwrap_or(0)])))
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect()
    } else if let Some(rest) = sample.strip_prefix(b"\xFE\xFF") {
        char::decode_utf16(rest.chunks_exact(2).map(|c| u16::from_be_bytes([c.first().copied().unwrap_or(0), c.get(1).copied().unwrap_or(0)])))
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect()
    } else {
        let body = sample.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(sample);
        if body.contains(&0) {
            return None;
        }
        match std::str::from_utf8(body) {
            Ok(s) => s.to_owned(),
            // Cut in the middle of a character at the end of the sample: still UTF-8.
            Err(e) if e.error_len().is_none() => String::from_utf8_lossy(body.get(..e.valid_up_to()).unwrap_or(body)).into_owned(),
            // Not UTF-8: read as Latin-1 (Windows-1252's letters), and judged below.
            Err(_) => body.iter().map(|&c| char::from(c)).collect(),
        }
    };
    let total = text.chars().count();
    let odd = text.chars().filter(|c| (c.is_control() && !matches!(*c, '\t' | '\n' | '\r' | '\u{c}' | '\u{1b}')) || *c == '\u{FFFD}').count();
    // More than 2% of characters no text has: binary.
    if total > 0 && odd.saturating_mul(50) > total {
        return None;
    }
    let lower = text.to_lowercase();
    if lower.trim_start().starts_with("[internetshortcut]") {
        return Some(RealType::Shortcut);
    }
    if let Some(clue) = SCRIPT_CLUES.iter().find(|c| lower.contains(*c)) {
        return Some(RealType::Text(TextKind::Script(*clue)));
    }
    if WEB_CLUES.iter().any(|c| lower.contains(c)) {
        return Some(RealType::Text(TextKind::Web));
    }
    Some(RealType::Text(TextKind::Plain))
}
