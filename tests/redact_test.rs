use serde_json::json;
use smem::bg::TaskNotification;
use smem::redact::{ExtraPattern, Redactor};
use smem::transcript::{Iteration, IterationInput, ToolCall, ToolResultData};

fn r() -> Redactor {
    Redactor::new(&[]).unwrap()
}

#[test]
fn masks_api_key_prefixes() {
    let cases = [
        (
            "key sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv end",
            "key [REDACTED:api_key] end",
        ),
        (
            "ghp_0123456789abcdefghijklmnopqrstuvwxyz",
            "[REDACTED:api_key]",
        ),
        ("t=xoxb-1234567890-abcdef", "t=[REDACTED:api_key]"),
        ("AKIAABCDEFGHIJKLMNOP", "[REDACTED:api_key]"),
    ];
    for (input, expected) in cases {
        assert_eq!(r().redact(input), expected, "{input}");
    }
}

#[test]
fn masks_bearer_token() {
    assert_eq!(
        r().redact("Authorization: Bearer abcdefghijklmnop.qrstuvwx"),
        "Authorization: [REDACTED:bearer]"
    );
}

#[test]
fn masks_pem_private_key_block() {
    let text = "a\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nxyz\n-----END RSA PRIVATE KEY-----\nb";
    assert_eq!(r().redact(text), "a\n[REDACTED:private_key]\nb");
}

#[test]
fn masks_env_style_values_but_keeps_name() {
    assert_eq!(
        r().redact("OPENAI_API_KEY=abcd1234efgh"),
        "OPENAI_API_KEY=[REDACTED:env]"
    );
    assert_eq!(
        r().redact("db_password: hunter2hunter2"),
        "db_password: [REDACTED:env]"
    );
    assert_eq!(
        r().redact("GITHUB_TOKEN = \"tok_12345678\""),
        "GITHUB_TOKEN = \"[REDACTED:env]\""
    );
}

#[test]
fn leaves_ordinary_text_alone() {
    for text in [
        "<task-notification> 처리", // "sk-" 부분 문자열
        "sk-learn 라이브러리",
        "max_tokens=8000",
        "PASSWORD=short",    // 8자 미만
        "Bearer token 설명", // 16자 미만
        "한국어 문장과 code_identifier",
    ] {
        assert_eq!(r().redact(text), text, "{text}");
    }
}

#[test]
fn redaction_is_idempotent() {
    let once = r().redact("API_KEY=abcdefgh123 and sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv");
    assert_eq!(r().redact(&once), once);
}

#[test]
fn extra_patterns_from_config() {
    let red = Redactor::new(&[ExtraPattern {
        kind: "internal".into(),
        pattern: r"HCI-\d{6}".into(),
    }])
    .unwrap();
    assert_eq!(red.redact("id HCI-123456"), "id [REDACTED:internal]");
}

#[test]
fn invalid_extra_pattern_is_reported() {
    let err = Redactor::new(&[ExtraPattern {
        kind: "bad".into(),
        pattern: "(".into(),
    }])
    .err()
    .unwrap();
    assert_eq!(err.kind, "bad");
    assert!(!err.message.is_empty());
}

#[test]
fn redacts_every_string_in_json_value() {
    let mut v = json!({"command": "export API_KEY=abcdefgh123", "nested": [{"h": "Bearer abcdefghijklmnopqrst"}], "n": 1});
    r().redact_value(&mut v);
    assert_eq!(
        v,
        json!({"command": "export API_KEY=[REDACTED:env]", "nested": [{"h": "[REDACTED:bearer]"}], "n": 1})
    );
}

#[test]
fn redacts_all_iteration_text_fields() {
    let secret = "SECRET_TOKEN=abcdefgh123";
    let mut it = Iteration {
        session_id: "s".into(),
        prompt_id: Some("p".into()),
        message_id: "m".into(),
        iteration_index: 0,
        cwd: "/w".into(),
        ts: "t".into(),
        inputs: vec![
            IterationInput::Prompt {
                text: secret.into(),
            },
            IterationInput::AgentMessage {
                text: secret.into(),
            },
            IterationInput::TaskNotification(TaskNotification {
                task_id: "t".into(),
                tool_use_id: "u".into(),
                output_file: None,
                status: "completed".into(),
                summary: secret.into(),
            }),
        ],
        texts: vec![secret.into()],
        tool_calls: vec![ToolCall {
            id: "c".into(),
            name: "Bash".into(),
            input: json!({"command": secret}),
            result: Some(ToolResultData {
                text: secret.into(),
                is_error: false,
                structured: Some(json!({"stdout": secret})),
            }),
        }],
    };
    r().redact_iteration(&mut it);
    let dumped = serde_json::to_string(&it).unwrap();
    assert!(!dumped.contains("abcdefgh123"), "{dumped}");
    assert_eq!(dumped.matches("[REDACTED:env]").count(), 7);
}

#[test]
fn masks_truncated_pem_without_end_marker() {
    let text = "a\n-----BEGIN PRIVATE KEY-----\nMIIEow\nxyz";
    assert_eq!(r().redact(text), "a\n[REDACTED:private_key]");
}

#[test]
fn extra_kind_is_not_treated_as_replacement_template() {
    let red = Redactor::new(&[ExtraPattern {
        kind: "$1${0}".into(),
        pattern: r"(HCI)-\d{6}".into(),
    }])
    .unwrap();
    assert_eq!(red.redact("id HCI-123456"), "id [REDACTED:$1${0}]");
}
