//! GUI에 넣는 이미지와 README 표시용 정리. (FR-GUI-11, FR-GUI-20)

use eframe::egui::{ColorImage, IconData};

pub const ICON: &[u8] = include_bytes!("../../assets/app/icon.png");
pub const ICON_SMALL: &[u8] = include_bytes!("../../assets/app/icon-small.png");
pub const LOGO: &[u8] = include_bytes!("../../assets/app/logo.png");
pub const LOGO_DARK: &[u8] = include_bytes!("../../assets/app/logo-dark.png");
pub const CHARACTER: &[u8] = include_bytes!("../../assets/app/character.png");

/// PNG를 해석한다. 해석하지 못하면 `None`.
pub fn decode_png(bytes: &[u8]) -> Option<ColorImage> {
    let icon = app_icon_from(bytes)?;
    let size = [
        usize::try_from(icon.width).ok()?,
        usize::try_from(icon.height).ok()?,
    ];
    Some(ColorImage::from_rgba_unmultiplied(size, &icon.rgba))
}

/// 창과 Dock 아이콘.
pub fn app_icon() -> Option<IconData> {
    app_icon_from(ICON)
}

fn app_icon_from(bytes: &[u8]) -> Option<IconData> {
    eframe::icon_data::from_png_bytes(bytes).ok()
}

/// 일반 텍스트로 보여 줄 README. 이미지만 담은 HTML 블록과 마크다운 이미지 줄을 뺀다.
pub fn readme_text(markdown: &str) -> String {
    let mut out = String::new();
    let mut closing: Option<&str> = None;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if let Some(end) = closing {
            if trimmed.contains(end) {
                closing = None;
            }
            continue;
        }
        let end = if trimmed.starts_with("<picture") {
            Some("</picture>")
        } else if trimmed.starts_with("<p>") || trimmed.starts_with("<p ") {
            Some("</p>")
        } else {
            None
        };
        if let Some(end) = end {
            if !trimmed.contains(end) {
                closing = Some(end);
            }
            continue;
        }
        if trimmed.starts_with("![") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}
