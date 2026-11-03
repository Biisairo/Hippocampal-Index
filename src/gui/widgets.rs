//! 화면 부품과 공통 스타일.

use eframe::egui::{
    self, Button, Color32, CornerRadius, FontId, Frame, Margin, RichText, TextEdit, TextStyle, Ui,
    vec2,
};

/// 폼의 라벨 열 너비.
pub const LABEL_WIDTH: f32 = 170.0;
const CARD_MARGIN: i8 = 12;
const CARD_RADIUS: u8 = 6;
const BUTTON_MIN: [f32; 2] = [96.0, 30.0];
const OUTPUT_MARGIN: i8 = 8;
const FORM_SPACING: [f32; 2] = [12.0, 10.0];
pub const FIELD_HEIGHT: f32 = 28.0;

/// 글자 크기, 간격, 버튼 높이를 정한다.
pub fn apply_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        let sizes = [
            (TextStyle::Heading, 18.0),
            (TextStyle::Body, 14.5),
            (TextStyle::Button, 14.5),
            (TextStyle::Monospace, 13.0),
            (TextStyle::Small, 12.0),
        ];
        for (text_style, size) in sizes {
            let family = if text_style == TextStyle::Monospace {
                FontId::monospace(size)
            } else {
                FontId::proportional(size)
            };
            style.text_styles.insert(text_style, family);
        }
        style.spacing.item_spacing = vec2(10.0, 8.0);
        style.spacing.button_padding = vec2(12.0, 5.0);
        style.spacing.interact_size.y = 28.0;
    });
}

/// 제목이 있는 테두리 영역. 가로로 남은 폭을 모두 쓴다.
pub fn card<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::group(ui.style())
        .inner_margin(Margin::same(CARD_MARGIN))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.label(RichText::new(title).strong().size(15.5));
                ui.add_space(4.0);
            }
            add(ui)
        })
        .inner
}

/// 라벨 열과 입력 열로 된 폼. `rows`는 입력 열의 폭을 받아 라벨 하나와 입력 하나씩을 그린다.
pub fn form(ui: &mut Ui, id: &str, rows: impl FnOnce(&mut Ui, f32)) {
    let width = (ui.available_width() - LABEL_WIDTH - FORM_SPACING[0]).max(LABEL_WIDTH);
    egui::Grid::new(id)
        .num_columns(2)
        .min_col_width(LABEL_WIDTH)
        .spacing(FORM_SPACING)
        .show(ui, |ui| rows(ui, width));
}

/// 폭을 고정한 한 줄 입력 칸.
pub fn text_field(ui: &mut Ui, width: f32, text: &mut String, hint: &str) -> egui::Response {
    ui.add_sized(
        [width, FIELD_HEIGHT],
        TextEdit::singleline(text).hint_text(hint),
    )
}

/// 강조한 주 버튼. 비활성이면 일반 버튼 모양으로 그린다.
pub fn primary(ui: &mut Ui, enabled: bool, text: String) -> egui::Response {
    let label = RichText::new(text).strong();
    let widget = if enabled {
        Button::new(label.color(Color32::WHITE)).fill(ui.visuals().selection.bg_fill)
    } else {
        Button::new(label)
    };
    ui.add_enabled(enabled, widget.min_size(vec2(BUTTON_MIN[0], BUTTON_MIN[1])))
}

pub fn button(text: String) -> Button<'static> {
    Button::new(text).min_size(vec2(BUTTON_MIN[0], BUTTON_MIN[1]))
}

/// 읽기 전용 고정폭 출력 칸. 남은 높이를 모두 쓴다. 비어 있으면 안내를 보여 준다.
pub fn output(ui: &mut Ui, id: &str, text: &str, empty_hint: &str) {
    Frame::default()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(OUTPUT_MARGIN))
        .show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt(id)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if text.is_empty() {
                        ui.weak(empty_hint);
                    } else {
                        ui.add(
                            TextEdit::multiline(&mut &*text)
                                .font(TextStyle::Monospace)
                                .frame(Frame::NONE)
                                .desired_width(f32::INFINITY),
                        );
                    }
                });
        });
}

/// 작은 상태 칸: 위에 이름, 아래에 값.
pub fn stat(ui: &mut Ui, label: &str, value: RichText) {
    Frame::group(ui.style())
        .inner_margin(Margin::symmetric(14, 8))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.weak(label);
                ui.label(value.size(16.0).strong());
            });
        });
}

/// 경고·안내 띠.
pub fn banner(ui: &mut Ui, color: Color32, add: impl FnOnce(&mut Ui)) {
    Frame::default()
        .fill(color.gamma_multiply(0.18))
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(add);
        });
}

/// 표의 칸에 넣는 한 줄 글. 넘치면 자르고, 마우스를 올리면 전체를 보여 준다.
pub fn cell(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(text).truncate());
}

pub fn mono_cell(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).monospace()).truncate());
}
