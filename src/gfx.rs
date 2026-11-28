//! Drawing helpers for the notch: icons (vector, from SVG path data), text and album art.

use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::core::BOOL;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows_numerics::{Matrix3x2, Vector2};

pub fn rgb(hex: u32, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a: alpha,
    }
}

pub fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Previous,
    Next,
    Play,
    Pause,
    Shuffle,
    Repeat,
    Note,
    Mic,
    Camera,
    Bolt,
    ChevronLeft,
    ChevronRight,
    Speaker,
    Mute,
    Sun,
    Headphones,
    ChevronDown,
    Check,
    Wifi,
    Bluetooth,
    Refresh,
    Lock,
    Pin,
    PinSolid,
    ArrowDown,
    Close,
    Gear,
    Grip,
}

/// Icons as 24×24 SVG-style paths (same shapes as the design).
/// `true` = filled, `false` = drawn as a 2px line.
const ICONS: [(Icon, bool, &str); 28] = [
    (Icon::Previous, true, "M19 20 L9 12 L19 4 Z M4 4 H6.5 V20 H4 Z"),
    (Icon::Next, true, "M5 4 L15 12 L5 20 Z M17.5 4 H20 V20 H17.5 Z"),
    (Icon::Play, true, "M7 4 L20 12 L7 20 Z"),
    (Icon::Pause, true, "M6 4 H10 V20 H6 Z M14 4 H18 V20 H14 Z"),
    (Icon::Shuffle, false, "M16 3 H21 V8 M4 20 L21 3 M21 16 V21 H16 M15 15 L21 21 M4 4 L9 9"),
    (
        Icon::Repeat,
        false,
        "M17 1 L21 5 L17 9 M3 11 V9 A4 4 0 0 1 7 5 H21 M7 23 L3 19 L7 15 M21 13 V15 A4 4 0 0 1 17 19 H3",
    ),
    (
        Icon::Note,
        false,
        "M9 18 V5 L21 3 V16 M9 18 A3 3 0 1 1 3 18 A3 3 0 1 1 9 18 M21 16 A3 3 0 1 1 15 16 A3 3 0 1 1 21 16",
    ),
    (
        Icon::Mic,
        false,
        "M12 2 A3 3 0 0 0 9 5 V12 A3 3 0 0 0 15 12 V5 A3 3 0 0 0 12 2 Z M19 10 V12 A7 7 0 0 1 5 12 V10 M12 19 V22",
    ),
    (Icon::Bolt, true, "M13 2 L4 14 H11 L10 22 L20 10 H13 Z"),
    (Icon::ChevronLeft, false, "M15 5 L8 12 L15 19"),
    (Icon::ChevronRight, false, "M9 5 L16 12 L9 19"),
    (
        Icon::Camera,
        false,
        "M16 10 L22 6 V18 L16 14 M4 6 H14 A2 2 0 0 1 16 8 V16 A2 2 0 0 1 14 18 H4 A2 2 0 0 1 2 16 V8 A2 2 0 0 1 4 6 Z",
    ),
    (Icon::Speaker, false, "M11 5 L6 9 H2 V15 H6 L11 19 Z M15.5 8.5 A5 5 0 0 1 15.5 15.5 M19 5 A10 10 0 0 1 19 19"),
    (Icon::Mute, false, "M11 5 L6 9 H2 V15 H6 L11 19 Z M22 9 L16 15 M16 9 L22 15"),
    (
        Icon::Sun,
        false,
        "M12 8 A4 4 0 1 1 12 16 A4 4 0 1 1 12 8 Z M12 2 V4 M12 20 V22 M4.9 4.9 L6.3 6.3 M17.7 17.7 L19.1 19.1 M2 12 H4 M20 12 H22 M4.9 19.1 L6.3 17.7 M17.7 6.3 L19.1 4.9",
    ),
    (
        Icon::Headphones,
        false,
        "M3 18 V12 A9 9 0 0 1 21 12 V18 M21 19 A2 2 0 0 1 19 21 H18 V15 H21 Z M3 19 A2 2 0 0 0 5 21 H6 V15 H3 Z",
    ),
    (Icon::ChevronDown, false, "M6 9 L12 15 L18 9"),
    (Icon::Check, false, "M20 6 L9 17 L4 12"),
    (
        Icon::Wifi,
        false,
        "M2 9 A15 15 0 0 1 22 9 M5.5 12.5 A10 10 0 0 1 18.5 12.5 M8.8 16 A5 5 0 0 1 15.2 16 M11.9 19.5 L12.1 19.5",
    ),
    (Icon::Bluetooth, false, "M7 7 L17 17 L12 22 V2 L17 7 L7 17"),
    (Icon::Refresh, false, "M21 12 A9 9 0 1 1 18.4 5.6 M21 3 V9 H15"),
    (Icon::Lock, false, "M8 11 V7 A4 4 0 0 1 16 7 V11 M5 11 H19 V21 H5 Z"),
    (Icon::Pin, false, "M8 3 H16 M9.5 3 V10 L6.5 14 H17.5 L14.5 10 V3 M12 14 V21"),
    // Same pin, filled in (shown while pinned).
    (Icon::PinSolid, true, "M7 2 H17 V4 H15.5 V9.6 L18.8 15 H5.2 L8.5 9.6 V4 H7 Z M11 15 H13 V22 H11 Z"),
    (Icon::ArrowDown, false, "M12 4 V15 M7 10 L12 15 L17 10 M5 20 H19"),
    (Icon::Close, false, "M6 6 L18 18 M18 6 L6 18"),
    (
        Icon::Gear,
        false,
        "M22.4 10.5 L22.4 13.5 L19.7 14.0 L18.9 16.1 L20.4 18.3 L18.3 20.4 L16.1 18.9 L14.0 19.7 L13.5 22.4 L10.5 22.4 L10.0 19.7 L7.9 18.9 L5.7 20.4 L3.6 18.3 L5.1 16.1 L4.3 14.0 L1.6 13.5 L1.6 10.5 L4.3 10.0 L5.1 7.9 L3.6 5.7 L5.7 3.6 L7.9 5.1 L10.0 4.3 L10.5 1.6 L13.5 1.6 L14.0 4.3 L16.1 5.1 L18.3 3.6 L20.4 5.7 L18.9 7.9 L19.7 10.0 Z M12 8.5 A3.5 3.5 0 1 1 12 15.5 A3.5 3.5 0 1 1 12 8.5 Z",
    ),
    // Drag handle (two columns of three dots, as short strokes)
    (Icon::Grip, false, "M9 6 L9 6.1 M15 6 L15 6.1 M9 12 L9 12.1 M15 12 L15 12.1 M9 18 L9 18.1 M15 18 L15 18.1"),
];

pub struct Fonts {
    pub title: IDWriteTextFormat,
    pub artist: IDWriteTextFormat,
    /// The song on the closed notch while music plays.
    pub closed_song: IDWriteTextFormat,
    pub tab: IDWriteTextFormat,
    pub tab_active: IDWriteTextFormat,
    pub time: IDWriteTextFormat,
    pub time_right: IDWriteTextFormat,
    pub badge: IDWriteTextFormat,
    pub clock: IDWriteTextFormat,
    pub clock_day: IDWriteTextFormat,
    pub clock_big: IDWriteTextFormat,
    pub marker: IDWriteTextFormat,
    pub date: IDWriteTextFormat,
    pub date_day: IDWriteTextFormat,
    pub tile_day: IDWriteTextFormat,
    pub tile_number: IDWriteTextFormat,
    pub battery_big: IDWriteTextFormat,
    pub battery_status: IDWriteTextFormat,
    pub caption: IDWriteTextFormat,
    pub bubble_title: IDWriteTextFormat,
    pub bubble_text: IDWriteTextFormat,
    /// Controls page / volume HUD
    pub pill: IDWriteTextFormat,
    pub percent: IDWriteTextFormat,
    pub kind: IDWriteTextFormat,
    /// System page
    pub ring_value: IDWriteTextFormat,
    pub label_center: IDWriteTextFormat,
    pub small_center: IDWriteTextFormat,
}

pub struct Gfx {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    family: HSTRING,
    pub fonts: Fonts,
    icons: Vec<(Icon, bool, ID2D1PathGeometry)>,
    round: ID2D1StrokeStyle,
    dashed: ID2D1StrokeStyle,
}

impl Gfx {
    pub unsafe fn new(factory: &ID2D1Factory, scale: f32) -> Result<Gfx> {
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let family = pick_family(&dwrite);
        let fonts = make_fonts(&dwrite, &family, scale)?;
        let mut icons = Vec::new();
        for (icon, filled, d) in ICONS {
            icons.push((icon, filled, svg_path(factory, d, filled)?));
        }
        let round = factory.CreateStrokeStyle(
            &D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            },
            None,
        )?;
        let dashed = factory.CreateStrokeStyle(
            &D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_CUSTOM,
                dashOffset: 0.0,
            },
            Some(&[3.0, 2.5]),
        )?;
        Ok(Gfx { factory: factory.clone(), dwrite, family, fonts, icons, round, dashed })
    }

    /// Text sizes follow the monitor's scaling.
    pub unsafe fn set_scale(&mut self, scale: f32) -> Result<()> {
        self.fonts = make_fonts(&self.dwrite, &self.family, scale)?;
        Ok(())
    }

    /// Draw an icon centred at (cx, cy), `size` px across.
    pub unsafe fn icon(&self, target: &ID2D1RenderTarget, icon: Icon, cx: f32, cy: f32, size: f32, brush: &ID2D1Brush) {
        let Some((_, filled, geometry)) = self.icons.iter().find(|(i, _, _)| *i == icon) else { return };
        // Icon placement, on top of whatever transform is active (e.g. a page sliding).
        let k = size / 24.0;
        let (tx, ty) = (cx - size / 2.0, cy - size / 2.0);
        let mut outer = Matrix3x2::identity();
        target.GetTransform(&mut outer);
        let m = Matrix3x2 {
            M11: k * outer.M11,
            M12: k * outer.M12,
            M21: k * outer.M21,
            M22: k * outer.M22,
            M31: tx * outer.M11 + ty * outer.M21 + outer.M31,
            M32: tx * outer.M12 + ty * outer.M22 + outer.M32,
        };
        target.SetTransform(&m);
        if *filled {
            target.FillGeometry(geometry, brush, None);
        } else {
            target.DrawGeometry(geometry, brush, 2.0, &self.round);
        }
        target.SetTransform(&outer);
    }

    /// A dashed rounded outline (drop zones).
    pub unsafe fn dashed_round_rect(&self, target: &ID2D1RenderTarget, r: D2D_RECT_F, radius: f32, width: f32, brush: &ID2D1Brush) {
        let shape = D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius };
        target.DrawRoundedRectangle(&shape, brush, width, &self.dashed);
    }

    /// A ring progress arc around (cx, cy), starting at 12 o'clock and going clockwise.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn arc(&self, target: &ID2D1RenderTarget, cx: f32, cy: f32, r: f32, width: f32, fraction: f32, brush: &ID2D1Brush) {
        let fraction = fraction.clamp(0.0, 1.0);
        if fraction <= 0.001 {
            return;
        }
        if fraction >= 0.999 {
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
            target.DrawEllipse(&e, brush, width, &self.round);
            return;
        }
        let angle = std::f32::consts::TAU * fraction - std::f32::consts::FRAC_PI_2;
        let Ok(geometry) = self.factory.CreatePathGeometry() else { return };
        let Ok(sink) = geometry.Open() else { return };
        sink.BeginFigure(Vector2 { X: cx, Y: cy - r }, D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: Vector2 { X: cx + r * angle.cos(), Y: cy + r * angle.sin() },
            size: D2D_SIZE_F { width: r, height: r },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: if fraction > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
        });
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        if sink.Close().is_ok() {
            target.DrawGeometry(&geometry, brush, width, &self.round);
        }
    }

    pub unsafe fn text(&self, target: &ID2D1RenderTarget, text: &str, format: &IDWriteTextFormat, r: D2D_RECT_F, brush: &ID2D1Brush) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        target.DrawText(&wide, format, &r, brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
    }

    /// Width a piece of text needs, in px.
    pub unsafe fn measure(&self, text: &str, format: &IDWriteTextFormat) -> f32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let Ok(layout) = self.dwrite.CreateTextLayout(&wide, format, 10_000.0, 1_000.0) else { return 0.0 };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        if layout.GetMetrics(&mut metrics).is_ok() {
            metrics.widthIncludingTrailingWhitespace
        } else {
            0.0
        }
    }
}

/// Upload premultiplied BGRA pixels as a Direct2D bitmap.
pub unsafe fn bitmap(target: &ID2D1RenderTarget, pixels: &[u8], size: u32) -> Result<ID2D1Bitmap> {
    target.CreateBitmap(
        D2D_SIZE_U { width: size, height: size },
        Some(pixels.as_ptr() as *const _),
        size * 4,
        &D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        },
    )
}

/// Draw a square bitmap into a rounded square.
pub unsafe fn rounded_image(target: &ID2D1RenderTarget, image: &ID2D1Bitmap, x: f32, y: f32, size: f32, radius: f32, opacity: f32) {
    let source = image.GetPixelSize().width.max(1) as f32;
    let Ok(brush) = target.CreateBitmapBrush(
        image,
        Some(&D2D1_BITMAP_BRUSH_PROPERTIES {
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
        }),
        None,
    ) else {
        return;
    };
    let k = size / source;
    brush.SetTransform(&Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: x, M32: y });
    brush.SetOpacity(opacity);
    let r = D2D1_ROUNDED_RECT { rect: rect(x, y, x + size, y + size), radiusX: radius, radiusY: radius };
    target.FillRoundedRectangle(&r, &brush);
}

unsafe fn pick_family(dwrite: &IDWriteFactory) -> HSTRING {
    let mut collection = None;
    if dwrite.GetSystemFontCollection(&mut collection, false).is_ok() {
        if let Some(collection) = collection {
            // Windows 11's UI font, else Windows 10's.
            for name in [w!("Segoe UI Variable Text"), w!("Segoe UI")] {
                let (mut index, mut exists) = (0u32, BOOL(0));
                if collection.FindFamilyName(name, &mut index, &mut exists).is_ok() && exists.as_bool() {
                    return HSTRING::from_wide(name.as_wide());
                }
            }
        }
    }
    HSTRING::from("Segoe UI")
}

unsafe fn make_fonts(dwrite: &IDWriteFactory, family: &HSTRING, s: f32) -> Result<Fonts> {
    let make = |size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT| -> Result<IDWriteTextFormat> {
        let format = dwrite.CreateTextFormat(
            PCWSTR(family.as_ptr()),
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size * s,
            w!("en-us"),
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        format.SetTextAlignment(align)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        // Long titles end in "…" instead of being cut mid-letter.
        let sign = dwrite.CreateEllipsisTrimmingSign(&format)?;
        let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
        format.SetTrimming(&trimming, &sign)?;
        Ok(format)
    };
    let leading = DWRITE_TEXT_ALIGNMENT_LEADING;
    Ok(Fonts {
        title: make(15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, leading)?,
        artist: make(13.0, DWRITE_FONT_WEIGHT_NORMAL, leading)?,
        closed_song: make(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, leading)?,
        tab: make(11.0, DWRITE_FONT_WEIGHT_MEDIUM, leading)?,
        tab_active: make(11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, leading)?,
        time: make(11.0, DWRITE_FONT_WEIGHT_NORMAL, leading)?,
        time_right: make(11.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
        badge: make(8.0, DWRITE_FONT_WEIGHT_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        clock: make(15.0, DWRITE_FONT_WEIGHT_BOLD, leading)?,
        clock_day: make(15.0, DWRITE_FONT_WEIGHT_BOLD, leading)?,
        clock_big: make(34.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, leading)?,
        marker: make(13.0, DWRITE_FONT_WEIGHT_MEDIUM, leading)?,
        date: make(13.0, DWRITE_FONT_WEIGHT_NORMAL, leading)?,
        date_day: make(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, leading)?,
        tile_day: make(11.0, DWRITE_FONT_WEIGHT_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        tile_number: make(25.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        battery_big: make(18.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
        battery_status: make(11.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
        caption: make(10.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        bubble_title: make(11.5, DWRITE_FONT_WEIGHT_SEMI_BOLD, leading)?,
        bubble_text: make(12.0, DWRITE_FONT_WEIGHT_NORMAL, leading)?,
        pill: make(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, leading)?,
        percent: make(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
        kind: make(9.0, DWRITE_FONT_WEIGHT_BOLD, leading)?,
        ring_value: make(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        label_center: make(11.0, DWRITE_FONT_WEIGHT_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        small_center: make(11.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER)?,
    })
}

/// Split SVG path data into commands and numbers. Commands may touch their first number
/// ("M19 20"), as is normal in SVG.
fn tokenize(d: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut number = String::new();
    for c in d.chars() {
        if c.is_ascii_alphabetic() {
            if !number.is_empty() {
                tokens.push(std::mem::take(&mut number));
            }
            tokens.push(c.to_string());
        } else if c.is_whitespace() || c == ',' {
            if !number.is_empty() {
                tokens.push(std::mem::take(&mut number));
            }
        } else {
            number.push(c);
        }
    }
    if !number.is_empty() {
        tokens.push(number);
    }
    tokens
}

/// Build a Direct2D path from a small subset of SVG path syntax:
/// absolute M, L, H, V, A and Z.
unsafe fn svg_path(factory: &ID2D1Factory, d: &str, filled: bool) -> Result<ID2D1PathGeometry> {
    let geometry = factory.CreatePathGeometry()?;
    let sink = geometry.Open()?;
    let begin = if filled { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW };

    let tokens = tokenize(d);
    let mut tokens = tokens.iter().map(|t| t.as_str());
    let mut open = false;
    let (mut x, mut y) = (0.0f32, 0.0f32);
    while let Some(command) = tokens.next() {
        let mut num = || tokens.next().and_then(|t| t.parse::<f32>().ok()).unwrap_or(0.0);
        match command {
            "M" => {
                if open {
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                }
                x = num();
                y = num();
                sink.BeginFigure(Vector2 { X: x, Y: y }, begin);
                open = true;
            }
            "L" => {
                x = num();
                y = num();
                sink.AddLine(Vector2 { X: x, Y: y });
            }
            "H" => {
                x = num();
                sink.AddLine(Vector2 { X: x, Y: y });
            }
            "V" => {
                y = num();
                sink.AddLine(Vector2 { X: x, Y: y });
            }
            "A" => {
                let (rx, ry, rotation, large, sweep) = (num(), num(), num(), num(), num());
                x = num();
                y = num();
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: Vector2 { X: x, Y: y },
                    size: D2D_SIZE_F { width: rx, height: ry },
                    rotationAngle: rotation,
                    sweepDirection: if sweep != 0.0 { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
                    arcSize: if large != 0.0 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                });
            }
            "Z" if open => {
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                open = false;
            }
            _ => {}
        }
    }
    if open {
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
    }
    sink.Close()?;
    Ok(geometry)
}

#[cfg(test)]
#[path = "tests/gfx.rs"]
mod tests;
