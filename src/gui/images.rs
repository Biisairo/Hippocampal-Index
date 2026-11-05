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

/// README의 상대 링크를 바꿀 저장소 주소. (FR-GUI-11)
pub const REPO_BLOB_URL: &str = "https://github.com/Biisairo/Hippocampal-Index/blob/main/";
const FENCE: &str = "```";
const KEPT_LINK_PREFIXES: [&str; 4] = ["http://", "https://", "mailto:", "#"];

/// 마크다운으로 보여 줄 README. 이미지를 빼고, 상대 링크를 저장소 주소로 바꾼다.
/// 코드 블록과 코드 글자(`` ` ``) 안은 바꾸지 않는다. (FR-GUI-11)
pub fn readme_markdown(markdown: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in readme_text(markdown).lines() {
        if line.trim_start().starts_with(FENCE) {
            fenced = !fenced;
            out.push_str(line);
        } else if fenced {
            out.push_str(line);
        } else {
            for (i, part) in line.split('`').enumerate() {
                if i > 0 {
                    out.push('`');
                }
                if i % 2 == 0 {
                    out.push_str(&absolute_links(part));
                } else {
                    out.push_str(part);
                }
            }
        }
        out.push('\n');
    }
    out
}

/// `](대상)`의 대상이 상대 경로이면 앞에 저장소 주소를 붙인다.
fn absolute_links(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("](") {
        let (head, tail) = rest.split_at(at + 2);
        out.push_str(head);
        if !KEPT_LINK_PREFIXES.iter().any(|p| tail.starts_with(p)) {
            out.push_str(REPO_BLOB_URL);
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}
