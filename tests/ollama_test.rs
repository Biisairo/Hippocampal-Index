mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use common::contract::embedder_contract;
use hippo::config::EmbedderConfig;
use hippo::embedder::{EmbedError, Embedder};
use hippo::ollama::OllamaEmbedder;
use hippo::vector::cosine;
use serde_json::{Value, json};

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

// ---- FR-EMB-1, 2: 서버 자동 시작 ----

/// 지정한 포트에서 여러 요청을 처리하는 가짜 Ollama. `/api/version`과 `/api/embed`에 응답한다.
fn start_fake_ollama(listener: TcpListener) {
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    return;
                }
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err()
                        || header == "\r\n"
                        || header.is_empty()
                    {
                        break;
                    }
                    if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let reply = if line.contains("/api/version") {
                    json!({"version": "fake"})
                } else {
                    let inputs: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let n = inputs["input"].as_array().map_or(0, Vec::len);
                    json!({"embeddings": vec![vec![0.6, 0.8]; n]})
                };
                let text = reply.to_string();
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
            });
        }
    });
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn unreachable_server_is_started_once_and_request_retried() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let port = free_port();
    let launches = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&launches);
    let launcher: hippo::ollama::ServerLauncher = Box::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        start_fake_ollama(listener);
        Ok(())
    });
    let e = Arc::new(
        OllamaEmbedder::try_new_with_launcher(
            &config(&format!("http://127.0.0.1:{port}"), 2),
            Some(launcher),
        )
        .unwrap(),
    );
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let e = Arc::clone(&e);
            thread::spawn(move || e.embed(&["x".into()]))
        })
        .collect();
    for w in workers {
        assert_eq!(w.join().unwrap().unwrap(), vec![vec![0.6, 0.8]]);
    }
    assert_eq!(
        launches.load(Ordering::SeqCst),
        1,
        "동시에 실패해도 한 번만 띄움"
    );
}

#[test]
fn failed_launch_is_unavailable() {
    let port = free_port();
    let launcher: hippo::ollama::ServerLauncher =
        Box::new(|| Err(std::io::Error::other("no ollama")));
    let e = OllamaEmbedder::try_new_with_launcher(
        &config(&format!("http://127.0.0.1:{port}"), 2),
        Some(launcher),
    )
    .unwrap();
    assert!(matches!(
        e.embed(&["x".into()]),
        Err(EmbedError::Unavailable(_))
    ));
}

#[test]
fn without_launcher_no_start_is_attempted() {
    let port = free_port();
    let e = OllamaEmbedder::try_new_with_launcher(
        &config(&format!("http://127.0.0.1:{port}"), 2),
        None,
    )
    .unwrap();
    let started = std::time::Instant::now();
    assert!(matches!(
        e.embed(&["x".into()]),
        Err(EmbedError::Unavailable(_))
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
#[ignore = "실제 ollama 실행 파일과 qwen3-embedding:0.6b 필요"]
fn live_spawn_ollama_on_alternate_port() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let url = format!("http://127.0.0.1:{port}");
    let pid = hippo::ollama::spawn_ollama("ollama", &url, &dir.path().join("ollama.log")).unwrap();
    let e = OllamaEmbedder::try_new_with_launcher(&config(&url, 1024), None).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut result = e.embed(&["안녕".into()]);
    while result.is_err() && std::time::Instant::now() < deadline {
        thread::sleep(std::time::Duration::from_millis(200));
        result = e.embed(&["안녕".into()]);
    }
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
    assert_eq!(result.unwrap()[0].len(), 1024);
}

// ---- FR-EMB-1: 실행 파일 찾기 ----

#[cfg(unix)]
fn fake_executable(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn resolves_command_from_path_then_fallbacks() {
    use hippo::ollama::resolve_command;
    use std::ffi::OsString;
    let dir = tempfile::tempdir().unwrap();
    let on_path = dir.path().join("a");
    let fallback = dir.path().join("brew");
    let in_fallback = fake_executable(&fallback, "ollama");
    let path_env = OsString::from(on_path.display().to_string());

    assert_eq!(
        resolve_command("ollama", Some(&path_env), &[fallback.as_path()]),
        in_fallback,
        "PATH에 없으면 대체 디렉터리"
    );
    let first = fake_executable(&on_path, "ollama");
    assert_eq!(
        resolve_command("ollama", Some(&path_env), &[fallback.as_path()]),
        first,
        "PATH가 우선"
    );
    assert_eq!(
        resolve_command("nothing", Some(&path_env), &[fallback.as_path()]),
        std::path::PathBuf::from("nothing"),
        "못 찾으면 그대로"
    );
    assert_eq!(
        resolve_command("/abs/ollama", None, &[fallback.as_path()]),
        std::path::PathBuf::from("/abs/ollama"),
        "경로가 있으면 그대로"
    );
    std::fs::write(fallback.join("plain"), "x").unwrap();
    assert_eq!(
        resolve_command("plain", None, &[fallback.as_path()]),
        std::path::PathBuf::from("plain"),
        "실행 권한 없는 파일은 무시"
    );
}
