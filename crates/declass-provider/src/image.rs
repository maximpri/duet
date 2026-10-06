// SPDX-License-Identifier: GPL-3.0-or-later
//! Images in conversations: validation, scaling and re-encoding, the forms
//! the dialects put on the wire, and the digests that stand in for image data
//! in audit records and outbound checks.
//!
//! Every image is decoded and encoded again before any model sees it: its
//! format is checked from its bytes (not its name), it is scaled down to a
//! longest side, and whatever the file carried besides the first frame's
//! pixels (EXIF location and camera serials, text chunks, later animation
//! frames) is dropped.

use base64::Engine as _;
use bytes::Bytes;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader, Limits, RgbImage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, Read, Seek, SeekFrom};

/// Largest image file read, before scaling.
pub const MAX_INPUT_BYTES: u64 = 20 * 1024 * 1024;
/// Largest encoded image a model is sent: its base64 form stays under the
/// 5 MB per-image limit of the strictest provider.
pub const MAX_ENCODED_BYTES: usize = 3_750_000;
/// Longest side after scaling unless configured (`images.max_side`).
pub const DEFAULT_MAX_SIDE: u32 = 1568;
/// Largest dimension decoded, and memory a decoder may take: a small file
/// that expands to a huge bitmap is refused rather than decoded.
const MAX_DIMENSION: u32 = 16_384;
const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;
/// JPEG quality of re-encoded photos, and of the fallback for large ones.
const JPEG_QUALITY: u8 = 90;
const JPEG_FALLBACK_QUALITY: u8 = 80;

/// The file extensions `read_file` treats as images.
pub const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// A prepared image: PNG or JPEG, scaled, without metadata. Serialized (in
/// transcripts) without its bytes; a run keeps those in its image store under
/// the digest and loads them back with [`Image::with_data`].
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Image {
    /// `image/png` or `image/jpeg`.
    pub media_type: String,
    /// SHA-256 (hex) of the encoded bytes.
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    /// Size of the encoded bytes.
    pub bytes: u64,
    #[serde(skip)]
    data: Bytes,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Image({}, {}x{}, {} bytes, sha256 {}, {})",
            self.media_type,
            self.width,
            self.height,
            self.bytes,
            &self.sha256[..self.sha256.len().min(12)],
            if self.is_loaded() {
                "loaded"
            } else {
                "not loaded"
            }
        )
    }
}

impl Image {
    /// An image already prepared (PNG or JPEG bytes, as [`prepare`] makes
    /// them), taken as it is; any other image is prepared first.
    pub fn from_encoded(bytes: Vec<u8>) -> Result<Self, String> {
        let format = format_of(&bytes).ok_or("it is not a PNG, JPEG, GIF or WebP image")?;
        if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
            return prepare(&bytes, DEFAULT_MAX_SIDE);
        }
        let (width, height) = ImageReader::with_format(InMemory::new(&bytes), format)
            .into_dimensions()
            .map_err(|e| format!("the image cannot be read: {e}"))?;
        Ok(Self {
            media_type: format.to_mime_type().to_owned(),
            sha256: hex::encode(Sha256::digest(&bytes)),
            width,
            height,
            bytes: bytes.len() as u64,
            data: Bytes::from(bytes),
        })
    }

    /// The encoded bytes (empty until loaded).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Whether the bytes are present (an image read back from a transcript
    /// has none until [`Image::with_data`]).
    pub fn is_loaded(&self) -> bool {
        !self.data.is_empty()
    }

    /// This image with its bytes, which must match the digest.
    pub fn with_data(mut self, data: impl Into<Bytes>) -> Result<Self, String> {
        let data = data.into();
        let digest = hex::encode(Sha256::digest(&data));
        if digest != self.sha256 {
            return Err(format!(
                "stored image {} does not match its digest",
                &self.sha256[..self.sha256.len().min(12)]
            ));
        }
        self.bytes = data.len() as u64;
        self.data = data;
        Ok(self)
    }

    pub fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.data)
    }

    /// `data:<media type>;base64,<data>`.
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.media_type, self.base64())
    }

    /// File extension of the encoded form.
    pub fn extension(&self) -> &'static str {
        if self.media_type == "image/jpeg" {
            "jpg"
        } else {
            "png"
        }
    }

    /// `800x600 png, 120 KB`.
    pub fn describe(&self) -> String {
        format!(
            "{}x{} {}, {}",
            self.width,
            self.height,
            self.extension(),
            human_bytes(self.bytes)
        )
    }

    /// Tokens a model is likely to charge for the image: about one per 750
    /// pixels, as the major providers document it for images of this size.
    pub fn estimated_tokens(&self) -> u64 {
        (u64::from(self.width) * u64::from(self.height))
            .div_ceil(750)
            .clamp(85, 1_600)
    }
}

/// Bytes in memory as the seekable reader the decoders take.
struct InMemory<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> InMemory<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn rest(&self) -> &'a [u8] {
        &self.data[self.at.min(self.data.len())..]
    }
}

impl Read for InMemory<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let rest = self.rest();
        let n = rest.len().min(buf.len());
        buf[..n].copy_from_slice(&rest[..n]);
        self.at += n;
        Ok(n)
    }
}

impl BufRead for InMemory<'_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        Ok(self.rest())
    }

    fn consume(&mut self, n: usize) {
        self.at = (self.at + n).min(self.data.len());
    }
}

impl Seek for InMemory<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let at = match to {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::End(d) => self.data.len() as i128 + i128::from(d),
            SeekFrom::Current(d) => self.at as i128 + i128::from(d),
        };
        if at < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before the start",
            ));
        }
        self.at = usize::try_from(at).unwrap_or(usize::MAX);
        Ok(self.at as u64)
    }
}

fn human_bytes(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", n.div_ceil(1024))
    }
}

/// The media type of an image file from its first bytes, if it is one of
/// the supported formats (PNG, JPEG, GIF, WebP).
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    format_of(bytes).map(|f| f.to_mime_type())
}

fn format_of(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::WebP)
    } else {
        None
    }
}

/// Whether `path` names an image by its extension.
pub fn has_image_extension(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| {
        EXTENSIONS
            .iter()
            .any(|e| e.eq_ignore_ascii_case(ext.trim()))
    })
}

/// Decodes `raw` (PNG, JPEG, GIF or WebP), scales it so its longest side is
/// at most `max_side` pixels and encodes it again: JPEG stays JPEG, anything
/// else becomes PNG (only the first frame of an animation is kept). An
/// encoding over [`MAX_ENCODED_BYTES`] is retried as JPEG and then smaller.
/// The error says why the image cannot be used.
pub fn prepare(raw: &[u8], max_side: u32) -> Result<Image, String> {
    if raw.len() as u64 > MAX_INPUT_BYTES {
        return Err(format!(
            "the file is {}; images up to {} are read",
            human_bytes(raw.len() as u64),
            human_bytes(MAX_INPUT_BYTES)
        ));
    }
    let format = format_of(raw).ok_or("it is not a PNG, JPEG, GIF or WebP image")?;
    let mut reader = ImageReader::with_format(InMemory::new(raw), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| {
        format!(
            "the {} image cannot be decoded: {e}",
            format.extensions_str().first().copied().unwrap_or("image")
        )
    })?;
    let side = max_side.max(1);
    let mut img = if decoded.width().max(decoded.height()) > side {
        decoded.resize(side, side, FilterType::CatmullRom)
    } else {
        decoded
    };
    let mut jpeg = format == ImageFormat::Jpeg;
    for _ in 0..8 {
        let encoded = encode(&img, jpeg)?;
        if encoded.len() <= MAX_ENCODED_BYTES {
            return Ok(Image {
                media_type: if jpeg { "image/jpeg" } else { "image/png" }.to_owned(),
                sha256: hex::encode(Sha256::digest(&encoded)),
                width: img.width(),
                height: img.height(),
                bytes: encoded.len() as u64,
                data: Bytes::from(encoded),
            });
        }
        if !jpeg {
            // A photo saved as PNG: JPEG is several times smaller.
            jpeg = true;
            continue;
        }
        let (w, h) = (img.width() * 3 / 4, img.height() * 3 / 4);
        if w < 16 || h < 16 {
            break;
        }
        img = img.resize(w, h, FilterType::CatmullRom);
    }
    Err(format!(
        "it does not fit in {} even scaled down",
        human_bytes(MAX_ENCODED_BYTES as u64)
    ))
}

fn encode(img: &DynamicImage, jpeg: bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let quality = if img.width().max(img.height()) > 1024 {
        JPEG_FALLBACK_QUALITY
    } else {
        JPEG_QUALITY
    };
    let result = if jpeg {
        let rgb = flatten(img);
        DynamicImage::ImageRgb8(rgb)
            .write_with_encoder(JpegEncoder::new_with_quality(&mut out, quality))
    } else {
        let img = match img {
            DynamicImage::ImageLuma8(_)
            | DynamicImage::ImageLumaA8(_)
            | DynamicImage::ImageRgb8(_)
            | DynamicImage::ImageRgba8(_) => img.clone(),
            other if other.color().has_alpha() => DynamicImage::ImageRgba8(other.to_rgba8()),
            other => DynamicImage::ImageRgb8(other.to_rgb8()),
        };
        img.write_with_encoder(PngEncoder::new_with_quality(
            &mut out,
            CompressionType::Default,
            PngFilter::Adaptive,
        ))
    };
    result.map_err(|e| format!("the image cannot be encoded: {e}"))?;
    Ok(out)
}

/// The image on a white background, without transparency (for JPEG).
fn flatten(img: &DynamicImage) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        let a = u32::from(p[3]);
        let mix = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
        image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])])
    })
}

/// A PNG of one colour (`width` x `height`), for probes and tests.
pub fn solid_png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, image::Rgb(rgb)));
    encode(&img, false).unwrap_or_default()
}

/// A PNG of pseudo-random pixels from `seed` (a distinct, poorly
/// compressible image), for tests.
#[cfg(any(test, feature = "test-support"))]
pub fn pattern_png(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    let img = RgbImage::from_fn(width, height, |_, _| {
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state & 0xFF) as u8
        };
        image::Rgb([next(), next(), next()])
    });
    encode(&DynamicImage::ImageRgb8(img), false).unwrap_or_default()
}

/// The marker that stands for an image's data in audit records.
pub fn marker(sha256: &str, bytes: usize) -> String {
    format!("[image sha256:{sha256}, {bytes} bytes]")
}

/// `body` with every image's data replaced by its [`marker`], and the
/// digests of the images, in order. Image data is recognized in the forms
/// the dialects send: `data:image/...;base64,` URLs (Chat Completions,
/// Responses) and `{"type": "base64", "media_type": "image/...", "data"}`
/// sources (Anthropic). Audit records and outbound checks work on this form:
/// image data is not text, and never stored in the log.
pub fn redact(body: &Value) -> (Value, Vec<String>) {
    let mut out = body.clone();
    let mut digests = Vec::new();
    redact_in(&mut out, &mut digests);
    (out, digests)
}

fn redact_in(v: &mut Value, digests: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            if let Some(payload) = s
                .strip_prefix("data:image/")
                .and_then(|rest| rest.split_once(";base64,"))
                .map(|(_, p)| p)
            {
                let (digest, n) = digest_of(payload);
                *s = marker(&digest, n);
                digests.push(digest);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| redact_in(x, digests)),
        Value::Object(m) => {
            let is_source = m.get("type").and_then(Value::as_str) == Some("base64")
                && m.get("media_type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.starts_with("image/"));
            if is_source && let Some(Value::String(data)) = m.get_mut("data") {
                let (digest, n) = digest_of(data);
                *data = marker(&digest, n);
                digests.push(digest);
            }
            m.values_mut().for_each(|x| redact_in(x, digests));
        }
        _ => {}
    }
}

/// The digest and size of the bytes a base64 payload holds (of the text
/// itself when it is not valid base64, so it still gets a stable name).
fn digest_of(payload: &str) -> (String, usize) {
    match base64::engine::general_purpose::STANDARD.decode(payload) {
        Ok(bytes) => (hex::encode(Sha256::digest(&bytes)), bytes.len()),
        Err(_) => (
            hex::encode(Sha256::digest(payload.as_bytes())),
            payload.len(),
        ),
    }
}

/// Chat Completions content part.
pub(crate) fn chat_part(img: &Image) -> Value {
    json!({"type": "image_url", "image_url": {"url": img.data_url()}})
}

/// Anthropic Messages content block.
pub(crate) fn anthropic_block(img: &Image) -> Value {
    json!({"type": "image", "source": {"type": "base64", "media_type": img.media_type,
        "data": img.base64()}})
}

/// Responses input content part.
pub(crate) fn responses_part(img: &Image) -> Value {
    json!({"type": "input_image", "image_url": img.data_url(), "detail": "auto"})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_recognized_by_their_bytes() {
        assert_eq!(sniff(&solid_png(4, 4, [1, 2, 3])), Some("image/png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a...."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"%PDF-1.7"), None);
        assert!(has_image_extension("docs/ui.PNG") && has_image_extension("a.webp"));
        assert!(!has_image_extension("src/png.rs") && !has_image_extension("jpg"));
    }

    #[test]
    fn large_images_are_scaled_and_every_image_is_re_encoded() {
        let big = solid_png(3000, 1000, [200, 10, 10]);
        let img = prepare(&big, 1568).unwrap();
        assert_eq!((img.width, img.height), (1568, 523));
        assert_eq!(img.media_type, "image/png");
        assert_eq!(img.bytes as usize, img.data().len());
        assert_eq!(img.sha256, hex::encode(Sha256::digest(img.data())));
        // Small images keep their size; the bytes are a fresh encoding.
        let small = solid_png(20, 10, [0, 0, 255]);
        let same = prepare(&small, 1568).unwrap();
        assert_eq!((same.width, same.height), (20, 10));
        assert!(prepare(b"not an image", 1568).is_err());
        let truncated = &big[..big.len() / 2];
        assert!(prepare(truncated, 1568).unwrap_err().contains("decoded"));
    }

    #[test]
    fn metadata_is_dropped_by_re_encoding() {
        // A PNG with a text chunk after the header: the prepared image has none.
        let png = solid_png(8, 8, [9, 9, 9]);
        let mut with_text = png[..33].to_vec();
        let chunk = b"tEXtComment\0secret-location-51.5N";
        with_text.extend_from_slice(&(chunk.len() as u32 - 4).to_be_bytes());
        with_text.extend_from_slice(chunk);
        with_text.extend_from_slice(&crc(chunk).to_be_bytes());
        with_text.extend_from_slice(&png[33..]);
        let img = prepare(&with_text, 1568).unwrap();
        let text = String::from_utf8_lossy(img.data());
        assert!(!text.contains("secret-location"), "metadata survived");
    }

    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }

    #[test]
    fn the_in_memory_reader_reads_and_seeks() {
        let mut r = InMemory::new(b"abcdef");
        let mut two = [0u8; 2];
        r.read_exact(&mut two).unwrap();
        assert_eq!(&two, b"ab");
        assert_eq!(r.seek(SeekFrom::End(-1)).unwrap(), 5);
        assert_eq!(r.fill_buf().unwrap(), b"f");
        assert_eq!(r.seek(SeekFrom::Current(-5)).unwrap(), 0);
        assert!(r.seek(SeekFrom::Current(-1)).is_err());
        assert_eq!(r.seek(SeekFrom::Start(10)).unwrap(), 10);
        assert_eq!(r.read(&mut two).unwrap(), 0);
    }

    #[test]
    fn a_prepared_image_is_taken_as_it_is() {
        let img = prepare(&solid_png(30, 20, [1, 2, 3]), 64).unwrap();
        assert_eq!(Image::from_encoded(img.data().to_vec()).unwrap(), img);
        assert!(Image::from_encoded(b"text".to_vec()).is_err());
    }

    #[test]
    fn stored_bytes_must_match_the_digest() {
        let img = prepare(&solid_png(4, 4, [1, 1, 1]), 64).unwrap();
        let data = img.data().to_vec();
        let json = serde_json::to_string(&img).unwrap();
        assert!(!json.contains(&img.base64()), "bytes are never serialized");
        let back: Image = serde_json::from_str(&json).unwrap();
        assert!(!back.is_loaded());
        assert!(back.clone().with_data(b"other".to_vec()).is_err());
        assert_eq!(back.with_data(data).unwrap(), img);
    }

    /// A conversation with an image in the operator's message and one in the
    /// first of two tool results.
    fn conversation(img: &Image) -> crate::types::Request {
        use crate::types::{Item, Request, ToolCall};
        let call = |id: &str| ToolCall {
            id: id.into(),
            name: "read_file".into(),
            arguments: serde_json::Map::new(),
            raw_arguments: format!("{{\"path\":\"{id}.png\"}}"),
        };
        Request {
            items: vec![
                Item::User {
                    text: "fix the layout".into(),
                },
                Item::Images {
                    call_id: None,
                    images: vec![img.clone()],
                },
                Item::Assistant {
                    text: String::new(),
                    reasoning: None,
                    tool_calls: vec![call("c1"), call("c2")],
                    replay: None,
                },
                Item::ToolResult {
                    call_id: "c1".into(),
                    content: "c1.png: image".into(),
                },
                Item::Images {
                    call_id: Some("c1".into()),
                    images: vec![img.clone()],
                },
                Item::ToolResult {
                    call_id: "c2".into(),
                    content: "c2.png: described".into(),
                },
                Item::User {
                    text: "go on".into(),
                },
            ],
            ..Request::default()
        }
    }

    fn calls(ids: &[&str]) -> Vec<Value> {
        ids.iter()
            .map(|id| {
                json!({"id": id, "type": "function", "function": {"name": "read_file",
                    "arguments": format!("{{\"path\":\"{id}.png\"}}")}})
            })
            .collect()
    }

    #[test]
    fn chat_completions_golden() {
        let img = prepare(&solid_png(4, 2, [10, 20, 30]), 64).unwrap();
        let body = crate::chat::build_body("m", &conversation(&img), false);
        let url = img.data_url();
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": [
                    {"type": "image_url", "image_url": {"url": url}},
                    {"type": "text", "text": "fix the layout"}]},
                {"role": "assistant", "content": "", "tool_calls": calls(&["c1", "c2"])},
                {"role": "tool", "tool_call_id": "c1", "content": "c1.png: image"},
                {"role": "tool", "tool_call_id": "c2", "content": "c2.png: described"},
                {"role": "user", "content": [
                    {"type": "text", "text": "[image from the result of tool call c1]"},
                    {"type": "image_url", "image_url": {"url": url}}]},
                {"role": "user", "content": "go on"}
            ])
        );
    }

    #[test]
    fn anthropic_messages_golden() {
        let img = prepare(&solid_png(4, 2, [10, 20, 30]), 64).unwrap();
        let body = crate::anthropic::build_body("m", &conversation(&img), false);
        let source = json!({"type": "base64", "media_type": "image/png", "data": img.base64()});
        let input = json!({"path": "c1.png"});
        let input2 = json!({"path": "c2.png"});
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": [
                    {"type": "image", "source": source},
                    {"type": "text", "text": "fix the layout"}]},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "c1", "name": "read_file", "input": input},
                    {"type": "tool_use", "id": "c2", "name": "read_file", "input": input2}]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "c1", "content": [
                        {"type": "text", "text": "c1.png: image"},
                        {"type": "image", "source": source}]},
                    {"type": "tool_result", "tool_use_id": "c2", "content": "c2.png: described"},
                    {"type": "text", "text": "go on", "cache_control": {"type": "ephemeral"}}]}
            ])
        );
    }

    #[test]
    fn responses_golden() {
        let img = prepare(&solid_png(4, 2, [10, 20, 30]), 64).unwrap();
        let body = crate::responses::build_body("m", &conversation(&img), false);
        let part = json!({"type": "input_image", "image_url": img.data_url(), "detail": "auto"});
        assert_eq!(
            body["input"],
            json!([
                {"role": "user", "content": [part, {"type": "input_text", "text": "fix the layout"}]},
                {"type": "function_call", "call_id": "c1", "name": "read_file",
                 "arguments": "{\"path\":\"c1.png\"}"},
                {"type": "function_call", "call_id": "c2", "name": "read_file",
                 "arguments": "{\"path\":\"c2.png\"}"},
                {"type": "function_call_output", "call_id": "c1", "output": [
                    {"type": "input_text", "text": "c1.png: image"}, part]},
                {"type": "function_call_output", "call_id": "c2", "output": "c2.png: described"},
                {"role": "user", "content": "go on"}
            ])
        );
    }

    #[test]
    fn images_not_loaded_or_masked_send_nothing_and_text_bodies_are_unchanged() {
        let img = prepare(&solid_png(4, 2, [10, 20, 30]), 64).unwrap();
        let mut req = conversation(&img);
        let text_only = |req: &crate::types::Request| crate::types::Request {
            items: req
                .items
                .iter()
                .filter(|i| !matches!(i, crate::types::Item::Images { .. }))
                .cloned()
                .collect(),
            ..crate::types::Request::default()
        };
        let plain = text_only(&req);
        // Transcript form: digests without bytes. Masked: no images at all.
        let unloaded: Image = serde_json::from_str(&serde_json::to_string(&img).unwrap()).unwrap();
        for item in &mut req.items {
            if let crate::types::Item::Images { images, .. } = item {
                *images = vec![unloaded.clone()];
            }
        }
        for dialect in [
            crate::Dialect::Chat,
            crate::Dialect::Anthropic,
            crate::Dialect::Responses,
        ] {
            assert_eq!(
                dialect.build_body("m", &req, false),
                dialect.build_body("m", &plain, false),
                "{dialect:?}"
            );
        }
    }

    #[test]
    fn redaction_replaces_every_wire_form_with_its_digest() {
        let img = prepare(&solid_png(4, 4, [1, 2, 3]), 64).unwrap();
        let body = json!({"messages": [
            {"content": [chat_part(&img), {"type": "text", "text": "x"}]},
            {"content": [anthropic_block(&img)]},
            {"content": [responses_part(&img)]}
        ]});
        let (redacted, digests) = redact(&body);
        assert_eq!(digests, vec![img.sha256.clone(); 3]);
        let text = redacted.to_string();
        assert!(!text.contains(&img.base64()));
        assert_eq!(
            text.matches(&marker(&img.sha256, img.data().len())).count(),
            3
        );
        assert_eq!(redacted["messages"][0]["content"][1]["text"], "x");
    }
}
