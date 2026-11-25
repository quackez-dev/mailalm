//! Pictures in the message: PNG, JPEG and GIF only, recognised by their first bytes (never by a
//! name or a declared type), their size read from their header and capped before anything is
//! decoded.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The largest picture, in bytes.
pub const MAX_IMAGE_BYTES: usize = 8 << 20;
/// The longest side of a picture, in pixels.
pub const MAX_IMAGE_SIDE: u32 = 16_384;
/// The most pixels in a picture.
pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;
/// The most pictures in a message.
pub const MAX_IMAGES: usize = 64;
/// The most bytes of pictures in a message.
pub const MAX_IMAGES_BYTES: usize = 25 << 20;
/// The longest content id kept.
const MAX_CONTENT_ID: usize = 200;

/// A picture shown in the message, sent as a part of its own (`Content-ID`) the HTML names with
/// `cid:`.
#[derive(Clone)]
pub struct InlineImage {
    /// Without angle brackets; only letters, digits and `.-_@+=$%`.
    pub content_id: String,
    /// `image/png`, `image/jpeg` or `image/gif`, from the content.
    pub content_type: &'static str,
    pub bytes: Arc<[u8]>,
    /// Its size in pixels, from its header.
    pub width: u32,
    pub height: u32,
}

impl PartialEq for InlineImage {
    fn eq(&self, other: &Self) -> bool {
        self.content_id == other.content_id
            && self.content_type == other.content_type
            && self.width == other.width
            && self.height == other.height
            && (Arc::ptr_eq(&self.bytes, &other.bytes) || self.bytes == other.bytes)
    }
}

impl Eq for InlineImage {}

impl fmt::Debug for InlineImage {
    /// Without the content.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InlineImage")
            .field("content_id", &self.content_id)
            .field("content_type", &self.content_type)
            .field("bytes", &self.bytes.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

/// Why a picture was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// More than [`MAX_IMAGE_BYTES`].
    TooLarge,
    /// Not a PNG, JPEG or GIF, or its header cannot be read.
    Unsupported,
    /// More than [`MAX_IMAGE_SIDE`] on a side or [`MAX_IMAGE_PIXELS`] in all.
    TooManyPixels,
    /// The message already has [`MAX_IMAGES`] pictures or [`MAX_IMAGES_BYTES`] of them.
    TooMany,
    /// A content id with characters MailALM does not write.
    BadContentId,
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            ImageError::TooLarge => format!("The picture is larger than {} MB.", MAX_IMAGE_BYTES >> 20),
            ImageError::Unsupported => "Only PNG, JPEG and GIF pictures can be put in a message.".to_owned(),
            ImageError::TooManyPixels => "The picture has too many pixels (40 megapixels at most).".to_owned(),
            ImageError::TooMany => format!("A message holds at most {MAX_IMAGES} pictures and {} MB of them.", MAX_IMAGES_BYTES >> 20),
            ImageError::BadContentId => "The picture's content id cannot be used.".to_owned(),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for ImageError {}

impl InlineImage {
    /// A picture from a file's bytes, with a new content id.
    pub fn new(bytes: Vec<u8>) -> Result<InlineImage, ImageError> {
        Self::with_content_id(bytes, &new_content_id())
    }

    /// A picture with the content id a received message gives it (for a reply or a forward).
    pub fn with_content_id(bytes: Vec<u8>, content_id: &str) -> Result<InlineImage, ImageError> {
        let content_id = content_id.trim().trim_start_matches('<').trim_end_matches('>');
        if !is_valid_content_id(content_id) {
            return Err(ImageError::BadContentId);
        }
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(ImageError::TooLarge);
        }
        let (content_type, width, height) = sniff(&bytes).ok_or(ImageError::Unsupported)?;
        if width == 0 || height == 0 {
            return Err(ImageError::Unsupported);
        }
        if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
            return Err(ImageError::TooManyPixels);
        }
        Ok(InlineImage { content_id: content_id.to_owned(), content_type, bytes: bytes.into(), width, height })
    }

    /// The part the MIME builder writes.
    pub fn to_mime(&self) -> mailalm_mime::InlineImage {
        mailalm_mime::InlineImage { content_id: self.content_id.clone(), content_type: self.content_type.to_owned(), bytes: self.bytes.to_vec() }
    }
}

/// Whether `id` can be written as a content id and in a `cid:` address as it is.
pub fn is_valid_content_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_CONTENT_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-@+=$%".contains(&b))
}

/// The type and size of a PNG, JPEG or GIF, from its first bytes.
pub fn sniff(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        if bytes.get(12..16)? != b"IHDR" {
            return None;
        }
        let w = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
        let h = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
        return Some(("image/png", w, h));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        let w = u16::from_le_bytes(bytes.get(6..8)?.try_into().ok()?);
        let h = u16::from_le_bytes(bytes.get(8..10)?.try_into().ok()?);
        return Some(("image/gif", u32::from(w), u32::from(h)));
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        let (w, h) = jpeg_size(bytes)?;
        return Some(("image/jpeg", w, h));
    }
    None
}

/// The size in a JPEG's first frame header (SOF), found by walking its segments.
fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2usize;
    // Each step moves forward; the count only bounds a hostile file's walk.
    for _ in 0..100_000 {
        let byte = *b.get(i)?;
        if byte != 0xff {
            i += 1;
            continue;
        }
        let marker = *b.get(i + 1)?;
        match marker {
            0xff => i += 1,
            0x01 | 0xd0..=0xd8 => i += 2,
            0xd9 | 0xda => return None,
            _ => {
                let len = usize::from(u16::from_be_bytes([*b.get(i + 2)?, *b.get(i + 3)?]));
                if len < 2 {
                    return None;
                }
                if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                    let h = u16::from_be_bytes([*b.get(i + 5)?, *b.get(i + 6)?]);
                    let w = u16::from_be_bytes([*b.get(i + 7)?, *b.get(i + 8)?]);
                    return Some((u32::from(w), u32::from(h)));
                }
                i = i.checked_add(2 + len)?;
            }
        }
    }
    None
}

/// A content id unique to this picture: a per-run random key, the time and a counter.
fn new_content_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut h = RandomState::new().build_hasher();
    h.write_u128(nanos);
    h.write_u64(n);
    format!("img{:016x}.{n}@mailalm", h.finish())
}

/// The picture's pixels (RGBA, straight alpha), at most `max_side` on a side, decoded with the
/// type found from its content and with size limits; `None` if it cannot be decoded.
pub(crate) fn decode(image: &InlineImage, max_side: u32) -> Option<([usize; 2], Vec<u8>)> {
    let format = match image.content_type {
        "image/png" => ::image::ImageFormat::Png,
        "image/jpeg" => ::image::ImageFormat::Jpeg,
        "image/gif" => ::image::ImageFormat::Gif,
        _ => return None,
    };
    let mut limits = ::image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(MAX_IMAGE_PIXELS * 4 + (64 << 20));
    let mut reader = ::image::ImageReader::with_format(std::io::Cursor::new(&image.bytes[..]), format);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    let max_side = max_side.max(16);
    let fitted = if decoded.width() > max_side || decoded.height() > max_side { decoded.thumbnail(max_side, max_side) } else { decoded };
    let rgba = fitted.to_rgba8();
    let size = [usize::try_from(rgba.width()).ok()?, usize::try_from(rgba.height()).ok()?];
    let raw = rgba.into_raw();
    (size[0].checked_mul(size[1])?.checked_mul(4)? == raw.len()).then_some((size, raw))
}
