//! 설정 키 설명. README 5장과 같은 내용이다. (FR-GUI-4)

use crate::i18n::Lang;

pub fn describe(section: &str, key: &str, lang: Lang) -> &'static str {
    let (en, ko) = match (section, key) {
        ("embedder", "backend") => ("ollama, or hash (tests only)", "ollama 또는 hash(테스트용)"),
        ("embedder", "model") => ("Embedding model name", "임베딩 모델 이름"),
        ("embedder", "dimension") => (
            "Vector dimension (Qwen3-Embedding supports 32–4096)",
            "벡터 차원 (Qwen3-Embedding은 32~4096 지원)",
        ),
        ("embedder", "max_input_tokens") => ("Model input limit in tokens", "모델 입력 한도(토큰)"),
        ("embedder", "url") => (
            "Embedding server URL. Only localhost, 127.0.0.1, ::1",
            "임베딩 서버 URL. localhost, 127.0.0.1, ::1만 허용",
        ),
        ("embedder", "keep_alive") => (
            "How long Ollama keeps the model in memory",
            "Ollama가 모델을 메모리에 두는 시간",
        ),
        ("embedder", "autostart") => (
            "Start `ollama serve` when the server is unreachable",
            "서버에 연결할 수 없으면 `ollama serve`를 띄움",
        ),
        ("embedder", "ollama_command") => {
            ("Path of the ollama executable", "ollama 실행 파일 경로")
        }
        ("embedder", "query_prefix") => (
            "Instruction put before search queries only. Empty for none. Restart needed, no reindex",
            "검색 질의 앞에만 붙이는 지시문. 비우면 붙이지 않음. 재시작만 필요, 재색인 불필요",
        ),
        ("vector", "backend") => (
            "lancedb, or memory (tests only)",
            "lancedb 또는 memory(테스트용)",
        ),
        ("vector", "name") => (
            "Directory: projects/<id>/vectors/<name>/",
            "디렉터리: projects/<id>/vectors/<name>/",
        ),
        ("chunking", "max_tokens") => (
            "Chunk limit = min(model limit, this) × chars_per_token",
            "청크 한도 = min(모델 한도, 이 값) × chars_per_token",
        ),
        ("chunking", "chars_per_token") => ("Characters per token", "토큰당 문자 수"),
        ("chunking", "overlap_chars") => (
            "Characters shared by neighboring chunks. Must be smaller than the chunk limit",
            "이웃 청크가 공유하는 문자 수. 청크 한도보다 작아야 함",
        ),
        ("embed_text", "max_tool_input_chars") => (
            "Cut tool input JSON to this length in the embedding text only",
            "임베딩 텍스트에서만 도구 입력 JSON을 이 길이로 자름",
        ),
        ("embed_text", "answer_tools") => (
            "Tools whose result (the user's answer) is stored as [answer]",
            "결과(사용자의 답)를 [answer]로 넣는 도구",
        ),
        ("inject", "threshold") => (
            "Minimum vector similarity for automatic injection",
            "자동 주입의 최소 벡터 유사도",
        ),
        ("inject", "top_k") => ("Number of turns to inject", "주입할 턴 수"),
        ("inject", "enabled") => (
            "Inject memories (false: paused). Applies from the next prompt",
            "기억 주입 (false면 일시정지). 다음 프롬프트부터 적용",
        ),
        ("capture", "enabled") => (
            "Save records (false: paused). Open sessions catch up after resuming",
            "기록 저장 (false면 일시정지). 재생하면 열린 세션은 밀린 기록도 저장",
        ),
        ("inject", "total_chars") => (
            "Total injection limit. Turns over it are skipped whole",
            "주입 전체 길이 한도. 넘는 턴은 통째로 뺌",
        ),
        ("inject", "budget_ms") => (
            "Time budget of the UserPromptSubmit hook",
            "UserPromptSubmit hook의 시간 예산",
        ),
        ("inject", "cold_budget_ms") => (
            "Budget when the daemon or model is cold. Not smaller than budget_ms",
            "데몬이나 모델이 콜드일 때의 예산. budget_ms 이상",
        ),
        ("inject", "min_prompt_chars") => (
            "Prompts shorter than this (trimmed, in characters) are not injected",
            "앞뒤 공백을 뺀 길이가 이 값보다 짧은 프롬프트는 주입하지 않음",
        ),
        ("index", "vector_index_threshold") => (
            "Build an ANN index when vectors reach this count",
            "벡터 수가 이 값에 이르면 ANN 인덱스를 만듦",
        ),
        ("index", "job_batch") => (
            "Embedding jobs per batch",
            "한 번에 처리하는 임베딩 작업 수",
        ),
        ("index", "optimize_every") => (
            "Compact the vector store and drop old versions after this many changes (0 = only when the queue is empty)",
            "벡터 저장소를 이만큼 수정하면 파일을 합치고 이전 버전을 지움(0이면 큐가 빌 때만)",
        ),
        ("daemon", "autostart") => (
            "Hooks, CLI and GUI start the daemon when it is not running",
            "데몬이 없으면 hook, CLI, GUI가 띄움",
        ),
        ("daemon", "idle_timeout_secs") => (
            "Exit after this many idle seconds (0: never)",
            "요청 없이 이 시간(초)이 지나면 종료 (0이면 종료하지 않음)",
        ),
        ("daemon", "max_open_projects") => (
            "Project vector stores kept open at once",
            "동시에 열어 둘 프로젝트 벡터 저장소 수",
        ),
        ("daemon", "prevent_sleep") => (
            "Keep the Mac from idle sleep while work is pending",
            "처리할 작업이 남은 동안 Mac의 유휴 잠자기를 막음",
        ),
        ("project", "git_timeout_ms") => (
            "git time limit. Over it, the folder counts as non-git",
            "git 실행 시간 제한. 넘으면 git이 아닌 폴더로 봄",
        ),
        ("import", "transcripts_dir") => (
            "Where past transcripts are found (`~` is not expanded)",
            "과거 transcript를 찾는 디렉터리 (`~`는 펼치지 않음)",
        ),
        ("ui", "language") => (
            "Output language: en or ko. Text for Claude stays English",
            "출력 언어: en 또는 ko. Claude에게 주는 텍스트는 항상 영어",
        ),
        ("ui", "theme") => (
            "GUI theme: system (follow macOS), light or dark",
            "GUI 테마: system(macOS 설정을 따름), light, dark",
        ),
        ("redact", "patterns") => (
            "Extra secret patterns (regular expressions)",
            "추가 비밀값 패턴 (정규식)",
        ),
        _ => ("", ""),
    };
    match lang {
        Lang::En => en,
        Lang::Ko => ko,
    }
}
