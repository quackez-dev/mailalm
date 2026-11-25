//! What a file's name and declared type say: its extension's class, a double extension, hidden
//! characters.

use crate::content::Family;

/// What an extension says a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtClass {
    /// No extension.
    None,
    /// Runs as a program (`.exe`, `.scr`, `.msi`, `.jar`…).
    Program,
    /// Runs as a script, or is read by a part of Windows that runs things (`.js`, `.vbs`, `.ps1`,
    /// `.bat`, `.hta`, `.chm`, `.reg`…).
    Script,
    Shortcut,
    DiskImage,
    Archive,
    /// A web page or SVG.
    Web,
    Office,
    /// An Office type that holds macros (`.docm`, `.xlsm`…).
    OfficeMacros,
    Pdf,
    Image,
    /// Plain text (`.txt`, `.csv`, `.log`…).
    Text,
    /// Anything else (`.eml`, `.ics`, `.mp3`…).
    Other,
}

/// The last extension of a name, lowercase, trimmed; `None` when there is none.
pub fn extension(name: &str) -> Option<String> {
    let (_, ext) = name.rsplit_once('.')?;
    let ext = ext.trim().to_lowercase();
    (!ext.is_empty() && ext.chars().count() <= 24).then_some(ext)
}

/// The class of an extension (lowercase, without the dot).
pub fn ext_class(ext: &str) -> ExtClass {
    match ext {
        "exe" | "com" | "scr" | "pif" | "dll" | "cpl" | "sys" | "drv" | "ocx" | "msi" | "msp" | "mst" | "jar" | "appx" | "appxbundle" | "msix"
        | "msixbundle" | "gadget" | "xll" | "xbap" | "apk" | "elf" | "run" => ExtClass::Program,
        "js" | "jse" | "vbs" | "vbe" | "vb" | "wsf" | "wsh" | "wsc" | "ws" | "ps1" | "ps1xml" | "ps2" | "ps2xml" | "psc1" | "psc2" | "psm1" | "psd1"
        | "bat" | "cmd" | "hta" | "sct" | "chm" | "hlp" | "reg" | "inf" | "scf" | "msc" | "application" | "appref-ms" | "settingcontent-ms"
        | "library-ms" | "search-ms" | "searchconnector-ms" | "py" | "pyw" | "pyc" | "pl" | "rb" | "sh" | "ksh" | "csh" | "bash" | "zsh"
        | "command" | "applescript" | "scpt" | "iqy" | "slk" | "mof" | "ins" | "isp" => ExtClass::Script,
        "lnk" | "url" | "website" | "desktop" | "webloc" => ExtClass::Shortcut,
        "iso" | "img" | "vhd" | "vhdx" | "dmg" | "udf" | "vmdk" => ExtClass::DiskImage,
        "zip" | "rar" | "7z" | "gz" | "tgz" | "tar" | "bz2" | "tbz" | "tbz2" | "xz" | "txz" | "zst" | "cab" | "arj" | "lzh" | "lha" | "ace" | "z"
        | "uue" | "xxe" | "wim" => ExtClass::Archive,
        "html" | "htm" | "xhtml" | "shtml" | "mht" | "mhtml" | "svg" | "svgz" | "xht" => ExtClass::Web,
        "doc" | "docx" | "dot" | "dotx" | "xls" | "xlsx" | "xlt" | "xltx" | "ppt" | "pptx" | "pps" | "ppsx" | "pot" | "potx" | "odt" | "ods"
        | "odp" | "rtf" => ExtClass::Office,
        "docm" | "dotm" | "xlsm" | "xltm" | "xlam" | "xlsb" | "xla" | "pptm" | "potm" | "ppam" | "ppsm" | "sldm" => ExtClass::OfficeMacros,
        "pdf" => ExtClass::Pdf,
        "png" | "jpg" | "jpeg" | "jpe" | "jfif" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico" | "heic" | "heif" | "avif" => ExtClass::Image,
        "txt" | "text" | "log" | "md" | "csv" | "tsv" | "json" | "xml" | "ini" | "cfg" | "yaml" | "yml" => ExtClass::Text,
        _ => ExtClass::Other,
    }
}

/// Whether the class is one whose file runs something when opened.
fn runs(class: ExtClass) -> bool {
    matches!(class, ExtClass::Program | ExtClass::Script | ExtClass::Shortcut)
}

/// A double extension that hides a dangerous type behind a harmless one: `invoice.pdf.exe`, also
/// with spaces before the last dot. Returns the two extensions.
pub(crate) fn double_extension(name: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() < 3 {
        return None;
    }
    let outer = parts.last()?.trim().to_lowercase();
    let inner = parts.get(parts.len().checked_sub(2)?)?.trim().to_lowercase();
    let (outer_class, inner_class) = (ext_class(&outer), ext_class(&inner));
    let harmless_looking = matches!(inner_class, ExtClass::Pdf | ExtClass::Image | ExtClass::Office | ExtClass::Text | ExtClass::Archive | ExtClass::Web);
    (runs(outer_class) && harmless_looking).then_some((inner, outer))
}

/// Whether a character changes how a name reads without showing itself: direction marks and
/// overrides (which turn `exe.pdf` into `fdp.exe`), zero-width characters, and the soft hyphen.
fn is_hidden(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}' | '\u{061c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}'
    )
}

/// Whether the name as the message announced it holds hidden or control characters.
pub fn has_hidden_characters(raw_name: &str) -> bool {
    raw_name.chars().any(|c| is_hidden(c) || c.is_control())
}

/// A name without its hidden and control characters, for showing.
pub(crate) fn visible(s: &str) -> String {
    s.chars().filter(|c| !is_hidden(*c) && !c.is_control()).collect()
}

/// The broad kind an extension class promises.
pub(crate) fn family_of_class(class: ExtClass) -> Option<Family> {
    match class {
        ExtClass::Program => Some(Family::Program),
        ExtClass::Script | ExtClass::Web | ExtClass::Text => Some(Family::Text),
        ExtClass::Shortcut => Some(Family::Shortcut),
        ExtClass::DiskImage => Some(Family::DiskImage),
        ExtClass::Archive => Some(Family::Archive),
        ExtClass::Office | ExtClass::OfficeMacros => Some(Family::Office),
        ExtClass::Pdf => Some(Family::Pdf),
        ExtClass::Image => Some(Family::Image),
        ExtClass::None | ExtClass::Other => None,
    }
}

/// The broad kind a declared type promises; `None` for the generic ones (`application/octet-stream`)
/// and the ones not checked.
pub(crate) fn family_of_declared(declared: &str) -> Option<Family> {
    let d = declared.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let d = d.as_str();
    if d == "application/pdf" || d == "application/x-pdf" {
        return Some(Family::Pdf);
    }
    if d == "image/svg+xml" {
        return Some(Family::Text);
    }
    if d.starts_with("image/") {
        return Some(Family::Image);
    }
    if matches!(d, "text/rtf" | "application/rtf" | "application/msword" | "application/vnd.ms-excel" | "application/vnd.ms-powerpoint" | "application/vnd.ms-word")
        || d.starts_with("application/vnd.openxmlformats-officedocument.")
        || d.starts_with("application/vnd.ms-word.")
        || d.starts_with("application/vnd.ms-excel.")
        || d.starts_with("application/vnd.ms-powerpoint.")
        || d.starts_with("application/vnd.oasis.opendocument.")
    {
        return Some(Family::Office);
    }
    if d.starts_with("text/") || matches!(d, "application/javascript" | "application/x-javascript" | "application/json" | "application/xml" | "application/x-sh") {
        return Some(Family::Text);
    }
    if matches!(
        d,
        "application/zip"
            | "application/x-zip-compressed"
            | "application/x-rar-compressed"
            | "application/vnd.rar"
            | "application/x-7z-compressed"
            | "application/gzip"
            | "application/x-gzip"
            | "application/x-tar"
            | "application/x-bzip2"
            | "application/vnd.ms-cab-compressed"
    ) {
        return Some(Family::Archive);
    }
    if matches!(
        d,
        "application/x-msdownload"
            | "application/x-msdos-program"
            | "application/x-dosexec"
            | "application/vnd.microsoft.portable-executable"
            | "application/x-ms-installer"
            | "application/x-msi"
            | "application/x-executable"
    ) {
        return Some(Family::Program);
    }
    if d == "application/x-iso9660-image" {
        return Some(Family::DiskImage);
    }
    None
}

/// Whether what a name or a declared type promises and what the content is agree.
pub(crate) fn families_agree(promised: Family, found: Family) -> bool {
    promised == found
        // An Office Open XML or OpenDocument file is a ZIP.
        || (promised == Family::Archive && found == Family::Office)
        // `.url`, `.desktop`: shortcuts written as text.
        || (promised == Family::Shortcut && found == Family::Text)
}

/// A declared type as the dialog shows it: printable ASCII, at most 80 characters.
pub(crate) fn short_declared(declared: &str) -> String {
    declared.chars().filter(|c| c.is_ascii_graphic()).take(80).collect()
}
