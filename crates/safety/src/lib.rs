//! What MailALM tells the user about an attachment before anything is done with it (PLAN.md,
//! section 10.1), and what it lets them do.
//!
//! - [`assess`] reads one attachment's bytes, its announced name and its declared type, and gives a
//!   [`Report`]: the **real type** found from the content ([`RealType`], never from the name), the
//!   [`Warning`]s, and the [`Policy`]: whether MailALM may preview it itself (pictures, PDF, text),
//!   and whether it may be opened with the program Windows uses for its type (an allowlist: PDF,
//!   pictures, plain text, Office documents without macros, and nothing with a warning). Saving is
//!   always possible.
//! - [`authentication`] reads the SPF, DKIM and DMARC results of the message from its first
//!   `Authentication-Results` header (the one the receiving provider added on top).
//! - [`files`] writes a saved attachment with Windows' "came from the internet" mark
//!   (Mark-of-the-Web), and keeps the folder of copies opened with Windows.
//!
//! Everything here reads hostile input: every read is bounds-checked, every loop is capped, and
//! nothing decompresses, decodes pictures or runs anything. The content checks are cheap looks
//! (magic numbers, a ZIP's central directory, an OLE file's directory entries, PDF name tokens):
//! they can miss what a determined sender hides, which is why opening stays an allowlist and
//! asks twice.

mod auth;
mod content;
pub mod files;
mod names;

pub use auth::{Authentication, Check, DmarcAlert, Verdict, authentication, dmarc_alert};
pub use content::{App, ArchiveInfo, ImageFormat, Macros, OfficeFormat, OfficeInfo, ProgramKind, RealType, TextKind, detect};
pub use names::{ExtClass, ext_class, extension, has_hidden_characters};

/// One attachment, as the message gives it.
#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    /// The name as the message announces it, before any cleaning (hidden characters are looked
    /// for here).
    pub raw_name: &'a str,
    /// The name as MailALM shows it and saves it (cleaned: no folder, no hidden character).
    pub name: &'a str,
    /// The type the message declares (`application/pdf`…), lowercase.
    pub declared_type: &'a str,
    /// The decoded content.
    pub bytes: &'a [u8],
}

/// What MailALM may show of the content itself, decoded in Rust (no external program).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preview {
    /// PNG, JPEG, GIF, BMP or WebP, decoded with size limits.
    Image,
    /// The first pages of a PDF, drawn as pictures.
    Pdf,
    /// The text itself (a script or a web page shows its source, never runs).
    Text,
}

/// What may be done with the attachment. Saving it (with the Mark-of-the-Web) is always allowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub preview: Option<Preview>,
    /// `Ok(extension)`: it may be opened with Windows, from a copy named with this extension
    /// (lowercase, on the allowlist). `Err(why)`: it may not.
    pub open: Result<&'static str, String>,
}

/// How serious a warning is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth knowing.
    Caution,
    /// The usual shape of an attack.
    Danger,
}

/// Something the user should know before doing anything with the attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    /// The name hides characters (direction overrides, zero-width or control characters) that
    /// can make `exe.pdf` show as `fdp.exe`.
    HiddenCharacters,
    /// Two extensions, the last one dangerous: `invoice.pdf.exe`. Holds the two (`pdf`, `exe`).
    DoubleExtension { inner: String, outer: String },
    /// A program (by its name or its content).
    Program,
    /// A script (by its name, or text that reads like one; the clue found).
    Script(String),
    /// A web page or SVG: it can run scripts when opened in a browser.
    WebPage,
    /// An Office document with macros (or a macro-enabled type).
    Macros,
    /// An old Office format whose macros cannot be checked cheaply.
    MacrosUnchecked,
    /// Office objects embedded in the document (OLE packages, ActiveX).
    EmbeddedObjects,
    /// An Equation Editor object: the old component behind many attacks.
    EquationEditor,
    /// A PDF with active parts: the names found (`JavaScript`, `Launch`, `EmbeddedFile`…).
    PdfActive(Vec<String>),
    /// An archive: what is inside is not checked here, except for the names below.
    Archive,
    /// Files inside the archive that are programs, scripts or shortcuts (the first few names).
    ArchiveHoldsPrograms(Vec<String>),
    /// An encrypted archive (its content cannot be checked).
    Encrypted,
    /// A shortcut (`.lnk`, `.url`): it starts something else.
    Shortcut,
    /// A disk image (`.iso`, `.img`, `.vhd`): Windows opens it as a drive, and its files lose
    /// the Mark-of-the-Web.
    DiskImage,
    /// The extension says one kind, the content another.
    ExtensionMismatch { extension: String, real: String },
    /// The declared type says one kind, the content another.
    DeclaredMismatch { declared: String, real: String },
    /// The file has no content.
    Empty,
}

impl Warning {
    pub fn severity(&self) -> Severity {
        match self {
            Warning::HiddenCharacters
            | Warning::DoubleExtension { .. }
            | Warning::Program
            | Warning::Script(_)
            | Warning::Macros
            | Warning::EquationEditor
            | Warning::ArchiveHoldsPrograms(_)
            | Warning::Shortcut
            | Warning::ExtensionMismatch { .. } => Severity::Danger,
            Warning::WebPage
            | Warning::MacrosUnchecked
            | Warning::EmbeddedObjects
            | Warning::PdfActive(_)
            | Warning::Archive
            | Warning::Encrypted
            | Warning::DiskImage
            | Warning::DeclaredMismatch { .. }
            | Warning::Empty => Severity::Caution,
        }
    }

    /// The warning in a sentence, as the approval dialog shows it.
    pub fn text(&self) -> String {
        match self {
            Warning::HiddenCharacters => "The name hides invisible or direction-changing characters: what you read may not be its real name.".to_owned(),
            Warning::DoubleExtension { inner, outer } => {
                format!("Double extension: it looks like a .{inner} file, but it is a .{outer} file.")
            }
            Warning::Program => "This is a program: it would run on this PC if opened.".to_owned(),
            Warning::Script(clue) => format!("This is a script ({clue}): it would run on this PC if opened."),
            Warning::WebPage => "A web page or SVG picture: it can run scripts when opened in a browser.".to_owned(),
            Warning::Macros => "An Office document with macros: macros are programs.".to_owned(),
            Warning::MacrosUnchecked => "An old Office format: MailALM cannot check it for macros.".to_owned(),
            Warning::EmbeddedObjects => "Objects are embedded in this document (they can hold programs).".to_owned(),
            Warning::EquationEditor => "It holds an Equation Editor object, a part of Office used in many attacks.".to_owned(),
            Warning::PdfActive(names) => format!("This PDF has active parts: {}.", names.join(", ")),
            Warning::Archive => "An archive: the files inside are not checked.".to_owned(),
            Warning::ArchiveHoldsPrograms(names) => format!("The archive holds programs, scripts or shortcuts: {}.", names.join(", ")),
            Warning::Encrypted => "The archive is encrypted: its content cannot be checked.".to_owned(),
            Warning::Shortcut => "A shortcut: it starts another file or a web page.".to_owned(),
            Warning::DiskImage => "A disk image: Windows would open it as a drive, and its files would lose the \"from the internet\" mark.".to_owned(),
            Warning::ExtensionMismatch { extension, real } => format!("The name says .{extension}, but the content is {real}."),
            Warning::DeclaredMismatch { declared, real } => format!("The message calls it {declared}, but the content is {real}."),
            Warning::Empty => "The file is empty.".to_owned(),
        }
    }
}

/// What MailALM found about one attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub real: RealType,
    /// The last extension of the shown name, lowercase, if any.
    pub extension: Option<String>,
    /// The most serious first.
    pub warnings: Vec<Warning>,
    pub policy: Policy,
}

impl Report {
    /// Whether any warning is a [`Severity::Danger`].
    pub fn dangerous(&self) -> bool {
        self.warnings.iter().any(|w| w.severity() == Severity::Danger)
    }
}

/// Looks at one attachment.
pub fn assess(input: &Input<'_>) -> Report {
    let real = detect(input.bytes);
    let ext = extension(input.name);
    let class = ext.as_deref().map_or(ExtClass::None, ext_class);
    let mut w: Vec<Warning> = Vec::new();

    if has_hidden_characters(input.raw_name) {
        w.push(Warning::HiddenCharacters);
    }
    if let Some((inner, outer)) = names::double_extension(input.name) {
        w.push(Warning::DoubleExtension { inner, outer });
    }
    if matches!(real, RealType::Program(_)) || class == ExtClass::Program {
        w.push(Warning::Program);
    }
    // Text that reads like a script is a script unless its name makes it plain text (a .txt
    // opens in Notepad, whatever it says).
    match (&real, class) {
        (_, ExtClass::Script) => w.push(Warning::Script(format!(".{} file", ext.as_deref().unwrap_or("")))),
        (RealType::Text(TextKind::Script(clue)), c) if c != ExtClass::Text => w.push(Warning::Script(format!("it contains \"{clue}\""))),
        _ => {}
    }
    if class == ExtClass::Web || (matches!(real, RealType::Text(TextKind::Web)) && class != ExtClass::Text) {
        w.push(Warning::WebPage);
    }
    if let RealType::Office(o) = &real {
        match o.macros {
            Macros::Present => w.push(Warning::Macros),
            Macros::Unknown => w.push(Warning::MacrosUnchecked),
            Macros::None => {}
        }
        if o.equation {
            w.push(Warning::EquationEditor);
        }
        if o.embedded {
            w.push(Warning::EmbeddedObjects);
        }
    }
    if class == ExtClass::OfficeMacros && !w.contains(&Warning::Macros) {
        w.push(Warning::Macros);
    }
    if let RealType::Pdf { active } = &real
        && !active.is_empty()
    {
        w.push(Warning::PdfActive(active.iter().map(|s| (*s).to_owned()).collect()));
    }
    if let RealType::Archive(a) = &real {
        w.push(Warning::Archive);
        if !a.programs.is_empty() {
            w.push(Warning::ArchiveHoldsPrograms(a.programs.clone()));
        }
        if a.encrypted {
            w.push(Warning::Encrypted);
        }
    } else if class == ExtClass::Archive && !matches!(real, RealType::Office(_)) {
        w.push(Warning::Archive);
    }
    if matches!(real, RealType::Shortcut) || class == ExtClass::Shortcut {
        w.push(Warning::Shortcut);
    }
    if matches!(real, RealType::DiskImage(_)) || class == ExtClass::DiskImage {
        w.push(Warning::DiskImage);
    }
    if let Some(e) = &ext
        && let Some(expected) = names::family_of_class(class)
        && let Some(found) = real.family()
        && !names::families_agree(expected, found)
    {
        w.push(Warning::ExtensionMismatch { extension: e.clone(), real: real.label() });
    }
    if let Some(expected) = names::family_of_declared(input.declared_type)
        && let Some(found) = real.family()
        && !names::families_agree(expected, found)
    {
        w.push(Warning::DeclaredMismatch { declared: names::short_declared(input.declared_type), real: real.label() });
    }
    if matches!(real, RealType::Empty) {
        w.push(Warning::Empty);
    }
    // The most serious first, otherwise in the order found.
    w.sort_by_key(|x| std::cmp::Reverse(x.severity()));

    let policy = policy(&real, ext.as_deref(), class, &w);
    Report { real, extension: ext, warnings: w, policy }
}

/// The extensions a copy opened with Windows may have, by the real type they must match.
const OPEN_IMAGE: [&str; 9] = ["png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "jfif"];
const OPEN_WORD: [&str; 3] = ["docx", "doc", "rtf"];
const OPEN_EXCEL: [&str; 1] = ["xlsx"];
const OPEN_POWERPOINT: [&str; 1] = ["pptx"];
const OPEN_ODF: [&str; 3] = ["odt", "ods", "odp"];

fn policy(real: &RealType, ext: Option<&str>, class: ExtClass, warnings: &[Warning]) -> Policy {
    let preview = match real {
        RealType::Image(f) if f.previewable() => Some(Preview::Image),
        RealType::Pdf { .. } => Some(Preview::Pdf),
        RealType::Text(_) => Some(Preview::Text),
        _ => None,
    };
    Policy { preview, open: open_policy(real, ext, class, warnings) }
}

fn open_policy(real: &RealType, ext: Option<&str>, class: ExtClass, warnings: &[Warning]) -> Result<&'static str, String> {
    const ONLY: &str = "Only PDFs, pictures, .txt files and Office documents without macros open with Windows. Save it instead.";
    if !warnings.is_empty() {
        return Err("It has warnings: save it instead, and check it before opening it.".to_owned());
    }
    let Some(ext) = ext else {
        return Err("It has no extension, so Windows would not know what opens it. Save it instead.".to_owned());
    };
    let allowed: &[&'static str] = match real {
        RealType::Pdf { .. } => &["pdf"],
        RealType::Image(_) => &OPEN_IMAGE,
        // Notepad shows a .txt, whatever it says.
        RealType::Text(_) if class == ExtClass::Text => &["txt"],
        RealType::Office(o) if o.macros == Macros::None && !o.embedded && !o.equation => match (o.format, o.app) {
            (OfficeFormat::Ooxml, App::Word) | (OfficeFormat::Ole, App::Word) | (OfficeFormat::Rtf, _) => &OPEN_WORD,
            (OfficeFormat::Ooxml, App::Excel) => &OPEN_EXCEL,
            (OfficeFormat::Ooxml, App::PowerPoint) => &OPEN_POWERPOINT,
            (OfficeFormat::OpenDocument, _) => &OPEN_ODF,
            _ => &[],
        },
        _ => &[],
    };
    allowed.iter().copied().find(|a| *a == ext).ok_or_else(|| ONLY.to_owned())
}

#[cfg(test)]
mod tests;
