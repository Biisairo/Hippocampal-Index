//! 저장 전 비밀값 마스킹. (FR-STO-6, QA-4)

use regex::{Captures, Regex};
use serde_json::Value;

use crate::transcript::{Iteration, IterationInput};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtraPattern {
    pub kind: String,
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidPattern {
    pub kind: String,
    pub message: String,
}

enum Replace {
    Whole,
    /// 캡처 그룹 1, 2(이름과 구분자)는 남기고 값만 가린다.
    ValueOnly,
}

struct Rule {
    kind: String,
    re: Regex,
    replace: Replace,
}

const BUILTIN: &[(&str, &str, bool)] = &[
    (
        "private_key",
        r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----(?:.*?-----END [A-Z ]*PRIVATE KEY-----|.*\z)",
        false,
    ),
    ("api_key", r"\bsk-[A-Za-z0-9_-]{20,}", false),
    ("api_key", r"\bgh[pousr]_[A-Za-z0-9]{36,}", false),
    ("api_key", r"\bxox[abprs]-[A-Za-z0-9-]{10,}", false),
    ("api_key", r"\bAKIA[0-9A-Z]{16}\b", false),
    ("bearer", r"\bBearer\s+[A-Za-z0-9._~+/=-]{16,}", false),
    (
        "env",
        r#"(?i)\b([A-Za-z0-9_]*(?:key|token|secret|password))(\s*[=:]\s*["']?)([^\s"']{8,})"#,
        true,
    ),
];

pub struct Redactor {
    rules: Vec<Rule>,
}

impl Redactor {
    pub fn new(extra: &[ExtraPattern]) -> Result<Self, InvalidPattern> {
        let builtin = BUILTIN.iter().map(|(kind, pattern, value_only)| {
            let replace = if *value_only {
                Replace::ValueOnly
            } else {
                Replace::Whole
            };
            (kind.to_string(), pattern.to_string(), replace)
        });
        let extras = extra
            .iter()
            .map(|p| (p.kind.clone(), p.pattern.clone(), Replace::Whole));
        let rules = builtin
            .chain(extras)
            .map(|(kind, pattern, replace)| match Regex::new(&pattern) {
                Ok(re) => Ok(Rule { kind, re, replace }),
                Err(e) => Err(InvalidPattern {
                    kind,
                    message: e.to_string(),
                }),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { rules })
    }

    pub fn redact(&self, text: &str) -> String {
        self.rules.iter().fold(text.to_string(), |acc, rule| {
            let mask = format!("[REDACTED:{}]", rule.kind);
            rule.re
                .replace_all(&acc, |caps: &Captures| match rule.replace {
                    Replace::Whole => mask.clone(),
                    Replace::ValueOnly => {
                        let group = |i| caps.get(i).map_or("", |m| m.as_str());
                        format!("{}{}{mask}", group(1), group(2))
                    }
                })
                .into_owned()
        })
    }

    pub fn redact_value(&self, v: &mut Value) {
        match v {
            Value::String(s) => *s = self.redact(s),
            Value::Array(items) => items.iter_mut().for_each(|i| self.redact_value(i)),
            Value::Object(map) => map.values_mut().for_each(|i| self.redact_value(i)),
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    pub fn redact_iteration(&self, it: &mut Iteration) {
        for input in &mut it.inputs {
            match input {
                IterationInput::Prompt { text } | IterationInput::AgentMessage { text } => {
                    *text = self.redact(text)
                }
                IterationInput::TaskNotification(n) => n.summary = self.redact(&n.summary),
            }
        }
        for text in &mut it.texts {
            *text = self.redact(text);
        }
        for call in &mut it.tool_calls {
            self.redact_value(&mut call.input);
            if let Some(result) = call.result.as_mut() {
                result.text = self.redact(&result.text);
                if let Some(structured) = result.structured.as_mut() {
                    self.redact_value(structured);
                }
            }
        }
    }
}
