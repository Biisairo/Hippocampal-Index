//! CLI 삭제 확인 절차와 미리 보기 출력. (FR-DEL-4)

use hippo::commands::{Confirm, confirm_delete, render_delete_report};
use hippo::i18n::Lang;
use hippo::protocol::DeleteReport;
use hippo::store::ForgetCounts;

#[test]
fn yes_flag_proceeds_without_reading_input() {
    let read = || -> Option<String> { panic!("입력을 읽지 않음") };
    assert_eq!(confirm_delete(true, false, read), Confirm::Proceed);
    assert_eq!(confirm_delete(true, true, read), Confirm::Proceed);
}

#[test]
fn non_terminal_without_yes_needs_yes_flag() {
    let read = || -> Option<String> { panic!("입력을 읽지 않음") };
    assert_eq!(confirm_delete(false, false, read), Confirm::NeedYes);
}

#[test]
fn terminal_answer_y_proceeds_and_anything_else_cancels() {
    for (answer, expected) in [
        (Some("y\n"), Confirm::Proceed),
        (Some(" Y \n"), Confirm::Proceed),
        (Some("n\n"), Confirm::Cancel),
        (Some("\n"), Confirm::Cancel),
        (Some("yes\n"), Confirm::Cancel),
        (None, Confirm::Cancel),
    ] {
        assert_eq!(
            confirm_delete(false, true, || answer.map(str::to_string)),
            expected,
            "{answer:?}"
        );
    }
}

#[test]
fn report_lists_project_and_counts() {
    let report = DeleteReport {
        project_id: "abc".into(),
        location: "/work/a".into(),
        counts: ForgetCounts {
            sessions: 1,
            turns: 2,
            iterations: 3,
            results: 4,
        },
    };
    let en = render_delete_report(&report, Lang::En);
    for part in [
        "abc",
        "/work/a",
        "sessions: 1",
        "turns: 2",
        "iterations: 3",
        "results: 4",
    ] {
        assert!(en.contains(part), "{en}");
    }
    let ko = render_delete_report(&report, Lang::Ko);
    assert!(ko.contains("반복 3개") && ko.contains("/work/a"), "{ko}");
}
