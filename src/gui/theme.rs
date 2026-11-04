//! 로고 색으로 만든 라이트·다크 테마. (FR-UI-3, FR-GUI-21)

use eframe::egui::{Color32, CornerRadius, Stroke, ThemePreference, Visuals};

use crate::config::Theme;

const CREAM: Color32 = Color32::from_rgb(0xFD, 0xF6, 0xEC);
const NAVY: Color32 = Color32::from_rgb(0x3B, 0x36, 0x54);
const NIGHT: Color32 = Color32::from_rgb(0x23, 0x20, 0x2F);
const CREAM_TEXT: Color32 = Color32::from_rgb(0xF3, 0xEE, 0xE6);
const WIDGET_RADIUS: u8 = 8;
const WINDOW_RADIUS: u8 = 12;

/// 테마마다 다른 색. 배경과 구분되는 밝기로 정한다.
struct Tones {
    bg: Color32,
    text: Color32,
    weak_text: Color32,
    field: Color32,
    faint: Color32,
    border: Color32,
    idle: Color32,
    hover: Color32,
    press: Color32,
    accent: Color32,
    link: Color32,
}

const LIGHT: Tones = Tones {
    bg: CREAM,
    text: NAVY,
    weak_text: Color32::from_rgb(0x6E, 0x67, 0x86),
    field: Color32::from_rgb(0xFF, 0xFD, 0xF9),
    faint: Color32::from_rgb(0xF6, 0xEE, 0xE3),
    border: Color32::from_rgb(0xE0, 0xD6, 0xEC),
    idle: Color32::from_rgb(0xEF, 0xE8, 0xF7),
    hover: Color32::from_rgb(0xE4, 0xDC, 0xF4),
    press: Color32::from_rgb(0xD3, 0xC9, 0xEE),
    accent: Color32::from_rgb(0xD9, 0xD1, 0xF2),
    link: Color32::from_rgb(0x5B, 0x4B, 0xA6),
};

const DARK: Tones = Tones {
    bg: NIGHT,
    text: CREAM_TEXT,
    weak_text: Color32::from_rgb(0xA9, 0xA2, 0xBD),
    field: Color32::from_rgb(0x1A, 0x18, 0x24),
    faint: Color32::from_rgb(0x2B, 0x27, 0x39),
    border: Color32::from_rgb(0x3E, 0x39, 0x52),
    idle: Color32::from_rgb(0x33, 0x2E, 0x45),
    hover: Color32::from_rgb(0x3D, 0x37, 0x53),
    press: Color32::from_rgb(0x4A, 0x43, 0x63),
    accent: Color32::from_rgb(0xB9, 0xAE, 0xE6),
    link: Color32::from_rgb(0xC8, 0xBE, 0xF2),
};

/// 상태 색: 실행 중, 경고, 변경, 오류.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub ok: Color32,
    pub warn: Color32,
    pub changed: Color32,
    pub error: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            ok: Color32::from_rgb(0x7B, 0xD3, 0x9A),
            warn: Color32::from_rgb(0xF2, 0xB4, 0x5A),
            changed: Color32::from_rgb(0x8F, 0xB3, 0xF5),
            error: Color32::from_rgb(0xF0, 0x8A, 0x8A),
        }
    } else {
        Palette {
            ok: Color32::from_rgb(0x2F, 0x86, 0x52),
            warn: Color32::from_rgb(0xA8, 0x5F, 0x12),
            changed: Color32::from_rgb(0x3E, 0x6A, 0xC8),
            error: Color32::from_rgb(0xBF, 0x37, 0x4B),
        }
    }
}

/// 설정값에 맞는 egui 테마 선택.
pub fn preference(theme: Theme) -> ThemePreference {
    match theme {
        Theme::System => ThemePreference::System,
        Theme::Light => ThemePreference::Light,
        Theme::Dark => ThemePreference::Dark,
    }
}

/// 로고 색으로 만든 화면 색과 모양.
pub fn visuals(dark: bool) -> Visuals {
    let (t, mut v) = if dark {
        (&DARK, Visuals::dark())
    } else {
        (&LIGHT, Visuals::light())
    };
    let p = palette(dark);
    v.panel_fill = t.bg;
    v.window_fill = t.bg;
    v.extreme_bg_color = t.field;
    v.text_edit_bg_color = Some(t.field);
    v.faint_bg_color = t.faint;
    v.code_bg_color = t.faint;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.hyperlink_color = t.link;
    v.error_fg_color = p.error;
    v.warn_fg_color = p.warn;
    v.selection.bg_fill = t.accent;
    v.selection.stroke = Stroke::new(1.0, NAVY);
    v.window_corner_radius = CornerRadius::same(WINDOW_RADIUS);
    v.menu_corner_radius = CornerRadius::same(WIDGET_RADIUS);
    let w = &mut v.widgets;
    for (state, fill) in [
        (&mut w.noninteractive, t.bg),
        (&mut w.inactive, t.idle),
        (&mut w.hovered, t.hover),
        (&mut w.active, t.press),
        (&mut w.open, t.press),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.fg_stroke = Stroke::new(state.fg_stroke.width, t.text);
        state.corner_radius = CornerRadius::same(WIDGET_RADIUS);
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    w.inactive.bg_stroke = Stroke::NONE;
    w.hovered.bg_stroke = Stroke::new(1.0, t.accent);
    w.active.bg_stroke = Stroke::new(1.0, t.accent);
    v.weak_text_color = Some(t.weak_text);
    v
}

/// WCAG 명도 대비(1~21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

fn luminance(c: Color32) -> f32 {
    let channel = |v: u8| {
        let s = f32::from(v) / 255.0;
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}
