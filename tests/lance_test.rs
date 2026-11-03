mod common;

use common::contract::{record, vector_store_contract};
use hippo::lance::LanceVectorStore;
use hippo::store::RecordKind;
use hippo::vector::{VectorError, VectorFilter, VectorStore};

#[test]
fn lancedb_satisfies_contract() {
    let dirs = std::sync::Mutex::new(Vec::new());
    vector_store_contract(&|| {
        let dir = tempfile::tempdir().unwrap();
        let store = LanceVectorStore::open(dir.path()).unwrap();
        dirs.lock().unwrap().push(dir);
        Box::new(store) as Box<dyn VectorStore>
    });
}

#[test]
fn data_and_info_persist_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let s = LanceVectorStore::open(dir.path()).unwrap();
        s.upsert(&[
            record(
                RecordKind::Iteration,
                "a",
                0,
                vec![1.0, 0.0, 0.0],
                Some("/p"),
                Some("s1"),
            ),
            record(RecordKind::Result, "r", 0, vec![0.0, 1.0, 0.0], None, None),
        ])
        .unwrap();
    }
    let s = LanceVectorStore::open(dir.path()).unwrap();
    assert_eq!(s.count().unwrap(), 2);
    let info = s.info().unwrap();
    assert_eq!(
        (info.embed_model.as_deref(), info.dimension),
        (Some("m1"), Some(3))
    );
    let hits = s
        .search(&[1.0, 0.0, 0.0], &VectorFilter::default(), 1)
        .unwrap();
    assert_eq!(hits[0].ref_id, "a");
    let err = s
        .upsert(&[record(
            RecordKind::Iteration,
            "b",
            0,
            vec![1.0, 0.0],
            None,
            None,
        )])
        .unwrap_err();
    assert!(matches!(
        err,
        VectorError::DimensionMismatch {
            expected: 3,
            actual: 2
        }
    ));
}

#[test]
fn filter_values_with_quotes_are_escaped() {
    let dir = tempfile::tempdir().unwrap();
    let s = LanceVectorStore::open(dir.path()).unwrap();
    let cwd = "/p/it's";
    s.upsert(&[record(
        RecordKind::Iteration,
        "a'b",
        0,
        vec![1.0, 0.0],
        Some(cwd),
        Some("s'1"),
    )])
    .unwrap();
    assert_eq!(
        s.search(&[1.0, 0.0], &VectorFilter::default(), 5)
            .unwrap()
            .len(),
        1
    );
    let ex = VectorFilter {
        exclude_session: Some("s'1".into()),
        ..Default::default()
    };
    assert!(s.search(&[1.0, 0.0], &ex, 5).unwrap().is_empty());
    s.delete(RecordKind::Iteration, "a'b").unwrap();
    assert_eq!(s.count().unwrap(), 0);
}

// ---- FR-STO-7, FR-STO-12 ----

fn versions(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir.join("vectors.lance").join("_versions"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "manifest"))
        })
        .count()
}

fn rec(ref_id: &str, chunk: u32, x: f32) -> hippo::vector::VectorRecord {
    record(
        RecordKind::Iteration,
        ref_id,
        chunk,
        vec![x, 1.0, 0.0],
        Some("/p"),
        Some("s"),
    )
}

#[test]
fn replace_is_one_delete_and_one_add() {
    let dir = tempfile::tempdir().unwrap();
    let s = LanceVectorStore::open(dir.path()).unwrap();
    s.upsert(&[rec("a", 0, 1.0), rec("a", 1, 2.0)]).unwrap();
    let (v0, p0) = (versions(dir.path()), s.pending_changes());
    let refs: Vec<(RecordKind, String)> = (0..64)
        .map(|i| (RecordKind::Iteration, format!("r{i}")))
        .chain([(RecordKind::Iteration, "a".to_string())])
        .collect();
    let records: Vec<_> = (0..64)
        .map(|i| rec(&format!("r{i}"), 0, i as f32))
        .chain([rec("a", 0, 9.0)])
        .collect();
    s.replace(&refs, &records).unwrap();
    assert_eq!(versions(dir.path()), v0 + 2, "묶음 하나가 버전 2개");
    assert_eq!(s.pending_changes(), p0 + 2);
    assert_eq!(s.count().unwrap(), 65, "a의 이전 청크 1은 지워짐");
}

#[test]
fn pending_changes_count_each_modification() {
    let dir = tempfile::tempdir().unwrap();
    let s = LanceVectorStore::open(dir.path()).unwrap();
    assert_eq!(s.pending_changes(), 0);
    s.upsert(&[rec("a", 0, 1.0)]).unwrap();
    assert_eq!(s.pending_changes(), 1);
    s.delete(RecordKind::Iteration, "a").unwrap();
    assert_eq!(s.pending_changes(), 2);
    s.replace(&[(RecordKind::Iteration, "b".into())], &[])
        .unwrap();
    assert_eq!(s.pending_changes(), 3, "지우기만 하면 1");
    s.replace(&[], &[rec("c", 0, 1.0)]).unwrap();
    assert_eq!(s.pending_changes(), 4, "넣기만 하면 1");
    s.replace(&[], &[]).unwrap();
    assert_eq!(s.pending_changes(), 4, "빈 replace는 수정 아님");
}

#[test]
fn optimize_keeps_only_latest_version_and_data() {
    let dir = tempfile::tempdir().unwrap();
    let s = LanceVectorStore::open(dir.path()).unwrap();
    for i in 0..30 {
        let id = format!("r{i}");
        s.replace(
            &[(RecordKind::Iteration, id.clone())],
            &[rec(&id, 0, i as f32)],
        )
        .unwrap();
    }
    assert!(versions(dir.path()) > 30);
    let query = [3.0, 1.0, 0.0];
    let before = s.search(&query, &VectorFilter::default(), 5).unwrap();
    s.optimize().unwrap();
    assert_eq!(versions(dir.path()), 1);
    assert_eq!(s.pending_changes(), 0);
    assert_eq!(s.count().unwrap(), 30);
    assert_eq!(
        s.search(&query, &VectorFilter::default(), 5).unwrap(),
        before
    );
    s.upsert(&[rec("after", 0, 1.0)]).unwrap();
    assert_eq!(s.count().unwrap(), 31, "정리 뒤에도 쓰기 가능");
}

#[test]
fn optimize_without_changes_makes_no_version() {
    let dir = tempfile::tempdir().unwrap();
    let s = LanceVectorStore::open(dir.path()).unwrap();
    s.optimize().unwrap();
    s.upsert(&[rec("a", 0, 1.0)]).unwrap();
    s.optimize().unwrap();
    let v = versions(dir.path());
    s.optimize().unwrap();
    assert_eq!(versions(dir.path()), v);
}

#[test]
fn first_optimize_cleans_store_written_by_old_code() {
    let dir = tempfile::tempdir().unwrap();
    {
        let s = LanceVectorStore::open(dir.path()).unwrap();
        for i in 0..40 {
            let id = format!("r{i}");
            s.delete(RecordKind::Iteration, &id).unwrap();
            s.upsert(&[rec(&id, 0, i as f32)]).unwrap();
        }
    }
    assert!(versions(dir.path()) >= 80);
    let s = LanceVectorStore::open(dir.path()).unwrap();
    assert_eq!(s.pending_changes(), 0);
    s.upsert(&[rec("new", 0, 1.0)]).unwrap();
    s.optimize().unwrap();
    assert_eq!(versions(dir.path()), 1);
    assert_eq!(s.count().unwrap(), 41);
}
