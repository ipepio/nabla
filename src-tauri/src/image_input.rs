//! Composer image input (S1-S2 of `odd/tasks/composer-image-input.md`).
//!
//! The rule this module protects: an image the webview shows is an image pi can
//! really receive. Two consequences drive everything here.
//!
//! 1. **The format comes from the bytes, never from the extension.** A file named
//!    `shot.png` that is not a PNG is refused instead of being sent as one; pi
//!    would otherwise convert or drop it silently.
//! 2. **The bounds are the app's, enforced where the webview cannot bypass them.**
//!    At most [`MAX_PROMPT_IMAGES`] images per turn and [`MAX_IMAGE_BYTES`] per
//!    image, checked on the file before it is read and on the base64 payload
//!    before it reaches the child's stdin.
//!
//! The payload is base64 because that is pi's own `ImageContent` (`type: "image"`,
//! `data`, `mimeType`), so nothing in the prompt path re-encodes an image. No
//! image ever touches the database, the settings file or a log line: this module
//! only answers the webview and the prompt request.
//!
//! [`read_image_attachment`] takes an absolute path because the human picks or
//! drops the file wherever it lives (a screenshot on the Desktop is not inside the
//! workspace). It is deliberately not a general file reader: the path must be a
//! regular file, its bytes must be one of the four formats, and it must be under
//! the size ceiling, so even a forged path cannot return text or an unbounded
//! file.
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// How many images one prompt can carry. Generous for screenshots (four), small
/// enough that a dropped folder is a refusal rather than a 200 MB write to pi.
pub const MAX_PROMPT_IMAGES: usize = 4;

/// Largest decoded image accepted, in bytes (10 MiB). pi resizes what it sends,
/// so this bounds the payload the webview and the pipe handle, not the model's
/// allowance.
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// The formats pi sends inline without converting (`normalizeSupportedImageMimeType`
/// in pi's `utils/image-process.js`).
pub const SUPPORTED_IMAGE_MIME_TYPES: [&str; 4] = [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
];

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// One image a prompt carries: pi's `ImageContent`, field for field. Mirrors
/// `PromptImage` in `src/contracts/types.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptImage {
    /// One of [`SUPPORTED_IMAGE_MIME_TYPES`].
    pub mime_type: String,
    /// Base64 image data, no data-url prefix.
    pub data: String,
}

/// One image file read for the composer, with the format sniffed from its bytes.
/// Mirrors `ImageAttachment` in `src/contracts/types.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachment {
    /// File name, shown as the preview's label.
    pub name: String,
    /// Sniffed media type, not the one the extension claims.
    pub mime_type: String,
    /// Base64 payload, ready to become a [`PromptImage`].
    pub data: String,
    /// Decoded size, so the caller can say how large it is without decoding it.
    pub byte_length: usize,
}

/// Why an image was refused. One enum for the two entry points (reading a file
/// and validating a prompt) so a rejection reads the same wherever it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageInputError {
    /// The path is not a regular file (a directory, or something gone).
    NotAFile(String),
    /// The file could not be read.
    Io(String),
    /// The bytes are not one of pi's inline formats; the word is what was found
    /// (or `unknown`).
    UnsupportedFormat(String),
    /// One image is over the ceiling.
    TooLarge { bytes: usize, limit: usize },
    /// The prompt carries more images than a turn accepts.
    TooMany { count: usize, limit: usize },
    /// The base64 payload is empty, malformed or not base64 at all.
    InvalidData(String),
}

impl fmt::Display for ImageInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageInputError::NotAFile(path) => {
                write!(formatter, "not a file: {path}")
            }
            ImageInputError::Io(reason) => write!(formatter, "could not read the image: {reason}"),
            ImageInputError::UnsupportedFormat(word) => write!(
                formatter,
                "unsupported image format ({word}): use PNG, JPEG, GIF or WebP"
            ),
            ImageInputError::TooLarge { bytes, limit } => write!(
                formatter,
                "the image is {bytes} bytes, over the {limit}-byte limit"
            ),
            ImageInputError::TooMany { count, limit } => {
                write!(formatter, "a message carries at most {limit} images, got {count}")
            }
            ImageInputError::InvalidData(reason) => {
                write!(formatter, "invalid image data: {reason}")
            }
        }
    }
}

impl std::error::Error for ImageInputError {}

/// The pi-known media type of a wire mime string, or `None`. `image/jpg` is the
/// same format under its other spelling, and parameters are not part of the type.
pub fn normalize_mime_type(raw: &str) -> Option<&'static str> {
    let base = raw
        .split(';')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    // `image/jpg` is the other spelling of the same format, not a fifth one.
    let base = if base == "image/jpg" {
        "image/jpeg".to_string()
    } else {
        base
    };
    SUPPORTED_IMAGE_MIME_TYPES
        .iter()
        .copied()
        .find(|mime| *mime == base.as_str())
}

/// The media type a byte sequence really is, from its magic number, or `None`.
/// This is the check that makes the file extension irrelevant.
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Base64 of some bytes, with the standard padding pi's decoder expects. Written
/// here instead of adding a dependency for one call: the alphabet is fixed and the
/// tests pin the standard vectors.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        out.push(BASE64_ALPHABET[(first >> 2) as usize] as char);
        out.push(BASE64_ALPHABET[(((first & 0b11) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_ALPHABET[(((second & 0b1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_ALPHABET[(third & 0b111111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Decoded size of a base64 payload, rejecting anything that is not one. The
/// payload is never decoded here: the size is what the ceiling needs, and pi
/// decodes the bytes itself.
pub fn decoded_base64_len(data: &str) -> Result<usize, ImageInputError> {
    if data.is_empty() {
        return Err(ImageInputError::InvalidData(
            "the image payload is empty".to_string(),
        ));
    }
    if !data.len().is_multiple_of(4) {
        return Err(ImageInputError::InvalidData(
            "the image payload is not base64".to_string(),
        ));
    }
    let padding = if data.ends_with("==") {
        2
    } else if data.ends_with('=') {
        1
    } else {
        0
    };
    let body = &data[..data.len() - padding];
    if body.is_empty() {
        return Err(ImageInputError::InvalidData(
            "the image payload is only padding".to_string(),
        ));
    }
    if !body
        .bytes()
        .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/'))
    {
        return Err(ImageInputError::InvalidData(
            "the image payload is not base64".to_string(),
        ));
    }
    Ok(data.len() / 4 * 3 - padding)
}

/// Normalizes and bounds one prompt's images, answering the payloads that may be
/// placed on the wire. A rejected list changes nothing: nothing is sent.
pub fn validate_prompt_images(images: &[PromptImage]) -> Result<Vec<PromptImage>, ImageInputError> {
    if images.len() > MAX_PROMPT_IMAGES {
        return Err(ImageInputError::TooMany {
            count: images.len(),
            limit: MAX_PROMPT_IMAGES,
        });
    }
    let mut validated = Vec::with_capacity(images.len());
    for image in images {
        let mime_type = normalize_mime_type(&image.mime_type).ok_or_else(|| {
            ImageInputError::UnsupportedFormat(image.mime_type.clone())
        })?;
        let bytes = decoded_base64_len(&image.data)?;
        if bytes > MAX_IMAGE_BYTES {
            return Err(ImageInputError::TooLarge {
                bytes,
                limit: MAX_IMAGE_BYTES,
            });
        }
        validated.push(PromptImage {
            mime_type: mime_type.to_string(),
            data: image.data.clone(),
        });
    }
    Ok(validated)
}

/// The `images` array of one `prompt` request, in pi's `ImageContent` shape.
pub fn wire_images(images: &[PromptImage]) -> Vec<serde_json::Value> {
    images
        .iter()
        .map(|image| {
            serde_json::json!({
                "type": "image",
                "data": image.data,
                "mimeType": image.mime_type,
            })
        })
        .collect()
}

/// Whether one pi `Model` object (as `get_state` or `set_model` returns it)
/// declares image input. `None` when the model carries no `input` array at all,
/// which is "this pi does not say", not "no images".
pub fn model_input_has_image(model: &serde_json::Value) -> Option<bool> {
    let input = model.get("input")?.as_array()?;
    Some(
        input
            .iter()
            .filter_map(serde_json::Value::as_str)
            .any(|entry| entry.eq_ignore_ascii_case("image")),
    )
}

/// Reads one image file for the composer, bounded before it is read whole.
///
/// The order matters: the metadata check is what keeps a 2 GB file from being
/// loaded into memory just to be refused. The payload is base64-encoded after the
/// bytes prove they are a supported format, so a rejected file never becomes a
/// large string in the webview.
pub fn read_image_attachment(path: &Path) -> Result<ImageAttachment, ImageInputError> {
    let display = path.display().to_string();
    let metadata = std::fs::metadata(path).map_err(|_| ImageInputError::NotAFile(display.clone()))?;
    if !metadata.is_file() {
        return Err(ImageInputError::NotAFile(display));
    }
    let size = metadata.len() as usize;
    if size > MAX_IMAGE_BYTES {
        return Err(ImageInputError::TooLarge {
            bytes: size,
            limit: MAX_IMAGE_BYTES,
        });
    }
    let bytes = std::fs::read(path).map_err(|error| ImageInputError::Io(error.to_string()))?;
    let mime_type = sniff_mime(&bytes).ok_or_else(|| {
        ImageInputError::UnsupportedFormat("not a PNG, JPEG, GIF or WebP".to_string())
    })?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ImageInputError::TooLarge {
            bytes: bytes.len(),
            limit: MAX_IMAGE_BYTES,
        });
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "image".to_string());
    Ok(ImageAttachment {
        name,
        mime_type: mime_type.to_string(),
        data: base64_encode(&bytes),
        byte_length: bytes.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory in the system temp dir, removed on drop: no fixture
    /// ever lives in the worktree.
    struct TempImages {
        dir: std::path::PathBuf,
    }

    impl TempImages {
        fn new(tag: &str) -> TempImages {
            let mut dir = std::env::temp_dir();
            dir.push(format!(
                "nabla_images_{}_{}_{tag}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            TempImages { dir }
        }

        fn write(&self, name: &str, bytes: &[u8]) -> std::path::PathBuf {
            let path = self.dir.join(name);
            std::fs::write(&path, bytes).expect("write image");
            path
        }
    }

    impl Drop for TempImages {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x01];

    /// pi's `ImageContent` shape is what leaves for the child; a wrong field name
    /// would be silently ignored by pi.
    #[test]
    fn wire_images_are_pis_image_content() {
        let images = vec![PromptImage {
            mime_type: "image/png".to_string(),
            data: "Zm9v".to_string(),
        }];
        assert_eq!(
            wire_images(&images),
            vec![serde_json::json!({
                "type": "image",
                "data": "Zm9v",
                "mimeType": "image/png",
            })]
        );
    }

    // S2: the four formats pi sends inline are the accepted ones; the alternate
    // spelling and parameters normalize instead of being refused as typos.
    #[test]
    fn mime_types_normalize_to_the_four_inline_formats() {
        assert_eq!(normalize_mime_type("image/png"), Some("image/png"));
        assert_eq!(normalize_mime_type("IMAGE/JPEG"), Some("image/jpeg"));
        assert_eq!(normalize_mime_type("image/jpg"), Some("image/jpeg"));
        assert_eq!(normalize_mime_type("image/gif; charset=binary"), Some("image/gif"));
        assert_eq!(normalize_mime_type("image/webp"), Some("image/webp"));
        for rejected in ["", "text/plain", "image/svg+xml", "image/bmp", "application/pdf"] {
            assert_eq!(normalize_mime_type(rejected), None, "{rejected} is refused");
        }
    }

    // S1: the format is read from the bytes, so a mislabeled file is refused
    // instead of being sent under the wrong media type.
    #[test]
    fn the_format_is_sniffed_from_the_bytes() {
        assert_eq!(sniff_mime(PNG), Some("image/png"));
        assert_eq!(sniff_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_mime(b"GIF89a\x01\x00"), Some("image/gif"));
        assert_eq!(sniff_mime(b"RIFF\x24\x00\x00\x00WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_mime(b"RIFF\x24\x00\x00\x00WAVEfmt "), None);
        assert_eq!(sniff_mime(b"plain text"), None);
        assert_eq!(sniff_mime(b""), None);
    }

    // The encoder has to be the standard one: pi decodes it with Node's base64,
    // and an off-by-one in the padding would corrupt every screenshot.
    #[test]
    fn base64_encoding_matches_the_standard_vectors() {
        for (bytes, encoded) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(bytes), encoded, "{bytes:?}");
            if !encoded.is_empty() {
                // An empty payload is refused by the prompt validator, so only the
                // encoder has an opinion about zero bytes.
                assert_eq!(
                    decoded_base64_len(encoded),
                    Ok(bytes.len()),
                    "decoded size of {encoded:?}"
                );
            }
        }
        let every_byte: Vec<u8> = (0..=255u8).collect();
        assert_eq!(base64_encode(&every_byte).len(), 344);
    }

    #[test]
    fn a_payload_that_is_not_base64_is_rejected() {
        for bad in ["", "Zg=", "Zm9v\n", "Zm9v Yg==", "not base64!", "===="] {
            assert!(
                decoded_base64_len(bad).is_err(),
                "{bad:?} must not be accepted"
            );
        }
    }

    // S2: the bounds are the app's, and a rejected list changes nothing.
    #[test]
    fn a_prompt_is_bounded_by_count_size_and_format() {
        let image = |mime: &str, data: &str| PromptImage {
            mime_type: mime.to_string(),
            data: data.to_string(),
        };
        assert_eq!(
            validate_prompt_images(&[image("image/png", "Zm9v")]).expect("accepted"),
            vec![image("image/png", "Zm9v")]
        );
        assert_eq!(
            validate_prompt_images(&[image("image/jpg", "Zm9v")]).expect("normalized"),
            vec![image("image/jpeg", "Zm9v")]
        );

        let too_many: Vec<PromptImage> = (0..=MAX_PROMPT_IMAGES)
            .map(|_| image("image/png", "Zm9v"))
            .collect();
        assert_eq!(
            validate_prompt_images(&too_many),
            Err(ImageInputError::TooMany {
                count: MAX_PROMPT_IMAGES + 1,
                limit: MAX_PROMPT_IMAGES,
            })
        );
        assert!(matches!(
            validate_prompt_images(&[image("text/plain", "Zm9v")]),
            Err(ImageInputError::UnsupportedFormat(_))
        ));
        assert!(matches!(
            validate_prompt_images(&[image("image/png", "")]),
            Err(ImageInputError::InvalidData(_))
        ));
        // A payload over the ceiling is refused on its measured size, not on its
        // string length.
        let oversized = "A".repeat((MAX_IMAGE_BYTES / 3 + 1) * 4);
        assert_eq!(
            validate_prompt_images(&[image("image/png", &oversized)]),
            Err(ImageInputError::TooLarge {
                bytes: (MAX_IMAGE_BYTES / 3 + 1) * 3,
                limit: MAX_IMAGE_BYTES,
            })
        );
    }

    // S1: reading a real file answers the sniffed format, the exact bytes and the
    // file name — and rejects a non-image, a directory and a path that is gone.
    #[test]
    fn reading_a_file_answers_pis_payload() {
        let images = TempImages::new("read");
        let path = images.write("Screenshot 2026-01-02 at 10.00.png", PNG);

        let attachment = read_image_attachment(&path).expect("read the png");
        assert_eq!(
            attachment.name,
            "Screenshot 2026-01-02 at 10.00.png",
            "the name stays human: it is the preview's label"
        );
        assert_eq!(attachment.mime_type, "image/png");
        assert_eq!(attachment.byte_length, PNG.len());
        assert_eq!(attachment.data, base64_encode(PNG));

        let mislabeled = images.write("not-an-image.png", b"hello");
        assert_eq!(
            read_image_attachment(&mislabeled),
            Err(ImageInputError::UnsupportedFormat(
                "not a PNG, JPEG, GIF or WebP".to_string()
            )),
            "the extension never decides the format"
        );

        let directory = images.write("folder", b"");
        std::fs::remove_file(&directory).expect("replace with a directory");
        std::fs::create_dir(&directory).expect("create directory");
        assert!(matches!(
            read_image_attachment(&directory),
            Err(ImageInputError::NotAFile(_))
        ));
        assert!(matches!(
            read_image_attachment(&images.dir.join("gone.png")),
            Err(ImageInputError::NotAFile(_))
        ));
    }

    // The size check happens before the bytes are read: an oversized file is
    // refused from its metadata alone.
    #[test]
    fn an_oversized_file_is_refused_before_reading_it() {
        let images = TempImages::new("oversized");
        let path = images.write("huge.png", PNG);
        let handle = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open for resize");
        handle
            .set_len((MAX_IMAGE_BYTES + 1) as u64)
            .expect("grow the file");
        drop(handle);
        assert_eq!(
            read_image_attachment(&path),
            Err(ImageInputError::TooLarge {
                bytes: MAX_IMAGE_BYTES + 1,
                limit: MAX_IMAGE_BYTES,
            })
        );
    }

    // S2: the model's own `input` list is the capability answer; a model that
    // declares no list is "unknown", not "text only".
    #[test]
    fn a_models_input_list_is_read_as_the_capability_answer() {
        assert_eq!(
            model_input_has_image(&serde_json::json!({ "input": ["text", "image"] })),
            Some(true)
        );
        assert_eq!(
            model_input_has_image(&serde_json::json!({ "input": ["text"] })),
            Some(false)
        );
        assert_eq!(model_input_has_image(&serde_json::json!({})), None);
        assert_eq!(model_input_has_image(&serde_json::json!({ "input": "image" })), None);
    }
}
