# 소프트웨어 요구사항 명세서 (SRS) — session-memory

| 항목 | 값 |
|------|----|
| 문서 버전 | 0.1 (초안) |
| 작성일 | 2026-11-02 |
| 작성자 | biisairo |
| 준거 표준 | ISO/IEC/IEEE 29148:2018 |
| 상태 | 검토 대기 |

---

## 1. 서론

### 1.1 목적
이 문서는 Claude Code 세션의 대화를 agentic loop 반복 단위로 로컬 벡터 DB에 저장하고, 이후 대화에서 그 기록을 검색해 답변에 활용하는 개인 기억 시스템 `session-memory`의 요구사항을 정의한다. 구현, 테스트, 인수 판단은 이 문서를 단일 기준으로 삼는다.

### 1.2 범위
`session-memory`는 단일 Rust 바이너리 `smem`으로 제공되며 다음을 수행한다.
- Claude Code hook으로 메인 agentic loop의 반복을 수집한다.
- 상주 데몬이 transcript를 해석해 반복과 도구 결과를 원본 저장소(SQLite)에 저장하고, 로컬 임베딩 모델로 벡터화해 벡터 저장소(기본 LanceDB)에 색인한다.
- 임베딩 모델과 벡터 저장소는 설정만 바꿔 교체할 수 있다. 벡터 저장소는 원본 저장소에서 언제든 다시 만들 수 있는 파생 인덱스이다.
- 사용자 프롬프트 제출 시 관련 기억을 자동 주입하고, Claude가 CLI로 추가 검색할 수 있게 한다.

모든 데이터와 연산은 사용자 로컬 장비 안에서 처리한다.

### 1.3 용어 정의

| 용어 | 정의 |
|------|------|
| 턴 (turn) | 사용자 프롬프트 1개를 받아 Claude가 제어를 사용자에게 돌려줄 때까지의 구간. `prompt_id` 1개에 대응한다. |
| agentic loop | 턴 안에서 "모델 호출 → 도구 실행"을 반복하는 내부 루프. |
| 반복 (iteration) | agentic loop의 1회 실행. transcript에서 같은 `message.id`를 가진 assistant 라인 묶음과, 그 안의 모든 `tool_use`에 대응하는 `tool_result` 묶음으로 구성된다. |
| 중간 반복 | `tool_use`를 1개 이상 포함하는 반복. |
| 마지막 반복 | `tool_use`를 포함하지 않는 반복. 턴을 끝낸다. |
| 반복 입력 | 반복 직전에 들어온 사용자 프롬프트, 백그라운드 완료 알림(`<task-notification>`), 서브에이전트 보고(`<agent-message>`). `tool_result`는 반복 입력에 포함하지 않는다. |
| 결과 (result) | `tool_result` 원문 또는 백그라운드 작업의 출력. |
| 의미 벡터 | 반복 1개의 입력, 도구 호출, LLM 응답 텍스트를 합쳐 만든 벡터. |
| 결과 벡터 | 결과 원문을 청크로 나눠 만든 벡터. |
| 백그라운드 작업 | `tool_input.run_in_background == true`로 시작된 Bash 명령 또는 Agent 호출. |
| 메인 루프 | hook 입력에 `agent_id` 필드가 없는 agentic loop. |
| 스풀 | 데몬이 응답하지 않을 때 hook 요청을 임시로 기록하는 로컬 파일. |
| 원본 저장소 | 반복, 결과 원문, 메타데이터, 전문 검색 인덱스를 담는 SQLite DB. 시스템의 기준 데이터이다. |
| 벡터 저장소 | 벡터와 필터용 메타데이터만 담는 파생 인덱스. 원본 저장소에서 재생성할 수 있다. |
| 임베더 | 텍스트를 벡터로 바꾸는 구성 요소. 임베딩 모델 1개와 그 호출 방식에 대응한다. |
| 백엔드 | 임베더 또는 벡터 저장소 인터페이스의 구체 구현. |

### 1.4 참조 문서
- [R1] Claude Code Hooks reference — https://code.claude.com/docs/en/hooks
- [R2] LanceDB 문서 — https://docs.lancedb.com
- [R3] Qwen3-Embedding — https://github.com/QwenLM/Qwen3-Embedding
- [R4] 실측 로그 — `probe/r_s1_notool` … `probe/r_s5_bg_bash` (부록 A)

### 1.5 문서 구성
2장은 제품 전체 설명, 3장은 개별 요구사항, 4장은 검증 방법, 부록은 실측 근거와 미해결 항목을 다룬다.

---

## 2. 전체 설명

### 2.1 제품 관점

```
Claude Code ──hook(stdin JSON)──▶ smem hook <event> ──Unix socket──▶ smem daemon
                                        │ (데몬 응답 없음)                   │
                                        └──────▶ 스풀 파일 ◀──── 시작 시 소진 ┤
                                                                           ├─▶ transcript JSONL 읽기
Claude (Bash) ──▶ smem search ... ──Unix socket──────────────────────────▶ ├─▶ 원본 저장소 SQLite (기준 데이터, FTS)
                                                                           ├─▶ Embedder 트레이트 ──▶ [기본] Ollama (localhost)
                                                                           └─▶ VectorStore 트레이트 ──▶ [기본] LanceDB
```

### 2.2 제품 기능 요약
1. 수집: 메인 루프의 모든 반복을 빠짐없이, 중복 없이 수집한다.
2. 백그라운드 연결: 백그라운드 작업의 시작 호출과 완료 결과를 한 쌍으로 연결한다.
3. 저장: 반복은 의미 벡터 1개로, 결과는 내용 해시로 중복을 제거해 저장한다.
4. 검색: 프롬프트 제출 시 자동 주입하고, CLI로 추가 검색한다.

### 2.3 사용자 특성
단일 사용자(개발자 본인)가 자신의 macOS 장비에서 사용한다. 다중 사용자 기능은 범위 밖이다.

### 2.4 제약 사항
- C-1 구현 언어는 Rust로 한다.
- C-2 원본 저장소는 SQLite(FTS5)로 한다. 벡터 저장소의 기본 백엔드는 LanceDB Rust SDK(임베디드)로 한다.
- C-3 임베더의 기본 백엔드는 Ollama `qwen3-embedding:0.6b`, 출력 차원 1024로 한다.
- C-4 데이터를 로컬 장비 밖으로 전송하지 않는다. 네트워크 통신은 `localhost`의 Ollama로 한정한다.
- C-5 MCP 서버를 사용하지 않는다. (사내 규정상 새 MCP 서버는 정보보호팀 승인 대상)
- C-6 1차 대상 플랫폼은 macOS(darwin)이다.
- C-7 `~/.claude` 설정 파일을 자동으로 수정하지 않는다. hook 설정은 사용자가 직접 적용한다.

### 2.5 가정과 의존성
- A-1 Claude Code의 hook 입력 스키마와 transcript JSONL 형식은 부록 A의 실측 결과(Claude Code 2.1.316)와 같다. 버전이 바뀌면 재검증한다.
- A-2 Ollama가 로컬에 설치되어 있고 대상 모델을 받아 둔 상태다.
- A-3 백그라운드 서브에이전트 완료 알림의 `<output-file>`은 서브에이전트 transcript를 가리키는 심볼릭 링크다. (부록 A, s4)
- A-4 백그라운드 shell의 `<output-file>`은 `/private/tmp` 아래에 있으며 이후 삭제될 수 있다.

### 2.6 비목표
- 서브에이전트 내부 반복 저장 (서브에이전트의 최종 보고만 결과로 저장한다)
- `thinking` 블록 저장
- LLM 요약 생성
- 원격 동기화, 다중 사용자, 웹 UI
- 보존 기간과 자동 삭제 정책
- MCP 도구 제공

---

## 3. 개별 요구사항

요구사항 형식: `ID` — 내용. 우선순위는 **필수(M)** / **권장(S)**로 표기한다.

### 3.1 외부 인터페이스 요구사항

#### 3.1.1 Hook 인터페이스
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| IF-HK-1 | `smem hook <event>`는 stdin으로 hook 입력 JSON 1개를 받아야 한다. `<event>`는 `post-tool-batch`, `stop`, `stop-failure`, `session-end`, `user-prompt-submit` 중 하나이다. | M |
| IF-HK-2 | `user-prompt-submit`을 뺀 모든 hook은 항상 종료 코드 0으로 끝나야 하며 stdout에 아무것도 출력하지 않아야 한다. | M |
| IF-HK-3 | `user-prompt-submit`은 주입할 기억이 있으면 `hookSpecificOutput.additionalContext`를 담은 JSON 1개를 stdout에 출력하고, 없으면 아무것도 출력하지 않아야 한다. 어떤 경우에도 프롬프트를 차단하지 않아야 한다(종료 코드 0). | M |
| IF-HK-4 | `smem hooks-config`는 Claude Code `settings.json`에 넣을 hook 설정 JSON을 stdout에 출력해야 한다. 수집 hook(`PostToolBatch`, `Stop`, `StopFailure`, `SessionEnd`)은 `async: true`로 설정한다. | M |

#### 3.1.2 CLI 인터페이스
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| IF-CLI-1 | `smem search <query> [--scope cwd\|all\|<path>] [--kind iteration\|result\|both] [--limit N]`는 검색 결과를 사람이 읽을 수 있는 텍스트로 출력해야 한다. 기본값은 `--scope cwd --kind iteration --limit 10`이다. | M |
| IF-CLI-2 | `smem show <prompt_id>`는 해당 턴의 반복을 순서대로 복원해 출력해야 한다. `--with-results` 옵션을 주면 연결된 결과 원문도 출력한다. | M |
| IF-CLI-3 | `smem daemon`은 데몬을 포그라운드로 실행해야 한다. | M |
| IF-CLI-4 | `smem launchd-plist`는 데몬 자동 실행용 launchd plist를 stdout에 출력해야 한다. | S |
| IF-CLI-5 | `smem status`는 데몬 상태, 스풀 대기 건수, 임베딩 대기 건수, 테이블별 행 수를 출력해야 한다. | S |

#### 3.1.3 데몬 소켓 인터페이스
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| IF-SK-1 | 데몬은 `~/.session-memory/smem.sock` Unix socket으로 줄 단위 JSON 요청과 응답을 주고받아야 한다. | M |
| IF-SK-2 | 요청 종류는 `ingest`(hook 이벤트 전달), `search`, `show`, `status`이다. | M |

#### 3.1.4 Skill
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| IF-SKL-1 | Claude가 과거 기억이 필요할 때 `smem search`와 `smem show`를 Bash로 호출하는 방법을 안내하는 skill 문서(`SKILL.md`)를 제공해야 한다. | M |

### 3.2 기능 요구사항

#### 3.2.1 수집 (CAP)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-CAP-1 | `PostToolBatch`, `Stop`, `StopFailure` 입력에 `agent_id` 필드가 있으면 아무 처리 없이 종료해야 한다. (서브에이전트 내부 루프 제외) | M |
| FR-CAP-2 | `PostToolBatch`(메인 루프)를 받으면 해당 세션에 대해 transcript 처리를 요청해야 한다. | M |
| FR-CAP-3 | `Stop`과 `StopFailure`(메인 루프)를 받으면 해당 세션에 대해 transcript 처리를 요청하고, 마지막 반복을 완료로 확정해야 한다. | M |
| FR-CAP-4 | `SessionEnd`를 받으면 해당 세션의 미처리 transcript 라인과 미처리 완료 알림을 모두 처리해야 한다. | M |
| FR-CAP-5 | hook 클라이언트는 데몬에 연결할 수 없으면 요청을 스풀 파일에 추가하고 종료해야 한다. | M |
| FR-CAP-6 | 데몬은 시작할 때 스풀에 쌓인 요청을 기록 순서대로 처리해야 한다. | M |

#### 3.2.2 transcript 해석 (EXT)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-EXT-1 | 데몬은 세션마다 처리 완료한 transcript 바이트 offset을 저장하고, 그 뒤의 라인만 읽어야 한다. offset은 해당 라인의 DB 반영이 끝난 뒤에만 갱신한다. | M |
| FR-EXT-2 | `type == "assistant"` 라인을 `message.id`로 묶어 반복 1개로 만들어야 한다. | M |
| FR-EXT-3 | 반복에서 `text` 블록은 저장하고, `thinking` 블록은 저장하지 않아야 한다. | M |
| FR-EXT-4 | 반복의 `tool_use` 블록마다 `name`, `input`, `id`를 저장해야 한다. | M |
| FR-EXT-5 | `type == "user"` 라인의 `tool_result` 블록은 `tool_use_id`로 해당 `tool_use`가 속한 반복에 연결해야 한다. | M |
| FR-EXT-6 | `type == "user"`이고 content가 문자열인 라인 중 다음은 반복 입력으로 다뤄야 한다: (a) 사용자 프롬프트, (b) `<task-notification>`을 포함한 메시지, (c) `<agent-message>`를 포함한 메시지. 내용이 `<system-reminder>` 블록만으로 이루어진 메시지는 제외한다. 반복 입력은 그 뒤에 처음 나오는 반복에 붙인다. | M |
| FR-EXT-7 | 중간 반복은 모든 `tool_use`에 대응하는 `tool_result`가 transcript에 나타난 뒤에 완료로 판단해야 한다. 미완료 반복은 다음 처리 때까지 보류한다. | M |
| FR-EXT-8 | 마지막 반복은 같은 `prompt_id`에 대한 `Stop`, `StopFailure`, `SessionEnd` 중 하나를 받은 뒤에 완료로 판단해야 한다. | M |
| FR-EXT-9 | `user`, `assistant`가 아닌 라인 타입(`attachment`, `system`, `queue-operation` 등)은 무시해야 한다. | M |
| FR-EXT-10 | 반복 id는 `session_id`와 `message.id`의 조합으로 만들어야 하며, 같은 반복을 다시 처리해도 DB에 중복 행이 생기지 않아야 한다. | M |

#### 3.2.3 백그라운드 작업 (BG)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-BG-1 | `tool_input.run_in_background == true`인 `Bash` 또는 `Agent` 호출은 `tool_use_id`를 키로 하는 대기(pending) 기록을 만들어야 한다. 작업 id는 Bash의 경우 `tool_response.backgroundTaskId`, Agent의 경우 tool_result 텍스트의 `agentId: <id>`에서 얻는다. | M |
| FR-BG-2 | 반복 입력에서 `<task-notification>`을 찾으면 `task-id`, `tool-use-id`, `output-file`, `status`, `summary`를 파싱해야 한다. | M |
| FR-BG-3 | 파싱한 `tool-use-id`로 대기 기록을 찾아 완료 결과를 연결해야 한다. 대기 기록이 없으면 알림만 반복 입력으로 저장한다. | M |
| FR-BG-4 | Bash 작업은 알림을 감지한 즉시 `output-file` 내용을 읽어 결과로 저장해야 한다. 파일이 없으면 `status`와 `summary`만 저장한다. | M |
| FR-BG-5 | Agent 작업은 `output-file`(서브에이전트 transcript)에서 마지막 `SubagentHandback` 호출의 `tool_input.message`를 결과로 저장해야 한다. 이 호출이 없으면 마지막 assistant `text`를 저장한다. | M |
| FR-BG-6 | 같은 `task-id`로 알림이 여러 번 오면 기존 결과를 덮어쓰지 않고 순번을 붙여 추가해야 한다. | M |

#### 3.2.4 저장 (STO)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-STO-1 | 원본 저장소의 `iterations` 테이블에 반복마다 행 1개를 저장해야 한다. 필드: `id`, `session_id`, `prompt_id`, `message_id`, `iteration_index`(턴 안 순번), `cwd`, `ts`, `input_text`, `tool_calls`(JSON), `response_text`, `result_ids`, `bg_links`, `embed_text`. 벡터 저장소에는 `id`, `vector`, `embed_model`과 필터용 메타데이터(`cwd`, `session_id`, `ts`)를 저장한다. | M |
| FR-STO-2 | `embed_text`는 다음 순서로 만든다. 비어 있는 섹션은 넣지 않는다.<br>`[입력] <반복 입력>`<br>`[툴] <name>(<input 요약 JSON>)` (호출마다 1줄)<br>`[응답] <text 블록 이어 붙임>` | M |
| FR-STO-3 | 원본 저장소의 `results` 테이블에 결과 원문을 저장해야 한다. 필드: `id`, `content_hash`(SHA-256), `tool_name`, `text`, `chunk_index`, `first_seen_ts`. 벡터는 벡터 저장소에 청크 단위로 저장한다. | M |
| FR-STO-4 | 같은 `content_hash`가 이미 있으면 결과를 새로 저장하거나 임베딩하지 않고 기존 결과 id를 반복에 연결해야 한다. | M |
| FR-STO-5 | 결과 원문이 임베더의 `max_input_tokens()`와 설정값(기본 8,000 토큰 상당) 중 작은 값을 넘으면 여러 청크로 나눠 청크마다 벡터를 만들어야 한다. | M |
| FR-STO-6 | 저장과 임베딩 전에 비밀값 패턴을 `[REDACTED:<종류>]`로 치환해야 한다. 기본 패턴: API 키 접두사(`sk-`, `ghp_`, `gho_`, `xox[bp]-`, `AKIA`), `Bearer <토큰>`, PEM 개인 키 블록, `*_KEY=`/`*_TOKEN=`/`*_SECRET=`/`PASSWORD=` 형태의 값. 패턴은 설정 파일로 추가할 수 있어야 한다. | M |
| FR-STO-7 | 임베딩은 반복 저장과 분리된 작업 큐로 처리해야 한다. Ollama가 응답하지 않으면 작업을 큐에 남기고 재시도해야 한다. | M |
| FR-STO-8 | 원본 저장소에서 `iterations.embed_text`와 `results.text`에 SQLite FTS5 인덱스(trigram 토크나이저)를 만들어야 한다. | M |
| FR-STO-9 | 벡터 행 수가 설정된 기준(기본 10만)을 넘으면 `VectorStore::ensure_index`를 호출해야 한다. | S |

#### 3.2.5 검색 (RET)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-RET-1 | `user-prompt-submit` hook은 프롬프트로 `iterations`를 검색해야 한다. 조건: `cwd`가 현재 cwd와 같을 것, `session_id`가 현재 세션과 다를 것. | M |
| FR-RET-2 | 검색은 벡터 검색과 FTS 결과를 합친 하이브리드 순위(RRF)로 해야 한다. | M |
| FR-RET-3 | 자동 주입은 유사도가 임계값(설정, 기본 0.55) 이상인 결과 중 상위 k개(설정, 기본 3개)만 대상으로 해야 한다. 대상이 없으면 아무것도 주입하지 않는다. | M |
| FR-RET-4 | 주입 텍스트는 결과마다 `ts`, `prompt_id`, `embed_text`(최대 길이 설정, 기본 800자)를 담고, 전체 길이를 설정값(기본 4,000자) 이하로 제한해야 한다. 끝에 `smem show <prompt_id>`로 상세 내용을 볼 수 있다는 안내를 붙인다. | M |
| FR-RET-5 | `user-prompt-submit` hook은 시간 예산(설정, 기본 800ms)을 넘으면 검색을 포기하고 아무것도 주입하지 않아야 한다. | M |
| FR-RET-6 | `smem search`는 IF-CLI-1의 옵션에 따라 범위와 종류를 바꿔 검색해야 한다. `--kind result` 결과에는 그 결과를 참조하는 반복의 `prompt_id`를 함께 출력한다. | M |
| FR-RET-7 | `smem show`는 `prompt_id`의 반복을 `iteration_index` 순서로 출력하고, 백그라운드 연결(`bg_links`)이 있으면 완료 결과가 저장된 반복을 함께 표시해야 한다. | M |

#### 3.2.6 교체 가능성 (SWP)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-SWP-1 | 임베더는 `Embedder` 트레이트로 추상화해야 한다. 트레이트는 최소한 `model_id()`, `dimension()`, `max_input_tokens()`, `embed(texts) -> vectors`를 제공한다. 임베더를 사용하는 코드는 구체 백엔드 타입에 의존하지 않아야 한다. | M |
| FR-SWP-2 | 벡터 저장소는 `VectorStore` 트레이트로 추상화해야 한다. 트레이트는 최소한 `upsert`, `delete`, `search(vector, filter, limit)`, `count`, `ensure_index`를 제공한다. 필터는 백엔드 중립 타입(`cwd`, `session_id` 제외, `kind`, `embed_model`)으로 표현한다. | M |
| FR-SWP-3 | 사용할 임베더 백엔드, 모델, 차원과 벡터 저장소 백엔드는 `config.toml`로 선택해야 한다. 백엔드를 추가할 때 수집, 해석, 검색 로직을 수정하지 않아야 한다. | M |
| FR-SWP-4 | 원본 저장소는 벡터 없이도 반복, 결과, 임베딩 대상 텍스트를 모두 보관해야 한다. 벡터 저장소의 내용은 원본 저장소에서 다시 만들 수 있어야 한다. | M |
| FR-SWP-5 | `smem reindex [--embedder <설정명>] [--store <설정명>]`는 원본 저장소를 기준으로 새 벡터 저장소를 만들어야 한다. 재색인 중에도 기존 벡터 저장소로 검색할 수 있어야 하며, 완료 후 활성 저장소를 전환한다. | M |
| FR-SWP-6 | 데몬은 시작할 때 활성 벡터 저장소의 `embed_model`, 차원과 설정의 임베더가 다르면 수집은 계속하되, 벡터 검색을 끄고 FTS 검색만 수행하며 `smem status`와 로그에 재색인 필요를 표시해야 한다. | M |
| FR-SWP-7 | 하이브리드 검색의 FTS 부분은 원본 저장소(SQLite FTS5)에서 수행해 벡터 저장소 백엔드와 무관하게 동작해야 한다. | M |
| FR-SWP-8 | 테스트용으로 결정론적 `Embedder`와 메모리 기반 `VectorStore` 백엔드를 제공하고, 모든 백엔드에 같은 계약 테스트를 적용해야 한다. | M |

#### 3.2.7 데몬 (DMN)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-DMN-1 | 데몬은 장비당 1개만 실행되어야 한다. 이미 실행 중이면 두 번째 실행은 오류 메시지를 출력하고 종료한다. | M |
| FR-DMN-2 | 원본 저장소와 벡터 저장소 쓰기는 데몬만 수행해야 한다. | M |
| FR-DMN-3 | 데몬은 세션 단위 처리 요청을 직렬로 처리해 같은 세션을 동시에 처리하지 않아야 한다. | M |
| FR-DMN-4 | 데몬은 처리 오류를 `~/.session-memory/logs/`에 기록하고 계속 동작해야 한다. | M |

### 3.3 성능 요구사항
| ID | 요구사항 |
|----|----------|
| PR-1 | 수집 hook 클라이언트는 실행 시작부터 종료까지 p95 50ms 이하여야 한다. |
| PR-2 | `user-prompt-submit` hook은 시간 예산(FR-RET-5)을 넘지 않아야 한다. 데몬이 준비된 상태에서 p95 300ms 이하를 목표로 한다. |
| PR-3 | `iterations` 1,000만 행에서 `smem search` 응답은 p95 500ms 이하여야 한다. |
| PR-4 | 데몬은 하루 3.5만 청크를 하루 안에 임베딩할 수 있어야 한다. |

### 3.4 논리 데이터 요구사항
| ID | 요구사항 |
|----|----------|
| DR-1 | 데이터 루트는 `~/.session-memory/`이며 하위에 `records.sqlite`(원본 저장소), `vectors/<저장소명>/`(벡터 저장소), `spool/`, `state/`(세션 offset, 대기 기록, 임베딩 큐), `logs/`, `config.toml`을 둔다. |
| DR-2 | 데이터 루트의 권한은 `0700`이어야 한다. |
| DR-3 | 모든 벡터 행은 생성에 쓴 `embed_model`과 차원을 기록해야 한다. |

### 3.5 품질 속성
| ID | 요구사항 |
|----|----------|
| QA-1 (신뢰성) | 데몬이 비정상 종료되어도 수집한 반복을 잃지 않아야 한다. 재시작하면 offset 이후부터 다시 처리한다. |
| QA-2 (신뢰성) | 같은 transcript를 여러 번 처리해도 결과 DB가 같아야 한다(멱등성). |
| QA-3 (보안) | 데이터는 로컬 장비 밖으로 전송되지 않아야 한다. (C-4) |
| QA-4 (보안) | 비밀값 마스킹(FR-STO-6)을 거치지 않은 텍스트는 DB에 기록되지 않아야 한다. |
| QA-5 (무간섭) | `smem`의 어떤 오류도 Claude Code 세션 진행을 막거나 오류 메시지를 띄우지 않아야 한다. |

---

## 4. 검증

| 요구사항 | 방법 | 검증 내용 |
|----------|------|-----------|
| IF-HK-*, FR-CAP-* | 테스트 | 부록 A의 실측 hook 입력을 fixture로 사용해 이벤트별 동작과 `agent_id` 필터를 확인 |
| FR-EXT-* | 테스트 | 실측 transcript fixture(s1~s5)로 반복 분리, 입력 연결, 완료 판정, 멱등성 확인 |
| FR-BG-* | 테스트 | s4, s5 fixture로 대기 기록 생성, 알림 파싱, 결과 연결, 반복 알림 순번 확인. output-file이 없는 경우 포함 |
| FR-STO-1~5, 7~9 | 테스트 | 임시 원본 저장소와 메모리 벡터 저장소에 저장 후 행, 해시 중복 제거, 청크 분할 확인. 임베딩은 테스트용 결정론적 임베더로 대체 |
| FR-STO-6, QA-4 | 테스트 | 패턴별 비밀값이 DB에 남지 않는지 확인 |
| FR-RET-* | 테스트 | 고정 데이터셋으로 필터, 임계값, 길이 제한, 시간 예산 초과 시 무주입 확인 |
| FR-SWP-* | 테스트 | 계약 테스트를 모든 백엔드에 적용. 임베더 교체 후 `reindex` 전후 검색 동작, 차원 불일치 시 FTS 전용 전환 확인 |
| FR-DMN-*, QA-1 | 테스트 | 중복 실행 거부, 강제 종료 후 재시작 시 누락과 중복 없음 확인 |
| PR-1~4 | 시연 | 벤치마크 스크립트로 측정 |
| QA-3 | 검사 | 코드 리뷰와 실행 중 네트워크 연결 목록 확인(`lsof -i`) |
| 부록 B 항목 | 시연 | 대화형 세션에서 확인 |

---

## 부록 A. 실측 근거 (Claude Code 2.1.316, `claude -p --settings probe/settings.json`)

| 시나리오 | 실행된 hook 순서 |
|----------|------------------|
| s1 도구 없음 | `Stop` |
| s2 병렬 Read 2개 | `PostToolUse`×2 → `PostToolBatch`×1 → `Stop` |
| s3 포그라운드 Agent | `SubagentStart` → [서브: `PostToolBatch`×2, `agent_id` 있음] → `SubagentStop` → [메인: `PostToolBatch`] → `Stop` |
| s4 백그라운드 Agent | `SubagentStart` → [메인: `PostToolBatch`(launched)] → `Stop` → [서브: `PostToolBatch`×2] → `SubagentStop` → [메인, 새 prompt_id: `Stop`] |
| s5 백그라운드 Bash | [메인: `PostToolBatch`(backgroundTaskId)] → `Stop` → (완료 시 hook 없음) → [메인, 새 prompt_id: `PostToolBatch`, `Stop`] |

확인한 사실:
- 모든 hook 입력에 `session_id`, `transcript_path`, `cwd`, `prompt_id`가 있다.
- `PostToolBatch.tool_calls[]`에는 `tool_name`, `tool_input`, `tool_use_id`, `tool_response`가 있고, 모델의 `text`와 `thinking`은 없다.
- `Stop`에는 `last_assistant_message`, `stop_hook_active`, `background_tasks`가 있다.
- 서브에이전트 내부 hook에는 `agent_id`, `agent_type`이 있고, 메인 루프 hook에는 없다.
- `SubagentStop`에는 `last_assistant_message`가 없다.
- `TaskCreated`, `TaskCompleted`, `Notification`은 s5에서 실행되지 않았다.
- 백그라운드 완료 알림은 메인 transcript에 `type: user`, 문자열 content의 `<task-notification>`으로 기록되며 `<tool-use-id>`가 시작 호출의 `tool_use_id`와 같다.

## 부록 B. 미해결 항목 (구현 전 또는 구현 중 확인)

| ID | 항목 | 영향 요구사항 |
|----|------|---------------|
| OPEN-1 | 대화형 세션에서 백그라운드 완료 알림이 새 턴을 시작하고 `Stop`이 실행되는지 | FR-BG-2, FR-CAP-3 |
| OPEN-2 | 사용자가 Esc로 중단했을 때 실행되는 hook과 transcript 기록 형태 | FR-EXT-8 |
| OPEN-3 | 컨텍스트 압축(compaction)이 transcript 파일을 다시 쓰는지, 덧붙이기만 하는지 | FR-EXT-1 |
| OPEN-4 | 실제 데이터로 `qwen3-embedding:0.6b`와 `bge-m3`의 검색 품질 비교 | C-3 |
| OPEN-5 | 자동 주입 임계값 기본값(0.55)의 적정성 | FR-RET-3 |
