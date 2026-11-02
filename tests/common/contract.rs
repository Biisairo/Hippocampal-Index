//! 모든 백엔드에 적용하는 계약 테스트. (FR-SWP-8)
#![allow(dead_code)]

use hippo::embedder::Embedder;
use hippo::store::RecordKind;
use hippo::vector::{VectorError, VectorFilter, VectorRecord, VectorStore};

pub fn record(
    kind: RecordKind,
    ref_id: &str,
    chunk: u32,
    vector: Vec<f32>,
    cwd: Option<&str>,
    session: Option<&str>,
) -> VectorRecord {
    VectorRecord {
        kind,
        ref_id: ref_id.into(),
        chunk_index: chunk,
        vector,
        embed_model: "m1".into(),
        cwd: cwd.map(str::to_string),
        session_id: session.map(str::to_string),
        ts: "2026-11-02T00:00:00Z".into(),
    }
}

fn ids(hits: &[hippo::vector::VectorHit]) -> Vec<String> {
    hits.iter()
        .map(|h| format!("{}#{}", h.ref_id, h.chunk_index))
        .collect()
}

pub fn vector_store_contract(make: &dyn Fn() -> Box<dyn VectorStore>) {
    use RecordKind::{Iteration, Result};

    // 빈 저장소
    let s = make();
    assert_eq!(s.count().unwrap(), 0);
    assert_eq!(s.info().unwrap().embed_model, None);
    assert!(
        s.search(&[1.0, 0.0, 0.0], &VectorFilter::default(), 10)
            .unwrap()
            .is_empty()
    );
    s.ensure_index().unwrap();

    // 저장, info, 정렬
    let s = make();
    s.upsert(&[
        record(
            Iteration,
            "a",
            0,
            vec![1.0, 0.0, 0.0],
            Some("/p"),
            Some("s1"),
        ),
        record(
            Iteration,
            "b",
            0,
            vec![0.8, 0.6, 0.0],
            Some("/q"),
            Some("s2"),
        ),
        record(Result, "r", 0, vec![0.0, 1.0, 0.0], None, None),
        record(Result, "r", 1, vec![0.0, 0.0, 1.0], None, None),
    ])
    .unwrap();
    assert_eq!(s.count().unwrap(), 4);
    let info = s.info().unwrap();
    assert_eq!(
        (info.embed_model.as_deref(), info.dimension),
        (Some("m1"), Some(3))
    );
    let hits = s
        .search(&[1.0, 0.0, 0.0], &VectorFilter::default(), 10)
        .unwrap();
    assert_eq!(ids(&hits)[..2], ["a#0".to_string(), "b#0".to_string()]);
    assert!((hits[0].score - 1.0).abs() < 1e-4);
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    assert_eq!(
        s.search(&[1.0, 0.0, 0.0], &VectorFilter::default(), 1)
            .unwrap()
            .len(),
        1
    );

    // 필터
    let q = [1.0, 0.0, 0.0];
    let f = |kind, ex: Option<&str>, model: Option<&str>| VectorFilter {
        kind,
        exclude_session: ex.map(str::to_string),
        embed_model: model.map(str::to_string),
    };
    let mut got = ids(&s.search(&q, &f(Some(Result), None, None), 10).unwrap());
    got.sort();
    assert_eq!(got, ["r#0", "r#1"]);
    let mut got = ids(&s.search(&q, &f(None, Some("s1"), None), 10).unwrap());
    got.sort();
    assert_eq!(
        got,
        ["b#0", "r#0", "r#1"],
        "세션 없는 결과 벡터는 제외 대상이 아님"
    );
    assert!(
        s.search(&q, &f(None, None, Some("other")), 10)
            .unwrap()
            .is_empty()
    );

    // 같은 id 교체
    s.upsert(&[record(
        Iteration,
        "a",
        0,
        vec![0.0, 0.0, 1.0],
        Some("/p"),
        Some("s1"),
    )])
    .unwrap();
    assert_eq!(s.count().unwrap(), 4);
    assert_eq!(
        ids(&s
            .search(&[0.0, 0.0, 1.0], &f(Some(Iteration), None, None), 1)
            .unwrap()),
        ["a#0"]
    );

    // 전체 읽기 (FR-SWP-2 scan)
    let mut scanned: Vec<String> = s.scan().unwrap().iter().map(VectorRecord::id).collect();
    scanned.sort();
    assert_eq!(
        scanned,
        ["iteration:a#0", "iteration:b#0", "result:r#0", "result:r#1"]
    );
    let a = s
        .scan()
        .unwrap()
        .into_iter()
        .find(|r| r.ref_id == "a")
        .unwrap();
    assert_eq!(a.vector, vec![0.0, 0.0, 1.0]);
    assert_eq!(
        (
            a.cwd.as_deref(),
            a.session_id.as_deref(),
            a.embed_model.as_str()
        ),
        (Some("/p"), Some("s1"), "m1")
    );
    assert!(make().scan().unwrap().is_empty());

    // ref_id 단위 삭제
    s.delete(Result, "r").unwrap();
    assert_eq!(s.count().unwrap(), 2);
    s.delete(Result, "a").unwrap();
    assert_eq!(s.count().unwrap(), 2, "kind가 다르면 지우지 않음");

    // 차원 불일치 거부, 부분 저장 없음
    let err = s
        .upsert(&[
            record(Iteration, "c", 0, vec![1.0, 0.0, 0.0], None, None),
            record(Iteration, "d", 0, vec![1.0, 0.0], None, None),
        ])
        .unwrap_err();
    assert!(matches!(
        err,
        VectorError::DimensionMismatch {
            expected: 3,
            actual: 2
        }
    ));
    assert_eq!(s.count().unwrap(), 2);
    s.ensure_index().unwrap();
}

pub fn embedder_contract(e: &dyn Embedder) {
    assert!(e.embed(&[]).unwrap().is_empty());
    let texts = vec![
        "로그인 버그".to_string(),
        "validateToken".to_string(),
        String::new(),
    ];
    let out = e.embed(&texts).unwrap();
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|v| v.len() == e.dimension()));
    assert_eq!(e.embed(&texts).unwrap(), out, "같은 입력은 같은 벡터");
    assert!(!e.model_id().is_empty());
    assert!(e.max_input_tokens() > 0);
}
