mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use common::contract::embedder_contract;
use serde_json::{Value, json};
use smem::config::EmbedderConfig;
use smem::embedder::{EmbedError, Embedder};
use smem::ollama::OllamaEmbedder;
use smem::vector::cosine;

/// 요청 한 건을 받아 본문을 돌려주고, 정해진 상태 코드와 본문으로 응답하는 가짜 서버.
fn fake_server(status: u16, body: Value) -> (String, mpsc::Receiver<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut length = 0;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            if header == "\r\n" {
                break;
            }
            if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
        }
        let mut buf = vec![0; length];
        reader.read_exact(&mut buf).unwrap();
        tx.send((request_line, serde_json::from_slice(&buf).unwrap()))
            .unwrap();
        let text = body.to_string();
        let mut stream = stream;
        write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).unwrap();
    });
    (url, rx)
}

fn config(url: &str, dimension: usize) -> EmbedderConfig {
    EmbedderConfig {
        url: url.into(),
        dimension,
        ..EmbedderConfig::default()
    }
}

#[test]
fn sends_expected_request_and_parses_embeddings() {
    let (url, rx) = fake_server(200, json!({"embeddings": [[0.6, 0.8], [1.0, 0.0]]}));
    let e = OllamaEmbedder::try_new(&config(&url, 2)).unwrap();
    let out = e.embed(&["가".into(), "나".into()]).unwrap();
    assert_eq!(out, vec![vec![0.6, 0.8], vec![1.0, 0.0]]);
    let (line, body) = rx.recv().unwrap();
    assert!(line.starts_with("POST /api/embed "));
    assert_eq!(body["model"], "qwen3-embedding:0.6b");
    assert_eq!(body["input"], json!(["가", "나"]));
    assert_eq!(body["dimensions"], 2);
    assert_eq!(body["keep_alive"], "30m");
    assert_eq!(e.model_id(), "qwen3-embedding:0.6b");
    assert_eq!(e.max_input_tokens(), 32_000);
}

#[test]
fn empty_input_sends_no_request() {
    let e = OllamaEmbedder::try_new(&config("http://127.0.0.1:9", 2)).unwrap();
    assert!(e.embed(&[]).unwrap().is_empty());
}

#[test]
fn unreachable_server_is_unavailable() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let err = OllamaEmbedder::try_new(&config(&url, 2))
        .unwrap()
        .embed(&["x".into()])
        .unwrap_err();
    assert!(matches!(err, EmbedError::Unavailable(_)), "{err:?}");
}

#[test]
fn error_status_is_invalid_response_with_message() {
    let (url, _rx) = fake_server(404, json!({"error": "model not found"}));
    let err = OllamaEmbedder::try_new(&config(&url, 2))
        .unwrap()
        .embed(&["x".into()])
        .unwrap_err();
    match err {
        EmbedError::InvalidResponse(m) => assert!(m.contains("model not found"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn wrong_count_or_dimension_is_rejected() {
    let (url, _rx) = fake_server(200, json!({"embeddings": [[0.6, 0.8]]}));
    assert!(matches!(
        OllamaEmbedder::try_new(&config(&url, 2))
            .unwrap()
            .embed(&["a".into(), "b".into()]),
        Err(EmbedError::InvalidResponse(_))
    ));
    let (url, _rx) = fake_server(200, json!({"embeddings": [[0.6, 0.8, 0.0]]}));
    assert!(matches!(
        OllamaEmbedder::try_new(&config(&url, 2))
            .unwrap()
            .embed(&["a".into()]),
        Err(EmbedError::InvalidResponse(_))
    ));
    let (url, _rx) = fake_server(200, json!({"nope": 1}));
    assert!(matches!(
        OllamaEmbedder::try_new(&config(&url, 2))
            .unwrap()
            .embed(&["a".into()]),
        Err(EmbedError::InvalidResponse(_))
    ));
}

// ---- 실제 Ollama (cargo test -- --ignored) ----

#[test]
#[ignore = "로컬 Ollama와 qwen3-embedding:0.6b 필요"]
fn live_ollama_satisfies_contract_and_ranks_korean() {
    let e = OllamaEmbedder::try_new(&EmbedderConfig::default()).unwrap();
    embedder_contract(&e);
    let v = e
        .embed(&[
            "로그인할 때 토큰 만료 비교가 반대로 되어 있던 버그".into(),
            "validateToken 함수에서 exp < now 조건을 고쳤다".into(),
            "주말에 공원에서 산책하고 커피를 마셨다".into(),
        ])
        .unwrap();
    assert!(cosine(&v[0], &v[1]) > cosine(&v[0], &v[2]));
}

// ---- 보안: 외부 전송 금지 (C-4) ----

#[test]
fn constructor_rejects_non_local_url() {
    for url in [
        "http://127.0.0.1:11434@evil.com",
        "https://api.example.com",
        "http://localhost:1/?x=1",
    ] {
        assert!(OllamaEmbedder::try_new(&config(url, 2)).is_err(), "{url}");
    }
    assert!(OllamaEmbedder::try_new(&config("http://[::1]:11434", 2)).is_ok());
}

#[test]
fn redirects_are_not_followed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = write!(
            stream,
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/api/embed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        listener.set_nonblocking(true).unwrap();
        thread::sleep(std::time::Duration::from_millis(300));
        if listener.accept().is_ok() {
            tx.send(()).unwrap();
        }
    });
    let err = OllamaEmbedder::try_new(&config(&url, 2))
        .unwrap()
        .embed(&["x".into()]);
    assert!(err.is_err());
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(600))
            .is_err(),
        "두 번째 요청 없음"
    );
}
