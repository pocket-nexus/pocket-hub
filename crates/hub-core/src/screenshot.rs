//! §6.2 `runtime.screenshot`: both screens as one PNG.
//!
//! The device streams the PICA targets as they sit in memory: format
//! `bgr8-rotated`, B, G, R bytes in the rotated column-major order of the
//! 3DS's framebuffers, the top surface's bytes and then the bottom's. Hub
//! composes one PNG with the surfaces one under the other, each centred, as
//! the reference command line does (`tools/pocket-link/v2/cli.ts`
//! `screenshotPng`).

use serde::Serialize;
use serde_json::Value;

/// One surface as the head event declares it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Surface {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

/// A whole screenshot: the device's head event and the composed PNG.
#[derive(Debug, Clone, Serialize)]
pub struct Screenshot {
    pub frame: u64,
    pub format: String,
    pub surfaces: Vec<Surface>,
    /// The width and height of the composed PNG.
    pub width: u32,
    pub height: u32,
    /// The PNG file's bytes; left out of serialized snapshots.
    #[serde(skip)]
    pub png: Vec<u8>,
}

/// The surfaces of a head event, and the bytes they declare in all. None when
/// the list is missing or a surface is not `{name, width, height, bytes}`.
pub fn surfaces_of(head: &serde_json::Map<String, Value>) -> Option<(Vec<Surface>, u64)> {
    let list = head.get("surfaces")?.as_array()?;
    let mut surfaces = Vec::with_capacity(list.len());
    let mut total = 0u64;
    for entry in list {
        let width = u32::try_from(entry.get("width")?.as_u64()?).ok()?;
        let height = u32::try_from(entry.get("height")?.as_u64()?).ok()?;
        let bytes = entry.get("bytes")?.as_u64()?;
        let name = entry.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
        total = total.checked_add(bytes)?;
        surfaces.push(Surface { name, width, height, bytes });
    }
    Some((surfaces, total))
}

#[derive(Debug, thiserror::Error)]
pub enum ComposeError {
    #[error("the screenshot format {0} is not bgr8-rotated")]
    Format(String),
    #[error("the surface {0} has the wrong byte count")]
    Size(String),
    #[error("PNG encoding failed: {0}")]
    Png(String),
}

/// Composes the surfaces one under the other, each centred, into an RGB PNG.
pub fn compose(frame: u64, format: &str, surfaces: Vec<Surface>, pixels: &[u8]) -> Result<Screenshot, ComposeError> {
    if format != "bgr8-rotated" {
        return Err(ComposeError::Format(format.to_owned()));
    }
    let width = surfaces.iter().map(|s| s.width).max().unwrap_or(0);
    let height: u32 = surfaces.iter().map(|s| s.height).sum();
    let mut rgb = vec![0u8; width as usize * height as usize * 3];
    let mut offset = 0usize;
    let mut top = 0usize;
    for surface in &surfaces {
        let (w, h) = (surface.width as usize, surface.height as usize);
        if surface.bytes != (w * h * 3) as u64 || offset + w * h * 3 > pixels.len() {
            return Err(ComposeError::Size(surface.name.clone()));
        }
        let left = (width as usize - w) / 2;
        for y in 0..h {
            for x in 0..w {
                // Column-major and rotated: column x holds the rows bottom to top.
                let source = offset + (x * h + (h - 1 - y)) * 3;
                let destination = ((top + y) * width as usize + left + x) * 3;
                rgb[destination] = pixels[source + 2];
                rgb[destination + 1] = pixels[source + 1];
                rgb[destination + 2] = pixels[source];
            }
        }
        offset += w * h * 3;
        top += h;
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| ComposeError::Png(e.to_string()))?;
        writer.write_image_data(&rgb).map_err(|e| ComposeError::Png(e.to_string()))?;
    }
    Ok(Screenshot { frame, format: format.to_owned(), surfaces, width, height, png })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_columns_become_rows() {
        // A 2×1 surface: column 0 holds one pixel, column 1 the other.
        let surfaces = vec![Surface { name: "top".into(), width: 2, height: 1, bytes: 6 }];
        let shot = compose(7, "bgr8-rotated", surfaces, &[1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!((shot.width, shot.height), (2, 1));
        let decoder = png::Decoder::new(std::io::Cursor::new(shot.png));
        let mut reader = decoder.read_info().unwrap();
        let mut out = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut out).unwrap();
        assert_eq!(&out[..6], &[3, 2, 1, 6, 5, 4]);
    }
}
