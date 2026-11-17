use anyhow::{anyhow, Context, Result};
use fontdue::Font;
use romstore_client::i18n::Language;
use sdl2::{
    pixels::{Color, PixelFormatEnum},
    rect::{FRect, Rect},
    render::{BlendMode, Canvas, ScaleMode, Texture, TextureCreator},
    video::{Window, WindowContext},
};
use std::{collections::HashMap, path::Path};

pub const BG: Color = Color::RGB(10, 17, 29);
pub const TEXT: Color = Color::RGB(237, 244, 250);
pub const MUTED: Color = Color::RGB(153, 172, 194);
pub const ACCENT: Color = Color::RGB(105, 235, 196);
pub const SURFACE: Color = Color::RGB(21, 33, 49);
pub const BORDER: Color = Color::RGB(43, 61, 78);
pub const GOLD: Color = Color::RGB(247, 195, 115);
pub const DANGER: Color = Color::RGB(249, 146, 151);

const MAX_TEXT_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TEXT_CACHE_ENTRIES: usize = 256;

/// Rasterize at least at 2x for small text, and at native density on HiDPI/4K.
/// The UI keeps its 1280x720 coordinates, including when letterboxed.
fn text_density(output: (u32, u32)) -> u32 {
    let scale = (output.0 as f32 / 1280.0).min(output.1 as f32 / 720.0);
    (scale.ceil() as u32).clamp(2, 6)
}

struct TextBitmap {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    offset: (f32, f32),
}

fn advance(font: &Font, previous: Option<char>, c: char, size: u32) -> (f32, f32) {
    let kern = previous
        .and_then(|left| font.horizontal_kern(left, c, size as f32))
        .unwrap_or(0.0);
    (kern, font.metrics(c, size as f32).advance_width)
}

fn rasterize_text(font: &Font, text: &str, size: u32, density: u32) -> TextBitmap {
    let scale = density as f32;
    let baseline = font
        .horizontal_line_metrics(size as f32)
        .map(|m| m.ascent.ceil())
        .unwrap_or(size as f32);
    // Keep advances in logical fractional coordinates. Fontdue Layout rounds
    // each advance up, which changes spacing and line widths with raster density.
    let mut pen = 0.0;
    let mut previous = None;
    let mut glyphs = Vec::new();
    for c in text.chars() {
        let (kern, width) = advance(font, previous, c, size);
        pen += kern;
        let (metrics, bitmap) = font.rasterize(c, size as f32 * scale);
        let x = (pen * scale).round() + metrics.xmin as f32;
        let y = (baseline * scale).round() - metrics.ymin as f32 - metrics.height as f32;
        glyphs.push((metrics, bitmap, x, y));
        pen += width;
        previous = Some(c);
    }
    // Keep overhanging glyphs and a transparent border for linear filtering.
    let left = glyphs.iter().map(|(_, _, x, _)| *x).fold(0.0, f32::min) - 1.0;
    let top = glyphs.iter().map(|(_, _, _, y)| *y).fold(0.0, f32::min) - 1.0;
    let right = glyphs
        .iter()
        .map(|(metrics, _, x, _)| x + metrics.width as f32)
        .fold((pen * scale).max(1.0), f32::max)
        .ceil()
        + 1.0;
    let bottom = glyphs
        .iter()
        .map(|(metrics, _, _, y)| y + metrics.height as f32)
        .fold((size + 8) as f32 * scale, f32::max)
        .ceil()
        + 1.0;
    let width = (right - left) as u32;
    let height = (bottom - top) as u32;
    // Transparent pixels must be white too: interpolating black transparent
    // edges would darken antialiased glyphs when SDL downsamples the texture.
    let mut pixels = vec![255_u8; width as usize * height as usize * 4];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 0;
    }
    for (metrics, bitmap, gx, gy) in glyphs {
        for y in 0..metrics.height {
            for x in 0..metrics.width {
                let px = (gx - left) as usize + x;
                let py = (gy - top) as usize + y;
                let alpha = &mut pixels[(py * width as usize + px) * 4 + 3];
                let coverage = bitmap[y * metrics.width + x] as u16;
                *alpha = (*alpha as u16 + (coverage * (255 - *alpha as u16) + 127) / 255) as u8;
            }
        }
    }
    TextBitmap {
        pixels,
        width,
        height,
        offset: (left / scale, top / scale),
    }
}

struct CachedText<'a> {
    texture: Texture<'a>,
    width: f32,
    height: f32,
    offset: (f32, f32),
}

pub fn dim(canvas: &mut Canvas<Window>, alpha: u8) -> Result<()> {
    let previous = canvas.blend_mode();
    canvas.set_blend_mode(BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(4, 10, 20, alpha));
    let result = canvas.fill_rect(None).map_err(|e| anyhow!(e));
    canvas.set_blend_mode(previous);
    result
}

/// Rounded panels without SDL_gfx, anti-aliasing libraries or per-frame textures.
pub fn rounded(canvas: &mut Canvas<Window>, rect: Rect, radius: u32, color: Color) -> Result<()> {
    let radius = radius.min(rect.width() / 2).min(rect.height() / 2);
    canvas.set_draw_color(color);
    if radius == 0 {
        return canvas.fill_rect(rect).map_err(|e| anyhow!(e));
    }
    canvas
        .fill_rect(Rect::new(
            rect.x(),
            rect.y() + radius as i32,
            rect.width(),
            rect.height() - 2 * radius,
        ))
        .map_err(|e| anyhow!(e))?;
    for y in 0..radius {
        let dy = radius as f32 - y as f32 - 0.5;
        let inset = (radius as f32 - (radius.pow(2) as f32 - dy * dy).sqrt()) as i32;
        for line in [rect.y() + y as i32, rect.bottom() - 1 - y as i32] {
            canvas
                .draw_line((rect.x() + inset, line), (rect.right() - 1 - inset, line))
                .map_err(|e| anyhow!(e))?;
        }
    }
    Ok(())
}

pub fn panel(canvas: &mut Canvas<Window>, rect: Rect, selected: bool) -> Result<()> {
    rounded(
        canvas,
        Rect::new(rect.x(), rect.y() + 5, rect.width(), rect.height()),
        14,
        Color::RGB(6, 12, 22),
    )?;
    rounded(canvas, rect, 14, if selected { ACCENT } else { BORDER })?;
    let inset = if selected { 2 } else { 1 };
    rounded(
        canvas,
        Rect::new(
            rect.x() + inset,
            rect.y() + inset,
            rect.width() - 2 * inset as u32,
            rect.height() - 2 * inset as u32,
        ),
        13,
        if selected {
            Color::RGB(30, 55, 66)
        } else {
            SURFACE
        },
    )
}

pub fn progress(
    canvas: &mut Canvas<Window>,
    rect: Rect,
    fraction: f64,
    color: Color,
) -> Result<()> {
    rounded(canvas, rect, rect.height() / 2, BORDER)?;
    let width = (rect.width() as f64 * fraction.clamp(0.0, 1.0)) as u32;
    if width > 0 {
        rounded(
            canvas,
            Rect::new(rect.x(), rect.y(), width, rect.height()),
            rect.height() / 2,
            color,
        )?;
    }
    Ok(())
}

fn contained(size: (u32, u32), rect: Rect) -> Rect {
    let scale = (rect.width() as f32 / size.0.max(1) as f32)
        .min(rect.height() as f32 / size.1.max(1) as f32);
    let w = ((size.0 as f32 * scale) as u32).max(1);
    let h = ((size.1 as f32 * scale) as u32).max(1);
    Rect::new(
        rect.x() + (rect.width() - w) as i32 / 2,
        rect.y() + (rect.height() - h) as i32 / 2,
        w,
        h,
    )
}

pub struct Painter<'a> {
    creator: &'a TextureCreator<WindowContext>,
    font: Font,
    language: Language,
    background: Texture<'a>,
    texts: HashMap<(String, u32), CachedText<'a>>,
    text_density: u32,
    text_cache_bytes: usize,
    art: Option<Texture<'a>>,
    preview: Option<(String, Texture<'a>)>,
    marquee: Option<(String, Texture<'a>)>,
    thumbnails: HashMap<String, Texture<'a>>,
}

impl<'a> Painter<'a> {
    pub fn new(
        creator: &'a TextureCreator<WindowContext>,
        font: &Path,
        language: Language,
    ) -> Result<Self> {
        let font = crate::fonts::load_font(font, language)?;
        // Bake the atmospheric gradient and dot grid once, not on every frame.
        let mut pixels = Vec::with_capacity(1280 * 720 * 4);
        for y in 0_i32..720 {
            for x in 0_i32..1280 {
                let glow =
                    (1.0 - ((x - 1100).pow(2) + (y - 100).pow(2)) as f32 / 900_000.0).max(0.0);
                let dot = if x % 32 == 0 && y % 32 == 0 { 9 } else { 0 };
                pixels.extend_from_slice(&[
                    BG.r + (glow * 3.0) as u8 + dot,
                    BG.g + (glow * 12.0) as u8 + dot,
                    BG.b + (glow * 15.0) as u8 + dot,
                    255,
                ]);
            }
        }
        let mut background = creator.create_texture_static(PixelFormatEnum::RGBA32, 1280, 720)?;
        background.update(None, &pixels, 1280 * 4)?;
        Ok(Self {
            creator,
            font,
            language,
            background,
            texts: HashMap::new(),
            text_density: 2,
            text_cache_bytes: 0,
            art: None,
            preview: None,
            marquee: None,
            thumbnails: HashMap::new(),
        })
    }

    pub fn background(&mut self, canvas: &mut Canvas<Window>) -> Result<()> {
        let density = text_density(canvas.output_size().map_err(|e| anyhow!(e))?);
        if density != self.text_density {
            self.text_density = density;
            self.texts.clear();
            self.text_cache_bytes = 0;
        }
        // Clear the physical output too, including letterboxing after a resize.
        canvas.set_draw_color(BG);
        canvas.clear();
        canvas
            .copy(&self.background, None, None)
            .map_err(|e| anyhow!(e))
    }

    fn image_texture(&self, art: image::RgbaImage) -> Result<Texture<'a>> {
        let mut texture = self.creator.create_texture_static(
            PixelFormatEnum::RGBA32,
            art.width(),
            art.height(),
        )?;
        texture.update(None, art.as_raw(), art.width() as usize * 4)?;
        texture.set_blend_mode(BlendMode::Blend);
        Ok(texture)
    }

    pub fn set_marquee(&mut self, id: String, art: image::RgbaImage) -> Result<()> {
        self.marquee = Some((id, self.image_texture(art)?));
        Ok(())
    }

    pub fn marquee(&self, canvas: &mut Canvas<Window>, id: &str, rect: Rect) -> Result<bool> {
        if let Some((_, texture)) = self.marquee.as_ref().filter(|(key, _)| key == id) {
            let size = texture.query();
            canvas
                .copy(texture, None, contained((size.width, size.height), rect))
                .map_err(|e| anyhow!(e))?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn pill(
        &mut self,
        canvas: &mut Canvas<Window>,
        label: &str,
        point: (i32, i32),
        color: Color,
    ) -> Result<u32> {
        self.pill_bounded(canvas, label, point, color, u32::MAX)
    }

    pub fn pill_bounded(
        &mut self,
        canvas: &mut Canvas<Window>,
        label: &str,
        point: (i32, i32),
        color: Color,
        max_width: u32,
    ) -> Result<u32> {
        let available = (1240 - point.0).max(24) as u32;
        let width = (self.width(label, 14).ceil() as u32 + 24)
            .min(available)
            .min(max_width.max(24));
        rounded(
            canvas,
            Rect::new(point.0, point.1, width, 28),
            8,
            Color::RGB(32, 48, 63),
        )?;
        self.text(
            canvas,
            label,
            (point.0 + 12, point.1 + 4),
            14,
            color,
            width - 24,
        )?;
        Ok(width)
    }

    pub fn hint(
        &mut self,
        canvas: &mut Canvas<Window>,
        key: &str,
        label: &str,
        x: i32,
    ) -> Result<i32> {
        let width = self.width(key, 13).ceil() as u32 + 18;
        rounded(canvas, Rect::new(x, 674, width, 26), 6, BORDER)?;
        self.text(canvas, key, (x + 9, 679), 13, TEXT, width - 18)?;
        self.text(canvas, label, (x + width as i32 + 9, 678), 16, MUTED, 240)?;
        Ok(x + width as i32 + self.width(label, 16).ceil() as i32 + 34)
    }

    pub fn placeholder(
        &mut self,
        canvas: &mut Canvas<Window>,
        rect: Rect,
        label: &str,
    ) -> Result<()> {
        rounded(canvas, rect, 9, Color::RGB(15, 25, 39))?;
        let cx = rect.x() + rect.width() as i32 / 2;
        let cy = rect.y() + rect.height() as i32 / 2 - 12;
        rounded(canvas, Rect::new(cx - 23, cy - 25, 46, 48), 6, BORDER)?;
        rounded(canvas, Rect::new(cx - 14, cy - 17, 28, 17), 2, SURFACE)?;
        rounded(canvas, Rect::new(cx - 12, cy + 10, 24, 5), 1, MUTED)?;
        let width = self.width(label, 13).ceil() as i32;
        self.text(
            canvas,
            label,
            (cx - width / 2, cy + 36),
            13,
            MUTED,
            rect.width().saturating_sub(12),
        )
    }

    pub(super) fn width(&self, text: &str, size: u32) -> f32 {
        let mut previous = None;
        text.chars()
            .map(|c| {
                let (kern, width) = advance(&self.font, previous, c, size);
                previous = Some(c);
                kern + width
            })
            .sum()
    }

    pub fn text(
        &mut self,
        canvas: &mut Canvas<Window>,
        text: &str,
        point: (i32, i32),
        size: u32,
        color: Color,
        max_width: u32,
    ) -> Result<()> {
        if max_width == 0 {
            return Ok(());
        }
        let mut text = text
            .chars()
            .take(4096)
            .collect::<String>()
            .replace(['\n', '\r', '\t'], " ");
        if self.width(&text, size) > max_width as f32 {
            let mut shortened = String::new();
            let mut width = self.width("…", size);
            if width > max_width as f32 {
                return Ok(());
            }
            let mut previous = None;
            for c in text.chars() {
                let (kern, step) = advance(&self.font, previous, c, size);
                width += kern + step;
                if width > max_width as f32 {
                    break;
                }
                shortened.push(c);
                previous = Some(c);
            }
            text = shortened + "…";
        }
        if text.is_empty() {
            return Ok(());
        }
        let key = (text, size);
        if !self.texts.contains_key(&key) {
            let bitmap = rasterize_text(&self.font, &key.0, size, self.text_density);
            let bytes = bitmap.pixels.len();
            if self.texts.len() >= MAX_TEXT_CACHE_ENTRIES
                || self.text_cache_bytes + bytes > MAX_TEXT_CACHE_BYTES
            {
                self.texts.clear();
                self.text_cache_bytes = 0;
            }
            let mut texture = self.creator.create_texture_static(
                PixelFormatEnum::RGBA32,
                bitmap.width,
                bitmap.height,
            )?;
            texture.update(None, &bitmap.pixels, bitmap.width as usize * 4)?;
            texture.set_blend_mode(BlendMode::Blend);
            texture.set_scale_mode(ScaleMode::Linear);
            self.texts.insert(
                key.clone(),
                CachedText {
                    texture,
                    width: bitmap.width as f32 / self.text_density as f32,
                    height: bitmap.height as f32 / self.text_density as f32,
                    offset: bitmap.offset,
                },
            );
            self.text_cache_bytes += bytes;
        }
        let cached = self.texts.get_mut(&key).context("Texte non rendu")?;
        cached.texture.set_color_mod(color.r, color.g, color.b);
        canvas
            .copy_f(
                &cached.texture,
                None,
                FRect::new(
                    point.0 as f32 + cached.offset.0,
                    point.1 as f32 + cached.offset.1,
                    cached.width,
                    cached.height,
                ),
            )
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    pub fn paragraph(
        &mut self,
        canvas: &mut Canvas<Window>,
        text: &str,
        point: (i32, i32),
        width: u32,
        lines: usize,
    ) -> Result<()> {
        if lines == 0 {
            return Ok(());
        }
        let mut line = String::new();
        let mut y = point.1;
        let mut count = 0;
        for word in text.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if self.width(&candidate, 22) > width as f32 && !line.is_empty() {
                if count + 1 == lines {
                    line.push('…');
                }
                self.text(canvas, &line, (point.0, y), 22, MUTED, width)?;
                y += 31;
                count += 1;
                if count >= lines {
                    return Ok(());
                }
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if count < lines {
            self.text(canvas, &line, (point.0, y), 22, MUTED, width)?;
        }
        Ok(())
    }

    pub fn set_art(&mut self, art: Option<image::RgbaImage>) -> Result<()> {
        self.art = if let Some(art) = art {
            let mut texture = self.creator.create_texture_static(
                PixelFormatEnum::RGBA32,
                art.width(),
                art.height(),
            )?;
            texture.update(None, art.as_raw(), art.width() as usize * 4)?;
            texture.set_blend_mode(BlendMode::Blend);
            Some(texture)
        } else {
            None
        };
        Ok(())
    }

    pub fn retain_thumbnails(&mut self, ids: &[String]) {
        self.thumbnails.retain(|id, _| ids.contains(id));
    }

    pub fn set_preview(&mut self, id: String, art: image::RgbaImage) -> Result<()> {
        let mut texture = self.creator.create_texture_static(
            PixelFormatEnum::RGBA32,
            art.width(),
            art.height(),
        )?;
        texture.update(None, art.as_raw(), art.width() as usize * 4)?;
        texture.set_blend_mode(BlendMode::Blend);
        self.preview = Some((id, texture));
        Ok(())
    }

    pub fn clear_preview(&mut self) {
        self.preview = None;
    }

    pub fn preview(
        &mut self,
        canvas: &mut Canvas<Window>,
        id: &str,
        rect: Rect,
        label: &str,
    ) -> Result<()> {
        if let Some((_, texture)) = self.preview.as_ref().filter(|(key, _)| key == id) {
            let size = texture.query();
            canvas
                .copy(texture, None, contained((size.width, size.height), rect))
                .map_err(|e| anyhow!(e))?;
        } else {
            self.placeholder(canvas, rect, label)?;
        }
        Ok(())
    }

    pub fn set_thumbnail(&mut self, id: String, art: image::RgbaImage) -> Result<()> {
        let mut texture = self.creator.create_texture_static(
            PixelFormatEnum::RGBA32,
            art.width(),
            art.height(),
        )?;
        texture.update(None, art.as_raw(), art.width() as usize * 4)?;
        texture.set_blend_mode(BlendMode::Blend);
        self.thumbnails.insert(id, texture);
        Ok(())
    }

    pub fn thumbnail(&mut self, canvas: &mut Canvas<Window>, id: &str, rect: Rect) -> Result<()> {
        rounded(canvas, rect, 9, Color::RGB(15, 25, 39))?;
        if let Some(texture) = self.thumbnails.get(id) {
            let size = texture.query();
            canvas
                .copy(texture, None, contained((size.width, size.height), rect))
                .map_err(|e| anyhow!(e))?;
        } else {
            self.placeholder(canvas, rect, self.language.text("cover_placeholder"))?;
        }
        Ok(())
    }

    pub fn cover_tile(
        &mut self,
        canvas: &mut Canvas<Window>,
        id: &str,
        rect: Rect,
        selected: bool,
    ) -> Result<()> {
        if let Some(texture) = self.thumbnails.get(id) {
            let size = texture.query();
            let target = contained((size.width, size.height), rect);
            panel(
                canvas,
                Rect::new(
                    target.x() - 4,
                    target.y() - 4,
                    target.width() + 8,
                    target.height() + 8,
                ),
                selected,
            )?;
            canvas.copy(texture, None, target).map_err(|e| anyhow!(e))?;
        } else {
            let target = contained((2, 3), rect);
            panel(
                canvas,
                Rect::new(
                    target.x() - 4,
                    target.y() - 4,
                    target.width() + 8,
                    target.height() + 8,
                ),
                selected,
            )?;
            self.placeholder(canvas, target, "—")?;
        }
        Ok(())
    }

    pub fn art(&mut self, canvas: &mut Canvas<Window>, rect: Rect) -> Result<()> {
        if let Some(art) = &self.art {
            let size = art.query();
            canvas
                .copy(art, None, contained((size.width, size.height), rect))
                .map_err(|e| anyhow!(e))?;
        } else {
            self.placeholder(canvas, rect, self.language.text("cover_placeholder"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_density_tracks_native_resolution_and_ignores_letterboxing() {
        for (output, expected) in [
            ((640, 480), 2),
            ((1280, 720), 2),
            ((1920, 1080), 2),
            ((2560, 1440), 2),
            ((3840, 2160), 3),
            ((5120, 2160), 3),
            ((2560, 720), 2),
            ((1280, 2160), 2),
            ((7680, 4320), 6),
            ((0, 0), 2),
        ] {
            assert_eq!(text_density(output), expected, "{output:?}");
        }
    }

    #[test]
    fn high_resolution_text_preserves_size_accents_and_smooth_transparent_edges() {
        let font = crate::fonts::load_font(
            Path::new("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
            Language::Fr,
        )
        .unwrap();
        for size in [13, 16, 22, 34] {
            let mut reference: Option<f32> = None;
            for density in [2, 3, 6] {
                let bitmap =
                    rasterize_text(&font, "Été — bibliothèque, déjà installé !", size, density);
                assert_eq!(
                    bitmap.pixels.len(),
                    bitmap.width as usize * bitmap.height as usize * 4
                );
                assert!(bitmap
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| p[..3] == [255, 255, 255]));
                assert!(bitmap
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[3] > 0 && p[3] < 255));
                assert!(bitmap.pixels.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
                // Padding prevents filtering from clipping accents and descenders.
                for x in 0..bitmap.width as usize {
                    assert_eq!(bitmap.pixels[x * 4 + 3], 0);
                    assert_eq!(
                        bitmap.pixels
                            [((bitmap.height as usize - 1) * bitmap.width as usize + x) * 4 + 3],
                        0
                    );
                }
                for y in 0..bitmap.height as usize {
                    assert_eq!(bitmap.pixels[(y * bitmap.width as usize) * 4 + 3], 0);
                    assert_eq!(
                        bitmap.pixels
                            [(y * bitmap.width as usize + bitmap.width as usize - 1) * 4 + 3],
                        0
                    );
                }
                let logical_width = bitmap.width as f32 / density as f32;
                if let Some(width) = reference {
                    assert!(
                        (logical_width - width).abs() < 2.0,
                        "Font size {size} changed at density {density}: {logical_width} vs {width}"
                    );
                } else {
                    reference = Some(logical_width);
                }
            }
        }
    }

    #[test]
    #[ignore = "Run with SDL_VIDEODRIVER=dummy and --ignored for the headless renderer"]
    fn sdl_text_rendering_refreshes_density_on_resize_and_bounds_the_cache() {
        assert_eq!(std::env::var("SDL_VIDEODRIVER").as_deref(), Ok("dummy"));
        let sdl = sdl2::init().unwrap();
        let video = sdl.video().unwrap();
        let window = video
            .window("Text rendering test", 1280, 720)
            .hidden()
            .allow_highdpi()
            .build()
            .unwrap();
        let mut canvas = window.into_canvas().software().build().unwrap();
        canvas.set_logical_size(1280, 720).unwrap();
        let creator = canvas.texture_creator();
        let mut painter = Painter::new(
            &creator,
            Path::new("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
            Language::Fr,
        )
        .unwrap();
        for (width, height) in [(1280, 720), (1920, 1080), (3840, 2160), (1280, 720)] {
            canvas.window_mut().set_size(width, height).unwrap();
            let old_density = painter.text_density;
            painter.background(&mut canvas).unwrap();
            assert_eq!(painter.text_density, text_density((width, height)));
            if old_density != painter.text_density {
                assert!(painter.texts.is_empty());
                assert_eq!(painter.text_cache_bytes, 0);
            }
            for (row, size) in [13, 16, 22, 34].into_iter().enumerate() {
                painter
                    .text(
                        &mut canvas,
                        "Été — bibliothèque : jeux déjà installés, à télécharger…",
                        (40, 40 + row as i32 * 60),
                        size,
                        TEXT,
                        1180,
                    )
                    .unwrap();
            }
            assert_eq!(painter.texts.len(), 4);
            assert!(painter
                .texts
                .values()
                .all(|cached| cached.texture.scale_mode() == ScaleMode::Linear));
            assert!(painter.text_cache_bytes < MAX_TEXT_CACHE_BYTES);
            if let Some(directory) = std::env::var_os("ROMSTORE_TEXT_SCREENSHOTS") {
                std::fs::create_dir_all(&directory).unwrap();
                let pixels = canvas.read_pixels(None, PixelFormatEnum::RGBA32).unwrap();
                image::save_buffer(
                    Path::new(&directory).join(format!("text-{width}x{height}.png")),
                    &pixels,
                    width,
                    height,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
            canvas.present();
        }
        painter.text_cache_bytes = MAX_TEXT_CACHE_BYTES;
        painter
            .text(&mut canvas, "New cache entry", (40, 400), 22, TEXT, 1180)
            .unwrap();
        assert_eq!(painter.texts.len(), 1);
        assert!(painter.text_cache_bytes < MAX_TEXT_CACHE_BYTES);
    }

    #[test]
    fn artwork_is_centered_without_changing_its_aspect_ratio() {
        let target = Rect::new(10, 20, 200, 100);
        assert_eq!(contained((100, 100), target), Rect::new(60, 20, 100, 100));
        assert_eq!(contained((400, 100), target), Rect::new(10, 45, 200, 50));
        assert_eq!(contained((100, 400), target), Rect::new(97, 20, 25, 100));
    }

    #[test]
    fn extreme_artwork_ratios_still_produce_nonempty_visible_rectangles() {
        let target = Rect::new(40, 100, 212, 124);
        for size in [(1, 4096), (4096, 1), (4096, 4096)] {
            let fitted = contained(size, target);
            assert!(target.contains_rect(fitted));
            assert!(fitted.width() > 0 && fitted.height() > 0);
        }
    }

    #[test]
    fn detail_mix_images_fit_completely_without_cropping_or_stretching() {
        let target = Rect::new(58, 202, 624, 400);
        for size in [(800, 600), (1920, 1080), (600, 800), (600, 600)] {
            let fitted = contained(size, target);
            assert!(target.contains_rect(fitted));
            let ratio = fitted.width() as f64 / fitted.height() as f64;
            assert!((ratio - size.0 as f64 / size.1 as f64).abs() < 0.01);
        }
    }
}
