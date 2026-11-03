//! Ollama 임베더 백엔드. localhost의 `/api/embed`를 호출한다. (C-3, C-4)

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::{EmbedderConfig, is_local_url};
use crate::embedder::{EmbedError, Embedder};
use crate::fsutil::{ensure_private_dir, open_private_append};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const EMBED_PATH: &str = "/api/embed";
const VERSION_PATH: &str = "/api/version";
const PS_PATH: &str = "/api/ps";
const DEFAULT_TAG: &str = ":latest";
const DEFAULT_PORT: u16 = 11434;
const SERVER_START_WAIT: Duration = Duration::from_secs(10);
const SERVER_POLL: Duration = Duration::from_millis(100);
#[cfg(unix)]
const EXECUTABLE_BITS: u32 = 0o111;

/// Ollama 서버를 띄우는 방법. (FR-EMB-1)
pub type ServerLauncher = Box<dyn Fn() -> std::io::Result<()> + Send + Sync>;

/// launchd나 hook처럼 PATH가 짧은 환경에서도 찾도록, PATH 다음에 살펴보는 디렉터리.
pub const FALLBACK_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

/// 경로 구분자가 없는 명령은 `path_env`의 디렉터리, 그다음 `fallbacks`에서 실행 파일을 찾는다.
/// 찾지 못하면 명령을 그대로 돌려준다. (FR-EMB-1)
pub fn resolve_command(
    command: &str,
    path_env: Option<&std::ffi::OsStr>,
    fallbacks: &[&Path],
) -> std::path::PathBuf {
    if command.contains('/') {
        return command.into();
    }
    let from_path = path_env.into_iter().flat_map(std::env::split_paths);
    let from_fallbacks = fallbacks.iter().map(|d| d.to_path_buf());
    from_path
        .chain(from_fallbacks)
        .map(|dir| dir.join(command))
        .find(|candidate| is_executable(candidate))
        .unwrap_or_else(|| command.into())
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & EXECUTABLE_BITS != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// `command serve`를 분리된 프로세스로 띄운다. `OLLAMA_HOST`는 URL의 host:port. 띄운 프로세스 id를 돌려준다.
pub fn spawn_ollama(command: &str, url: &str, log: &Path) -> std::io::Result<u32> {
    let uri: ureq::http::Uri = url
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e}")))?;
    let authority = uri
        .authority()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "url has no host"))?;
    let host = format!(
        "{}:{}",
        authority.host(),
        authority.port_u16().unwrap_or(DEFAULT_PORT)
    );
    if let Some(dir) = log.parent() {
        ensure_private_dir(dir)?;
    }
    let stdout = open_private_append(log)?;
    let stderr = stdout.try_clone()?;
    let fallbacks: Vec<&Path> = FALLBACK_BIN_DIRS.iter().map(Path::new).collect();
    let program = resolve_command(command, std::env::var_os("PATH").as_deref(), &fallbacks);
    let mut child = Command::new(program);
    child
        .arg("serve")
        .env("OLLAMA_HOST", host)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        child.process_group(0);
    }
    Ok(child.spawn()?.id())
}

pub struct OllamaEmbedder {
    endpoint: String,
    model: String,
    dimension: usize,
    max_input_tokens: usize,
    keep_alive: String,
    agent: ureq::Agent,
    version_endpoint: String,
    ps_endpoint: String,
    launcher: Option<ServerLauncher>,
    launch_lock: Mutex<()>,
}

impl OllamaEmbedder {
    /// 서버 자동 시작 없이 만든다.
    pub fn try_new(config: &EmbedderConfig) -> Result<Self, EmbedError> {
        Self::try_new_with_launcher(config, None)
    }

    /// URL이 localhost가 아니면 거부한다. 설정 파싱을 거치지 않는 경로도 막기 위해 여기서 다시 검사한다. (C-4)
    pub fn try_new_with_launcher(
        config: &EmbedderConfig,
        launcher: Option<ServerLauncher>,
    ) -> Result<Self, EmbedError> {
        if !is_local_url(&config.url) {
            return Err(EmbedError::Config(format!(
                "embedder url must point to localhost: {}",
                config.url
            )));
        }
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .build()
            .into();
        Ok(Self {
            endpoint: format!("{}{EMBED_PATH}", config.url.trim_end_matches('/')),
            model: config.model.clone(),
            dimension: config.dimension,
            max_input_tokens: config.max_input_tokens,
            keep_alive: config.keep_alive.clone(),
            agent,
            version_endpoint: format!("{}{VERSION_PATH}", config.url.trim_end_matches('/')),
            ps_endpoint: format!("{}{PS_PATH}", config.url.trim_end_matches('/')),
            launcher,
            launch_lock: Mutex::new(()),
        })
    }

    fn server_up(&self) -> bool {
        self.agent
            .get(&self.version_endpoint)
            .call()
            .is_ok_and(|r| r.status().is_success())
    }

    /// 서버를 한 번만 띄우고 응답할 때까지 기다린다. 다른 스레드가 이미 띄웠으면 기다리기만 한다. (FR-EMB-2)
    fn start_server(&self, launcher: &ServerLauncher) -> Result<(), EmbedError> {
        let _guard = self
            .launch_lock
            .lock()
            .map_err(|e| EmbedError::Unavailable(e.to_string()))?;
        if self.server_up() {
            return Ok(());
        }
        launcher().map_err(|e| EmbedError::Unavailable(format!("cannot start ollama: {e}")))?;
        let deadline = Instant::now() + SERVER_START_WAIT;
        while Instant::now() < deadline {
            if self.server_up() {
                return Ok(());
            }
            std::thread::sleep(SERVER_POLL);
        }
        Err(EmbedError::Unavailable(
            "ollama did not start in time".into(),
        ))
    }

    fn post(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.post_body(texts)
    }

    fn post_body(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let body = json!({
            "model": self.model,
            "input": texts,
            "dimensions": self.dimension,
            "keep_alive": self.keep_alive,
        });
        let mut response = self
            .agent
            .post(&self.endpoint)
            .send_json(&body)
            .map_err(|e| EmbedError::Unavailable(e.to_string()))?;
        let status = response.status().as_u16();
        let payload: Value = response
            .body_mut()
            .read_json()
            .map_err(|e| EmbedError::InvalidResponse(format!("status {status}: {e}")))?;
        if !(200..300).contains(&status) {
            let message = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(EmbedError::InvalidResponse(format!(
                "status {status}: {message}"
            )));
        }
        let vectors: Vec<Vec<f32>> = payload
            .get("embeddings")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| EmbedError::InvalidResponse(e.to_string()))?
            .ok_or_else(|| EmbedError::InvalidResponse("missing embeddings".into()))?;
        if vectors.len() != texts.len() {
            return Err(EmbedError::InvalidResponse(format!(
                "expected {} vectors, got {}",
                texts.len(),
                vectors.len()
            )));
        }
        if let Some(bad) = vectors.iter().find(|v| v.len() != self.dimension) {
            return Err(EmbedError::InvalidResponse(format!(
                "expected dimension {}, got {}",
                self.dimension,
                bad.len()
            )));
        }
        Ok(vectors)
    }
}

impl Embedder for OllamaEmbedder {
    fn model_id(&self) -> &str {
        &self.model
    }
    fn dimension(&self) -> usize {
        self.dimension
    }
    fn max_input_tokens(&self) -> usize {
        self.max_input_tokens
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        match (self.post(texts), &self.launcher) {
            (Err(EmbedError::Unavailable(_)), Some(launcher)) => {
                self.start_server(launcher)?;
                self.post(texts)
            }
            (result, _) => result,
        }
    }

    /// `GET /api/ps`의 `models[].name`에 설정 모델이 있는지. 태그가 없으면 `:latest`로 본다. (FR-RET-5)
    fn is_loaded(&self) -> bool {
        let Ok(mut response) = self.agent.get(&self.ps_endpoint).call() else {
            return false;
        };
        if !response.status().is_success() {
            return false;
        }
        let Ok(payload) = response.body_mut().read_json::<Value>() else {
            return false;
        };
        let tagged = if self.model.contains(':') {
            self.model.clone()
        } else {
            format!("{}{DEFAULT_TAG}", self.model)
        };
        payload
            .get("models")
            .and_then(Value::as_array)
            .is_some_and(|models| {
                models
                    .iter()
                    .filter_map(|m| m.get("name").and_then(Value::as_str))
                    .any(|name| name == tagged)
            })
    }
}
