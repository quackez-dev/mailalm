// Adapted from WordCraft (https://github.com/storytold/wordcraft), commit ca7f262a597c75da4a157b730c68aa2db8d12734, crates/control-key/src/lib.rs (where the key file lives) and apps/wordcraft/src/control_server.rs (random_key, write_key_file, keys_match). MIT OR Apache-2.0.
// Changed: one key file and one port file in %LOCALAPPDATA%\MailALM (no Flatpak instance, no port in the name, no environment override); the key is 32 bytes from ring's SystemRandom (not getrandom), kept as bytes, and a key file must be exactly 64 lowercase hexadecimal characters; files written through a temporary file and a rename, read with a size cap; removed only while they still hold this server's key.

//! The control key and port files, one rule for MailALM (which writes them) and `mailalm-mcp`
//! (which reads them): `%LOCALAPPDATA%\MailALM\control-key` and `control-port`.
//!
//! MailALM writes a new key at every start of its control server and removes both files when it
//! stops. The folder is in the user's profile, which Windows gives to that user (and to SYSTEM and
//! the administrators) only; the files inherit that. On Unix they are created with mode 0600.

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use ring::rand::{SecureRandom, SystemRandom};

/// The key file's name in [`profile_dir`].
pub const KEY_FILE: &str = "control-key";
/// The port file's name in [`profile_dir`].
pub const PORT_FILE: &str = "control-port";
/// Bytes in a key.
pub const KEY_BYTES: usize = 32;
/// Characters in a key file: [`KEY_BYTES`] as lowercase hexadecimal.
pub const KEY_HEX_LEN: usize = KEY_BYTES * 2;
/// Longest port file read (a port has at most 5 digits).
const PORT_FILE_MAX: usize = 5;

/// `%LOCALAPPDATA%\MailALM` (on other systems, the platform's local data folder).
pub fn profile_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("MailALM"))
}

/// What can go wrong with the control files or the server.
#[derive(Debug)]
pub enum ControlError {
    /// No local data folder (`%LOCALAPPDATA%`).
    NoProfileDir,
    /// The system gave no random bytes.
    NoRandom,
    /// A file or folder could not be used.
    Io { path: PathBuf, source: io::Error },
    /// The key file is not exactly 64 lowercase hexadecimal characters.
    BadKeyFile(PathBuf),
    /// The port file is not a port number.
    BadPortFile(PathBuf),
    /// No port on 127.0.0.1.
    Bind(io::Error),
    /// The server's thread did not start.
    Thread(io::Error),
}

impl ControlError {
    /// The file does not exist: MailALM is not running, or its MCP access is off.
    pub fn is_missing_file(&self) -> bool {
        matches!(self, ControlError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound)
    }
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlError::NoProfileDir => write!(f, "no local data folder (%LOCALAPPDATA%) for MailALM's control files"),
            ControlError::NoRandom => write!(f, "the system gave no random bytes for the control key"),
            ControlError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            ControlError::BadKeyFile(path) => {
                write!(f, "{}: not a MailALM control key ({KEY_HEX_LEN} lowercase hexadecimal characters expected)", path.display())
            }
            ControlError::BadPortFile(path) => write!(f, "{}: not a port number", path.display()),
            ControlError::Bind(e) => write!(f, "cannot listen on 127.0.0.1: {e}"),
            ControlError::Thread(e) => write!(f, "cannot start the control thread: {e}"),
        }
    }
}

impl std::error::Error for ControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ControlError::Io { source, .. } => Some(source),
            ControlError::Bind(e) | ControlError::Thread(e) => Some(e),
            _ => None,
        }
    }
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> ControlError + '_ {
    move |source| ControlError::Io { path: path.to_path_buf(), source }
}

/// A control key: 32 random bytes. Its `Debug` never shows them.
#[derive(Clone, PartialEq, Eq)]
pub struct Key([u8; KEY_BYTES]);

impl Key {
    /// A new key from the system's secure random numbers.
    pub fn generate() -> Result<Key, ControlError> {
        let mut bytes = [0u8; KEY_BYTES];
        SystemRandom::new().fill(&mut bytes).map_err(|_| ControlError::NoRandom)?;
        Ok(Key(bytes))
    }

    /// A key from its text: exactly 64 lowercase hexadecimal characters, nothing else (no
    /// spaces, no line end).
    pub fn parse(text: &str) -> Option<Key> {
        let digits = text.as_bytes();
        if digits.len() != KEY_HEX_LEN {
            return None;
        }
        let mut bytes = [0u8; KEY_BYTES];
        for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
            let (Some(high), Some(low)) = (pair.first().copied().and_then(hex_value), pair.get(1).copied().and_then(hex_value)) else {
                return None;
            };
            *byte = (high << 4) | low;
        }
        Some(Key(bytes))
    }

    /// The key as the files and requests carry it: 64 lowercase hexadecimal characters.
    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(KEY_HEX_LEN);
        for b in self.0 {
            s.push(char::from(DIGITS[usize::from(b >> 4)]));
            s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
        }
        s
    }

    /// Whether `given` (a request's `key`) is this key. The comparison of the bytes takes the
    /// same time wherever they differ; a text that is not a key never matches.
    pub fn matches(&self, given: &str) -> bool {
        match Key::parse(given) {
            Some(other) => same_bytes(&self.0, &other.0),
            None => false,
        }
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key(…)")
    }
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// Constant-time equality: every byte is compared, the differences are folded together.
fn same_bytes(a: &[u8; KEY_BYTES], b: &[u8; KEY_BYTES]) -> bool {
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

/// Write a new key to `dir`'s key file, replacing any old one, and return it.
pub fn write_key(dir: &Path) -> Result<Key, ControlError> {
    let key = Key::generate()?;
    write_private(&dir.join(KEY_FILE), key.to_hex().as_bytes())?;
    Ok(key)
}

/// A new key in place of the old one: requests with the old key are refused from now on.
pub fn rotate_key(dir: &Path) -> Result<Key, ControlError> {
    write_key(dir)
}

/// The key in `dir`'s key file; a file that is not exactly a key is refused.
pub fn read_key(dir: &Path) -> Result<Key, ControlError> {
    let path = dir.join(KEY_FILE);
    let text = read_small(&path, KEY_HEX_LEN)?;
    Key::parse(&text).ok_or(ControlError::BadKeyFile(path))
}

/// Write the port the server listens on.
pub fn write_port(dir: &Path, port: u16) -> Result<(), ControlError> {
    write_private(&dir.join(PORT_FILE), port.to_string().as_bytes())
}

/// The port in `dir`'s port file: decimal digits only, 1 to 65535.
pub fn read_port(dir: &Path) -> Result<u16, ControlError> {
    let path = dir.join(PORT_FILE);
    let text = read_small(&path, PORT_FILE_MAX)?;
    let port = if !text.is_empty() && text.bytes().all(|c| c.is_ascii_digit()) { text.parse::<u16>().ok() } else { None };
    match port {
        Some(p) if p != 0 => Ok(p),
        _ => Err(ControlError::BadPortFile(path)),
    }
}

/// Remove the key and port files, only while the key file still holds `key` (another MailALM may
/// have written its own since).
pub fn remove_files(dir: &Path, key: &Key) {
    if read_key(dir).is_ok_and(|k| k == *key) {
        for name in [KEY_FILE, PORT_FILE] {
            let path = dir.join(name);
            if let Err(e) = fs::remove_file(&path)
                && e.kind() != io::ErrorKind::NotFound
            {
                log::warn!("MCP control: cannot remove {}: {e}", path.display());
            }
        }
    }
}

/// Read a file of at most `max` bytes as UTF-8; a longer one is refused.
fn read_small(path: &Path, max: usize) -> Result<String, ControlError> {
    let file = fs::File::open(path).map_err(io_error(path))?;
    let mut bytes = Vec::with_capacity(max + 1);
    file.take(u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1)).read_to_end(&mut bytes).map_err(io_error(path))?;
    if bytes.len() > max {
        return Err(too_long(path));
    }
    String::from_utf8(bytes).map_err(|_| too_long(path))
}

fn too_long(path: &Path) -> ControlError {
    if path.file_name().is_some_and(|n| n == KEY_FILE) {
        ControlError::BadKeyFile(path.to_path_buf())
    } else {
        ControlError::BadPortFile(path.to_path_buf())
    }
}

/// Write `bytes` to `path` for the current user only: a new temporary file next to it (mode 0600
/// on Unix), synced, then renamed over the old one. The folder is created if needed.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ControlError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(io_error(dir))?;
    let mut temp_name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    temp_name.push(".new");
    let temp = dir.join(temp_name);
    // A leftover from a run that stopped half-way.
    let _ = fs::remove_file(&temp);
    let mut open = fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let written = open.open(&temp).and_then(|mut f| {
        f.write_all(bytes)?;
        f.sync_all()
    });
    let renamed = written.and_then(|()| fs::rename(&temp, path));
    if let Err(e) = renamed {
        let _ = fs::remove_file(&temp);
        return Err(ControlError::Io { path: path.to_path_buf(), source: e });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mailalm-control-key-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn keys_are_256_random_bits_in_lowercase_hex() {
        let (a, b) = (Key::generate().unwrap(), Key::generate().unwrap());
        assert_ne!(a, b);
        let hex = a.to_hex();
        assert_eq!(hex.len(), 64);
        assert!(hex.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)), "{hex}");
        assert_eq!(Key::parse(&hex), Some(a.clone()));
        assert_eq!(format!("{a:?}"), "Key(…)", "Debug never shows the key");
    }

    #[test]
    fn only_exactly_64_lowercase_hex_characters_are_a_key() {
        let good = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(Key::parse(good).is_some());
        for bad in [
            "",
            &good[..63],
            &format!("{good}0"),
            &format!("{good}\n"),
            &format!(" {}", &good[1..]),
            &good.to_uppercase(),
            &good.replace('f', "g"),
            &good.replace('0', "é"),
        ] {
            assert!(Key::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn keys_compare_whole() {
        let key = Key::generate().unwrap();
        let hex = key.to_hex();
        assert!(key.matches(&hex));
        assert!(!key.matches(&hex[..63]));
        assert!(!key.matches(""));
        assert!(!key.matches(&hex.to_uppercase()));
        let mut other = hex.into_bytes();
        let last = other.len() - 1;
        other[last] = if other[last] == b'0' { b'1' } else { b'0' };
        assert!(!key.matches(&String::from_utf8(other).unwrap()));
    }

    #[test]
    fn key_and_port_files_round_trip_and_rotate() {
        let dir = temp_dir("files");
        assert!(read_key(&dir).unwrap_err().is_missing_file());
        let key = write_key(&dir).unwrap();
        assert_eq!(read_key(&dir).unwrap(), key);
        assert_eq!(fs::read_to_string(dir.join(KEY_FILE)).unwrap(), key.to_hex());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(dir.join(KEY_FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let new = rotate_key(&dir).unwrap();
        assert_ne!(new, key);
        assert_eq!(read_key(&dir).unwrap(), new);
        assert!(!dir.join("control-key.new").exists(), "no temporary file left");

        write_port(&dir, 49_152).unwrap();
        assert_eq!(read_port(&dir).unwrap(), 49_152);

        // The old key does not remove the new files; the current one does.
        remove_files(&dir, &key);
        assert!(dir.join(KEY_FILE).exists() && dir.join(PORT_FILE).exists());
        remove_files(&dir, &new);
        assert!(!dir.join(KEY_FILE).exists() && !dir.join(PORT_FILE).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_key_and_port_files_are_refused() {
        let dir = temp_dir("bad");
        fs::create_dir_all(&dir).unwrap();
        let good = Key::generate().unwrap().to_hex();
        for bad in [String::new(), format!("{good}\n"), good.to_uppercase(), "x".repeat(10_000), good[..10].to_string()] {
            fs::write(dir.join(KEY_FILE), &bad).unwrap();
            assert!(matches!(read_key(&dir), Err(ControlError::BadKeyFile(_))), "{bad:?}");
        }
        for bad in ["", "0", "65536", "80 ", "-1", "8o", "123456"] {
            fs::write(dir.join(PORT_FILE), bad).unwrap();
            assert!(matches!(read_port(&dir), Err(ControlError::BadPortFile(_))), "{bad:?}");
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
