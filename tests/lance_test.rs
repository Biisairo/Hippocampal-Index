mod common;

use common::contract::{record, vector_store_contract};
use hippo::lance::LanceVectorStore;
use hippo::store::RecordKind;
use hippo::vector::{VectorError, VectorFilter, VectorStore};

#[test]
fn lancedb_satisfies_contract() {
    vector_store_contract(&|| {
        let dir = tempfile::tempdir().unwrap().keep();
        Box::new(LanceVectorStore::open(&dir).unwrap()) as Box<dyn VectorStore>
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
    let f = VectorFilter {
        cwd: Some(cwd.into()),
        ..Default::default()
    };
    assert_eq!(s.search(&[1.0, 0.0], &f, 5).unwrap().len(), 1);
    let ex = VectorFilter {
        exclude_session: Some("s'1".into()),
        ..Default::default()
    };
    assert!(s.search(&[1.0, 0.0], &ex, 5).unwrap().is_empty());
    s.delete(RecordKind::Iteration, "a'b").unwrap();
    assert_eq!(s.count().unwrap(), 0);
}
