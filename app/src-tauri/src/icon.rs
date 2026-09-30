//! Tray icons, drawn at run time: the battery percentage, a filled badge when it's low, and a mouse
//! outline when there's no reading. Monochrome, like the rest of the app: white on a dark taskbar,
//! black on a light one. Digits use the system's bold UI font.

use std::path::PathBuf;
use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, OutlinedGlyph, PxScale, ScaleFont, point};
use attack_shark_x11::battery::LOW;

/// Icon edge in pixels; Windows scales it for the tray.
pub const SIZE: u32 = 32;

fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| font_paths().into_iter().find_map(|path| FontVec::try_from_vec(std::fs::read(path).ok()?).ok()))
        .as_ref()
}

fn font_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(windows) = std::env::var_os("WINDIR") {
        let fonts = PathBuf::from(windows).join("Fonts");
        paths.extend([fonts.join("segoeuib.ttf"), fonts.join("arialbd.ttf")]);
    }
    paths.extend(
        [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        ]
        .map(PathBuf::from),
    );
    paths
}

/// Coverage per pixel, 0 to 1.
struct Mask {
    alpha: Vec<f32>,
}

/// Signed distance from `(x, y)` to a rounded rectangle; negative inside.
fn rounded_rect(x: f32, y: f32, [x0, y0, x1, y1]: [f32; 4], radius: f32) -> f32 {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let qx = (x - cx).abs() - ((x1 - x0) / 2.0 - radius);
    let qy = (y - cy).abs() - ((y1 - y0) / 2.0 - radius);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

impl Mask {
    fn new() -> Self {
        Self { alpha: vec![0.0; (SIZE * SIZE) as usize] }
    }

    fn shade(&mut self, coverage: impl Fn(f32, f32) -> f32) {
        for (i, alpha) in self.alpha.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % SIZE) as f32 + 0.5, (i as u32 / SIZE) as f32 + 0.5);
            *alpha = alpha.max(coverage(x, y).clamp(0.0, 1.0));
        }
    }

    fn fill_rounded(&mut self, rect: [f32; 4], radius: f32) {
        self.shade(|x, y| 0.5 - rounded_rect(x, y, rect, radius));
    }

    fn stroke_rounded(&mut self, rect: [f32; 4], radius: f32, width: f32) {
        self.shade(|x, y| width / 2.0 + 0.5 - rounded_rect(x, y, rect, radius).abs());
    }

    /// Draw `text` as large as fits in `max_width` x `max_height`, centered. With `erase`, cut it
    /// out of what is already drawn instead.
    fn text(&mut self, font: &FontVec, text: &str, max_width: f32, max_height: f32, erase: bool) {
        let mut size = max_height * 1.4;
        let (glyphs, [min_x, min_y, max_x, max_y]) = loop {
            let (glyphs, bounds) = layout(font, text, PxScale::from(size));
            let [x0, y0, x1, y1] = bounds;
            if (x1 - x0 <= max_width && y1 - y0 <= max_height) || size < 4.0 {
                break (glyphs, bounds);
            }
            size -= 0.5;
        };
        let edge = SIZE as f32;
        let dx = ((edge - (max_x - min_x)) / 2.0 - min_x).round();
        let dy = ((edge - (max_y - min_y)) / 2.0 - min_y).round();
        for glyph in glyphs {
            let bounds = glyph.px_bounds();
            glyph.draw(|x, y, coverage| {
                let px = (bounds.min.x + dx) as i32 + x as i32;
                let py = (bounds.min.y + dy) as i32 + y as i32;
                if (0..SIZE as i32).contains(&px) && (0..SIZE as i32).contains(&py) {
                    let alpha = &mut self.alpha[(py as u32 * SIZE + px as u32) as usize];
                    *alpha = if erase { *alpha * (1.0 - coverage) } else { alpha.max(coverage) };
                }
            });
        }
    }

    fn into_rgba(self, ink: [u8; 3]) -> Vec<u8> {
        self.alpha.iter().flat_map(|alpha| [ink[0], ink[1], ink[2], (alpha * 255.0).round() as u8]).collect()
    }
}

/// Glyphs laid out on one line, and their pixel bounds as `[min x, min y, max x, max y]`.
fn layout(font: &FontVec, text: &str, scale: PxScale) -> (Vec<OutlinedGlyph>, [f32; 4]) {
    let scaled = font.as_scaled(scale);
    let mut caret = 0.0;
    let mut previous = None;
    let mut glyphs = Vec::new();
    for character in text.chars() {
        let id = scaled.glyph_id(character);
        if let Some(previous) = previous {
            caret += scaled.kern(previous, id);
        }
        let glyph = id.with_scale_and_position(scale, point(caret, scaled.ascent()));
        caret += scaled.h_advance(id);
        previous = Some(id);
        glyphs.extend(font.outline_glyph(glyph));
    }
    let bounds = glyphs
        .iter()
        .map(OutlinedGlyph::px_bounds)
        .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |[x0, y0, x1, y1], rect| {
            [x0.min(rect.min.x), y0.min(rect.min.y), x1.max(rect.max.x), y1.max(rect.max.y)]
        });
    (glyphs, bounds)
}

/// RGBA pixels, `SIZE` x `SIZE`, for a battery `level` (or none).
pub fn render(level: Option<u8>, light_taskbar: bool) -> Vec<u8> {
    let ink = if light_taskbar { [18, 18, 18] } else { [245, 245, 245] };
    let edge = SIZE as f32;
    let mut mask = Mask::new();
    match (level, font()) {
        (Some(level), Some(font)) if level <= LOW => {
            mask.fill_rounded([0.0, 0.0, edge, edge], edge * 0.22);
            mask.text(font, &level.to_string(), edge * 0.78, edge * 0.62, true);
        }
        (Some(level), Some(font)) => mask.text(font, &level.to_string(), edge * 0.98, edge * 0.8, false),
        _ => {
            let stroke = (edge / 14.0).max(2.0);
            mask.stroke_rounded([edge * 0.3, edge * 0.1, edge * 0.7, edge * 0.9], edge * 0.2, stroke);
            mask.fill_rounded([edge / 2.0 - stroke / 2.0, edge * 0.1, edge / 2.0 + stroke / 2.0, edge * 0.4], 0.0);
        }
    }
    mask.into_rgba(ink)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(pixels: &[u8], x: u32, y: u32) -> u8 {
        pixels[((y * SIZE + x) * 4 + 3) as usize]
    }

    #[test]
    fn icons_have_the_right_size_and_ink() {
        for level in [None, Some(3), Some(15), Some(36), Some(100)] {
            for light in [false, true] {
                let pixels = render(level, light);
                assert_eq!(pixels.len(), (SIZE * SIZE * 4) as usize);
                assert!(pixels.chunks(4).any(|pixel| pixel[3] > 0), "{level:?} draws something");
                let ink = if light { 18 } else { 245 };
                assert!(pixels.chunks(4).all(|pixel| pixel[0] == ink));
            }
        }
    }

    #[test]
    fn low_levels_are_a_filled_badge() {
        let top_edge = SIZE / 2;
        assert_eq!(alpha(&render(Some(15), false), top_edge, 1), 255);
        assert_eq!(alpha(&render(Some(36), false), top_edge, 1), 0);
        assert_eq!(alpha(&render(None, false), top_edge, 1), 0);
    }

    /// Writes raw RGBA previews to target/tray-preview for a visual check.
    #[test]
    #[ignore = "writes preview files"]
    fn preview() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tray-preview");
        std::fs::create_dir_all(&dir).unwrap();
        for level in [None, Some(3), Some(15), Some(36), Some(88), Some(100)] {
            for light in [false, true] {
                let name = format!("{}-{}.rgba", level.map_or("none".into(), |l| l.to_string()), light);
                std::fs::write(dir.join(name), render(level, light)).unwrap();
            }
        }
    }
}
