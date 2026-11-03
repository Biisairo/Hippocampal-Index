//! hook 설정과 launchd plist 생성. hook 명령의 경로는 셸 표기, plist는 절대 경로이다. (IF-HK-4, IF-CLI-4, FR-INS-10)

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config::Paths;

const CAPTURE_HOOKS: &[(&str, &str)] = &[
    ("PostToolBatch", "post-tool-batch"),
    ("Stop", "stop"),
    ("StopFailure", "stop-failure"),
    ("SessionEnd", "session-end"),
];
const LAUNCHD_LABEL: &str = "com.hippo.daemon";
const HOME_PREFIX: &str = "~/";

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// 셸 표기: `$HOME` 아래면 `~/<나머지>`, 아니면 절대 경로. 셸 특수 문자가 있으면 그 부분만 작은따옴표로 감싼다. (FR-INS-10)
pub fn shell_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if !rest.as_os_str().is_empty() => {
            format!("{HOME_PREFIX}{}", shell_quote(&rest.to_string_lossy()))
        }
        _ => shell_quote(&path.to_string_lossy()),
    }
}

/// 셸에서 그대로 쓸 수 있는 문자만 있으면 그대로, 아니면 작은따옴표로 감싼다.
fn shell_quote(text: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "/._-+".contains(c);
    if !text.is_empty() && text.chars().all(safe) {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub fn hooks_config(exe: &Path, inject_timeout_secs: u64) -> Value {
    let exe = shell_path(exe, home_dir().as_deref());
    let mut hooks = serde_json::Map::new();
    for (event, arg) in CAPTURE_HOOKS {
        hooks.insert(
            (*event).to_string(),
            json!([{"hooks": [{"type": "command", "command": format!("{exe} hook {arg}"), "async": true}]}]),
        );
    }
    hooks.insert(
        "UserPromptSubmit".to_string(),
        json!([{"hooks": [{"type": "command", "command": format!("{exe} hook user-prompt-submit"), "timeout": inject_timeout_secs}]}]),
    );
    json!({"hooks": hooks})
}

pub fn launchd_plist(exe: &Path, paths: &Paths) -> String {
    let logs = paths.logs();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>daemon</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>StandardOutPath</key>
  <string>{out}</string>
  <key>StandardErrorPath</key>
  <string>{err}</string>
</dict>
</plist>
"#,
        exe = exe.display(),
        out = logs.join("launchd.out.log").display(),
        err = logs.join("launchd.err.log").display(),
    )
}
