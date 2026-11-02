mod common;

use common::contract::{embedder_contract, vector_store_contract};
use hippo::embedder::Embedder;
use hippo::store::RecordKind;
use hippo::testing::{HashEmbedder, MemoryVectorStore};
use hippo::vector::{VectorStore, cosine, vector_id};

#[test]
fn memory_store_satisfies_contract() {
    vector_store_contract(&|| Box::new(MemoryVectorStore::default()) as Box<dyn VectorStore>);
}

#[test]
fn hash_embedder_satisfies_contract() {
    embedder_contract(&HashEmbedder::new(64, 512));
}

#[test]
fn hash_embedder_reflects_shared_words() {
    let e = HashEmbedder::new(256, 512);
    let v = e
        .embed(&[
            "로그인 토큰 만료 버그".into(),
            "토큰 만료 비교 버그".into(),
            "날씨 산책 커피".into(),
        ])
        .unwrap();
    assert!(cosine(&v[0], &v[1]) > cosine(&v[0], &v[2]));
    let norm: f32 = v[0].iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-4);
    assert_eq!(e.model_id(), "test-hash-256");
}

#[test]
fn cosine_edge_cases() {
    assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
    assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0, "영벡터는 0");
    assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0, "길이가 다르면 0");
}

#[test]
fn vector_id_format() {
    assert_eq!(
        vector_id(RecordKind::Iteration, "s:m", 2),
        "iteration:s:m#2"
    );
    assert_eq!(vector_id(RecordKind::Result, "abc", 0), "result:abc#0");
}
