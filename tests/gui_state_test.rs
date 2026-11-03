use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use hippo::gui::state::{
    ImportForm, ImportProgress, InstallAction, InstallForm, resolve_import_args, run_import,
};
use hippo::i18n::Lang;
use hippo::import::{Candidate, ImportStatus};
use hippo::ingest::ImportOutcome;
use hippo::protocol::{ImportSessionReport, Request, Response};

fn candidate(id: &str, status: ImportStatus) -> Candidate {
    Candidate {
        session_id: id.into(),
        transcript_path: PathBuf::from(format!("/t/{id}.jsonl")),
        started_at: String::new(),
        start_cwd: "/w".into(),
        size: 1,
        first_prompt: String::new(),
        status,
    }
}

fn plan(ids: &[(&str, ImportStatus)]) -> Response {
    Response::ImportPlan {
        project_id: "p1".into(),
        source_id: None,
        summary: "No change\n".into(),
        sessions: ids.iter().map(|(id, s)| candidate(id, *s)).collect(),
    }
}

fn report() -> Response {
    Response::ImportSession(ImportSessionReport {
        outcome: ImportOutcome::Imported,
        iterations: 2,
        completions: 0,
        fork_skipped: 0,
        pending_embeds: 5,
    })
}

// ---- FR-GUI-7 ----

#[test]
fn preview_selects_new_and_live_only_sessions() {
    let mut form = ImportForm::new("/w");
    assert_eq!((form.from.as_str(), form.to.as_str()), ("/w", "/w"));
    assert!(!form.preview_current());
    let Response::ImportPlan {
        summary, sessions, ..
    } = plan(&[
        ("a", ImportStatus::New),
        ("b", ImportStatus::LiveOnly),
        ("c", ImportStatus::Imported),
        ("d", ImportStatus::OtherProject),
    ])
    else {
        unreachable!()
    };
    form.apply_preview("/w".into(), "/w".into(), summary, sessions);
    assert!(form.preview_current());
    assert_eq!(form.selected, vec![true, true, false, false]);
    assert_eq!(form.selected_ids(), vec!["a", "b"]);
    form.set_all(true);
    assert_eq!(form.selected_ids().len(), 4);
    form.set_all(false);
    assert!(form.selected_ids().is_empty());
}

#[test]
fn changing_inputs_invalidates_preview() {
    let mut form = ImportForm::new("/w");
    form.apply_preview("/w".into(), "/w".into(), String::new(), Vec::new());
    assert!(form.preview_current());
    form.from = "/old".into();
    assert!(!form.preview_current());
    form.from = "/w".into();
    form.to = "/x".into();
    assert!(!form.preview_current());
}

#[test]
fn import_args_resolve_against_launch_path() {
    let id = "0123456789abcdef0123456789abcdef";
    assert_eq!(
        resolve_import_args("/w", "", "sub"),
        ("/w".to_string(), "/w/sub".to_string())
    );
    assert_eq!(
        resolve_import_args("/w", id, "/abs"),
        (id.to_string(), "/abs".to_string())
    );
}

// ---- FR-GUI-8 ----

#[test]
fn import_sends_only_selected_sessions_in_response_order() {
    let mut sent: Vec<Request> = Vec::new();
    let mut send = |req: &Request| {
        sent.push(req.clone());
        Ok(match req {
            Request::ImportPrepare { .. } => plan(&[
                ("a", ImportStatus::New),
                ("b", ImportStatus::New),
                ("c", ImportStatus::LiveOnly),
            ]),
            _ => report(),
        })
    };
    let progress = Mutex::new(ImportProgress::default());
    let selected = vec!["c".to_string(), "a".to_string()];
    run_import(
        &mut send,
        "/old",
        "/w",
        &selected,
        &AtomicBool::new(false),
        &progress,
        Lang::En,
    );
    assert_eq!(
        sent[0],
        Request::ImportPrepare {
            from: "/old".into(),
            to: "/w".into(),
            dry_run: false
        }
    );
    let ids: Vec<&str> = sent[1..]
        .iter()
        .map(|r| match r {
            Request::ImportSession {
                session_id,
                project_id,
                ..
            } => {
                assert_eq!(project_id, "p1");
                session_id.as_str()
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, vec!["a", "c"]);
    let p = progress.lock().unwrap();
    assert_eq!((p.done, p.total), (2, 2));
    assert_eq!(p.project_id.as_deref(), Some("p1"));
    assert!(p.lines[0].starts_with("[1/2] a imported"), "{:?}", p.lines);
    assert!(
        p.summary.as_deref().unwrap().contains("imported 2"),
        "{p:?}"
    );
    assert!(p.finished);
}

#[test]
fn stop_prevents_sending_the_next_session() {
    let stop = AtomicBool::new(false);
    let mut sessions_sent = 0;
    let mut send = |req: &Request| {
        Ok(match req {
            Request::ImportPrepare { .. } => {
                plan(&[("a", ImportStatus::New), ("b", ImportStatus::New)])
            }
            _ => {
                sessions_sent += 1;
                stop.store(true, Ordering::SeqCst);
                report()
            }
        })
    };
    let progress = Mutex::new(ImportProgress::default());
    let selected = vec!["a".to_string(), "b".to_string()];
    run_import(&mut send, "/w", "/w", &selected, &stop, &progress, Lang::En);
    assert_eq!(sessions_sent, 1);
    let p = progress.lock().unwrap();
    assert_eq!((p.done, p.total), (1, 2));
    assert!(p.finished && p.summary.is_some());
}

#[test]
fn prepare_error_is_reported_without_sending_sessions() {
    let mut count = 0;
    let mut send = |_: &Request| {
        count += 1;
        Ok(Response::Error {
            message: "Cannot import while reindexing".into(),
        })
    };
    let progress = Mutex::new(ImportProgress::default());
    run_import(
        &mut send,
        "/w",
        "/w",
        &["a".to_string()],
        &AtomicBool::new(false),
        &progress,
        Lang::En,
    );
    assert_eq!(count, 1);
    let p = progress.lock().unwrap();
    assert_eq!(p.error.as_deref(), Some("Cannot import while reindexing"));
    assert!(p.finished);
}

#[test]
fn session_error_is_counted_and_import_continues() {
    let mut send = |req: &Request| {
        Ok(match req {
            Request::ImportPrepare { .. } => {
                plan(&[("a", ImportStatus::New), ("b", ImportStatus::New)])
            }
            Request::ImportSession { session_id, .. } if session_id == "a" => Response::Error {
                message: "boom".into(),
            },
            _ => report(),
        })
    };
    let progress = Mutex::new(ImportProgress::default());
    let selected = vec!["a".to_string(), "b".to_string()];
    run_import(
        &mut send,
        "/w",
        "/w",
        &selected,
        &AtomicBool::new(false),
        &progress,
        Lang::En,
    );
    let p = progress.lock().unwrap();
    assert!(p.lines[0].contains("error: boom"), "{:?}", p.lines);
    assert_eq!(p.done, 2);
    assert!(p.summary.as_deref().unwrap().contains("errors 1"));
}

// ---- FR-GUI-10 ----

#[test]
fn apply_and_remove_need_a_preview_with_the_same_inputs() {
    let mut form = InstallForm::new("/w");
    assert!(!form.can_run(InstallAction::Install));
    form.mark_previewed(form.key(InstallAction::Install));
    assert!(form.can_run(InstallAction::Install));
    assert!(!form.can_run(InstallAction::Uninstall), "다른 동작");
    form.project = "/other".into();
    assert!(!form.can_run(InstallAction::Install), "다른 프로젝트");
    form.project = "/w".into();
    assert!(form.can_run(InstallAction::Install));
    form.user_scope = true;
    assert!(!form.can_run(InstallAction::Install), "다른 범위");
    form.user_scope = false;
    form.clear_preview();
    assert!(
        !form.can_run(InstallAction::Install),
        "실행 뒤에는 다시 미리 보기"
    );
}

// ---- FR-GUI-13~17 기록 탭 ----

use hippo::gui::state::{
    DeleteTarget, PAGE_SIZE, RecordsView, Reload, embed_state, page_count, skip_label,
    verdict_label,
};
use hippo::protocol::Scope;
use hippo::search::{SkipReason, Verdict};
use hippo::store::ForgetTarget;

fn browsing() -> RecordsView {
    let mut v = RecordsView::default();
    v.select_project("p1");
    v.select_session("s1");
    v.select_turn("t1");
    v.reload = Reload::default();
    v
}

#[test]
fn selecting_a_level_resets_the_levels_below() {
    let mut v = browsing();
    v.select_session("s2");
    assert_eq!(
        (v.session.as_deref(), v.prompt.as_deref()),
        (Some("s2"), None)
    );
    assert_eq!(
        v.reload,
        Reload {
            sessions: false,
            turns: true,
            detail: false
        }
    );
    v.select_project("p2");
    assert_eq!(v.project.as_deref(), Some("p2"));
    assert_eq!((v.session.clone(), v.prompt.clone()), (None, None));
    assert_eq!(
        v.reload,
        Reload {
            sessions: true,
            turns: false,
            detail: false
        }
    );
    let mut same = browsing();
    same.select_project("p1");
    assert_eq!(
        same.session.as_deref(),
        Some("s1"),
        "같은 프로젝트는 그대로"
    );
}

#[test]
fn requests_use_project_id_scope_and_page_offsets() {
    let mut v = browsing();
    v.set_session_filter("abc");
    v.session_page_to(2, 500);
    let Some(Request::SessionList {
        scope,
        filter,
        offset,
        limit,
        ..
    }) = v.session_request("/cwd")
    else {
        panic!()
    };
    assert_eq!(scope, Scope::Id("p1".into()));
    assert_eq!(
        (filter.as_str(), offset, limit),
        ("abc", 2 * PAGE_SIZE, PAGE_SIZE)
    );
    v.session_page_to(99, 120);
    assert_eq!(v.session_page, 2, "마지막 페이지로 제한");
    v.set_session_filter("x");
    assert_eq!(v.session_page, 0, "필터를 바꾸면 첫 페이지");
    v.turn_page_to(1, 60);
    let Some(Request::TurnList {
        session_id, offset, ..
    }) = v.turn_request("/cwd")
    else {
        panic!()
    };
    assert_eq!((session_id.as_str(), offset), ("s1", PAGE_SIZE));
    let Some(Request::TurnDetail { prompt_id, .. }) = v.detail_request("/cwd") else {
        panic!()
    };
    assert_eq!(prompt_id, "t1");
    let empty = RecordsView::default();
    assert!(empty.session_request("/").is_none());
    assert!(empty.turn_request("/").is_none());
    assert!(empty.detail_request("/").is_none());
}

#[test]
fn page_count_has_at_least_one_page() {
    assert_eq!(page_count(0), 1);
    assert_eq!(page_count(50), 1);
    assert_eq!(page_count(51), 2);
}

#[test]
fn delete_targets_build_dry_run_and_real_requests() {
    let session = DeleteTarget::Session {
        project: "p1".into(),
        session_id: "s1".into(),
    };
    assert_eq!(
        session.request("/cwd", true),
        Request::Forget {
            target: ForgetTarget::Session("s1".into()),
            scope: Some(Scope::Id("p1".into())),
            cwd: "/cwd".into(),
            dry_run: true,
        }
    );
    let turn = DeleteTarget::Turn {
        project: "p1".into(),
        prompt_id: "t1".into(),
    };
    assert!(matches!(
        turn.request("/cwd", false),
        Request::Forget {
            target: ForgetTarget::Prompt(_),
            dry_run: false,
            ..
        }
    ));
    assert_eq!(
        DeleteTarget::Project("p1".into()).request("/cwd", true),
        Request::ProjectRemove {
            project: "p1".into(),
            cwd: "/cwd".into(),
            dry_run: true,
        }
    );
}

#[test]
fn after_delete_clears_the_deleted_selection_and_reloads_lists() {
    let mut v = browsing();
    v.after_delete(&DeleteTarget::Turn {
        project: "p1".into(),
        prompt_id: "t1".into(),
    });
    assert_eq!(
        (v.session.as_deref(), v.prompt.as_deref()),
        (Some("s1"), None)
    );
    assert_eq!(
        v.reload,
        Reload {
            sessions: true,
            turns: true,
            detail: false
        }
    );

    let mut v = browsing();
    v.after_delete(&DeleteTarget::Session {
        project: "p1".into(),
        session_id: "s1".into(),
    });
    assert_eq!((v.session.clone(), v.prompt.clone()), (None, None));
    assert_eq!(
        v.reload,
        Reload {
            sessions: true,
            turns: false,
            detail: false
        }
    );

    let mut v = browsing();
    v.after_delete(&DeleteTarget::Project("p1".into()));
    assert_eq!(
        (v.project.clone(), v.session.clone(), v.prompt.clone()),
        (None, None, None)
    );
}

#[test]
fn open_turn_from_search_selects_project_and_detail() {
    let mut v = browsing();
    v.open_turn("p2", "t9");
    assert_eq!(v.project.as_deref(), Some("p2"));
    assert_eq!(v.session, None);
    assert_eq!(v.prompt.as_deref(), Some("t9"));
    assert_eq!(
        v.reload,
        Reload {
            sessions: true,
            turns: false,
            detail: true
        }
    );
}

#[test]
fn labels_follow_the_srs_wording() {
    assert_eq!(embed_state(0, Lang::Ko), "완료");
    assert_eq!(embed_state(3, Lang::Ko), "대기 3");
    assert_eq!(embed_state(3, Lang::En), "pending 3");
    let ko: Vec<_> = [
        Verdict::Selected,
        Verdict::BelowThreshold,
        Verdict::FtsOnly,
        Verdict::SameTurn,
        Verdict::OverLength,
        Verdict::OverCount,
    ]
    .into_iter()
    .map(|v| verdict_label(v, Lang::Ko))
    .collect();
    assert_eq!(
        ko,
        [
            "선택",
            "임계값 미만",
            "FTS 전용",
            "같은 턴",
            "길이 한도 초과",
            "개수 초과"
        ]
    );
    assert_eq!(
        skip_label(SkipReason::TooShort, Lang::Ko),
        "min_prompt_chars 미만"
    );
    assert_eq!(
        skip_label(SkipReason::VectorUnavailable, Lang::Ko),
        "벡터 검색 불가"
    );
}
