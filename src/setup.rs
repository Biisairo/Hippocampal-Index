//! hook 설정과 launchd plist 생성. (IF-HK-4, IF-CLI-4)

use std::path::Path;

use serde_json::{Value, json};

use crate::config::Paths;

const CAPTURE_HOOKS: &[(&str, &str)] = &[
    ("PostToolBatch", "post-tool-batch"),
    ("Stop", "stop"),
    ("StopFailure", "stop-failure"),
    ("SessionEnd", "session-end"),
];
const LAUNCHD_LABEL: &str = "com.hippo.daemon";

pub fn hooks_config(exe: &Path, inject_timeout_secs: u64) -> Value {
    let exe = exe.display();
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
  <true/>
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
