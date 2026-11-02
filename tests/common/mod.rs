#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use smem::transcript::{Event, Extractor, ExtractorState, Iteration};

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn read_fixture(name: &str) -> String {
    fs::read_to_string(fixture_path(name)).expect("fixture")
}

/// 라인과 바이트 범위 (start, end). end는 개행 문자 뒤를 가리킨다.
pub fn lines_with_offsets(content: &str) -> Vec<(&str, u64, u64)> {
    let mut out = Vec::new();
    let mut start = 0u64;
    for raw in content.split_inclusive('\n') {
        let end = start + raw.len() as u64;
        out.push((raw.trim_end_matches('\n'), start, end));
        start = end;
    }
    out
}

pub fn feed_all(ex: &mut Extractor, content: &str) -> Vec<Event> {
    lines_with_offsets(content)
        .into_iter()
        .flat_map(|(l, s, e)| ex.feed_line(l, s, e))
        .collect()
}

pub fn iterations(events: &[Event]) -> Vec<Iteration> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Iteration(it) => Some(it.clone()),
            Event::Notification { .. } => None,
        })
        .collect()
}

pub fn run_fixture(name: &str) -> (Extractor, Vec<Event>) {
    let mut ex = Extractor::new(ExtractorState::default());
    let events = feed_all(&mut ex, &read_fixture(name));
    (ex, events)
}

pub fn user_line(uuid: &str, prompt_id: &str, content: Value) -> String {
    json!({
        "type": "user", "uuid": uuid, "sessionId": "S", "cwd": "/w",
        "timestamp": "2026-11-02T00:00:00Z", "promptId": prompt_id,
        "message": {"role": "user", "content": content}
    })
    .to_string()
}

pub fn assistant_line(uuid: &str, message_id: &str, content: Value) -> String {
    json!({
        "type": "assistant", "uuid": uuid, "sessionId": "S", "cwd": "/w",
        "timestamp": "2026-11-02T00:00:01Z",
        "message": {"id": message_id, "role": "assistant", "content": content}
    })
    .to_string()
}

pub fn joined(lines: &[String]) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}
