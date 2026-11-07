# 소프트웨어 요구사항 명세서 (SRS) — Hippocampal Index (`hippo`)

| 항목 | 값 |
|------|----|
| 문서 버전 | 0.26 (관찰 기록 추가(3.2.16 OBS): 로그 줄의 로컬 시각과 5 MiB 순환, 데몬의 세션별 수집 처리 로그, hook 결과 로그(`logs/hook.log`), 반복의 저장 시각과 저장 경로, 자동 주입 기록과 에이전트의 기억 활용 기록, 상태의 최근 7일 주입 통계. 0.25의 끝난 턴의 마지막 반복이 저장되지 않던 결함 수정: transcript의 턴 종료 표시 라인으로 반복을 확정하고(FR-EXT-7 d, FR-EXT-9), 데몬이 읽지 않은 꼬리가 남은 세션을 따라잡아 읽음(FR-CAP-9). 가져오기 탭의 프로젝트 선택 목록(FR-GUI-7). 0.24의 주입 전체 길이 기본값 `total_chars`를 12,000자에서 8,000자로 낮춰 Claude Code의 hook `additionalContext` 한도 10,000자 안에 둠(FR-RET-4). 0.23의 GUI 탭 순서 변경(FR-GUI-1). 창을 열 때의 시작 탭 설정 `[ui] start_tab`과 마지막으로 본 탭 기록(FR-UI-4, FR-GUI-25). 검색 탭의 주입 미리 보기(FR-GUI-9). 정보 탭의 README 마크다운 렌더링과 상대 링크의 GitHub 주소 변환(FR-GUI-11). 0.22의 데몬이 처리할 작업이 있는 동안 Mac의 유휴 잠자기를 막음(FR-DMN-13, `[daemon] prevent_sleep`). 처리할 작업이 있는 동안 유휴 종료하지 않음(FR-DMN-8). 0.21의 macOS 앱(`hippo.app`) 배포. 데이터 루트를 실행 파일 위치(`<루트>/bin/hippo`)로 정하고 전역 옵션 `--root` 삭제. 설치는 `--data-dir`, `--claude-dir`을 받고 `--scope` 삭제. `~/.local/bin/hippo` 링크로 PATH 등록. 제거는 실행 파일을 남기고, 새 명령 `purge`가 데이터 루트를 지움. 새 명령 `stop`과 GUI 상태 탭의 멈추기로 데몬과 hippo가 띄운 Ollama를 멈춤. 주입·저장 일시정지(`[inject] enabled`, `[capture] enabled`)와 GUI 일시정지/재생 버튼. GUI 경로 입력란의 폴더 찾아보기. GUI의 데이터 루트·Claude 폴더 선택과 설치·업데이트 상태. 0.20의 메모리 파일의 CRLF 줄 끝 유지, 기록 탭 목록의 잘린 프롬프트에 `…` 표시, IF-SK-2의 `cwd` 문구 정정. 0.19의 GUI 라이트·다크 테마와 `[ui] theme`, GUI 아이콘·로고·캐릭터 이미지, 정보 탭의 README 이미지 줄 제외. 0.18의 삭제가 임베딩 묶음에 끼어듦. 기록 탭의 기본 프로젝트, 토스트 메시지, 삭제 대기 표시, 질의 결과 초기화. 실제 가져오기 요청도 시간 제한 없이 결과를 기다림. 설치 뒤 제거하면 메모리 파일이 바이트 단위로 되돌아감. 0.17의 실제 삭제 요청은 시간 제한 없이 결과를 기다림. 0.16의 요청 범위 프로젝트 id와 `search` 구조화 결과, 0.15의 LanceDB 버전 누적 결함 수정, 0.14의 삭제·기록 탭 요구사항 포함) |
| 작성일 | 2026-11-07 |
| 작성자 | biisairo |
| 준거 표준 | ISO/IEC/IEEE 29148:2018 |
| 상태 | 검토 대기 |

---

## 1. 서론

### 1.1 목적
이 문서는 개인 기억 시스템 Hippocampal Index(줄여서 `hippo`)의 요구사항을 정의한다. 이 시스템은 Claude Code 세션의 대화를 agentic loop 반복 단위로 로컬 벡터 DB에 저장하고, 이후 대화에서 그 기록을 검색해 답변에 활용한다. 구현, 테스트, 인수 판단은 이 문서를 단일 기준으로 삼는다.

### 1.2 범위
`hippo`는 단일 Rust 바이너리 `hippo`로 제공되며 다음을 수행한다.
- Claude Code hook으로 메인 agentic loop의 반복을 수집한다.
- 상주 데몬이 transcript를 해석해 반복과 도구 결과를 원본 저장소(SQLite)에 저장하고, 로컬 임베딩 모델로 벡터화해 벡터 저장소(기본 LanceDB)에 색인한다.
- 원본 저장소와 벡터 저장소는 프로젝트마다 따로 둔다. 검색과 자동 주입은 기본적으로 현재 프로젝트 안에서만 한다.
- 임베딩 모델과 벡터 저장소는 설정만 바꿔 교체할 수 있다. 벡터 저장소는 원본 저장소에서 언제든 다시 만들 수 있는 파생 인덱스이다.
- 사용자 프롬프트 제출 시 관련 기억을 자동 주입하고, Claude가 CLI로 추가 검색할 수 있게 한다.
- `hippo gui`로 네이티브 창을 열어 설정 편집, 상태 확인, 가져오기, 명령 실행, Claude 적용과 해제, 문서 보기를 할 수 있게 한다.
- macOS 앱(`hippo.app`)으로 배포한다. 앱은 같은 바이너리를 감싸며, Finder에서 열면 GUI를 띄운다(3.2.14).

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
| 프로젝트 | 저장 단위. 무작위 id로 식별하며, 위치 1개와 루트 커밋 목록을 가진다. 정체는 위치가 아니라 id이다. |
| 프로젝트 저장소 | 프로젝트 하나의 원본 저장소와 벡터 저장소. `projects/<id>/` 아래에 둔다. |
| 목록 DB | 프로젝트, 경로, git common dir, 루트 커밋, 세션→프로젝트 고정값을 담는 전역 SQLite DB(`registry.sqlite`). |
| 세션 시작 경로 | transcript에서 `cwd` 필드를 가진 첫 라인의 `cwd`. 세션 중 `cd`로 바뀐 경로가 아니다. |
| 프로젝트 위치 | 경로가 git 작업 트리 안이면 그 저장소의 git common dir(`git rev-parse --path-format=absolute --git-common-dir`), 아니면 경로 자체. 원본 작업 트리와 그 모든 worktree는 같은 common dir을 가지므로 같은 위치가 된다. 위치는 프로젝트마다 1개이며, 경로 목록이나 경로→프로젝트 라우팅은 기록하지 않는다. |
| 루트 커밋 | `git rev-list --max-parents=0 HEAD`가 돌려주는 부모 없는 커밋. 저장소에 따라 없거나 여러 개일 수 있다. |
| 기준 턴 | 처음 보는 세션의 처리 요청에 담긴 hook 입력의 `prompt_id`가 가리키는 턴. 실시간 수집은 이 턴의 첫 라인부터 시작한다(FR-EXT-12). |
| 시작 offset | 세션의 기록을 저장하기 시작한 transcript 위치. 실시간 수집으로 처음 본 세션은 기준 턴의 첫 라인 위치이고, 처음부터 가져온 세션은 0이다. |
| 가져오기 (import) | 과거 transcript를 읽어 지정한 프로젝트에 기록을 추가하는 작업(3.2.11). |
| fork 복사본 | `--fork-session` 등으로 만든 새 세션 transcript에 복사된 이전 세션의 라인. `sessionId`만 새 값이고 `uuid`와 `message.id`는 원본과 같다(부록 A). |
| GUI | `hippo gui`가 여는 네이티브 창(3.2.12). 데몬의 클라이언트이며, 데몬과 같은 요청(IF-SK-2)을 쓴다. |
| 실행 경로 | `hippo gui`를 실행한 현재 디렉터리. GUI가 보내는 요청의 `cwd`이며, 경로 입력란의 기본값이다. Finder에서 앱을 열면 `/`이다. |
| 데이터 루트 | 데몬의 데이터, 설정, 실행 파일을 두는 디렉터리(DR-1). 설치 위치와 같다. 기본값은 `~/.hippo`이다. |
| 설치 경로 | 데이터 루트의 `bin/hippo`. 설치(FR-INS-8)가 실행 파일을 복사하는 곳이다. |
| 관리 실행 파일 | 링크를 푼 실제 경로가 `<디렉터리>/bin/hippo`인 실행 파일. 그 `<디렉터리>`가 이 실행 파일의 데이터 루트이다(FR-ROOT-1). |
| PATH 링크 | `~/.local/bin/hippo`. 설치가 만드는, 설치 경로를 가리키는 심볼릭 링크이다(FR-INS-11). |
| Claude 폴더 | hook과 안내 블록을 넣을 Claude Code 설정 디렉터리. 기본값은 `~/.claude`이다. |
| 앱 실행 파일 | `hippo.app/Contents/MacOS/hippo`. 관리 실행 파일이 아니다. |
| 가져오기 상태 | 후보 세션과 대상 프로젝트의 관계(FR-IMP-8). `새 세션`, `실시간만`, `가져옴`, `다른 프로젝트` 중 하나이다. |
| 재색인 필요 키 | 바꾸면 기존 벡터와 새 벡터가 달라지는 설정 키. `[embedder] backend`, `model`, `dimension`, `max_input_tokens`, `[vector]`의 모든 키, `[chunking]`의 모든 키, `[embed_text]`의 모든 키이다. |

### 1.4 참조 문서
- [R1] Claude Code Hooks reference — https://code.claude.com/docs/en/hooks
- [R2] LanceDB 문서 — https://docs.lancedb.com
- [R3] Qwen3-Embedding — https://github.com/QwenLM/Qwen3-Embedding
- [R4] 실측 hook 입력 — `tests/fixtures/hooks/r_s1_notool` … `tests/fixtures/hooks/r_s5_bg_bash` (부록 A)

### 1.5 문서 구성
2장은 제품 전체 설명, 3장은 개별 요구사항, 4장은 검증 방법, 부록은 실측 근거와 미해결 항목을 다룬다.

---

## 2. 전체 설명

### 2.1 제품 관점

```
Claude Code ──hook(stdin JSON)──▶ hippo hook <event> ──Unix socket──▶ hippo daemon
                                        │ (데몬 응답 없음)                   │
                                        └──────▶ 스풀 파일 ◀──── 시작 시 소진 ┤
                                                                           ├─▶ transcript JSONL 읽기
사용자 ──▶ hippo gui (네이티브 창) ──Unix socket──────────────────────────▶ ┤
Claude (Bash) ──▶ hippo search ... ──Unix socket──────────────────────────▶ ├─▶ 목록 DB ──▶ 프로젝트별 원본 저장소 SQLite (기준 데이터, FTS)
                                                                           ├─▶ Embedder 트레이트 ──▶ [기본] Ollama (localhost)
                                                                           └─▶ VectorStore 트레이트 ──▶ [기본] LanceDB (프로젝트별)
```

### 2.2 제품 기능 요약
1. 수집: 메인 루프의 모든 반복을 빠짐없이, 중복 없이 수집한다.
2. 백그라운드 연결: 백그라운드 작업의 시작 호출과 완료 결과를 한 쌍으로 연결한다.
3. 저장: 반복은 의미 벡터 1개로, 결과는 내용 해시로 중복을 제거해 저장한다.
4. 검색: 프롬프트 제출 시 자동 주입하고, CLI로 추가 검색한다.
5. GUI: 설정, 상태, 가져오기, 명령, 설치를 창 하나에서 다룬다.

### 2.3 사용자 특성
단일 사용자(개발자 본인)가 자신의 macOS 장비에서 사용한다. 다중 사용자 기능은 범위 밖이다.

### 2.4 제약 사항
- C-1 구현 언어는 Rust로 한다.
- C-2 원본 저장소는 SQLite(FTS5)로 한다. 벡터 저장소의 기본 백엔드는 LanceDB Rust SDK(임베디드)로 한다.
- C-3 임베더의 기본 백엔드는 Ollama `qwen3-embedding:0.6b`, 출력 차원 1024로 한다.
- C-4 데이터를 로컬 장비 밖으로 전송하지 않는다. 네트워크 통신은 `localhost`의 Ollama로 한정한다. 임베더 URL은 URL 파서로 해석하며, 스킴이 `http`/`https`가 아니거나, host가 `localhost`, `127.0.0.1`, `::1`이 아니거나, userinfo(`@`)·쿼리·프래그먼트가 있으면 거부한다. 이 검사는 설정을 읽을 때와 임베더를 만들 때 모두 수행한다. HTTP 리다이렉트는 따르지 않는다.
- C-5 MCP 서버를 사용하지 않는다. 별도 서버를 띄우지 않고 hook과 CLI만으로 동작하게 하기 위해서다.
- C-6 1차 대상 플랫폼은 macOS(darwin)이다.
- C-7 Claude Code 설정 파일, 메모리 파일(FR-INS-7), PATH 링크(FR-INS-11)는 사용자가 `hippo install`/`hippo uninstall`/`hippo purge`를 실행하거나 GUI에서 적용·해제·완전 삭제를 확인했을 때(FR-GUI-10)만 수정한다. 그 밖의 명령, GUI 동작, 데몬은 이 파일들을 수정하지 않는다. 셸 설정 파일(`~/.zshrc` 등)은 어떤 경우에도 수정하지 않는다.
- C-8 GUI는 egui/eframe로 만든 네이티브 창이다. 웹뷰, 브라우저, GUI용 HTTP 서버를 쓰지 않는다. GUI는 `hippo` 바이너리에 항상 포함한다(별도 바이너리나 cargo feature로 나누지 않는다). 근거: egui를 링크해도 프로세스 시작 시간은 약 1ms(2.4→3.4ms, p50) 늘어날 뿐이다(2026-11-03 실측, 최소 예제).
- C-9 `hippo`의 라이선스는 MIT이다. 저장소 루트의 `LICENSE`와 `Cargo.toml`의 `license`에 표기한다.

### 2.5 가정과 의존성
- A-1 Claude Code의 hook 입력 스키마와 transcript JSONL 형식은 부록 A의 실측 결과(Claude Code 2.1.316)와 같다. 버전이 바뀌면 재검증한다.
- A-2 Ollama가 로컬에 설치되어 있고 대상 모델을 받아 둔 상태다.
- A-3 백그라운드 서브에이전트 완료 알림의 `<output-file>`은 서브에이전트 transcript를 가리키는 심볼릭 링크다. (부록 A, s4)
- A-4 백그라운드 shell의 `<output-file>`은 `/private/tmp` 아래에 있으며 이후 삭제될 수 있다.
- A-5 git 정보는 `git` 실행 파일로 얻는다. `git`이 없거나 실행이 실패하거나 시간 제한(설정 `[project] git_timeout_ms`, 기본 1,000)을 넘으면 git이 아닌 경로로 본다.
- A-6 transcript의 `user`, `assistant` 라인에는 그 시점의 `cwd`가 있다. (부록 A)
- A-7 `message.id`는 API 응답마다 고유하다. 다른 세션의 transcript에 같은 `message.id`가 나오는 경우는 fork 복사본뿐이다. (부록 A)
- A-8 Claude Code transcript는 `<transcripts 디렉터리>/<slug>/<session_id>.jsonl`에 있고, 서브에이전트 transcript는 `<slug>/<session_id>/subagents/` 아래에 있다. `--resume`과 `--continue`는 같은 파일에 이어 쓴다. (부록 A)

### 2.6 비목표
- 서브에이전트 내부 반복 저장 (서브에이전트의 최종 보고만 결과로 저장한다)
- `thinking` 블록 저장
- LLM 요약 생성
- 원격 동기화, 다중 사용자, 웹 UI (브라우저나 웹뷰로 여는 화면. 네이티브 GUI(3.2.12)는 범위 안이다)
- GUI에서 제3자 라이선스 목록 보기, README의 이미지 표시, 바이너리에 넣지 않은 문서(`docs/`)를 GUI 안에서 보기 (링크는 브라우저로 연다)
- 데몬이 GUI에 진행 상황을 밀어 보내는 채널 (GUI는 폴링한다)
- 보존 기간과 자동 삭제 정책
- MCP 도구 제공
- 단일 저장소 배치(v0.1, `~/.hippo/records.sqlite`)의 데이터를 프로젝트별 배치로 옮기는 마이그레이션 (v0.1 데이터는 남아 있지 않다)
- 경로를 다른 프로젝트로 보내는 영구 라우팅 표 (경로가 바뀌면 `hippo import --from <이전 경로>`로 기록을 현재 경로의 프로젝트로 가져온다)
- 실시간 수집에서 기준 턴 앞의 기록 저장 (과거 기록은 `hippo import`로만 가져온다)
- 가져오기의 날짜 범위 지정, 경로가 없어진 과거 세션의 프로젝트 자동 추정
- 서브에이전트 transcript(`subagents/`) 가져오기 (메인 transcript의 보고만 결과로 저장한다)
- 프로젝트 사이의 결과 원문 중복 제거 (중복 제거는 프로젝트 안에서만 한다)
- 삭제한 기록의 복구, 삭제한 세션·턴의 재수집 차단 (지운 세션은 다시 가져오거나, 열린 세션이 이어지면 다시 저장된다)
- 반복 하나 단위의 삭제 (삭제 단위는 프로젝트, 세션, 턴이다)
- CLI의 기록 탐색과 주입 미리 보기 (GUI 기록 탭에서만 제공한다)
- 프로젝트 범위 설치(`--scope project`, `CLAUDE.local.md`). 프로젝트의 `.claude` 디렉터리를 `--claude-dir`로 줄 수는 있지만, 그때도 파일 이름은 `settings.json`과 `CLAUDE.md`이다
- 앱을 열 때의 자동 설치·자동 업데이트 (설치와 교체는 사용자가 install을 실행할 때만 한다)
- Developer ID 코드 서명, 공증, DMG 등 배포 패키지 (앱은 ad-hoc 서명만 한다)
- 셸 설정 파일이나 `/etc/paths.d`를 통한 PATH 등록
- 데이터 루트 옮기기 (다른 데이터 루트로 바꾸려면 새 루트에 설치한 뒤 `hippo import`로 기록을 가져온다)

---

## 3. 개별 요구사항

요구사항 형식: `ID` — 내용. 우선순위는 **필수(M)** / **권장(S)**로 표기한다.

### 3.1 외부 인터페이스 요구사항

#### 3.1.1 Hook 인터페이스
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| IF-HK-1 | `hippo hook <event>`는 stdin으로 hook 입력 JSON 1개를 받아야 한다. `<event>`는 `post-tool-batch`, `stop`, `stop-failure`, `session-end`, `user-prompt-submit` 중 하나이다. | M |
| IF-HK-2 | `user-prompt-submit`을 뺀 모든 hook은 항상 종료 코드 0으로 끝나야 하며 stdout에 아무것도 출력하지 않아야 한다. | M |
| IF-HK-3 | `user-prompt-submit`은 주입할 기억이 있으면 `hookSpecificOutput.additionalContext`를 담은 JSON 1개를 stdout에 출력하고, 없으면 아무것도 출력하지 않아야 한다. 어떤 경우에도 프롬프트를 차단하지 않아야 한다(종료 코드 0). | M |
| IF-HK-4 | `hippo hooks-config`는 Claude Code `settings.json`에 넣을 hook 설정 JSON을 stdout에 출력해야 한다. 수집 hook(`PostToolBatch`, `Stop`, `StopFailure`, `SessionEnd`)은 `async: true`로 설정한다. `UserPromptSubmit`은 동기로 두고 `timeout`(초)을 콜드 시작 예산(`cold_budget_ms`, FR-RET-5)보다 크게 설정한다. 명령 경로는 실행 중인 `hippo`의 절대 경로를 쓴다. `hippo install`이 설정 파일에 넣을 때는 FR-INS-8의 설치 경로를 쓴다. 경로는 FR-INS-10의 셸 표기로 쓴다. | M |
| IF-HK-5 | 수집 hook 클라이언트는 데몬에 요청을 보내고 처리 완료 응답을 기다린다(기본 10초). 요청이 실패하면 FR-DMN-7에 따라 데몬을 띄운 뒤 한 번 다시 보낸다. 그래도 실패하면 FR-CAP-5에 따라 스풀에 기록한다. | M |

#### 3.1.2 CLI 인터페이스
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| IF-CLI-1 | `hippo search <query> [--scope project\|all\|<path>] [--kind iteration\|result\|both] [--limit N]`는 검색 결과를 사람이 읽을 수 있는 텍스트로 출력해야 한다. 기본값은 `--scope project --kind iteration --limit 10`이다. `project`는 현재 디렉터리가 속한 프로젝트, `<path>`는 그 경로가 속한 프로젝트이다(FR-PRJ-5). `search`, `show`, `status`, `reindex`, `project`, `import`는 데몬에 요청하며 클라이언트의 현재 디렉터리를 함께 보낸다. 데몬이 없으면 FR-DMN-7에 따라 띄운 뒤 요청하며, 그래도 응답이 없으면 오류 메시지를 stderr에 출력하고 종료 코드 1로 끝난다. | M |
| IF-CLI-2 | `hippo show <prompt_id> [--scope project\|<path>]`는 해당 턴의 반복을 순서대로 복원해 출력해야 한다. `--with-results` 옵션을 주면 연결된 결과 원문도 출력한다. 프로젝트를 찾는 순서는 FR-RET-7을 따른다. | M |
| IF-CLI-3 | `hippo daemon`은 데몬을 포그라운드로 실행해야 한다. | M |
| IF-CLI-4 | `hippo launchd-plist`는 데몬 자동 실행용 launchd plist를 stdout에 출력해야 한다. 데몬이 유휴 종료(정상 종료)한 뒤 바로 다시 띄우지 않도록, 비정상 종료일 때만 재시작하게 설정한다. | S |
| IF-CLI-6 | `hippo install [--data-dir <경로>] [--claude-dir <경로>] [--dry-run]`는 FR-INS-*의 설치를 해야 한다. `--data-dir`의 기본값은 실행 파일이 관리 실행 파일이면 그 데이터 루트, 아니면 `~/.hippo`이다. `--claude-dir`의 기본값은 FR-INS-12의 기록 값, 없으면 `~/.claude`이다. `hippo uninstall [--claude-dir <경로>] [--dry-run]`는 실행 파일의 데이터 루트로 FR-INS-*의 제거를 해야 한다. 전역 옵션 `--root`는 없다. | M |
| IF-CLI-13 | `hippo purge [--claude-dir <경로>] [--yes]`는 실행 파일의 데이터 루트로 FR-INS-13의 완전 삭제를 해야 한다. 확인 절차는 FR-DEL-4와 같다(`--yes`, 터미널이 아니고 `--yes`가 없으면 종료 코드 1). 확인 전에 지울 데이터 루트의 절대 경로를 출력한다. | M |
| IF-CLI-14 | 인자 없이 실행하면, 앱 실행 파일(링크를 푼 실제 경로가 `<이름>.app/Contents/MacOS/` 안)일 때는 `hippo gui`와 같이 동작하고, 그 밖에는 도움말을 출력해야 한다. | M |
| IF-CLI-15 | `hippo stop`은 실행 파일의 데이터 루트로 수동 중지(FR-DMN-12)를 하고 결과(멈춘 데몬, 멈춘 Ollama, 또는 실행 중인 것이 없음)를 출력해야 한다. 실패하면 사유를 stderr에 출력하고 종료 코드 1로 끝난다. 멈춘 뒤에도 다음 hook이나 CLI 요청이 FR-DMN-7에 따라 데몬을 다시 띄운다. | M |
| IF-CLI-5 | `hippo status`는 데몬 상태, 스풀 대기 건수, 재색인 상태와 프로젝트별 표(id, 위치와 위치 존재 여부, 반복 수, 결과 수, 임베딩 대기 건수, 벡터 수, 호환성)를 출력해야 한다. 임베딩 대기 건수가 있으면 그중 실시간 작업(`bulk = 0`)과 가져오기 작업(`bulk = 1`, FR-IMP-5)의 건수를 나눠 함께 출력한다. 임베딩 대기 건수가 있고 최근 5분 동안 처리한 작업이 있으면 그 프로젝트의 처리 속도(분당 작업 수)와 남은 예상 시간(대기 건수 ÷ 속도)을 함께 출력한다. 속도는 데몬 메모리에서 센다. | S |
| IF-CLI-7 | `hippo reindex [--scope all\|project\|<path>]`는 재색인을 요청해야 한다. 기본값은 `all`이다. | M |
| IF-CLI-8 | `hippo project list`는 FR-PRJ-6의 목록을 출력해야 한다. | M |
| IF-CLI-9 | `hippo import [--from <경로 또는 프로젝트 id>] [--to <경로>] [--wait] [--dry-run]`는 FR-IMP-1~7의 가져오기를 실행해야 한다. 생략한 인자는 현재 디렉터리이다. `--dry-run`이면 FR-IMP-9의 미리 보기만 하고 아무것도 바꾸지 않는다. `--dry-run`과 `--wait`을 함께 주면 `--wait`은 무시한다. `--wait`이면 가져오기가 끝난 뒤 대상 프로젝트의 임베딩 대기 건수가 0이 될 때까지 2초마다 상태를 물어 한 줄 진행률(남은 건수, 속도, 남은 예상 시간)을 갱신하고, 0이 되면 완료를 출력한다. 기다리는 중에 중단해도 데몬의 처리는 계속된다. | M |

| IF-CLI-11 | `hippo project remove <프로젝트 id 또는 경로> [--yes] [--dry-run]`는 FR-DEL-1의 프로젝트 삭제를 요청해야 한다. 경로는 FR-PRJ-5의 조회 전용 판정으로 프로젝트를 찾는다. 확인 절차는 FR-DEL-4를 따른다. | M |
| IF-CLI-12 | `hippo forget (--session <session_id> \| --prompt <prompt_id>) [--scope project\|<path>] [--yes] [--dry-run]`는 FR-DEL-2의 세션 삭제 또는 FR-DEL-3의 턴 삭제를 요청해야 한다. 둘 중 하나만 받는다. 프로젝트를 찾는 순서는 FR-RET-7(`show`)과 같다. 확인 절차는 FR-DEL-4를 따른다. | M |
| IF-CLI-10 | `hippo gui`는 GUI 창(3.2.12)을 열고, 창이 닫힐 때까지 포그라운드로 실행한 뒤 종료 코드 0으로 끝나야 한다. 창을 만들 수 없으면 오류 메시지를 stderr에 출력하고 종료 코드 1로 끝난다. GUI의 데이터 루트는 FR-GUI-22를 따른다. | M |

#### 3.1.3 데몬 소켓 인터페이스
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| IF-SK-1 | 데몬은 `<데이터 루트>/hippo.sock` Unix socket으로 줄 단위 JSON 요청과 응답을 주고받아야 한다. | M |
| IF-SK-2 | 요청 종류는 `ingest`(hook 이벤트 전달), `inject`(자동 주입 텍스트 생성), `search`, `show`, `status`, `reindex`, `project_list`, `import_prepare`, `import_session`, `shutdown`, `project_remove`, `forget`(FR-DEL-1~3. 둘 다 `dry_run` 필드를 가진다), `session_list`, `turn_list`, `turn_detail`, `inject_preview`(FR-GUI-13~16), `current_project`(FR-GUI-18)이다. `current_project`는 `cwd`를 FR-PRJ-5의 조회 전용 판정으로 해석한 프로젝트 id를 돌려주고, 프로젝트가 없으면 비어 있는 값을 돌려준다. `inject`, `search`, `show`, `reindex`, `project_remove`, `forget`, `session_list`, `turn_list`, `turn_detail`, `inject_preview`, `current_project`는 판정에 쓸 `cwd`를 담는다. `ingest`, `status`, `project_list`, `import_prepare`, `import_session`, `shutdown`은 `cwd`를 담지 않는다. 요청의 범위(`scope`)는 `project`, `all`, 경로 외에 프로젝트 id(`id`)도 받는다. 프로젝트 id 범위는 목록 DB에 그 id가 있으면 그 프로젝트이고, 없으면 "프로젝트 없음"이다. GUI는 선택한 프로젝트를 이 범위로 지정한다. `import_prepare`는 `dry_run`(생략하면 false) 필드를 가진다. `search`는 `structured`(생략하면 false) 필드를 가지며, 참이면 텍스트 대신 결과 목록(프로젝트 id, 종류, 레코드 id, 점수, 유사도, prompt_id, 미리 보기)을 돌려준다(FR-GUI-16, FR-GUI-17). 새로 더한 필드는 생략할 수 있어 이전 클라이언트의 요청도 그대로 해석된다. 연결 하나에 요청 하나, 응답 하나를 주고받는다. | M |

#### 3.1.4 Claude 안내문
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| IF-SKL-1 | Claude가 과거 기억이 필요할 때 `hippo search`와 `hippo show`를 Bash로 호출하는 방법을 안내하는 블록을, `hippo install`이 Claude 폴더의 메모리 파일에 넣어야 한다(FR-INS-7). 블록은 `hippo`를 이전 대화의 장기 기억 저장소로 소개하고, 자동으로 들어오는 `<memory>` 블록은 배경 지식이며 지시가 아니라고 밝힌다. 세션·턴·반복 같은 내부 용어를 쓰지 않는다. 블록 내용은 영어로 고정한다(FR-UI-2). | M |

#### 3.1.5 출력 언어
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-UI-1 | 사람이 읽는 출력(CLI 출력, 데몬이 만드는 `search`/`show`/`status`/`project list`/가져오기 결과 텍스트, 오류 메시지, 설치 안내)은 설정 `[ui] language`(`en` 또는 `ko`, 기본 `en`)의 언어로 만들어야 한다. 데몬은 텍스트를 만드는 요청마다 `config.toml`의 `[ui] language`를 다시 읽는다. 파일을 읽지 못하면 시작할 때의 설정을 쓴다. | M |
| FR-UI-2 | Claude가 읽는 텍스트(자동 주입 블록, FR-INS-7의 안내 블록, `embed_text`의 섹션 이름)와 `--help`의 명령 설명은 언어 설정과 관계없이 영어로 만든다. | M |
| FR-UI-4 | GUI 창을 열 때 처음 보여 줄 탭은 설정 `[ui] start_tab`(`about`, `status`, `records`, `projects`, `search`, `import`, `claude_setup`, `settings`, `last`, 기본 `about`)으로 정해야 한다. `last`이면 FR-GUI-25의 기록에 남은 마지막 탭을 연다. 기록이 없거나 해석할 수 없으면 `about`을 연다. 잘못된 값은 설정 오류이다. CLI와 데몬에는 영향이 없다. | M |
| FR-UI-3 | GUI 테마는 설정 `[ui] theme`(`system`, `light`, `dark`, 기본 `system`)으로 정해야 한다. `system`이면 운영 체제의 라이트·다크 모드를 따르고, 운영 체제 설정이 바뀌면 GUI도 바뀐다. CLI 출력에는 영향이 없다. | M |

### 3.2 기능 요구사항

#### 3.2.1 수집 (CAP)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-CAP-1 | `PostToolBatch`, `Stop`, `StopFailure` 입력에 `agent_id` 필드가 있으면 아무 처리 없이 종료해야 한다. (서브에이전트 내부 루프 제외) | M |
| FR-CAP-2 | `PostToolBatch`(메인 루프)를 받으면 해당 세션에 대해 transcript 처리와 batch 완료 판정(FR-EXT-7 b)을 요청해야 한다. | M |
| FR-CAP-3 | `Stop`과 `StopFailure`(메인 루프)를 받으면 해당 세션에 대해 transcript 처리를 요청하고, 마지막 반복을 완료로 확정해야 한다. | M |
| FR-CAP-4 | `SessionEnd`를 받으면 해당 세션의 미처리 transcript 라인을 모두 처리하고, 처리 후에도 반복에 붙지 않은 반복 입력은 버려야 한다. 완료 알림은 FR-BG-2에 따라 이미 처리된 상태다. | M |
| FR-CAP-5 | hook 클라이언트는 데몬에 연결할 수 없으면 요청을 스풀 파일(`spool/pending.jsonl`, 권한 0600)에 한 줄로 추가하고 종료해야 한다. 스풀에는 이벤트, `session_id`, `transcript_path`, `prompt_id`(hook 입력에 있을 때)만 기록한다. | M |
| FR-CAP-7 | 수집 처리 요청은 hook 입력의 `prompt_id`를 담아야 한다. 입력에 없으면 비워 둔다. | M |
| FR-CAP-6 | 데몬은 시작할 때 스풀에 쌓인 요청을 기록 순서대로 처리해야 한다. 처리 전에 스풀 파일 이름을 바꿔, 처리 중에 새로 들어오는 요청이 새 스풀 파일에 쌓이게 한다. 이전 실행에서 처리하지 못한 이름 바뀐 스풀 파일이 있으면 먼저 처리한다. | M |
| FR-CAP-8 | 수집 hook(`post-tool-batch`, `stop`, `stop-failure`, `session-end`)은 설정 `[capture] enabled`(기본 true)가 false이면(저장 일시정지) 데몬에 요청하지도, 스풀에 쓰지도 않고 바로 끝나야 한다(IF-HK-2). hook은 매번 설정 파일을 읽으므로 데몬을 다시 시작하지 않아도 다음 hook부터 적용된다. 일시정지는 저장 위치(안전 offset)를 바꾸지 않는다. 그래서 재생한 뒤 같은 세션의 다음 수집이 일시정지 동안의 기록도 저장한다. 일시정지 동안 처음 시작해 재생 뒤에 처음 수집된 세션은 FR-EXT-12에 따라 그 기준 턴부터 저장된다. | M |
| FR-CAP-9 | 데몬은 hook 처리가 놓친 transcript 꼬리를 따라잡아 읽어야 한다(따라잡기). 쓰기 스레드는 처리할 작업이 없을 때, 시작 직후(스풀 처리 뒤) 한 번 따라잡기를 하고, 그 뒤에는 마지막 따라잡기를 마친 때부터 60초가 지날 때마다 한다. 대상은 목록 DB의 모든 프로젝트에서 세션 상태가 있는 세션 중 다음을 모두 만족하는 세션이다. (a) 저장된 transcript 경로가 일반 파일이다. (b) 파일 길이가 안전 offset보다 길다. (c) 파일 수정 시각에서 5초 이상 지났다. (d) 이 데몬 실행에서 그 세션을 마지막으로 따라잡았을 때와 (파일 길이, 수정 시각)이 다르다. 대상 세션은 안전 offset 뒤를 읽고, FR-EXT-7의 (a)와 (d)로 완료된 반복만 저장한다. (b)와 (c)의 확정은 하지 않는다. 시작 offset은 바꾸지 않는다. 세션 하나에서 오류가 나면 로그에 남기고 다음 세션으로 넘어간다. 데몬이 멈추는 중이면 남은 세션을 처리하지 않는다. 따라잡기는 요청이 아니므로 유휴 종료 시각(FR-DMN-8)을 늦추지 않는다. | M |

#### 3.2.2 transcript 해석 (EXT)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-EXT-1 | 데몬은 세션마다(세션이 고정된 프로젝트 저장소에) 안전 offset, 시작 offset, 해석기 상태(턴별 다음 `iteration_index` 등)를 저장하고, 안전 offset 뒤의 라인만 읽어야 한다. 안전 offset은 아직 완료되지 않은 반복이나 아직 반복에 붙지 않은 반복 입력이 시작되는 라인 앞이며, 그런 것이 없으면 마지막으로 읽은 라인 끝이다. offset은 완료된 반복의 DB 반영이 끝난 뒤에만 갱신한다. 개행으로 끝나지 않은 마지막 라인은 기록 중인 것으로 보고 읽지 않는다. transcript 길이가 안전 offset보다 짧으면(파일이 다시 쓰인 경우) 상태를 지우고 처음 보는 세션으로 처리한다(FR-EXT-12). 이때 FR-EXT-10에 따라 중복 행은 생기지 않는다. | M |
| FR-EXT-12 | 실시간 수집에서 세션 상태가 없으면(처음 보는 세션) transcript에서 `promptId`가 요청의 `prompt_id`와 같은 첫 user 라인을 찾아, 그 라인의 시작 위치를 안전 offset과 시작 offset으로 삼아 읽어야 한다. 그 앞의 라인은 저장하지 않는다. 요청에 `prompt_id`가 없거나 그런 라인이 아직 없으면 아무것도 처리하지 않고 상태도 저장하지 않는다. 설치 전 세션의 재개, fork 세션, 설치 때 이미 열려 있던 세션은 이 규칙에 따라 기준 턴부터 저장된다. | M |
| FR-EXT-2 | `type == "assistant"` 라인을 `message.id`로 묶어 반복 1개로 만들어야 한다. | M |
| FR-EXT-3 | 반복에서 `text` 블록은 저장하고, `thinking` 블록은 저장하지 않아야 한다. `isApiErrorMessage == true`인 assistant 라인의 `text` 블록도 저장하지 않는다. 이 텍스트는 Claude Code가 만든 API 오류 안내문이며 모델의 응답이 아니다. 반복 자체(입력, 순번)는 그대로 만든다. | M |
| FR-EXT-4 | 반복의 `tool_use` 블록마다 `name`, `input`, `id`를 저장해야 한다. | M |
| FR-EXT-5 | `type == "user"` 라인의 `tool_result` 블록은 `tool_use_id`로 해당 `tool_use`가 속한 반복에 연결해야 한다. 라인의 `toolUseResult`에 `handbackReport.text`가 있으면 tool_result 본문 대신 그 값을 결과 텍스트로 쓴다. 이 경우 tool_result 본문에는 보고 대신 안내 문구만 들어 있다(부록 A, s3). | M |
| FR-EXT-6 | `type == "user"`이고 `tool_result` 블록이 없는 라인은 반복 입력으로 다뤄야 한다. content가 문자열이면 그 문자열을, 블록 배열이면 `text` 블록을 이어 붙인 텍스트를 쓴다. 종류는 (a) `<task-notification>`을 포함하면 완료 알림, (b) `<agent-message`를 포함하면 서브에이전트 보고, (c) 그 밖에는 사용자 프롬프트이다. 다음은 제외한다: `isMeta == true`인 라인(단 `<agent-message`를 포함한 라인은 제외하지 않는다), 내용이 `<system-reminder>` 블록만으로 이루어진 라인, 텍스트가 빈 라인. 반복 입력은 그 뒤에 처음 나오는 반복에 붙인다. | M |
| FR-EXT-7 | 반복은 다음 중 하나가 일어나면 완료로 판단해야 한다: (a) 다른 `message.id`를 가진 assistant 라인이 뒤에 나타남, (b) `PostToolBatch` 처리 요청을 받았고 반복의 모든 `tool_use`에 `tool_result`가 있음, (c) `Stop`, `StopFailure`, `SessionEnd` 처리 요청을 받음, (d) 턴 종료 표시 라인(`type == "system"`이고 `subtype`이 `turn_duration` 또는 `stop_hook_summary`인 라인)이 뒤에 나타남. (d)는 Claude Code가 transcript에 마지막 assistant 라인을 기록하기 전에 `Stop` hook이 transcript를 읽은 경우에도, 다음 처리에서 그 반복을 확정하기 위한 규칙이다. 턴 종료 표시 라인은 반복 입력을 버리지 않는다. 병렬 호출의 `tool_use`와 `tool_result`는 transcript에 교차로 기록될 수 있으므로(부록 A, s2) 결과가 모두 모인 것만으로 완료로 판단하지 않는다. (a), (c), (d)로 완료될 때 결과가 없는 `tool_use`는 결과 없음으로 저장한다. | M |
| FR-EXT-8 | 완료되지 않은 반복은 다음 처리 때까지 보류해야 한다. | M |
| FR-EXT-9 | `user`, `assistant`가 아닌 라인 타입(`attachment`, `system`, `queue-operation` 등)은 무시해야 한다. 단 턴 종료 표시 라인(FR-EXT-7 d)은 반복의 완료 판정에만 쓴다. | M |
| FR-EXT-11 | 반복의 `prompt_id`는 그 반복보다 앞에 있는 가장 최근 user 라인의 `promptId`로 정해야 한다. `ts`와 `cwd`는 반복의 첫 assistant 라인 값으로 정한다. | M |
| FR-EXT-10 | 반복 id는 `session_id`와 `message.id`의 조합으로 만들어야 하며, 같은 반복을 다시 처리해도 DB에 중복 행이 생기지 않아야 한다. | M |

#### 3.2.3 백그라운드 작업 (BG)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-BG-1 | `tool_input.run_in_background == true`인 `Bash` 또는 `Agent` 호출은 `tool_use_id`를 키로 하는 대기(pending) 기록을 만들어야 한다. 작업 id는 해당 tool_result 라인의 `toolUseResult` 객체에서 얻는다(Bash: `backgroundTaskId`, Agent: `agentId`). | M |
| FR-BG-2 | 반복 입력에서 `<task-notification>`을 찾으면 반복 완료를 기다리지 않고 즉시 `task-id`, `tool-use-id`, `output-file`, `status`, `summary`를 파싱해 처리해야 한다. 알림은 transcript 라인의 `uuid`로 식별하며, 같은 `uuid`의 알림을 다시 처리해도 결과가 바뀌지 않아야 한다. | M |
| FR-BG-3 | 파싱한 `tool-use-id`로 대기 기록을 찾아 완료 결과를 연결해야 한다. 대기 기록이 없으면 알림만 반복 입력으로 저장한다. | M |
| FR-BG-7 | 작업 종류(Bash/Agent)와 작업 id는 같은 처리 단위의 시작 호출이나 원본 저장소의 대기 기록에서 찾는다. 다음을 모두 만족할 때만 `output-file`을 읽어야 한다: (a) 시작 호출을 찾음, (b) 알림의 `task-id`가 시작 호출의 작업 id와 같음, (c) `output-file`이 `.`/`..` 없는 절대 경로이고 `…/<session_id>/tasks/<task_id>.output` 형태임, (d) Bash는 링크를 따라간 실제 경로도 (c)의 형태이고, 그 경로를 링크를 따르지 않고 연 핸들이 일반 파일임, (e) Agent는 링크를 따라간 실제 경로가 `<transcript 디렉터리>/<session_id>/subagents/agent-<task_id>.jsonl`임. 하나라도 만족하지 않으면 결과 없이 상태와 요약만 저장한다. 위조된 알림으로 임의 파일을 읽지 않기 위한 제한이다. 출력 경로를 Claude Code 임시 디렉터리 루트에 고정하지는 않는다. 이 루트는 Claude Code 내부 배치라 버전마다 바뀔 수 있기 때문이다. 남는 위험은 (a)~(e)를 모두 맞춘 위조에 한정된다. | M |
| FR-BG-4 | Bash 작업은 알림을 감지한 즉시 `output-file` 내용을 읽어 결과로 저장해야 한다. 파일이 없으면 `status`와 `summary`만 저장한다. | M |
| FR-BG-5 | Agent 작업은 `output-file`(서브에이전트 transcript)에서 마지막 `SubagentHandback` 호출의 `tool_input.message`를 결과로 저장해야 한다. 이 호출이 없으면 마지막 assistant `text`를 저장한다. | M |
| FR-BG-6 | 같은 `task-id`로 `uuid`가 다른 알림이 여러 번 오면 기존 결과를 덮어쓰지 않고 순번을 붙여 추가해야 한다. | M |

#### 3.2.4 저장 (STO)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-STO-1 | 원본 저장소의 `iterations` 테이블에 반복마다 행 1개를 저장해야 한다. 필드: `id`(`<session_id>:<message_id>`), `session_id`, `prompt_id`, `message_id`, `iteration_index`(턴 안 순번), `cwd`, `ts`, `inputs`(JSON), `tool_calls`(JSON: `id`, `name`, `input`, `is_error`. 결과 원문은 넣지 않는다), `response_text`, `embed_text`. 반복과 결과의 연결은 `iteration_results`(`iteration_id`, `tool_use_id`, `result_id`)에 저장한다. 벡터 저장소에는 `id`, `vector`, `embed_model`과 메타데이터(`cwd`, `session_id`, `ts`)를 저장한다. `ts`는 반복 시각(FR-EXT-11)이며 가져오기에서도 transcript의 원래 값을 쓴다. 자동 주입(FR-RET-4)과 `show` 출력에 이 시각을 표시해 같은 주제의 기록 사이 선후를 알 수 있게 한다. | M |
| FR-STO-2 | `embed_text`는 다음 순서로 만든다. 비어 있는 섹션은 넣지 않는다.<br>`[input] <반복 입력>` (입력마다 1줄. 완료 알림은 `<status>: <summary>`)<br>`[tool] <name>(<input compact JSON>)` (호출마다 1줄. JSON이 설정 길이(기본 500자)를 넘으면 그 길이에서 자르고 `…`를 붙인다. 원문 입력은 `tool_calls`에 그대로 남는다. hippo 호출(FR-STO-11)은 넣지 않는다)<br>`[answer] <결과 텍스트>` (사용자 응답 도구 호출 바로 뒤에 1줄. 사용자 응답 도구는 설정 `[embed_text] answer_tools`(기본 `AskUserQuestion`, `ExitPlanMode`)에 있는 도구이며, 결과가 있을 때만 넣는다. 결과 라인의 `toolUseResult`에 `answers` 객체(질문 → 답)가 있으면 답만 넣는다. 질문 순서는 `toolUseResult.questions[].question` 순서를 따르고, 여러 개면 ` / `로 잇는다. `annotations[질문].notes`가 있으면 그 답 뒤에 ` (note: <메모>)`를 붙인다. `answers`가 없으면 결과 텍스트를 넣는다. 질문은 `[tool]` 줄에 있으므로 되풀이하지 않는다. 자르지 않으며, 긴 텍스트는 청크 분할(FR-STO-5)이 처리한다)<br>`[response] <text 블록을 개행으로 이어 붙임>` | M |
| FR-STO-3 | 원본 저장소의 `results` 테이블에 결과 원문을 원문 단위로 저장해야 한다. 필드: `id`(원문의 SHA-256 hex), `tool_name`(처음 저장한 도구), `text`, `first_seen_ts`. 빈 원문은 저장하지 않는다. 벡터는 벡터 저장소에 청크 단위로 저장한다. | M |
| FR-STO-4 | 같은 프로젝트 저장소에 같은 `content_hash`가 이미 있으면 결과를 새로 저장하거나 임베딩하지 않고 기존 결과 id를 반복에 연결해야 한다. | M |
| FR-STO-5 | 결과 원문이나 반복의 `embed_text`가 청크 한도를 넘으면 여러 청크로 나눠 청크마다 벡터를 만들어야 한다. 청크 한도 L(문자 수)은 임베더의 `max_input_tokens()`와 설정 `[chunking] max_tokens`(기본 1,000) 중 작은 값에 `chars_per_token`(기본 1.0)을 곱한 값이다. 겹침 O는 설정 `[chunking] overlap_chars`(기본 200)이며 L보다 작아야 한다. 그렇지 않으면 설정 오류이다. 텍스트 길이가 L 이하이면 청크 1개이다. 그보다 길면 i번째 청크(0부터)는 문자 위치 `[i·(L−O), i·(L−O)+L)`이고, 마지막 청크는 텍스트 끝에서 끝난다. 이웃한 두 청크는 O자를 공유한다. 각 청크에서 앞 청크와 겹치는 O자를 빼고 이어 붙이면 원문과 같아야 한다. 검색 질의가 L보다 길면 첫 L자만 임베딩하고, FTS에는 질의 전체를 쓴다. | M |
| FR-STO-6 | 저장과 임베딩 전에 비밀값 패턴을 `[REDACTED:<종류>]`로 치환해야 한다. 기본 패턴: API 키(`sk-` 뒤 20자 이상, `gh[pousr]_` 뒤 36자 이상, `xox[abprs]-`, `AKIA` 뒤 16자), `Bearer <토큰 16자 이상>`, PEM 개인 키 블록(END 마커가 없으면 BEGIN부터 원문 끝까지), 이름이 `KEY`/`TOKEN`/`SECRET`/`PASSWORD`로 끝나는(대소문자 무시) `이름=값` 또는 `이름: 값`의 값(8자 이상, 이름은 남김). 마스킹은 반복 입력, 응답 텍스트, 도구 입력 JSON의 모든 문자열, 결과 원문에 적용한다. 패턴은 설정 파일로 추가할 수 있어야 하며, 잘못된 패턴은 시작 시 오류로 보고한다. | M |
| FR-STO-7 | 임베딩은 반복 저장과 분리된 작업 큐로 처리해야 한다. 작업은 반복 저장과 같은 트랜잭션으로 프로젝트 원본 저장소의 `embed_jobs`(`kind`, `ref_id`, `enqueued_at`, `bulk`)에 등록한다. `bulk`는 가져오기(FR-IMP-5)와 병합(FR-PRJ-9)으로 등록한 작업이면 1, 그 밖에는 0이다. 작업은 묶음(FR-DMN-6) 단위로 처리한다. 묶음의 작업마다 원본 저장소에서 대상 텍스트(반복은 `embed_text`, 결과는 `text`)를 읽고 → 청크로 나누고(FR-STO-5) → 임베딩한다. 그다음 임베딩을 마친 작업들의 벡터를 `VectorStore::replace` 한 번으로 저장하고(그 레코드들의 기존 청크 벡터를 모두 지우고 새 벡터를 넣는다) → 그 작업들을 완료 처리한다. 대상 텍스트가 비어 있으면 벡터 없이 완료 처리한다. 묶음 도중 임베딩이 실패하거나 멈춤 조건(FR-DMN-6 (b))이 생기면, 그때까지 임베딩을 마친 작업만 저장하고 완료 처리한다. 임베딩이나 벡터 저장이 실패한 작업은 큐에 남기고 다음 처리 때 재시도해야 한다. 임베딩 요청은 청크마다 하나씩 보낸다. | M |
| FR-STO-11 | hippo 호출은 도구 이름이 `Bash`이고, `command`를 bash 문법으로 파싱했을 때 실행되는 명령 중 하나라도 hippo인 호출이다. 파서는 `brush-parser`를 쓴다. 명령은 `;`, `&&`, `||`, `|`, 줄바꿈, 서브셸, 명령 치환(`$( )`, 백틱) 안의 명령까지 모두 본다. 명령의 실행 단어는 앞의 `이름=값` 할당과 래퍼(`env`, `time`, `sudo`, `nohup`, `exec`, `command`)를 건너뛴 첫 단어이며, 따옴표를 벗긴 값의 파일 이름이 `hippo`이거나, `$이름`·`${이름}` 형태이고 같은 명령 줄에서 그 이름에 파일 이름이 `hippo`인 값을 할당했으면 hippo 실행이다. 인자 자리의 `hippo`(예: `cargo test -p hippo`)는 실행이 아니다. 파싱에 실패하면 첫 단어 규칙(따옴표를 벗긴 첫 단어의 파일 이름)으로 판정한다. hippo 호출은 `embed_text`에 넣지 않는다(FR-STO-2). 그 결과 `embed_text`가 비면 반복은 원본 저장소에만 저장되고 벡터와 FTS 결과에 나오지 않는다. hippo 호출의 결과 원문을 처음 저장할 때는 `results.indexed`를 0으로 저장하고 임베딩 작업과 FTS 행을 만들지 않는다. 재색인(FR-SWP-5)과 병합(FR-PRJ-9)도 `indexed`가 0인 결과를 색인하지 않는다. `hippo show`는 이 반복과 결과를 그대로 보여 준다. 자기 참조(기억을 찾던 장면)가 검색 결과에 다시 나오지 않게 하기 위한 규칙이다. | M |
| FR-STO-8 | 원본 저장소에서 `iterations.embed_text`와 `results.text`에 SQLite FTS5 인덱스(trigram 토크나이저)를 만들어야 한다. FTS 검색어는 공백으로 나눈 토큰 중 3자 이상인 것을 OR로 묶는다. 해당 토큰이 없으면 FTS 결과는 비어 있다. | M |
| FR-STO-10 | 반복, 결과, 반복-결과 연결, 백그라운드 기록, 임베딩 작업, 세션 해석기 상태의 변경은 한 처리 단위마다 하나의 트랜잭션으로 반영해야 한다. | M |
| FR-STO-9 | 프로젝트 벡터 저장소의 행 수가 설정된 기준(기본 10만)을 넘으면 그 저장소의 `VectorStore::ensure_index`를 호출해야 한다. | S |
| FR-STO-12 | 벡터 저장소는 쓰기로 쌓이는 이전 버전과 작은 데이터 파일을 정리해야 한다. 정리(`VectorStore::optimize`)는 작은 데이터 파일 합치기(compaction)와, 최신 버전을 뺀 모든 이전 버전의 삭제(prune)이다. 어느 버전에도 속하지 않는 파일은 지우지 않는다. 정리는 그 저장소의 다른 읽기·쓰기와 겹치지 않게 같은 핸들 안에서 실행한다. 저장소는 마지막 정리 뒤의 수정 연산(추가, 삭제) 수를 세어 `pending_changes`로 돌려준다. 정리 시점: (a) 벡터 저장소에 쓴 쪽(임베딩 스레드, 재색인)은 쓰기 직후 `pending_changes`가 설정 `[index] optimize_every`(기본 20) 이상이면 바로 정리한다. 값이 0이면 (a)를 하지 않는다. (b) 임베딩 스레드는 큐가 비면, 열려 있고 `pending_changes`가 0보다 큰 저장소를 정리한다. 마지막 정리 뒤 수정이 없으면 `optimize`는 아무것도 하지 않는다. 재색인(FR-SWP-5)은 원본 저장소의 페이지 단위로 `replace`하고, (a)의 규칙으로 정리하며, 끝나면 한 번 더 정리한다. 임베딩 스레드에서 정리가 실패하면 로그에 남기고 저장과 검색을 계속한다. 재색인에서 정리가 실패하면 그 프로젝트의 재색인을 실패로 처리한다. 버전을 남기지 않는 백엔드(메모리)의 `optimize`는 아무것도 하지 않고, `pending_changes`는 0이다. 근거: 부록 D. | M |

#### 3.2.5 검색 (RET)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-RET-1 | `user-prompt-submit` hook은 프롬프트로 현재 세션의 프로젝트 저장소에서 `iterations`를 검색해야 한다. 현재 세션의 프로젝트는 목록 DB의 세션 고정값이며, 고정값이 없으면 요청의 `cwd`로 FR-PRJ-5의 조회 전용 판정을 한다. 프로젝트가 없으면 아무것도 주입하지 않는다. `session_id`가 현재 세션과 같은 반복은 제외한다. 다른 프로젝트는 검색하지 않는다. 프롬프트가 `<task-notification>`을 포함하면(FR-EXT-6 (a)의 완료 알림) 검색하지 않고 아무것도 주입하지 않는다. 프롬프트의 앞뒤 공백을 뺀 문자 수(유니코드 문자 단위)가 설정 `[inject] min_prompt_chars`(기본 11) 미만이어도 검색하지 않고 아무것도 주입하지 않는다. "진행", "그거 알려줘"처럼 대상이 없는 짧은 지시에는 과거 기억이 도움이 되지 않기 때문이다. | M |
| FR-RET-2 | 검색은 벡터 검색과 FTS 결과를 합친 하이브리드 순위(RRF)로 해야 한다. 점수는 각 목록에서의 순위 r(1부터)에 대해 `Σ 1/(60 + r)`이다. 각 목록에서 후보를 `limit × 4`개까지 가져온다. 한 레코드의 여러 청크가 벡터 검색에 걸리면 가장 높은 유사도 하나로 합친다. 결과 검색에 범위 필터가 있으면 벡터 검색 후 원본 저장소의 반복-결과 연결로 걸러낸다. | M |
| FR-RET-3 | 자동 주입 후보는 벡터 유사도가 임계값(설정 `[inject] threshold`, 기본 0.50. 근거는 부록 C) 이상인 반복이다. FTS로만 찾은 반복은 후보가 아니다. 후보를 RRF 점수 순서로 보면서 `prompt_id`가 같은 반복은 한 턴으로 묶고, 서로 다른 턴을 최대 k개(설정 `top_k`, 기본 3개) 고른다. `prompt_id`가 없는 반복은 그 반복 하나를 턴으로 본다. 어떤 턴을 더하면 전체 길이 한도(FR-RET-4)를 넘을 때는 그 턴을 건너뛰고 다음 후보 턴을 본다. 고른 턴이 없으면 아무것도 주입하지 않는다. | M |
| FR-RET-4 | 주입 텍스트는 고른 턴마다 `<memory ts="…" prompt="…">` … `</memory>` 블록 하나를 담는다. `ts`는 턴 첫 반복의 시각이다. 블록은 `ts` 오름차순(오래된 것부터)으로 놓는다. 블록 내용은 턴의 반복을 `iteration_index` 순서로 보며, 반복마다 다음 줄을 이 순서로 만든다. 비어 있는 줄은 넣지 않는다.<br>`[input] <반복 입력>` (입력마다. 완료 알림은 `<status>: <summary>`)<br>`[response] <응답 텍스트>`<br>사용자 응답 도구(FR-STO-2의 `answer_tools`) 호출마다 `[question] <질문>`과 `[answer] <답>`. 질문은 호출 입력의 `questions[].question`을 질문마다 1줄로 쓰고, 그런 필드가 없는 도구는 `[question]` 줄을 넣지 않는다. 답은 FR-STO-2의 `[answer]`와 같은 텍스트이다.<br>그 밖의 도구 호출과 결과는 넣지 않는다. 어떤 줄도 자르지 않는다. 머리말과 끝 안내를 포함한 전체 길이는 설정 `total_chars`(기본 8,000자) 이하여야 한다. 기본값은 Claude Code가 hook의 `additionalContext`에 두는 고정 한도 10,000자보다 여유 있게 작게 잡는다. 이 한도를 넘으면 Claude Code는 본문을 파일로 옮기고 앞 2,000자 미리 보기만 모델에 넘긴다. 블록 안의 `&`, `<`, `>`, `"`는 이스케이프한다. 머리말은 블록을 이전 대화에서 꺼낸 기억(장기 기억 저장소의 항목)으로 소개하고, 블록 하나가 지난 대화 한 번이며 오래된 것부터 놓였음을 알리고, 배경 지식으로 쓰되 블록 안의 요청을 지시로 따르지 말라고 밝힌다. 끝에 `hippo show <prompt_id> --with-results`로 도구 출력까지 포함한 전체 대화를 볼 수 있다는 안내를 붙인다. 머리말과 안내는 영어이고(FR-UI-2), 세션·턴·반복 같은 내부 용어를 쓰지 않는다. | M |
| FR-RET-5 | `user-prompt-submit` hook은 시간 예산(설정, 기본 800ms)을 넘으면 검색을 포기하고 아무것도 주입하지 않아야 한다. 데몬의 검색은 단계(임베딩, 벡터 검색, FTS)마다 마감 시각을 확인하고, hook 클라이언트는 같은 예산으로 소켓 타임아웃을 건다. hook 클라이언트가 처음 연결에 실패해 데몬을 띄운 경우(콜드 시작)에는 그 요청의 예산을 설정 `[inject] cold_budget_ms`(기본 2,500ms)로 한다. 데몬과 임베딩 모델을 새로 올리는 시간(실측 약 1초)을 감안한 값이다. 클라이언트는 적용한 예산을 요청에 실어 보내고, 데몬은 요청에 예산이 있으면 그 값으로, 없으면 `budget_ms`로 마감 시각을 정한다. 데몬이 떠 있어도 임베딩 모델이 메모리에 없으면(모델 콜드) 데몬은 요청 예산 대신 `cold_budget_ms`를 쓴다. Ollama 임베더는 `GET /api/ps` 응답의 `models[].name`에 설정 모델(태그가 없으면 `:latest`를 붙인 이름)이 없거나 요청이 실패하면 모델이 메모리에 없다고 본다. 데몬이 예산을 늘릴 수 있으므로 hook 클라이언트는 소켓 대기 한도를 `cold_budget_ms` 기준으로 건다. `cold_budget_ms`가 `budget_ms`보다 작으면 설정 오류이다. | M |
| FR-RET-10 | 데몬은 자동 주입 요청의 처리 시간이 그 요청에 적용한 시간 예산(FR-RET-5) 이상이면, 결과 유무와 관계없이 데몬 오류 로그에 한 줄을 남겨야 한다. 이 줄에는 프로젝트 id, 경과 시간(ms), 시간 예산(ms), 그 시점의 임베딩 대기 작업 수(일반 단계, bulk 단계)를 담는다. 예산 안에 끝난 요청은 기록하지 않는다. | M |
| FR-RET-11 | 벡터 검색(자동 주입과 `hippo search`)은 질의 앞에 설정 `[embedder] query_prefix`를 붙인 텍스트를 임베딩해야 한다. 기본값은 Qwen3-Embedding이 권장하는 질의 지시문 `Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:`(`\n`은 줄바꿈)이다. 빈 문자열이면 아무것도 붙이지 않는다. FR-STO-5의 질의 길이 제한(첫 L자)은 접두사를 붙이기 전의 질의에 적용한다. FTS에는 접두사를 붙이지 않은 질의를 쓴다. 저장하는 반복·결과의 임베딩 텍스트에는 붙이지 않으므로, 이 값을 바꿔도 재색인은 필요 없고 데몬 재시작만 필요하다. 임베딩 모델을 바꾸면 그 모델이 권장하는 질의 형식으로 이 값을 바꾼다. | M |
| FR-RET-12 | `user-prompt-submit` hook은 설정 `[inject] enabled`(기본 true)가 false이면(주입 일시정지) 데몬에 묻지 않고 아무것도 출력하지 않은 채 종료 코드 0으로 끝나야 한다(IF-HK-3). 다음 hook부터 적용된다. | M |
| FR-RET-6 | `hippo search`는 IF-CLI-1의 옵션에 따라 범위와 종류를 바꿔 검색해야 한다. `--kind result` 결과에는 그 결과를 참조하는 반복의 `prompt_id`를 함께 출력한다. 범위의 프로젝트가 없으면 "프로젝트 없음"을 출력하고 종료 코드 0으로 끝난다. | M |
| FR-RET-9 | `--scope all`은 프로젝트마다 FR-RET-2의 하이브리드 검색을 해 순위 목록을 얻고, 그 목록들을 같은 RRF 식으로 합쳐 상위 `limit`개를 출력해야 한다. 결과마다 프로젝트 id를 표시한다. 한 프로젝트에서 벡터 검색이 실패하거나 호환되지 않으면 그 프로젝트만 FTS 결과를 쓰고 FR-RET-8의 안내를 붙인다. | M |
| FR-RET-8 | `hippo search` 처리 중 임베딩이나 벡터 검색이 실패하면 FTS 결과만으로 응답하고, 출력 첫 줄에 벡터 검색을 쓰지 못했다는 안내와 사유를 붙여야 한다. 자동 주입은 이 경우 아무것도 주입하지 않는다. | M |
| FR-RET-7 | `hippo show`는 `--scope`가 없으면 현재 프로젝트에서 먼저 찾고, 없으면 나머지 프로젝트를 마지막 활동 시각이 최근인 순서로 찾는다. `--scope`가 있으면 그 프로젝트에서만 찾는다. 찾은 `prompt_id`의 반복을 `iteration_index` 순서로 출력하고, 반복에 백그라운드 시작 호출이 있으면 그 작업의 완료 기록(순번, 상태, 요약, 결과 id)을 함께 표시해야 한다. | M |

#### 3.2.8 설치 (INS)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-INS-1 | 대상 설정 파일은 `<Claude 폴더>/settings.json`이다. Claude 폴더는 `--claude-dir`(IF-CLI-6)로 준다. 상대 경로는 현재 디렉터리 기준 절대 경로로 바꾼다. | M |
| FR-INS-2 | hippo hook 항목은 `command`가 `hippo hook <post-tool-batch\|stop\|stop-failure\|session-end\|user-prompt-submit>`로 끝나고, 실행 파일 이름이 `hippo`인 항목으로 식별한다. 설정 파일에 별도 표식 키를 넣지 않는다. | M |
| FR-INS-3 | 설치는 기존 hippo hook 항목을 모두 지운 뒤 IF-HK-4의 hook 설정을 이벤트마다 추가해야 한다. 다른 hook과 다른 설정 키는 내용과 순서를 그대로 유지한다. 비게 된 matcher 그룹과 이벤트는 지운다. | M |
| FR-INS-4 | 제거는 hippo hook 항목만 지워야 한다. 비게 된 matcher 그룹, 이벤트, `hooks` 키는 지운다. 그 결과 설정이 빈 객체(`{}`)이면 파일을 백업 없이 지우고, 설정 파일이 있던 디렉터리가 비면 그 디렉터리도 지운다. | M |
| FR-INS-5 | 결과가 기존 내용과 같으면 파일을 쓰지 않고 "변경 없음"을 출력한다. 다르면 변경 내용(diff)을 출력하고, 기존 파일이 있으면 `<파일>.hippo-backup-<유닉스 시각>`(이미 있으면 `-<번호>`를 붙임)으로 백업한 뒤 임시 파일에 쓰고 이름을 바꿔 원자적으로 교체한다. 기존 파일 권한을 유지한다. 임시 파일은 같은 디렉터리에 무작위 이름으로 배타적으로 만들고, 백업은 링크를 따르지 않는 배타적 생성(권한 0600)으로 만든다. 설정 파일이나 바로 위 디렉터리가 심볼릭 링크이면 수정하지 않고 오류로 끝낸다. `--dry-run`이면 diff만 출력하고 아무것도 쓰지 않는다. | M |
| FR-INS-7 | 설치는 Claude 메모리 파일 `<Claude 폴더>/CLAUDE.md`에 안내 블록을 넣어야 한다. 블록은 `<!-- hippo:begin -->` 줄과 `<!-- hippo:end -->` 줄로 감싼다(Claude Code는 블록 HTML 주석을 컨텍스트에 넣기 전에 지운다). 블록이 이미 있으면 그 블록만 새 내용으로 바꾸고, 없으면 파일 끝에 빈 줄 하나를 두고 덧붙인다. 블록 밖의 내용은 그대로 둔다. 파일 끝 줄바꿈의 유무는 원래 파일을 따른다. 기존 줄의 줄 끝(LF, CRLF)은 줄마다 그대로 두고, 새로 넣는 줄은 원래 파일 첫 줄의 줄 끝을 쓴다(첫 줄에 줄 끝이 없으면 LF). 그래서 블록이 없던 파일에 설치한 뒤 제거하면, 줄 끝이 섞인 파일도 원래 파일과 바이트 단위로 같다. 제거는 블록만 지우고, 그 결과 파일이 공백뿐이면 파일을 백업 없이 지운다. 블록의 명령은 FR-INS-8의 설치 경로를 FR-INS-10의 셸 표기로 쓴다. `hippo`가 PATH에 없어도 Claude가 안내대로 실행할 수 있게 하기 위해서다. 시작 표식만 있고 끝 표식이 없으면 수정하지 않고 오류로 끝낸다. 쓰기, 백업, 변경 없음, diff, `--dry-run`, 심볼릭 링크 거부는 FR-INS-5와 같다. | M |
| FR-INS-8 | 설치는 실행 중인 `hippo` 실행 파일을 `--data-dir`의 설치 경로(`<데이터 루트>/bin/hippo`)로 복사하고, hook 명령(IF-HK-4)과 안내 블록(FR-INS-7)은 설치 경로를 써야 한다. 빌드 폴더나 앱을 지우거나 옮겨도 hook이 동작하게 하기 위해서다. 복사는 같은 디렉터리의 임시 파일에 쓴 뒤 이름을 바꿔 원자적으로 하고, 디렉터리 권한은 0700, 실행 파일 권한은 0700이다. 설치 경로의 파일이 실행 중인 파일과 내용이 같거나 실행 중인 파일 자체이면 복사하지 않는다. 설치 경로가 심볼릭 링크이면 오류로 끝낸다. 복사해서 파일을 바꿨으면, 그 데이터 루트의 데몬에 `shutdown`(FR-DMN-11)을 요청해 이전 실행 파일의 데몬을 내린다. 데몬이 없으면 아무것도 하지 않고, 거부되면(재색인 중) 데몬을 나중에 다시 시작하라는 경고를 출력한다. 다음 요청이 새 실행 파일로 데몬을 띄운다(FR-DMN-7). `--dry-run`이면 복사하지 않고 설치 경로를 출력한다. | M |
| FR-INS-9 | 제거는 지정한 Claude 폴더의 hook(FR-INS-4)과 안내 블록(FR-INS-7), PATH 링크(FR-INS-11)만 지워야 한다. 설치 경로의 실행 파일과 데이터 루트의 다른 내용은 지우지 않는다. 그래서 제거한 뒤에도 그 실행 파일로 기록을 조회하거나 `purge`를 실행할 수 있다. 지정한 Claude 폴더가 FR-INS-12의 기록 값과 같으면 기록 파일도 지운다. 지울 것이 없으면 "변경 없음"을 출력하고 종료 코드 0으로 끝난다. `--dry-run`이면 아무것도 바꾸지 않는다. | M |
| FR-INS-10 | hook 명령과 안내 블록에 쓰는 경로(셸 표기)는, 경로가 `$HOME` 아래이면 `~/<나머지>`로 쓰고 아니면 절대 경로로 쓴다. 표기할 부분(`~/` 뒤의 나머지 또는 절대 경로)에 셸에서 그대로 쓸 수 없는 문자가 있으면 그 부분만 작은따옴표로 감싼다. 생성된 설정 파일이나 메모리 파일이 공유되어도 계정 이름이 드러나지 않게 하기 위해서다. Claude Code는 hook 명령을 셸로 실행하므로 `~`가 펼쳐진다(부록 A). 셸을 거치지 않는 launchd plist(IF-CLI-4)는 절대 경로를 쓴다. | M |
| FR-INS-6 | 기존 파일이 JSON으로 해석되지 않거나, 최상위가 객체가 아니거나, `hooks`가 객체가 아니면 수정하지 않고 오류로 끝낸다. 파일이 없으면 새로 만든다(디렉터리 포함). | M |
| FR-INS-11 | 설치는 PATH 링크 `~/.local/bin/hippo`를 설치 경로를 가리키는 심볼릭 링크(대상은 설치 경로의 절대 경로)로 만들어야 한다. (1) `~/.local/bin`이 없으면 권한 0755로 만든다. (2) 그 경로에 이미 같은 설치 경로를 가리키는 링크가 있으면 바꾸지 않는다. 다른 곳을 가리키는 링크나 링크가 아닌 파일이 있으면 아무것도 바꾸지 않고(hook, 안내 블록, 복사 포함) 오류로 끝낸다. 그래서 설치는 PATH 링크 검사를 다른 변경보다 먼저 한다. (3) 제거는 PATH 링크가 실행 파일의 설치 경로를 가리킬 때만 지운다. 다른 곳을 가리키면 그대로 두고 그 사실을 출력한다. (4) 설치 뒤 환경 변수 `PATH`에 `~/.local/bin`이 없으면, 터미널에서 `hippo`를 바로 쓰려면 `~/.local/bin`을 PATH에 더해야 한다는 경고를 출력한다. 셸 설정 파일은 바꾸지 않는다(C-7). `--dry-run`이면 만들거나 지울 링크만 출력한다. | M |
| FR-INS-12 | 설치는 사용한 Claude 폴더의 절대 경로를 `<데이터 루트>/bin/claude-dir`(한 줄, 권한 0600)에 기록해야 한다. 같은 루트로 다른 Claude 폴더에 다시 설치하면 마지막 값으로 바꾼다. `--claude-dir`을 생략했을 때의 기본값과 GUI의 Claude 폴더 기본값(FR-GUI-22)은 이 값이다. 파일이 없거나 비어 있으면 `~/.claude`를 쓴다. | M |
| FR-INS-13 | 완전 삭제(`purge`)는 다음 순서로 해야 한다. (1) 지정한 Claude 폴더(생략하면 FR-INS-12의 기본값)로 제거(FR-INS-9)를 한다. 다른 Claude 폴더에 남은 설치는 찾지 않는다. (2) 확인을 받는다(IF-CLI-13, FR-GUI-10). 취소하면 (1)의 제거는 되돌리지 않는다. (3) 그 데이터 루트로 수동 중지(FR-DMN-12)를 해 데몬과 hippo가 띄운 Ollama 서버를 멈춘다. 실패하면 데이터 루트를 지우지 않고 오류로 끝낸다. (4) 데이터 루트 디렉터리를 통째로 지운다. 데이터 루트가 심볼릭 링크이거나, `$HOME` 자체이거나, `/`이면 지우지 않고 오류로 끝낸다. | M |
| FR-INS-14 | 관리 실행 파일이 아닌 실행 파일(앱 실행 파일, 빌드 폴더 등)에서는 `install`, `gui`, 도움말, 버전 출력만 허용해야 한다. 다른 명령은 아무것도 하지 않고 "먼저 `hippo install`로 설치한 뒤 설치 경로의 `hippo`를 쓰라"는 안내를 stderr에 출력하고 종료 코드 1로 끝낸다. hook 명령도 같지만 IF-HK-2·IF-HK-3에 따라 종료 코드 0으로 끝낸다. | M |

#### 3.2.9 임베딩 서버 자동 시작 (EMB)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-EMB-1 | Ollama 임베더는 서버에 연결할 수 없으면, 설정 `[embedder] autostart`(기본 true)가 켜져 있을 때 `[embedder] ollama_command`(기본 `ollama`. 경로 구분자가 없으면 PATH, 그다음 `/opt/homebrew/bin`, `/usr/local/bin`에서 실행 파일을 찾는다)로 `serve`를 분리된 프로세스로 띄워야 한다. 이때 환경 변수 `OLLAMA_HOST`를 설정 URL의 host:port로 지정한다. 출력은 `logs/ollama.log`에 덧붙인다. | M |
| FR-EMB-2 | 서버를 띄운 뒤 `/api/version`이 응답할 때까지 최대 10초 기다리고, 임베딩을 한 번 다시 시도해야 한다. 여러 스레드가 동시에 실패해도 서버는 한 번만 띄운다. | M |
| FR-EMB-3 | hippo는 자신이 띄운 Ollama 서버를 수동 중지(FR-DMN-12)와 완전 삭제(FR-INS-13) 때 말고는 종료하지 않는다. 다른 도구가 같은 서버를 쓸 수 있고, 모델 메모리는 `keep_alive`가 지나면 Ollama가 해제하기 때문이다. 모델을 자동으로 내려받지도 않는다. | M |
| FR-EMB-4 | FR-EMB-1로 Ollama 서버를 띄우면 그 pid를 `<데이터 루트>/ollama.pid`(권한 0600)에 기록해야 한다. 수동 중지는 이 pid의 프로세스가 실행 중이고 실행 파일 이름이 `ollama`일 때만 종료 신호(SIGTERM)를 보낸다. 그 밖에는 아무것도 하지 않는다. | M |

#### 3.2.10 프로젝트 (PRJ)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-PRJ-1 | 데몬은 세션의 처리 요청을 받았을 때 목록 DB에 그 세션의 프로젝트 고정값이 없으면, transcript를 처음부터 읽어 세션 시작 경로를 얻고 FR-PRJ-4로 프로젝트를 정해 `session_id → 프로젝트 id`로 기록해야 한다. 이후 그 세션의 처리는 모두 그 프로젝트 저장소에서 한다. 세션 중에 `cwd`가 바뀌어도 고정값을 바꾸지 않는다. 개행으로 끝나는 라인 중 `cwd`를 가진 라인이 아직 없으면 아무것도 처리하지 않는다. | M |
| FR-PRJ-2 | 판정에 쓰는 경로는 심볼릭 링크를 풀어 절대 경로로 바꿔야 한다. 경로가 존재하지 않으면 받은 문자열을 그대로 쓴다. 목록 DB에 기록하는 경로와 git common dir도 같은 방식으로 바꾼 값이다. | M |
| FR-PRJ-3 | 경로의 git 정보는 작업 트리 최상위(`git -C <경로> rev-parse --show-toplevel`), git common dir(`git -C <경로> rev-parse --path-format=absolute --git-common-dir`), 루트 커밋 목록(`git -C <경로> rev-list --max-parents=0 HEAD`)이다. 커밋이 없으면 루트 커밋 목록은 비어 있다. git 작업 트리가 아니면(A-5 포함) git 정보는 없다. git을 실행하는 것은 프로젝트 위치를 정할 때뿐이다. | M |
| FR-PRJ-4 | 기록을 포함한 판정은 세션 시작 경로의 프로젝트 위치와 git 정보로 다음 순서를 따른다. 처음 맞는 규칙에서 끝난다. (a) 위치가 어떤 프로젝트의 위치와 같으면 그 프로젝트다. worktree는 이 규칙으로 원본 작업 트리의 프로젝트에 통합된다. (b) git 저장소이고 작업 트리 최상위가 어떤 git이 아닌 프로젝트의 위치와 같으면(그 폴더에서 나중에 `git init`한 경우), 그 프로젝트의 위치를 현재 위치로 바꾼다. (c) git 저장소이고 루트 커밋 중 하나가 어떤 프로젝트의 루트 커밋 목록에 있으며 그 프로젝트의 위치가 존재하지 않으면, 폴더를 옮긴 것으로 보고 그 프로젝트의 위치를 현재 위치로 바꾼다. 후보가 여럿이면 마지막 활동 시각이 가장 최근인 프로젝트를 고른다. (d) 그 밖에는 새 프로젝트를 만든다. (a)~(d) 모두 현재 루트 커밋 중 그 프로젝트에 없는 것을 추가한다. 판정과 기록은 목록 DB 트랜잭션 하나로 한다. 위치를 바꿀 때 이전 위치는 남기지 않는다. | M |
| FR-PRJ-5 | 조회 전용 판정은 주어진 경로의 git 정보로 FR-PRJ-4의 (a)~(c)를 같은 순서로 적용하되 목록 DB를 바꾸지 않는다. 맞는 규칙이 없으면 "프로젝트 없음"이다. CLI의 `--scope project`/`<path>`, 세션 고정값이 없는 자동 주입, 가져오기의 `from`/`to` 판정(FR-PRJ-7)은 조회 전용 판정을 쓴다. | M |
| FR-PRJ-6 | `hippo project list`는 프로젝트마다 id, 위치와 위치 존재 여부, 반복 수, 마지막 활동 시각을 마지막 활동 시각이 최근인 순서로 출력해야 한다. 시각은 로컬 시간대의 `YYYY-MM-DD HH:MM:SS`로 표시한다(문자열 순서가 시간 순서와 같다). | M |
| FR-PRJ-7 | 가져오기 준비(`import_prepare`)는 원본 프로젝트와 대상 프로젝트를 정해 다음과 같이 처리해야 한다. 원본 프로젝트: `from`이 32자 소문자 hex이고 그 id의 프로젝트가 있으면 그 프로젝트이다. 아니면 경로로 보고 FR-PRJ-2로 바꾼 뒤, 위치가 그 경로와 같거나 `<경로>/.git`과 같은 프로젝트이고, 없으면 그 경로의 조회 전용 판정 결과이다. 원본 프로젝트가 없을 수 있다. 대상 프로젝트는 `to`의 조회 전용 판정 결과이다. (a) 둘 다 없으면: `to`로 FR-PRJ-4의 기록 포함 판정을 해 대상 프로젝트를 만든다. (b) 원본만 있으면: 원본의 위치를 `to`의 프로젝트 위치로 바꾸고 루트 커밋을 추가한다. 데이터는 옮기지 않는다. 원본이 대상이 된다. (c) 대상만 있거나 원본과 대상이 같으면: 바꾸지 않는다. (d) 둘이 따로 있으면: FR-PRJ-9에 따라 원본의 기록을 대상으로 병합한다. 어느 경우든 이전 위치는 남기지 않는다. 재색인 중이면 오류를 돌려준다. 결과로 처리 경우, 대상 프로젝트 id, 병합한 반복 수, 결과 수, 벡터 처리 방식(복사한 벡터 수 또는 등록한 임베딩 작업 수)을 돌려준다. | M |
| FR-PRJ-8 | 목록 DB에는 `projects`(`id` 기본 키, `location` 고유, `kind`(`git`/`dir`), `created_at`, `last_active_at`), `project_roots`(`root_commit`, `project_id`, 둘을 묶어 기본 키), `session_projects`(`session_id` 기본 키, `project_id`)를 둔다. 목록 DB 쓰기는 데몬만 한다. `last_active_at`은 그 프로젝트의 수집을 처리할 때 갱신한다. | M |
| FR-PRJ-9 | 병합은 수집 스레드에서 다음 순서로 해야 한다. (1) 원본 프로젝트 원본 저장소의 `iterations`, `results`, `iteration_results`, `bg_tasks`, `bg_completions`, `sessions`, `embed_jobs` 행을 대상 원본 저장소에 추가한다. 같은 키의 기존 행은 바꾸지 않는다. (2) 원본과 대상의 활성 벡터 저장소의 `embed_model`과 차원이 같고 둘 다 설정의 임베더와 호환되면, 원본 벡터를 `scan`으로 읽어 대상에 `upsert`한다. 그렇지 않으면 (1)에서 추가한 반복과 결과마다 대상에 임베딩 작업을 등록한다. (1)과 작업 등록은 대상 원본 저장소의 트랜잭션 하나로 한다. (3) 목록 DB 트랜잭션 하나로 원본 세션의 고정값을 대상으로 바꾸고, 원본의 루트 커밋을 대상에 추가하고, 원본 프로젝트와 그 루트 커밋 행을 지운다. (4) 원본 프로젝트를 닫고 `projects/<원본 id>/` 디렉터리를 지운다. 어느 단계에서 실패해도 같은 가져오기를 다시 실행하면 중복 행 없이 끝까지 진행되어야 한다. | M |

#### 3.2.11 가져오기 (IMP)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-IMP-1 | `hippo import`는 `from`과 `to`(생략하면 현재 디렉터리)를 데몬에 보내 가져오기를 준비하고(FR-PRJ-7), 돌려받은 후보 세션마다 `import_session` 요청을 하나씩 순서대로 보내야 한다. 요청마다 진행률 한 줄(`[n/m]`, session id 앞 8자, 처리 결과, 저장한 반복 수, 대상 프로젝트의 남은 임베딩 작업 수)을 출력하고, 끝나면 합계를 출력한다. 중간에 중단해도 같은 명령을 다시 실행하면 끝난 세션은 "변경 없음"으로 지나가고 남은 세션을 이어서 처리한다. 실제 `import_prepare`(`dry_run`이 거짓)와 `import_session` 요청은 시간 제한 없이 데몬의 응답을 기다린다. 응답 전에 연결이 끊기면 그 내용을 stderr에 출력하고 종료 코드 1로 끝난다. | M |
| FR-IMP-2 | 후보 세션은 설정 `[import] transcripts_dir`(기본 `~/.claude/projects`) 바로 아래 디렉터리들에 있는 `*.jsonl` 파일이다. 하위 디렉터리(`subagents/` 등)는 읽지 않는다. session id는 파일 이름에서 `.jsonl`을 뺀 값이다. 세션 시작 경로를 FR-PRJ-2로 바꾼 값을 S, `from` 경로를 같은 방식으로 바꾼 값을 F라 할 때 다음 중 하나를 만족하면 후보이다. (a) S가 F와 같거나 F 아래에 있다. (b) S가 존재하고, S의 프로젝트 위치가 F의 프로젝트 위치와 같다(worktree 포함). `from`이 프로젝트 id이면 F는 그 프로젝트의 위치이며, 위치가 `<경로>/.git`이면 `<경로>`이다. 세션 시작 경로가 없는 파일은 후보가 아니다. 후보는 transcript 첫 `timestamp`의 오름차순으로 정렬하고, 같으면 파일 경로 순으로 정렬한다. `timestamp`가 없는 파일은 그 뒤에 파일 수정 시각 순으로 둔다. | M |
| FR-IMP-3 | `import_session`은 대상 프로젝트에서 다음과 같이 세션 하나를 처리해야 한다. 세션 고정값이 다른 프로젝트이면 처리하지 않고 "다른 프로젝트"로 돌려준다. 고정값이 없으면 대상 프로젝트로 고정한다. (a) 세션 상태가 없으면 transcript를 처음부터 읽고 시작 offset을 0으로 저장한다. (b) 상태가 있고 시작 offset이 0보다 크면, 새 해석기로 0부터 시작 offset 앞까지 읽어 시작 offset 위치에서 턴을 완료로 확정하고(FR-EXT-7 c와 같다), 그 반복을 저장한 뒤 시작 offset을 0으로 바꾼다. 안전 offset과 다른 해석기 상태는 바꾸지 않는다. (c) 상태가 있고 시작 offset이 0이면 실시간 수집과 같이 안전 offset 뒤를 읽는다. (a)와 (c)에서는 세션이 대기 중인지 끝났는지 구분하지 않고, 읽은 범위 끝의 마지막 반복을 모양으로 판단한다. 그 반복의 모든 `tool_use`에 `tool_result`가 있으면(`tool_use`가 없는 반복 포함) 완료로 저장하고, 결과가 없는 `tool_use`가 있으면 보류한다(FR-EXT-8). 반복에 붙지 않은 반복 입력도 보류한다. 보류한 것은 이후의 실시간 수집이나 가져오기에서 이어서 처리한다. 저장한 것이 없으면 "변경 없음"이다. | M |
| FR-IMP-4 | 가져오기에서는 `message.id`가 같고 `session_id`가 다른 반복이 대상 원본 저장소에 이미 있으면, 그 반복(결과 연결과 임베딩 작업 포함)을 저장하지 않아야 한다. 후보를 오래된 순서로 처리하므로 fork 복사본은 원본 세션에 남는다. 반복 id 규칙(FR-EXT-10)은 바꾸지 않는다. | M |
| FR-IMP-5 | 가져오기로 등록한 임베딩 작업은 `bulk = 1`로 등록해야 한다. | M |
| FR-IMP-6 | 데몬은 쓰기 스레드에서 `import_session`보다 다른 쓰기 작업(실시간 수집, 가져오기 준비)을 먼저 처리해야 한다. `import_session` 하나는 세션 하나만 처리한다. 데몬은 실제 `import_prepare`와 `import_session`의 응답을 쓰기 스레드가 처리를 끝낼 때까지 시간 제한 없이 기다렸다가 보낸다. 그래서 오류 응답은 처리하지 않았다는 뜻이다(FR-DEL-6과 같다). | M |
| FR-IMP-7 | `import_session`의 응답은 처리 결과(`가져옴`, `변경 없음`, `다른 프로젝트`, `오류: <사유>`), 저장한 반복 수, 완료 알림 수, fork 복사본으로 건너뛴 반복 수, 대상 프로젝트의 남은 임베딩 작업 수를 담아야 한다. 재색인 중이면 오류를 돌려준다. | M |
| FR-IMP-8 | `import_prepare` 응답의 후보마다 session id와 transcript 경로에 더해 다음을 담아야 한다. (1) 시작 시각: 첫 `timestamp`(없으면 비움). (2) 세션 시작 경로. (3) 파일 크기(바이트). (4) 첫 프롬프트: `type`이 `user`이고 `isMeta`가 true가 아니며 `tool_result`가 아닌 첫 라인의 텍스트(문자열이거나 `text` 블록을 이은 것) 중 `<task-notification>`으로 시작하지 않는 것. 공백 문자열은 공백 하나로 줄이고, 비밀값 마스킹(FR-STO-6)을 거친 뒤 앞 80자(문자 단위)만 담는다. 없으면 비움. (5) 가져오기 상태: 세션 고정값이 대상 프로젝트가 아닌 프로젝트이면 `다른 프로젝트`, 대상 원본 저장소에 세션 상태가 없으면 `새 세션`, 시작 offset이 0보다 크면 `실시간만`, 0이면 `가져옴`이다. | M |
| FR-IMP-9 | `dry_run`이 true인 `import_prepare`는 FR-PRJ-7과 같은 규칙으로 처리 경우((a)~(d))와 대상 프로젝트를 정하되, 목록 DB와 원본 저장소를 바꾸지 않아야 한다. 재색인 중에도 처리한다. 응답은 처리 경우, 대상 프로젝트 id(경우 (a)이면 비움), 원본 프로젝트 id(있으면), FR-IMP-8의 후보 목록이다. 가져오기 상태는 실행했을 때의 대상 프로젝트를 기준으로 한다. 경우 (b)는 원본 프로젝트를, 경우 (d)는 원본 프로젝트에 고정된 세션도 대상 프로젝트에 고정된 것으로 보고, 원본 저장소의 세션 상태도 함께 본다. `hippo import --dry-run`은 처리 경우를 설명하는 한 줄과 후보마다 한 줄(번호, 시작 시각(로컬 시간대 `YYYY-MM-DD HH:MM:SS`), session id 앞 8자, 가져오기 상태, 파일 크기, 세션 시작 경로, 첫 프롬프트)을 출력하고, 끝에 상태별 후보 수를 출력한다. 출력은 QA-6에 따라 제어 문자를 이스케이프한다. | M |

#### 3.2.12 GUI
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-GUI-1 | GUI는 창 하나에 탭 8개를 정보, 상태, 기록, 프로젝트, 검색, 가져오기, Claude 적용, 설정 순서로 두어야 한다. 창을 열 때의 탭은 FR-UI-4를 따른다. 데몬 요청은 GUI 데이터 루트(FR-GUI-22)의 소켓으로 CLI와 같은 클라이언트를 써서 보내고, 요청의 `cwd`는 실행 경로이다. 데몬 자동 시작(FR-DMN-7)은 GUI 데이터 루트의 설치 경로를 실행한다. GUI 자신의 실행 파일로 데몬을 띄우지 않는다. 설치 상태(FR-GUI-23)가 `설치됨`이 아니면 Claude 적용 탭과 정보 탭 말고 다른 탭은 비활성화하고, 상태 폴링도 하지 않는다. | M |
| FR-GUI-2 | 데몬 요청, 파일 읽기와 쓰기, 설치 작업은 화면을 그리는 스레드 밖에서 실행해야 한다. 같은 탭의 작업이 진행 중이면 그 작업을 다시 시작하는 버튼을 비활성화한다. 실패하면 오류 메시지를 그 탭에 표시한다. | M |
| FR-GUI-3 | 상태 탭은 창이 열려 있는 동안 2초마다 `status`를 요청해 IF-CLI-5의 항목(데몬 상태, 스풀 대기 건수, 재색인 상태, 프로젝트별 표, 처리 속도, 남은 예상 시간)과 마지막 갱신 시각을 표시해야 한다. 프로젝트 표 위에는 임베딩 큐 요약을 둔다: 모든 프로젝트의 실시간 대기 합계, 가져오기 대기 합계, 처리 속도 합계, 남은 예상 시간(대기 합계 ÷ 속도 합계). 대기가 없으면 `대기 없음`을 표시한다. 프로젝트 표는 실시간 대기와 가져오기 대기를 다른 칸에 두고, 처리 속도와 남은 예상 시간은 잘리지 않게 따로 표시한다. 폴링은 다른 탭을 보고 있을 때도 계속한다. 폴링도 FR-DMN-7에 따라 데몬을 띄우므로, 창이 열려 있는 동안 데몬은 유휴 종료하지 않는다. 데몬에 연결할 수 없으면 `연결 안 됨`과 사유를 표시하고 다음 주기에 다시 시도한다. 상태 탭에는 `멈추기` 버튼을 둔다. 누르면 수동 중지(FR-DMN-12)를 하고, 폴링을 멈춘 뒤 `멈춤`과 결과를 표시하고 `다시 시작` 버튼을 보여 준다. 폴링이 데몬을 다시 띄우지 않게 하기 위해서다. `다시 시작`을 누르면 폴링을 다시 시작한다(폴링이 FR-DMN-7에 따라 데몬을 띄운다). 상태 탭에는 주입과 저장 각각의 현재 상태(`동작 중`, `일시정지`)와 `일시정지`/`재생` 버튼(FR-CAP-8, FR-RET-12)을 둔다. 누르면 그 키만 FR-GUI-5의 방식으로 `config.toml`에 저장하고 바로 표시를 바꾼다. 데이터 루트를 바꾸면 그 루트의 설정을 다시 읽는다. | M |
| FR-GUI-4 | 설정 탭은 `Config`의 모든 키를 섹션별로 보여 주고 편집할 수 있어야 한다. 키마다 현재 값, 기본값인지 여부, 설명을 표시한다. 목록 값(`answer_tools`, `redact.patterns`)은 행을 더하고 지울 수 있다. 줄바꿈이 들어갈 수 있는 `[embedder] query_prefix`는 여러 줄 입력창으로 편집한다. 열 때 `config.toml`을 읽으며, 파일이 없으면 모든 키를 기본값으로 보여 준다. 파일이 `Config::parse`로 해석되지 않으면 오류를 표시하고 저장을 막는다. | M |
| FR-GUI-5 | 설정 저장은 다음을 따라야 한다. (1) 파일의 기존 텍스트를 `toml_edit`로 읽어, 이번에 GUI에서 바꾼 키만 고친다. 다른 키, 주석, 순서는 그대로 둔다. (2) 새 값이 기본값과 같으면 그 키를 지우고, 그 결과 비게 된 테이블도 지운다. 그 밖에는 키를 쓰거나 바꾼다. (3) 결과 텍스트를 `Config::parse`로 검증하고, 실패하면 파일을 쓰지 않고 오류를 표시한다. (4) FR-INS-5와 같은 방식(같은 디렉터리의 임시 파일, 이름 바꾸기, 심볼릭 링크 거부)으로 원자적으로 쓰며 권한은 0600이다. 백업은 만들지 않는다. (5) 결과 텍스트가 기존과 같으면 쓰지 않는다. 결과가 빈 텍스트이고 파일이 없었으면 파일을 만들지 않는다. | M |
| FR-GUI-6 | 저장 뒤 `[ui] language`, `[ui] theme`, `[ui] start_tab`, `[inject] enabled`, `[capture] enabled` 말고 다른 키가 바뀌었으면 데몬 재시작이 필요하다는 안내와 `데몬 다시 시작` 버튼을 표시해야 한다. 버튼은 `shutdown`(FR-DMN-11)을 요청하며, 다음 폴링이 새 설정으로 데몬을 띄운다. 재색인 필요 키가 바뀌었으면 재시작 뒤 `reindex`가 필요하다는 안내를 함께 표시한다. `[ui] language`를 바꾸면 GUI 문구는 저장 즉시 그 언어로 바뀌고, `[ui] theme`을 바꾸면 GUI 테마가 저장 즉시 바뀐다. | M |
| FR-GUI-7 | 가져오기 탭은 `from`과 `to` 입력란(기본값은 실행 경로)과 `미리 보기` 버튼을 두어야 한다. 두 입력란 옆에는 프로젝트 선택 목록을 둔다. 목록은 상태 폴링으로 받은 프로젝트를 id 앞부분과 위치로 보여 준다. 하나를 고르면 그 프로젝트의 폴더 경로(위치가 `<경로>/.git`이면 `<경로>`, 아니면 위치)를 그 입력란에 넣는다. 프로젝트가 없으면 목록을 비활성화한다. 미리 보기는 `dry_run` `import_prepare`(FR-IMP-9)를 요청해 처리 경우와 후보 표(선택, 시작 시각, session id 앞 8자, 가져오기 상태, 파일 크기, 세션 시작 경로, 첫 프롬프트)를 표시한다. 가져오기 상태가 `새 세션`이나 `실시간만`인 후보를 처음에 선택해 두고, 전체 선택과 전체 해제를 둔다. `가져오기`를 누르면 실제 `import_prepare`를 요청하고, 그 응답의 후보 중 선택한 session id만 응답 순서대로 `import_session`으로 하나씩 보낸다. 두 요청은 FR-IMP-1과 같이 시간 제한 없이 응답을 기다리고, 기다리는 동안 화면을 막지 않는다. 미리 보기 뒤 `from`이나 `to`를 바꾸면 다시 미리 보기를 해야 가져오기를 할 수 있다. | M |
| FR-GUI-8 | 가져오기 진행은 2초 주기 화면 갱신으로 보여 주어야 한다. 진행 막대(`n/m`), 세션마다 FR-IMP-1과 같은 결과 줄, 끝난 뒤 합계를 표시한다. 그 뒤의 임베딩 진행은 상태 폴링 값(대상 프로젝트의 임베딩 대기 건수, 속도, 남은 예상 시간)으로 표시한다. `멈춤`을 누르면 처리 중인 `import_session` 응답을 받은 뒤 다음 세션을 보내지 않는다. | M |
| FR-GUI-9 | 검색 탭은 `search`(질의, 범위 `project`/`all`/경로, 종류, 개수)와 `show`(prompt_id, 범위, 결과 포함 여부)를 실행해 응답 텍스트를 표시해야 한다. 검색 카드에는 `검색`과 `주입 미리 보기` 두 모드를 둔다. 주입 미리 보기는 범위 입력란의 프로젝트(`project`는 실행 경로의 프로젝트, 경로는 그 경로의 프로젝트)로 `inject_preview`를 요청해 FR-GUI-16과 같은 형식으로 표시한다. 자동 주입은 프로젝트 하나 안에서만 하므로, 범위가 `all`이거나 비어 있으면 실행 버튼을 비활성화하고 그 사유를 표시한다. 주입 미리 보기 모드에서는 종류와 개수를 쓰지 않는다. 프로젝트 탭은 `project list`의 결과를 표시하고, `reindex`(범위 `all`/`project`/경로)를 실행할 수 있어야 한다. 재색인 진행은 상태 폴링의 재색인 상태로 표시한다. | M |
| FR-GUI-10 | Claude 적용 탭은 데이터 루트와 Claude 폴더 입력란(기본값은 FR-GUI-22)을 두고, 설치 상태(FR-GUI-23), hook 적용 여부(설정 파일에 hippo hook 항목이 있는지), PATH 링크 상태(없음, 이 루트를 가리킴, 다른 곳을 가리킴)를 표시해야 한다. `미리 보기`는 `--dry-run`과 같은 결과(diff, 설치 경로, 링크)를 표시하고 아무것도 바꾸지 않는다. `적용`과 `해제`는 같은 입력으로 미리 보기를 한 뒤에만 누를 수 있다. `완전 삭제`는 데이터 루트의 절대 경로와 모든 기록이 지워진다는 안내를 확인 창으로 보여 준다. 세 버튼 모두 확인했을 때만 `hippo install`/`hippo uninstall`/`hippo purge`와 같은 동작(FR-INS-*)을 GUI 프로세스 안에서 실행하고 결과 텍스트를 표시한다. 이때 데이터 루트와 Claude 폴더는 입력란의 값을 쓴다. 설치 경로로 복사할 실행 파일은 실행 중인 `hippo`(앱에서는 앱 실행 파일)이다. 작업이 끝나면 설치 상태를 다시 계산한다. 이 탭의 작업(미리 보기 포함)이 진행 중이면 상태 폴링을 하지 않는다. 완전 삭제는 확인한 뒤 진행 중인 상태 요청이 끝나기를 기다렸다가 시작하고, 끝나면 GUI를 수동 중지 뒤와 같은 멈춤 상태(FR-GUI-3)로 둔다. 폴링의 데몬 자동 시작(FR-DMN-7)이 지운 데이터 루트에 `logs/`를 다시 만들거나 데몬을 다시 띄우지 않게 하기 위해서다. | M |
| FR-GUI-11 | 정보 탭은 `README.md`(언어가 `ko`이면 `README.ko.md`)를 마크다운 서식(제목, 목록, 표, 코드, 강조, 링크)으로 렌더링하고, `LICENSE`는 일반 텍스트로 스크롤해 볼 수 있게 해야 한다. 두 파일은 빌드할 때 바이너리에 넣는다. 맨 위에 현재 테마에 맞는 로고(FR-GUI-20)와 버전(`Cargo.toml`의 `version`)을 표시한다. README를 보여 줄 때 이미지만 담은 HTML 블록(`<p` 또는 `<picture`로 시작해 그 블록이 닫힐 때까지의 줄)과 마크다운 이미지 줄(`![`로 시작하는 줄)은 뺀다. 바이너리에 그림 파일을 넣지 않기 때문이다. 마크다운 링크 대상이 상대 경로(스킴이 없고 `#`으로 시작하지 않음)이면 `https://github.com/Biisairo/Hippocampal-Index/blob/main/<경로>`로 바꾼다. 링크를 누르면 기본 브라우저로 연다. | M |
| FR-GUI-13 | 기록 탭은 프로젝트 목록 → 세션 목록 → 턴 목록 → 턴 상세 순서로 기록을 볼 수 있어야 한다. 세션 목록(`session_list`)은 세션마다 시작 시각(첫 반복의 `ts`), session id 앞 8자, 첫 프롬프트(저장할 때 이미 마스킹되었으므로 공백을 한 칸으로 줄이고 80자로 자름. 80자를 넘으면 끝에 `…`를 붙여 잘렸음을 보인다), 턴 수, 반복 수를 시작 시각 내림차순으로 보여 준다. 턴 목록(`turn_list`)은 턴마다 시각, prompt_id 앞 8자, 프롬프트 앞부분(첫 프롬프트와 같은 방식으로 80자로 자르고 `…`를 붙임), 반복 수, 임베딩 상태(`완료` 또는 `대기 n`. n은 그 턴의 반복과 연결 결과 중 `embed_jobs`에 남은 수)를 시각 오름차순으로 보여 준다. 두 목록은 50개씩 페이지로 받는다. 턴 상세(`turn_detail`)는 IF-CLI-2의 `show --with-results`와 같은 내용을 반복별로 나눠 보여 주며, 결과 원문은 처음에 접어 둔다. | M |
| FR-GUI-14 | 세션 목록과 턴 목록 위에 글자 필터를 두어야 한다. 필터가 3자 이상이면 FTS(trigram)로, 3자 미만이면 부분 문자열 비교로, `embed_text`에 필터 글자가 들어 있는 반복을 가진 세션과 턴만 보여 준다. 비어 있으면 거르지 않는다. | M |
| FR-GUI-15 | 기록 탭은 프로젝트, 세션, 턴마다 `삭제` 버튼을 두어야 한다. 누르면 `dry_run` 요청의 미리 보기(FR-DEL-4와 같은 항목)를 확인 창으로 보여 주고, 확인하면 삭제를 요청한다. 실제 삭제 요청은 FR-DEL-4와 같이 시간 제한 없이 응답을 기다리고, 기다리는 동안 화면을 막지 않는다. 성공하면 목록을 다시 불러오고, 실패하면 오류를 표시한다. 실제 삭제를 기다리는 동안에는 진행 표시와 경과 시간(초)을 보여 주고, 다른 삭제 버튼을 비활성화한다. | M |
| FR-GUI-16 | 기록 탭은 선택한 프로젝트를 대상으로 하는 질의 입력란과 두 모드를 두어야 한다. (1) 검색: FR-RET-2의 하이브리드 검색 결과(반복 id, 점수, 유사도, prompt_id, 미리 보기)를 보여 주고, 결과를 누르면 그 턴 상세를 연다. (2) 주입 미리 보기(`inject_preview`): FR-RET-1~4와 같은 설정과 규칙으로 후보를 계산해, 무주입 사유(빈 프롬프트, 완료 알림, `min_prompt_chars` 미만, 프로젝트 없음, 벡터 검색 불가)가 있으면 그 사유를 보여 주고, 없으면 후보 반복마다 RRF 순위, 유사도, 판정(`선택`, `임계값 미만`, `FTS 전용`, `같은 턴`, `길이 한도 초과`, `개수 초과`)과 최종 주입 텍스트를 보여 준다. 세션 제외는 하지 않고 시간 예산은 적용하지 않으며, 걸린 시간을 함께 표시한다. 질의 결과는 다른 프로젝트를 고르면 지우고, 같은 프로젝트에서 새로 질의하면 새 결과로 바꾼다. | M |
| FR-GUI-17 | 검색 탭의 반복 결과마다 `기록에서 열기`를 두어, 누르면 기록 탭에서 그 반복의 프로젝트와 턴 상세를 열어야 한다. | M |
| FR-GUI-20 | GUI는 저장소 `assets/app/`의 이미지를 빌드할 때 바이너리에 넣어 다음 위치에 써야 한다. (1) 창과 Dock 아이콘: 앱 아이콘. (2) 상단 탭 막대 왼쪽 `hippo` 글자 앞: 작은 앱 아이콘. (3) 정보 탭 머리글: 라이트 테마에서는 원본 로고, 다크 테마에서는 어두운 배경용 로고. (4) 빈 상태(상태 탭의 `연결 안 됨`, 상태 탭과 기록 탭의 `아직 프로젝트가 없습니다`): 캐릭터 이미지를 안내 문구와 함께 표시. 이미지를 해석하지 못하면 그 이미지만 빼고 나머지는 그대로 표시한다. | M |
| FR-GUI-21 | GUI는 라이트와 다크 두 테마를 로고 색으로 만들어야 한다. 라이트는 크림색 배경(`#FDF6EC`)과 남색 글자(`#3B3654`), 다크는 짙은 남색 배경(`#23202F`)과 크림색 글자(`#F3EEE6`)를 쓴다. 두 테마 모두 선택 항목과 주 버튼의 강조색은 라벤더 계열로 하고, 그 위의 글자와 배경의 명도 대비는 4.5:1 이상이어야 한다. 상태 색(실행 중, 경고, 변경, 오류)은 테마마다 배경과의 명도 대비가 3:1 이상이 되게 따로 정한다. 카드, 버튼, 입력란의 모서리는 둥글게 한다. FR-UI-3의 설정을 따른다. | M |
| FR-GUI-18 | 기록 탭은 선택한 프로젝트가 없거나 선택한 프로젝트가 목록에서 사라지면, `current_project`로 실행 경로의 프로젝트를 물어 그 프로젝트를 골라야 한다. 그 프로젝트가 없거나 목록에 없으면 목록 첫 항목을 고른다. | M |
| FR-GUI-19 | 기록 탭의 작업 결과 메시지(삭제 성공, 오류)는 탭 위쪽에 토스트로 보여 주어야 한다. 성공 메시지는 성공한 때부터 5초 뒤 사라지고, 그 전에도 닫을 수 있다. 오류 메시지는 자동으로 사라지지 않고 사용자가 닫을 때만 사라진다. 새 메시지는 따로 더하며, 새 작업을 시작하거나 새 메시지가 와도 남아 있는 메시지를 지우지 않는다. | M |
| FR-GUI-12 | GUI 문구는 FR-UI-1의 언어로 표시해야 한다. 한글이 깨지지 않게, 기본 폰트에 없는 글자는 macOS 시스템 폰트에서 찾아 쓴다(eframe 기본 기능 `system_fonts`, `system_font_fallback`). 데몬이 만든 응답 텍스트는 받은 그대로 표시한다. | M |
| FR-GUI-22 | GUI 데이터 루트의 기본값은 (1) GUI 실행 파일이 관리 실행 파일이면 그 데이터 루트, (2) 아니면 PATH 링크가 가리키는 실제 경로가 `<디렉터리>/bin/hippo`일 때 그 `<디렉터리>`, (3) 그 밖에는 `~/.hippo`이다. Claude 폴더의 기본값은 그 데이터 루트의 FR-INS-12 기록 값, 없으면 `~/.claude`이다. GUI는 이 값들을 데이터 루트 밖의 파일에 저장하지 않는다. Claude 적용 탭에서 데이터 루트를 바꾸면 GUI의 데몬 요청 대상과 설치 상태도 그 루트로 바꾼다. | M |
| FR-GUI-23 | GUI는 시작할 때와 데이터 루트가 바뀔 때 설치 상태를 다음과 같이 계산해야 한다. `설치 필요`: 설치 경로에 파일이 없다. `업데이트 필요`: 설치 경로의 파일이 GUI 실행 파일과 내용이 다르다(GUI 실행 파일이 설치 경로 자체이면 비교하지 않는다). `설치됨`: 그 밖. `설치 필요`와 `업데이트 필요`이면 상단에 그 상태와 Claude 적용 탭에서 적용하라는 안내를 표시한다. | M |
| FR-GUI-25 | GUI는 보이는 탭이 바뀔 때마다 그 탭 이름(FR-UI-4의 값 중 `last`를 뺀 것)을 `<데이터 루트>/gui-last-tab`(한 줄, 권한 0600)에 기록해야 한다. 기록은 화면을 그리는 스레드 밖에서 하고, 데이터 루트 디렉터리가 없으면 만들지 않고 건너뛴다. 기록에 실패해도 오류를 표시하지 않는다. 설치 상태(FR-GUI-23)가 `설치됨`이 아니어서 Claude 적용 탭으로 바꾼 경우에도 같은 규칙을 따른다. 시작 탭이 비활성 탭이면 FR-GUI-1에 따라 Claude 적용 탭을 연다. | M |
| FR-GUI-24 | 경로를 받는 입력란(가져오기의 `from`·`to`, 검색과 턴 보기의 범위, 재색인 범위, Claude 적용 탭의 데이터 루트와 Claude 폴더, 설정 탭의 `[import] transcripts_dir`) 옆에는 `찾아보기` 버튼을 두어야 한다. 누르면 macOS 폴더 선택 창(AppleScript `choose folder`, 숨김 폴더 표시)을 화면을 그리는 스레드 밖의 별도 프로세스(`osascript`)로 띄운다. 처음 위치는 입력란의 값이 가리키는 폴더가 있으면 그 폴더(상대 경로는 실행 경로 기준, `~`는 홈), 없으면 홈 폴더이다. 폴더를 고르면 그 절대 경로(끝의 `/`는 뺌, 루트는 `/`)를 입력란에 넣고, 취소하거나 실패하면 입력란을 바꾸지 않는다. 선택 창이 열려 있는 동안 다른 `찾아보기` 버튼은 비활성화한다. | M |

#### 3.2.13 삭제 (DEL)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-DEL-1 | 데몬은 `project_remove` 요청을 받으면 그 프로젝트를 통째로 지워야 한다. 순서: (1) 메모리 큐에서 그 프로젝트의 임베딩 작업을 뺀다. (2) 열어 둔 저장소 핸들을 닫는다(FR-DMN-9). (3) 목록 DB에서 그 프로젝트의 `projects`, `project_roots`, `session_projects` 행을 지운다. (4) `projects/<id>/` 디렉터리를 지운다. 이후 그 경로에서 처리되는 세션은 FR-PRJ-2에 따라 새 프로젝트로 판정된다. | M |
| FR-DEL-2 | 데몬은 `forget` 세션 요청을 받으면 그 세션의 기록을 프로젝트 저장소에서 지워야 한다. 대상은 그 `session_id`의 반복 전부와 FTS 색인, 반복-결과 연결, 그 세션의 백그라운드 시작 기록(`bg_tasks`)과 완료 기록(`bg_completions`), 대상 반복의 임베딩 작업과 벡터(모든 청크), 수집 위치 기록(`sessions` 행), 목록 DB의 세션 고정값(`session_projects` 행)이다. 결과 원문은 FR-DEL-5를 따른다. 열린 세션이면 다음 hook부터 FR-EXT-12에 따라 처음 보는 세션으로 다시 저장된다. | M |
| FR-DEL-3 | 데몬은 `forget` 턴 요청을 받으면 그 프로젝트에서 `prompt_id`가 같은 반복 전부(여러 세션에 있으면 모두)와 FTS 색인, 반복-결과 연결, 대상 반복에서 시작한 백그라운드 작업 기록과 그 완료 기록, 임베딩 작업, 벡터를 지워야 한다. 세션의 수집 위치와 세션 고정값은 그대로 둔다. 결과 원문은 FR-DEL-5를 따른다. | M |
| FR-DEL-4 | CLI는 삭제 전에 `dry_run` 요청으로 받은 미리 보기(프로젝트 id와 위치, 지울 세션·턴·반복·결과 수. 지운 반복과 결과의 벡터는 청크 수와 관계없이 모두 지운다)를 출력해야 한다. `--dry-run`이면 미리 보기만 출력하고 종료 코드 0으로 끝난다. `--yes`가 없으면 표준 입력이 터미널일 때 `y`를 입력받은 경우에만 지우고, 그 밖의 입력은 취소로 보고 종료 코드 0으로 끝난다. 표준 입력이 터미널이 아니고 `--yes`도 없으면 지우지 않고 안내를 stderr에 출력한 뒤 종료 코드 1로 끝난다. 대상이 없으면 "대상 없음"을 출력하고 종료 코드 0으로 끝난다. 실제 삭제 요청(`dry_run`이 거짓)은 시간 제한 없이 데몬의 응답을 기다린다. 응답이 성공이면 "삭제함"을 출력하고 종료 코드 0으로, 오류 응답을 받거나 응답 전에 연결이 끊기면 그 내용을 stderr에 출력하고 종료 코드 1로 끝난다. | M |
| FR-DEL-5 | 결과 원문(`results` 행, FTS 색인, 임베딩 작업, 벡터)은 삭제 뒤 남은 반복-결과 연결과 완료 기록 어디에서도 참조하지 않을 때만 지워야 한다. | M |
| FR-DEL-6 | 삭제는 쓰기 스레드(FR-DMN-6)가 다른 쓰기 작업과 같은 우선순위 규칙으로 처리해야 한다. 재색인 중이면 오류를 돌려준다. 삭제가 끝난 뒤에는 지운 레코드의 벡터가 남거나 다시 저장되지 않아야 한다. 이를 위해 삭제는 메모리 큐에서 대상 작업을 뺀 뒤, 임베딩 잠금을 얻어 수행한다. 삭제는 다른 임베딩 작업보다 먼저 처리한다. 임베딩 스레드는 잠금을 기다리는 삭제가 있으면 묶음 종류(실시간, 가져오기)와 관계없이 처리 중인 작업 1건을 마친 뒤 묶음을 멈추고 잠금을 놓으며, 기다리는 삭제가 없어질 때까지 다음 묶음을 시작하지 않는다. 처리하지 못한 작업은 큐에 남아 다음 묶음에서 처리한다. 임베딩 스레드는 원본 레코드가 없는 작업을 벡터 없이 완료로 처리한다. 미리 보기와 실제 삭제는 같은 대상 선정 규칙을 쓴다. 데몬은 실제 삭제 요청의 응답을 쓰기 스레드가 삭제를 끝낼 때까지 시간 제한 없이 기다렸다가 보낸다. 그래서 성공 응답은 삭제가 끝났다는 뜻이고, 오류 응답은 삭제하지 않았다는 뜻이다. 쓰기 스레드가 멈춰 결과를 받을 수 없으면 오류를 응답한다. | M |

#### 3.2.14 앱 배포 (APP)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-APP-1 | 저장소의 스크립트 `scripts/bundle-app.sh`는 release 빌드를 한 뒤 `target/hippo.app`을 만들어야 한다. 번들 구성: `Contents/MacOS/hippo`(release 실행 파일), `Contents/Resources/hippo.icns`, `Contents/Info.plist`. 이미 있는 `target/hippo.app`은 지우고 새로 만든다. 새 crate 의존성을 더하지 않고 macOS 기본 도구(`sips`, `iconutil`, `plutil`, `codesign`)만 쓴다. | M |
| FR-APP-2 | `Info.plist`는 `CFBundleExecutable` = `hippo`, `CFBundleIdentifier` = `io.hippocampal-index.hippo`, `CFBundleName` = `hippo`, `CFBundlePackageType` = `APPL`, `CFBundleIconFile` = `hippo`, `CFBundleShortVersionString`과 `CFBundleVersion` = `Cargo.toml`의 `version`, `NSHighResolutionCapable` = true를 담아야 한다. 개인 정보(계정 이름, 이메일, 개인 경로)를 넣지 않는다. | M |
| FR-APP-3 | `.icns`는 `assets/icon.png`에서 `iconutil`의 iconset 규격 크기(16, 32, 128, 256, 512와 각 @2x)로 만들어야 한다. | M |
| FR-APP-4 | 번들은 ad-hoc 서명(`codesign --force --sign -`)만 해야 한다. 개발자 인증서 서명과 공증은 하지 않는다(2.6). | M |
| FR-APP-5 | 앱을 열어도 설치, 복사, 링크 생성, 데몬 시작을 하지 않아야 한다. 앱은 IF-CLI-14에 따라 GUI만 띄우며, 설치는 사용자가 GUI에서 적용(FR-GUI-10)을 확인했을 때만 한다. | M |

#### 3.2.15 데이터 루트 (ROOT)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-ROOT-1 | 실행 파일의 데이터 루트는 다음과 같이 정해야 한다. `std::env::current_exe`의 경로에서 심볼릭 링크를 모두 푼 실제 경로를 얻는다(PATH 링크로 실행해도 링크가 아니라 설치 경로를 보기 위해서다). 그 경로의 파일 이름이 `hippo`이고 부모 디렉터리 이름이 `bin`이면, 부모의 부모가 데이터 루트이다(관리 실행 파일). 그렇지 않으면 데이터 루트가 없다(FR-INS-14). 데이터 루트를 바꾸는 명령줄 옵션이나 환경 변수는 두지 않는다. | M |
| FR-ROOT-2 | 데이터 루트를 받는 곳은 설치(`--data-dir`, FR-INS-8)와 GUI(FR-GUI-22)뿐이어야 한다. 라이브러리 함수는 데이터 루트 경로(`Paths`)를 인자로 받으며, 실행 파일 위치에서 경로를 정하는 일은 바이너리 진입점만 한다. 테스트는 라이브러리 함수에 임시 경로를 넘기고, 실행 파일을 띄우는 테스트는 `<임시 디렉터리>/bin/hippo`에 빌드한 실행 파일의 복사본(테스트마다 별도 inode)을 두어 그것을 실행한다. 하드 링크는 쓰지 않는다. 같은 inode를 여러 경로로 동시에 처음 실행하면 macOS AMFI가 서명 검사에 실패해 프로세스를 SIGKILL로 끝내는 일이 있기 때문이다. | M |

#### 3.2.16 관찰 (OBS)
저장과 주입이 실제로 일어났는지를 사후에 확인할 수 있게 하는 기록이다.

| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-OBS-1 | 로그 파일(`logs/hippo.log`, `logs/hook.log`)의 각 줄은 `YYYY-MM-DD HH:MM:SS <내용>` 형식이어야 한다. 시각은 로컬 시간대이다. 줄을 덧붙이기 전에 파일 크기가 5 MiB 이상이면 파일 이름을 `<이름>.1`로 바꾼다(이미 있으면 바꿔 쓴다). 이름 바꾸기나 쓰기가 실패해도 원래 처리는 실패로 만들지 않는다. 파일 권한은 0600이다. | M |
| FR-OBS-2 | 데몬은 세션 하나의 수집 처리(hook 요청과 스풀 요청, 따라잡기 FR-CAP-9, 세션 가져오기 FR-IMP-3)를 마칠 때마다 `logs/hippo.log`에 한 줄을 남겨야 한다. 형식은 `capture <종류> session=<session id> project=<프로젝트 id> saved=<저장한 반복 수> completions=<저장한 완료 기록 수> offset=<처리 전 안전 offset>-><처리 후 안전 offset>`이다. 종류는 `post-tool-batch`, `stop`, `stop-failure`, `session-end`, `catch-up`, `import`이다. 처리 전 상태가 없으면 처리 전 offset은 `-`이다. 아무것도 처리하지 않았으면 offset 대신 `skipped=<사유>`를 쓴다. 사유는 `no-start-path`(세션 시작 경로를 아직 알 수 없음, 이때 `project=-`), `no-anchor`(FR-EXT-12의 기준 턴을 찾지 못함)이다. 따라잡기는 FR-CAP-9의 조건으로 읽은 세션만 남긴다. 오류는 지금처럼 오류 줄로 남긴다. | M |
| FR-OBS-3 | hook 클라이언트는 hook을 처리할 때마다 `logs/hook.log`에 `hook <이벤트 인자> session=<session id> <결과> elapsed=<ms>ms` 한 줄을 남겨야 한다. 수집 hook의 결과는 `delivered`(데몬이 응답함), `started-daemon`(첫 요청이 실패해 FR-DMN-7로 데몬을 확인하거나 띄운 뒤, 다시 보낸 요청이 응답받음), `spooled`(스풀에 기록함), `paused`(FR-CAP-8)이다. 주입 hook의 결과는 `injected chars=<글자 수>`, `no-context`(데몬이 주입할 내용이 없다고 응답함), `no-daemon`(데몬에 연결하지 못함), `timeout`(예산 안에 응답이 없음), `paused`(FR-RET-12)이다. hook 입력을 해석할 수 없거나 `agent_id`가 있어 건너뛴 경우(FR-CAP-1)는 남기지 않는다. | M |
| FR-OBS-4 | 반복을 저장할 때 저장 시각(`stored_at`, Unix 밀리초)과 저장 경로(`source`)를 함께 저장해야 한다. 경로는 `live`(hook 요청과 스풀 요청), `catch_up`(FR-CAP-9), `import`(FR-IMP-3)이다. 병합(FR-PRJ-9)으로 옮긴 반복은 원본 저장소의 두 값을 그대로 옮기고, 원본에 값이 없으면 비운다. 이 기능 전에 저장된 반복은 두 값이 비어 있다. 원본 저장소를 열 때 두 열이 없으면 추가한다. `hippo show`와 GUI 턴 상세는 반복마다 두 값이 있으면 `stored <로컬 시각> <경로>`를 반복 머리 줄에 표시한다. | M |
| FR-OBS-5 | 데몬은 자동 주입 요청(`inject`)을 처리할 때마다 대상 프로젝트의 원본 저장소 `injections`에 한 행(session id, 요청 시각(Unix 밀리초), 결과, 무주입 사유, 주입한 턴 수, 주입 텍스트 글자 수, 걸린 시간(ms), 주입한 턴의 `prompt_id` 목록)을 남겨야 한다. 결과는 `injected`(주입 텍스트를 돌려줌), `no_match`(후보 계산은 했지만 선택된 턴이 없음), `skipped`(FR-RET 규칙의 무주입 사유가 있음. 사유는 `empty_prompt`, `task_notification`, `too_short`, `vector_unavailable`), `timeout`(시간 예산 초과), `error`이다. 대상 프로젝트가 없으면 행을 남기지 않고 `logs/hippo.log`에 `inject session=<session id> no-project`를 남긴다. 기록이 실패해도 주입 응답은 바꾸지 않는다. | M |
| FR-OBS-6 | 반복을 저장할 때 반복의 도구 호출 중 hippo를 실행하는 Bash 호출(FR-STO-11의 판정)마다 원본 저장소 `memory_uses`에 한 행(반복 id, `tool_use_id`, session id, 반복의 `ts`, 종류, 대상 `prompt_id`)을 남겨야 한다. 종류는 다음 순서로 정한다. (a) 명령 문자열에 같은 세션의 `injected` 주입 기록(FR-OBS-5)에 있는 `prompt_id`가 들어 있으면 `opened`이고, 대상 `prompt_id`는 그 값이다. (b) 명령을 공백으로 나눈 단어에 `search`가 있으면 `search`이다. (c) 그 밖에 `show`가 있으면 `show`이다. (d) 어디에도 맞지 않으면 행을 남기지 않는다. 같은 (반복 id, `tool_use_id`)는 한 번만 남긴다. | M |
| FR-OBS-7 | `status` 응답은 프로젝트마다 최근 7일의 주입 통계를 담아야 한다. 주입 요청 수, 결과별 수(`injected`, `no_match`, `skipped`, `timeout`, `error`), 무주입 사유별 수, `memory_uses`의 종류별 수(`opened`, `search`, `show`)이다. 7일은 주입 기록은 요청 시각, 활용 기록은 반복의 `ts`로 센다. `hippo status`는 프로젝트마다 `inject (7d): requests <n>, injected <n> (<비율>%), no match <n>, skipped <n>, timeout <n>, error <n>; agent: opened <n>, search <n>, show <n>` 한 줄을 더 출력한다. 요청이 0이면 비율을 쓰지 않는다. GUI 상태 탭은 프로젝트 표 아래에 같은 값을 프로젝트별 표로 보여 주고, 무주입 사유별 수를 함께 보여 준다. | M |
| FR-OBS-8 | 세션 삭제(FR-DEL-2)는 그 세션의 `injections`와 `memory_uses` 행도 지워야 한다. 턴 삭제(FR-DEL-3)는 지운 반복의 `memory_uses` 행을 지운다. 삭제 미리 보기의 항목은 바꾸지 않는다. | M |

#### 3.2.6 교체 가능성 (SWP)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-SWP-1 | 임베더는 `Embedder` 트레이트로 추상화해야 한다. 트레이트는 최소한 `model_id()`, `dimension()`, `max_input_tokens()`, `embed(texts) -> vectors`를 제공한다. 임베더를 사용하는 코드는 구체 백엔드 타입에 의존하지 않아야 한다. | M |
| FR-SWP-2 | 벡터 저장소는 `VectorStore` 트레이트로 추상화해야 한다. 트레이트는 최소한 `info`, `upsert`, `replace(refs, records)`(`refs`의 (`kind`, `ref_id`)마다 모든 청크 벡터를 지운 뒤 `records`를 저장함. FR-STO-7), `delete`, `search(vector, filter, limit)`, `count`, `ensure_index`, `optimize`와 `pending_changes`(FR-STO-12), `scan`(모든 벡터 레코드를 차례로 읽음. FR-PRJ-9의 복사에 쓴다)을 제공한다. 필터는 백엔드 중립 타입(`kind`, `session_id` 제외, `embed_model`)으로 표현한다. 경로 범위는 프로젝트 저장소 단위로 나누므로 필터에 두지 않는다. 벡터 레코드 id는 `<kind>:<ref_id>#<chunk_index>`이다. 결과 벡터에는 `cwd`와 `session_id`를 넣지 않는다. 검색 점수는 코사인 유사도이며 클수록 가깝다. 저장소는 처음 저장한 벡터의 `embed_model`과 차원을 `info`로 돌려주고, 차원이 다른 벡터의 저장은 오류로 거부해야 한다. | M |
| FR-SWP-3 | 사용할 임베더 백엔드, 모델, 차원과 벡터 저장소 백엔드는 `config.toml`로 선택해야 한다. 백엔드를 추가할 때 수집, 해석, 검색 로직을 수정하지 않아야 한다. | M |
| FR-SWP-4 | 원본 저장소는 벡터 없이도 반복, 결과, 임베딩 대상 텍스트를 모두 보관해야 한다. 벡터 저장소의 내용은 원본 저장소에서 다시 만들 수 있어야 한다. | M |
| FR-SWP-5 | `hippo reindex`는 데몬에 재색인을 요청하고, 데몬은 범위(IF-CLI-7)의 프로젝트마다 차례로, 현재 설정의 임베더와 벡터 저장소 백엔드로 그 프로젝트의 원본 저장소 전체를 새 세대 디렉터리(`projects/<id>/vectors/<이름>/<세대>/`)에 다시 색인해야 한다. 재색인은 백그라운드로 실행하며 그동안 기존 저장소로 검색할 수 있어야 한다(호환되지 않으면 FTS 전용). 재색인 중에는 임베딩 작업 스레드를 멈춰, 그 사이 들어온 작업이 전환 후 새 저장소에 반영되게 한다. 프로젝트마다 완료하면 그 프로젝트의 `vectors/<이름>/ACTIVE` 포인터를 원자적으로 바꿔 전환하고 호환성을 다시 판정한다. 한 프로젝트가 실패하면 그 프로젝트를 실패로 기록하고 다음 프로젝트를 계속한다. 이미 재색인 중이면 오류를 돌려준다. 교체 절차: 설정 변경 → 데몬 재시작 → `hippo reindex`. | M |
| FR-SWP-9 | 데몬은 프로젝트 저장소를 열 때 `ACTIVE`가 가리키지 않는 세대 디렉터리를 지워야 한다. `ACTIVE`가 없으면 첫 세대를 만들어 가리키게 한다. 재색인 중인 프로젝트의 새 세대는 지우지 않는다. | M |
| FR-SWP-10 | `hippo status`는 재색인 상태(`idle`, `running`, `done`, `failed: <사유>`. 실패한 프로젝트 id를 포함)를 보여야 한다. 벡터가 0개인데 반복이 있는 프로젝트가 있으면 재색인이 필요하다고 안내한다. | M |
| FR-SWP-6 | 데몬은 프로젝트 저장소를 열 때 그 프로젝트의 활성 벡터 저장소의 `embed_model`, 차원과 설정의 임베더가 다르면 그 프로젝트에 대해 수집은 계속하되, 벡터 검색을 끄고 FTS 검색만 수행하며(이 상태에서는 자동 주입을 하지 않는다) `hippo status`와 로그에 재색인 필요를 표시해야 한다. | M |
| FR-SWP-7 | 하이브리드 검색의 FTS 부분은 원본 저장소(SQLite FTS5)에서 수행해 벡터 저장소 백엔드와 무관하게 동작해야 한다. | M |
| FR-SWP-8 | 테스트용으로 결정론적 `Embedder`와 메모리 기반 `VectorStore` 백엔드를 제공하고, 모든 백엔드에 같은 계약 테스트를 적용해야 한다. | M |

#### 3.2.7 데몬 (DMN)
| ID | 요구사항 | 우선순위 |
|----|----------|----------|
| FR-DMN-1 | 데몬은 장비당 1개만 실행되어야 한다. 이미 실행 중이면 두 번째 실행은 오류 메시지를 출력하고 종료한다. | M |
| FR-DMN-2 | 원본 저장소와 벡터 저장소 쓰기는 데몬만 수행해야 한다. | M |
| FR-DMN-3 | 데몬은 세션 단위 처리 요청을 직렬로 처리해 같은 세션을 동시에 처리하지 않아야 한다. | M |
| FR-DMN-4 | 데몬은 처리 오류를 `<데이터 루트>/logs/`에 기록하고 계속 동작해야 한다. | M |
| FR-DMN-5 | 데몬은 시작할 때 데이터 루트와 하위 디렉터리를 권한 0700으로 만들거나 맞추고, 잠금 파일(`hippo.lock`)의 권한을 0600으로 맞추고, 잠금을 얻은 뒤 남아 있는 소켓 파일을 지우고 새로 만들어야 한다. 소켓 파일 권한은 0600이다. | M |
| FR-DMN-7 | hook 클라이언트와 CLI는 데몬에 연결할 수 없으면, 설정 `[daemon] autostart`(기본 true)가 켜져 있을 때 데이터 루트의 설치 경로(`<데이터 루트>/bin/hippo`)로 `daemon` 명령을 분리된 프로세스(별도 프로세스 그룹)로 띄워야 한다. 데몬의 stderr는 `logs/daemon.err`에 덧붙인다. 띄운 뒤에는 소켓에 연결될 때까지 기다린다(수집과 CLI는 최대 5초, 자동 주입은 남은 시간 예산까지). 여러 클라이언트가 동시에 띄워도 FR-DMN-1에 따라 하나만 남는다. | M |
| FR-DMN-8 | 데몬은 마지막 요청을 받은 뒤 `[daemon] idle_timeout_secs`(기본 1800, 0이면 종료하지 않음) 동안 요청이 없으면 스스로 종료해야 한다. 처리할 작업(FR-DMN-13의 정의. 재색인 포함)이 있는 동안에는 종료하지 않는다. 이 조건은 `[daemon] prevent_sleep` 값과 관계없다. 종료할 때 소켓 파일을 지운다. 임베딩 실패로 재시도를 기다리는 동안 종료하면, 남은 임베딩 작업은 원본 저장소에 남아 다음 실행 때 처리된다. | M |
| FR-DMN-6 | 수집, 가져오기 준비(FR-PRJ-7), 세션 가져오기(FR-IMP-3)는 전용 쓰기 스레드 하나가 처리해야 한다. 쓰기 스레드는 FR-IMP-6의 우선순위를 따르고, 같은 우선순위 안에서는 요청 순서대로 처리한다. 임베딩 작업은 별도 스레드 하나가 처리한다. 데몬은 메모리에 (프로젝트 id, 작업) 큐를 두 단계(일반, `bulk`)로 두고, 커밋될 때 등록된 작업을 `bulk` 값에 맞는 단계의 끝에 넣는다. 데몬이 시작할 때 모든 프로젝트의 `embed_jobs`를 `enqueued_at` 순으로 읽어 `bulk` 값에 맞는 단계에 넣는다. 임베딩 스레드는 일반 단계가 비어 있을 때만 `bulk` 단계에서 꺼내며, 한 단계의 앞에서부터 같은 프로젝트의 작업을 `job_batch`개까지 묶어 처리한다. 실패하면 작업을 큐에 남기고 일정 시간 뒤 재시도한다. 벡터 저장소가 호환되지 않는 프로젝트(FR-SWP-6)의 작업은 큐에서 빼고 원본 저장소에만 남긴다. 그 프로젝트의 재색인이 끝나면 남은 작업을 다시 큐에 넣는다. 우선순위: (a) 임베딩 스레드는 청크 임베딩 요청을 보내기 전마다, 처리 중인 자동 주입 요청이 있으면 그 요청이 끝날 때까지 기다린다. (b) `bulk` 단계의 작업을 처리하는 중에 일반 단계에 작업이 들어오면, 처리 중인 작업 1건을 마친 뒤 남은 `bulk` 작업은 큐에 그대로 두고 일반 단계부터 처리한다. | M |
| FR-DMN-9 | 데몬은 프로젝트의 벡터 저장소와 호환성 상태를 처음 필요할 때 열고, 열어 둔 프로젝트 수가 설정 `[daemon] max_open_projects`(기본 8)를 넘으면 가장 오래 쓰지 않은 프로젝트를 닫아야 한다. 사용 중인 저장소는 마지막 사용이 끝난 뒤 닫는다. 원본 저장소는 작업마다 연다. | M |
| FR-DMN-10 | 데몬은 시작한 직후 별도 스레드에서 임베더로 짧은 텍스트 하나를 임베딩해 모델을 메모리에 올려야 한다(워밍업). 실패하면 오류 로그에 남기고 계속 동작한다. 워밍업은 다른 요청 처리를 막지 않는다. | M |
| FR-DMN-11 | 데몬은 `shutdown` 요청을 받으면 `Done`으로 응답한 뒤 유휴 종료(FR-DMN-8)와 같은 경로로 종료해야 한다. 새 연결을 받지 않고, 소켓 파일을 지우고, 쓰기 스레드는 처리 중인 작업 1건을, 임베딩 스레드는 처리 중인 묶음 1개(최대 `job_batch`개 작업)를 마친 뒤 끝낸다. 남은 작업은 원본 저장소와 스풀에 남아 다음 실행 때 처리된다. 재색인 중이면 종료하지 않고 오류를 돌려준다. 로그에 종료 사유(`shutdown requested`)를 남긴다. | M |
| FR-DMN-12 | 수동 중지는 다음 순서로 해야 한다. (1) 데이터 루트의 소켓 파일이 있으면 `shutdown`(FR-DMN-11)을 요청한다. 연결할 수 없으면 데몬이 없는 것으로 본다. 재색인 중이라 거부되면 아무것도 멈추지 않고 오류로 끝낸다. (2) 소켓 파일이 없어지고, 데이터 루트의 잠금 파일(`hippo.lock`)의 잠금이 풀릴 때까지(데몬 프로세스가 끝날 때까지) 최대 120초 기다린다. 데몬은 종료 요청 뒤에도 처리 중인 임베딩 묶음을 마치므로(FR-DMN-11), 소켓 파일이 없어진 것만으로는 끝난 것이 아니다. 소켓 파일이 없어도 잠금이 잡혀 있으면 데몬이 남아 있는 것으로 보고 기다린다. 시간 안에 끝나지 않으면 오류로 끝낸다. (3) hippo가 띄운 Ollama 서버(FR-EMB-4)가 실행 중이면 종료 신호를 보낸다. | M |
| FR-DMN-13 | 데몬은 설정 `[daemon] prevent_sleep`(기본 true)이 켜져 있으면, 처리할 작업이 있는 동안 시스템 유휴 잠자기를 막아야 한다. 처리할 작업은 쓰기 스레드에 보냈지만 아직 처리를 마치지 않은 작업(수집, 시작 시 스풀 처리, 가져오기 준비, 세션 가져오기, 삭제), 메모리 임베딩 큐의 작업(일반 단계와 `bulk` 단계 모두), 진행 중인 재색인이다. 다만 마지막 임베딩 시도가 실패해 재시도를 기다리는 동안(FR-STO-7)에는 임베딩 큐의 작업을 세지 않는다. 임베딩 서버가 멈춘 채로 잠자기를 계속 막지 않기 위해서다. 다음 임베딩이 성공하면 다시 센다. 데몬은 1초마다 이 상태를 확인한다. 작업이 있고 잠자기를 막고 있지 않으면 `caffeinate -i -w <데몬 pid>`를 자식 프로세스로 띄우고, 작업이 없으면 그 프로세스를 끝낸다. 데몬이 끝날 때도 그 프로세스를 끝낸다. `-w`를 쓰므로 데몬이 비정상 종료해도 잠자기 방지가 남지 않는다. 띄운 프로세스가 스스로 끝났으면 다음 확인 때 다시 띄운다. `caffeinate`를 띄우지 못하면 오류 로그에 한 번 남기고, 데몬은 잠자기 방지 없이 계속 동작한다. 이 기능은 유휴 잠자기만 막는다. 덮개를 닫거나 사용자가 직접 잠자기를 고른 경우는 막지 않는다. 값은 데몬 시작 때 읽는다. 근거: Claude Code도 작업하는 동안 같은 방식(`caffeinate -i`)으로 잠자기를 막는다. Claude Code가 작업을 마친 뒤 남은 hippo 작업(임베딩, 가져오기)이 잠자기로 멈추지 않게 하기 위해서다. | M |

### 3.3 성능 요구사항
| ID | 요구사항 |
|----|----------|
| PR-1 | 수집 hook 클라이언트는 실행 시작부터 종료까지 p95 50ms 이하여야 한다. |
| PR-2 | `user-prompt-submit` hook은 시간 예산(FR-RET-5)을 넘지 않아야 한다. 데몬이 준비된 상태에서 p95 300ms 이하를 목표로 한다. |
| PR-3 | `iterations` 1,000만 행에서 `hippo search` 응답은 p95 500ms 이하여야 한다. |
| PR-4 | 데몬은 하루 3.5만 청크를 하루 안에 임베딩할 수 있어야 한다. |

### 3.4 논리 데이터 요구사항
| ID | 요구사항 |
|----|----------|
| DR-1 | 데이터 루트는 FR-ROOT-1로 정하며 하위에 `bin/hippo`(설치 경로), `bin/claude-dir`(FR-INS-12), `registry.sqlite`(목록 DB, FR-PRJ-8), `projects/<id>/records.sqlite`(프로젝트 원본 저장소. 세션 해석기 상태, 백그라운드 기록, 임베딩 작업 큐 포함), `projects/<id>/vectors/<저장소명>/<세대>/`(벡터 저장소)와 `projects/<id>/vectors/<저장소명>/ACTIVE`(사용 중인 세대), `spool/`, `logs/`(`hippo.log`, `hook.log`, `daemon.err`, FR-OBS-1), `config.toml`을 둔다. 원본 저장소는 주입 기록(`injections`)과 기억 활용 기록(`memory_uses`)도 담는다(FR-OBS-5, FR-OBS-6). 가져오기는 transcript를 읽기만 하며 `[import] transcripts_dir` 아래를 바꾸지 않는다. 프로젝트 id는 무작위 128비트 값의 소문자 hex 32자이다. |
| DR-2 | 데이터 루트와 하위 디렉터리의 권한은 `0700`, SQLite 파일의 권한은 `0600`이어야 한다. |
| DR-3 | 모든 벡터 행은 생성에 쓴 `embed_model`과 차원을 기록해야 한다. |

### 3.5 품질 속성
| ID | 요구사항 |
|----|----------|
| QA-1 (신뢰성) | 데몬이 비정상 종료되어도 수집한 반복을 잃지 않아야 한다. 재시작하면 offset 이후부터 다시 처리한다. |
| QA-2 (신뢰성) | 같은 transcript를 여러 번 처리해도 결과 DB가 같아야 한다(멱등성). |
| QA-3 (보안) | 데이터는 로컬 장비 밖으로 전송되지 않아야 한다. (C-4) |
| QA-4 (보안) | 비밀값 마스킹(FR-STO-6)을 거치지 않은 텍스트는 DB에 기록되지 않아야 한다. |
| QA-6 (보안) | 주입 텍스트와 CLI 출력에서 `\n`, `\t`를 뺀 제어 문자(C0, DEL, C1)는 `\u{..}` 형태로 이스케이프해야 한다. |
| QA-5 (무간섭) | `hippo`의 어떤 오류도 Claude Code 세션 진행을 막거나 오류 메시지를 띄우지 않아야 한다. |

---

## 4. 검증

| 요구사항 | 방법 | 검증 내용 |
|----------|------|-----------|
| IF-HK-*, FR-CAP-* | 테스트 | 부록 A의 실측 hook 입력을 fixture로 사용해 이벤트별 동작과 `agent_id` 필터를 확인 |
| FR-EXT-* | 테스트 | 실측 transcript fixture(s1~s5)로 반복 분리, 입력 연결, 완료 판정, 멱등성 확인. 처음 보는 세션이 기준 턴부터 저장되는지, `prompt_id`가 없거나 찾지 못하면 아무것도 저장하지 않는지, 파일이 다시 쓰이면 기준 턴부터 다시 읽는지 확인. `isApiErrorMessage` 라인의 텍스트가 응답으로 저장되지 않는지 확인. 턴 종료 표시 라인 두 종류가 열린 반복을 확정하고 반복 입력은 남기는지, 다른 `system` 라인은 확정하지 않는지 확인 |
| FR-BG-* | 테스트 | s4, s5 fixture로 대기 기록 생성, 알림 파싱, 결과 연결, 반복 알림 순번 확인. output-file이 없는 경우 포함 |
| FR-STO-11 | 테스트 | hippo 호출 판정(절대 경로, 따옴표, `&&`·`;`·`|` 뒤, 변수 할당 후 `$H`·`"$H"`, `$( )`·백틱 안, env·sudo 래퍼, 인자 자리의 hippo, 파싱 실패 시 첫 단어 규칙), `embed_text` 제외, hippo 호출 결과의 FTS·임베딩·재색인·병합 제외, `show` 표시 확인 |
| FR-STO-1~5, 7~9 | 테스트 | 임시 원본 저장소와 메모리 벡터 저장소에 저장 후 행, 해시 중복 제거, 청크 분할(경계 길이, 겹침, 겹침 제거 후 원문 복원, 겹침 설정 오류), 청크당 요청 1개 확인. 임베딩은 테스트용 결정론적 임베더로 대체 |
| FR-STO-7, FR-STO-12 | 테스트 | LanceDB 백엔드에서 묶음 하나가 수정 연산 2번(삭제 1번, 추가 1번)으로 저장되는지, `replace`가 기존 청크를 모두 바꾸는지, 묶음 도중 실패하면 앞의 작업만 저장·완료되는지 확인. `pending_changes`가 추가·삭제마다 늘고 정리 뒤 0이 되는지, 정리 뒤 버전이 1개이고 검색 결과가 정리 전과 같은지, 수정이 없으면 `optimize`가 버전을 만들지 않는지, 이전 버전이 많이 쌓인 저장소가 첫 정리로 버전 1개가 되는지 확인. 데몬에서 임베딩을 여러 묶음 해도 버전 수가 `optimize_every` + 2를 넘지 않는지, `optimize_every = 0`이면 (a)가 일어나지 않는지, 큐가 비면 수정된 저장소가 정리되는지, 재색인한 세대의 버전 수가 1개인지 확인 |
| FR-STO-12 (규모) | 시연 | 해시 임베더, LanceDB 백엔드, 격리 루트로 레코드 2만 개를 임베딩한다. 큐가 빈 뒤 벡터 저장소의 버전이 2개 이하이고, 디렉터리 크기가 원시 벡터 크기(레코드 수 × 차원 × 4바이트)의 2배 이하인지 확인한다. 수정 전 코드와 크기·버전 수·처리 시간을 비교한다 |
| FR-STO-6, QA-4 | 테스트 | 패턴별 비밀값이 DB에 남지 않는지 확인 |
| FR-RET-* | 테스트 | 고정 데이터셋으로 필터, 임계값, 턴 단위 묶음과 중복 제거, 시각 오름차순 정렬, 줄 구성(입력·응답·질문·답, 도구 제외), 자르지 않음, 한도를 넘는 턴 건너뜀, 완료 알림 프롬프트 무주입, `min_prompt_chars` 경계(공백을 뺀 길이가 기준 미만이면 무주입, 기준과 같으면 주입, 한글을 문자 단위로 셈), 시간 예산 초과 시 무주입 확인. 질의 접두사가 질의 임베딩 입력에만 붙고(길이 제한 뒤) FTS 질의와 저장 텍스트에는 붙지 않는지, 빈 접두사면 질의를 그대로 임베딩하는지 확인. 예산 0으로 띄운 데몬에서 FR-RET-10의 로그 줄이 남는지, 정상 예산에서는 남지 않는지 확인 |
| FR-SWP-* | 테스트 | 계약 테스트를 모든 백엔드에 적용. 임베더 교체 후 `reindex` 전후 검색 동작, 차원 불일치 시 FTS 전용 전환 확인 |
| FR-DMN-*, QA-1 | 테스트 | 중복 실행 거부, 강제 종료 후 재시작 시 누락과 중복 없음 확인. `max_open_projects`를 넘을 때 닫힘, 여러 프로젝트 작업의 단일 FIFO 처리 순서 확인. 자동 주입 처리 중 청크 요청 대기, bulk 처리 중 일반 작업 도착 시 작업 1건 뒤 전환 확인 데몬 시작 직후 워밍업 임베딩 1회, 콜드 시작 시 요청 예산 적용, 모델 콜드 시 콜드 예산 적용 확인 |
| FR-PRJ-* | 테스트 | 임시 디렉터리에 git 저장소를 만들어(`git init`, 커밋, `git worktree add`, `mv`, 두 번째 clone) worktree 통합, 판정 규칙 (a)~(d), 세션 중 `cd`해도 고정값 유지, 조회 전용 판정이 목록 DB를 바꾸지 않음 확인. 가져오기 준비의 네 경우(FR-PRJ-7), 벡터 복사와 재임베딩 분기, 중간 실패 후 재실행 멱등성 확인 |
| FR-IMP-* | 테스트 | 임시 transcripts 디렉터리로 후보 선택(하위 경로, worktree, 없어진 경로, 프로젝트 id, `subagents/` 제외, 정렬), 세션 상태별 처리 (a)~(c), 끝 반복의 완료·보류 판정, 다른 프로젝트 세션 건너뜀, fork 복사본 건너뜀, 재실행 시 "변경 없음", `bulk` 작업 등록과 임베딩 큐 우선순위, 쓰기 작업 우선순위, 쓰기 스레드가 조회 시간 제한(30초)보다 오래 막혀도 실제 `import_prepare`가 끝까지 기다렸다가 결과를 응답하는지 확인 |
| FR-INS-*, IF-SKL-1 | 테스트 | 임시 HOME, 임시 데이터 루트, 임시 Claude 폴더에서 실행 파일 복사(같은 내용이면 생략, 심볼릭 링크 거부, dry-run, 바꿨을 때만 데몬 `shutdown`), hook과 안내 블록이 설치 경로를 가리킴, hook 병합·제거, 안내 블록 추가·교체·제거(블록 밖 내용 유지, 끝 줄바꿈이 없던 파일과 CRLF·혼합 줄 끝 파일도 설치 뒤 제거하면 바이트 단위로 같음, 공백만 남으면 파일 삭제), 끝 표식 없는 블록 거부, 심볼릭 링크 거부, `--dry-run`. PATH 링크: 없으면 생성(`~/.local/bin` 생성 포함), 같은 대상이면 유지, 다른 대상·일반 파일이면 아무것도 바꾸지 않고 오류, 제거는 이 루트를 가리킬 때만 삭제, PATH에 없을 때 경고. `bin/claude-dir` 기록과 기본값. 제거 뒤 실행 파일이 남음. 완전 삭제: 제거 뒤 데이터 루트 삭제, 데몬 종료 대기, 거부 시 루트 유지, 링크·`$HOME`·`/` 거부, pid 파일의 프로세스 이름 확인 |
| FR-DMN-12, IF-CLI-15 | 테스트 | 데몬이 없을 때 아무것도 하지 않음, 실행 중인 데몬이 멈추고 소켓 파일이 없어짐, pid 파일이 없거나 이름이 `ollama`가 아닌 프로세스는 멈추지 않음, `hippo stop`의 출력과 종료 코드. GUI 멈추기 뒤 폴링 중단은 실행 화면으로 확인 |
| FR-OBS-* | 테스트 | 로그 줄 형식과 5 MiB 순환(기존 `.1` 바꿔 쓰기). 수집 hook의 `delivered`·`spooled`·`paused`와 주입 hook의 `injected`·`no-context`·`no-daemon` 줄. 데몬의 `capture` 줄(종류별, `skipped=no-anchor`, 처리 전 상태 없음의 `-`). 경로별 `stored_at`·`source` 저장, 이전 스키마 저장소를 열면 열이 추가되고 기존 행은 비어 있음, 병합의 값 유지, `show` 머리 줄 표시. 주입 결과 다섯 가지와 사유, 프로젝트 없음의 로그 줄. 활용 판정 (a)~(d)와 중복 없음. 7일 경계, `status` 줄 형식과 요청 0일 때 비율 생략. 세션·턴 삭제의 행 정리. GUI 표는 실행 화면으로 확인 |
| FR-CAP-9 | 테스트 | `Stop` 처리가 마지막 assistant 라인이 기록되기 전에 읽었어도 데몬 시작 때의 따라잡기가 그 반복을 저장하는지, 결과가 없는 `tool_use`로 끝난 꼬리는 확정하지 않는지, 수정 시각에서 5초가 지나지 않은 세션과 파일이 없는 세션은 건너뛰는지, 시작 offset을 바꾸지 않는지 확인. 60초 주기는 코드 검토로 확인 |
| FR-CAP-8, FR-RET-12 | 테스트 | 두 키의 기본값(true)과 해석, 설정 탭 필드와 재시작 불필요 판정, 일시정지면 수집 hook이 데몬 요청과 스풀 쓰기를 하지 않고 주입 hook이 아무것도 출력하지 않음(종료 코드 0), 재생하면 다시 동작함. GUI 버튼은 실행 화면으로 확인 |
| FR-DMN-12 (종료 대기), IF-CLI-5·FR-GUI-3 (큐 구분) | 테스트 | 잠금 파일이 잡혀 있으면 데몬이 남은 것으로 판정, 풀리면 끝난 것으로 판정, 파일이 없으면 끝난 것. 기록 수 집계가 가져오기 작업 수를 따로 세는지, 상태 출력이 실시간·가져오기 건수를 나눠 보이는지, 요약 합계와 남은 시간 계산. GUI 표시는 실행 화면으로 확인 |
| FR-GUI-7 (프로젝트 선택) | 테스트 | 위치에서 폴더 경로를 구하는 규칙(`.git` 위치, git이 아닌 위치). 목록 표시는 실행 화면으로 확인 |
| FR-GUI-24 | 테스트 | AppleScript 문자열의 따옴표·역슬래시 이스케이프, 처음 위치 판정(있는 폴더, 상대 경로, `~`, 없는 경로는 홈), 출력 해석(끝 `/` 제거, 루트, 취소·실패면 없음). 선택 창은 실행 화면으로 확인 |
| FR-ROOT-*, FR-INS-14, IF-CLI-13, IF-CLI-14 | 테스트 | 링크를 푼 실행 파일 경로에서 데이터 루트 판정(`<d>/bin/hippo`, PATH 링크 경유, `bin`이 아닌 위치, 이름이 `hippo`가 아닌 경우). 관리 실행 파일이 아닐 때 허용 명령과 거부 명령의 종료 코드, hook의 종료 코드 0. `--root`가 없음. `purge`의 `--yes`·터미널 아님 처리. 인자 없는 실행의 앱 판정 |
| FR-GUI-22, FR-GUI-23 | 테스트 | 기본 데이터 루트 선택 순서, Claude 폴더 기본값, 설치 상태 세 가지 판정. 설치 상태가 `설치됨`이 아닐 때 비활성 탭. 화면은 실행 화면으로 확인 |
| FR-APP-* | 시연 | `scripts/bundle-app.sh`로 만든 `hippo.app`의 구성, `plutil -lint`, `codesign --verify`, Finder에서 열면 GUI가 뜨고 아무것도 설치되지 않음, GUI 적용 뒤 hook·안내 블록·PATH 링크·설치 경로 확인 |
| FR-UI-* | 테스트 | 기본(en)과 `ko` 설정에서 CLI·데몬 출력 언어 확인, 설정 파일 변경이 데몬 재시작 없이 다음 요청에 반영됨 확인, 주입 블록·안내 블록·`embed_text`가 언어 설정과 관계없이 영어임 확인 |
| FR-IMP-8, FR-IMP-9, IF-CLI-9 | 테스트 | 후보 정보(시작 시각, 시작 경로, 크기, 첫 프롬프트의 메타·tool_result·완료 알림 제외, 공백 축약, 마스킹, 80자 자름)와 가져오기 상태 네 가지 확인. `dry_run` 준비가 경우 (a)~(d) 모두에서 목록 DB와 원본 저장소를 바꾸지 않고, 실제 준비와 같은 경우·대상을 돌려주는지 확인. 재색인 중 dry-run 허용. `--dry-run` 출력 형식과 제어 문자 이스케이프 확인 |
| FR-DMN-8 | 테스트 | 요청이 없어도 임베딩 대기 작업이 있으면 유휴 종료하지 않고, 큐가 비면 종료함, 임베딩 실패로 재시도를 기다리는 동안에는 유휴 종료함 |
| FR-DMN-11 | 테스트 | `shutdown` 뒤 `Done` 응답, 소켓 파일 삭제, 데몬 스레드 종료, 남은 임베딩 작업이 다음 실행에서 처리됨, 재색인 중 거부 확인 |
| FR-DMN-13 | 테스트 | 작업이 있으면 데몬 pid를 `-w`로 받은 `caffeinate` 자식 프로세스가 생기고, 큐가 비면 없어짐, `prevent_sleep = false`이면 생기지 않음, 임베딩 실패로 재시도를 기다리는 동안 없어짐, 데몬 종료 시 없어짐, 자식 프로세스가 끝나면 다시 띄움, 실행 파일을 띄우지 못해도 데몬이 동작함. 설정 기본값과 GUI 설정 항목 확인 |
| FR-GUI-5, FR-GUI-6 | 테스트 | 설정 저장 함수: 바꾼 키만 수정(주석·순서·다른 키 유지), 기본값이면 키와 빈 테이블 삭제, 목록 값 저장, 검증 실패 시 파일 미변경, 변경 없으면 미기록, 파일 없음에서 빈 결과면 파일 미생성, 권한 0600, 심볼릭 링크 거부. 재시작 필요·재색인 필요 판정 확인 |
| FR-GUI-7, FR-GUI-8, FR-GUI-10 | 테스트 | 화면과 분리한 상태 로직: 후보 기본 선택, 입력 변경 시 미리 보기 무효화, 선택한 session id만 응답 순서대로 전송, 멈춤 뒤 다음 세션 미전송, 미리 보기 전 적용·해제 불가 확인 |
| FR-DEL-*, IF-CLI-11, IF-CLI-12 | 테스트 | 프로젝트 삭제 뒤 목록 DB 행·디렉터리·큐 작업이 없고 같은 경로의 새 세션이 새 프로젝트가 되는지, 세션 삭제와 턴 삭제가 반복·FTS·연결·백그라운드 기록·임베딩 작업·벡터(모든 청크)를 지우고 다른 세션·턴은 남기는지, 다른 기록이 참조하는 결과는 남고 참조가 없어진 결과는 지워지는지, 세션 삭제 뒤 같은 세션의 hook이 기준 턴부터 다시 저장하는지, `dry_run`이 아무것도 바꾸지 않고 실제 삭제와 같은 수를 돌려주는지, 재색인 중 거부, 처리 중인 임베딩 묶음이 지운 레코드의 벡터를 저장하지 않는지, 삭제가 기다리면 다른 프로젝트의 묶음이 작업 1건 뒤 멈추고 남은 작업은 삭제 뒤에 처리되는지, 임베딩 묶음이 조회 시간 제한(30초)보다 오래 걸려도 실제 삭제가 오류 없이 끝까지 기다렸다가 성공을 응답하는지, 응답 전에 연결이 끊기면 클라이언트가 실패로 보고하는지 확인. CLI의 `--dry-run`, `--yes`, 터미널이 아닐 때 `--yes` 없으면 종료 코드 1, 대상 없음 확인 |
| FR-UI-4, FR-GUI-1, FR-GUI-25 | 테스트 | `[ui] start_tab` 해석(기본 `about`, 잘못된 값은 오류)과 재시작 불필요 판정, 탭 순서, 시작 탭 결정(`last`이면 기록한 탭, 기록이 없거나 깨졌으면 `about`), 마지막 탭 기록의 쓰기·읽기, 데이터 루트가 없으면 쓰지 않음 확인 |
| FR-GUI-9 | 테스트 | 검색 탭 주입 미리 보기의 범위 판정(`all`과 빈 값은 불가, `project`와 경로는 가능). 화면은 실행 화면으로 확인 |
| FR-UI-3, FR-GUI-21 | 테스트 | `[ui] theme` 해석(기본 `system`, 잘못된 값은 오류), 설정 탭 필드와 재시작 불필요 판정, 테마별 배경·글자색과 다크 여부, 강조색 위 글자와 상태 색의 명도 대비 기준 확인. 화면은 실행 화면으로 확인 |
| FR-GUI-11, FR-GUI-20 | 테스트 | 바이너리에 넣은 이미지가 모두 PNG로 해석되고 크기가 정한 값인지, README에서 이미지 블록과 이미지 줄만 빠지고 나머지 줄은 그대로인지, 상대 링크만 GitHub 주소로 바뀌고 절대 주소·`#` 링크·코드 안의 글자는 그대로인지 확인. 화면 배치는 실행 화면으로 확인 |
| FR-GUI-18, FR-GUI-19 | 테스트 | `current_project`가 조회 전용 판정 결과(프로젝트 없음 포함)를 돌려주는지, 기본 프로젝트 선택 규칙(현재 프로젝트, 없으면 첫 항목, 선택한 프로젝트가 사라지면 다시 선택), 프로젝트를 바꾸면 질의 결과를 지우는지, 성공 토스트가 5초 뒤 사라지고 오류 토스트는 새 메시지가 와도 닫을 때까지 남는지 확인 |
| FR-GUI-13~16 | 테스트 | 데몬 요청 단위로 세션·턴 목록의 정렬·페이지·턴 수·임베딩 상태, 필터(3자 이상 FTS, 미만 부분 문자열), 턴 상세 내용, 주입 미리 보기의 무주입 사유와 후보별 판정이 실제 `inject` 결과와 같은 턴을 고르는지 확인 |
| FR-GUI-1~17, IF-CLI-10 | 시연 | `hippo gui`로 탭마다 동작, 한글 표시, 창을 닫으면 종료 코드 0 확인 |
| C-9 | 검사 | `LICENSE` 파일과 `Cargo.toml` `license = "MIT"` 확인 |
| PR-1~4 | 시연 | 벤치마크 스크립트로 측정 |
| QA-3 | 검사 | 코드 리뷰와 실행 중 네트워크 연결 목록 확인(`lsof -i`) |
| 부록 B 항목 | 시연 | 대화형 세션에서 확인 |

---

## 부록 A. 실측 근거 (Claude Code 2.1.316, 격리 설정 파일을 준 `claude -p --settings <파일>`)

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
- 병렬 호출은 transcript에 `tool_use → tool_result → tool_use → tool_result`처럼 교차로 기록될 수 있다(s2).
- assistant 라인에는 `promptId`가 없고 user 라인에만 있다.
- 백그라운드 서브에이전트 보고(`<agent-message>`)와 하네스 안내 메시지는 `isMeta: true`로 기록된다. `<task-notification>`은 `isMeta`가 아니다(s4).
- tool_result 라인의 `toolUseResult`에 백그라운드 작업 id가 있다(Bash: `backgroundTaskId`, Agent: `agentId`).
- 포그라운드 Agent의 tool_result 본문은 안내 문구이고, 서브에이전트 보고는 `toolUseResult.handbackReport.text`에 있다(s3).
- `SubagentStop`에는 `last_assistant_message`가 없다.
- `TaskCreated`, `TaskCompleted`, `Notification`은 s5에서 실행되지 않았다.
- 백그라운드 완료 알림은 메인 transcript에 `type: user`, 문자열 content의 `<task-notification>`으로 기록되며 `<tool-use-id>`가 시작 호출의 `tool_use_id`와 같다.

재개와 분기(2026-11-02, 격리 설정 `claude -p`):
- `--resume <id>`와 `--continue`는 같은 session id로 같은 transcript 파일에 이어 쓴다. 이전 라인을 복사하지 않는다(uuid 중복 0). `SessionStart`의 `source`는 `resume`이다.
- `--resume <id> --fork-session`은 새 session id로 새 파일을 만들고 이전 라인을 모두 복사한다. 복사한 라인은 `sessionId`만 새 값이고 `uuid`와 `message.id`는 원본과 같다. `SessionStart`의 `source`는 `fork`이다.
- 실제 로그 335개에서 assistant `message.id` 24,824개 중 두 파일 이상에 나온 것은 101개였고, 모두 시작 시각이 같은 한 쌍(fork)이었다.
- `Stop`과 `UserPromptSubmit` 입력의 `prompt_id`는 그 턴 user 라인의 `promptId`와 같다.
- hook `command`의 `~/…` 경로는 펼쳐져 실행된다(`UserPromptSubmit`, `Stop`에서 확인, 2026-11-03).
- transcript에 턴 종료를 일정하게 표시하는 라인은 없다(`system` 라인의 `turn_duration`, `stop_hook_summary`는 환경에 따라 있거나 없다).

## 부록 B. 미해결 항목 (구현 전 또는 구현 중 확인)

| ID | 항목 | 영향 요구사항 |
|----|------|---------------|
| OPEN-1 | 대화형 세션에서 백그라운드 완료 알림이 새 턴을 시작하고 `Stop`이 실행되는지 | FR-BG-2, FR-CAP-3 |
| OPEN-2 | 사용자가 Esc로 중단했을 때 실행되는 hook과 transcript 기록 형태 | FR-EXT-8 |
| OPEN-3 | 컨텍스트 압축(compaction)이 transcript 파일을 다시 쓰는지, 덧붙이기만 하는지 | FR-EXT-1 |
| OPEN-4 | 실제 데이터로 `qwen3-embedding:0.6b`와 `bge-m3`의 검색 품질 비교 | C-3 |
| OPEN-7 | `SessionEnd` 입력에 `prompt_id`가 있는지. 없으면 처음 보는 세션의 `SessionEnd`만으로는 아무것도 저장하지 않는다(FR-EXT-12) | FR-CAP-4 |
| OPEN-8 | 대화형 `/resume`과 대화형 분기가 `claude -p`의 `--resume`, `--fork-session`과 같은 방식으로 transcript를 쓰는지 | FR-EXT-12, FR-IMP-4 |
| OPEN-6 | `Stop` hook 실행 시점에 마지막 assistant 라인이 transcript에 기록되어 있는지. 기록 전이면 그 반복은 응답 없이 저장되고 이후 같은 `message.id` 라인은 버려진다 | FR-EXT-7, FR-EXT-10 |

## 부록 C. 자동 주입 품질 실측 (0.13, 2026-11-03)

이 프로젝트의 과거 기록(반복 1,413개, 벡터 6,355개, `qwen3-embedding:0.6b`)을 격리 데이터 루트로 가져와 측정했다. 평가셋은 저장소 밖에 둔다(실제 대화를 담기 때문).

- 평가셋: 정답 질문 60개(과거 결정을 다시 묻는 질문 → 그 결정이 내려진 턴)와 오주입 질문 30개(기억이 필요 없는 프롬프트. 24개는 실제 과거 프롬프트이며, 측정할 때 그 프롬프트가 들어 있던 턴은 후보에서 뺀다).
- 지표: recall@3(주입된 턴 3개 안에 정답 턴이 있는 정답 질문 수), 오주입(무엇이든 주입된 오주입 질문 수).

| 설정 | recall@3 | 오주입 |
|------|----------|--------|
| 접두사 없음, 임계값 0.40 (0.12) | 53/60 | 28/30 |
| Qwen 기본 질의 지시문, 임계값 0.50 | 53/60 | 18/30 |
| Qwen 기본 질의 지시문, 임계값 0.50, 10자 이하 무주입 (0.13 기본값) | 53/60 | 16/30 |
| Qwen 기본 질의 지시문, 임계값 0.60, 10자 이하 무주입 | 약 46/60 | 약 9/30 |

- 위 표는 벡터 유사도 순서로 계산했다. 실제 주입은 RRF 순서와 전체 길이 한도를 함께 쓴다.
- 평가셋을 반으로 나눠 한쪽에서 고른 설정을 다른 쪽에 적용하는 교차 확인을 20회 했다. 20회 모두 질의 지시문을 쓴 설정이 0.12 설정보다 나았다. 임계값의 최적점은 0.39~0.60 사이에서 흔들렸으므로, 임계값은 recall을 유지하는 0.50으로 정했다.
- 길이 기준을 15자로 올리면 오주입이 14/30으로 줄지만, 정답 질문이 모두 17자 이상인 평가셋의 특성일 수 있어 10자 이하로 정했다.
- 메타 질문("방금 주입됐어?")과 작업 지시("설치 전으로 돌려줘")는 과거에 같은 말을 한 턴과 유사도가 0.7 이상이어서, 임계값과 질의 지시문으로는 걸러지지 않는다.

재순위 모델 시험(후보 10턴, 같은 평가셋의 앞 24개, Apple M4):

| 모델 | recall@3 (18개) | 후보 1개당 지연 | 결과 |
|------|-----------------|-----------------|------|
| 재순위 없음 (RRF 순서) | 16/18 | – | – |
| `bge-reranker-v2-m3` | 17/18 | 약 275ms | 메타 질문 오주입은 걸러지지 않음 |
| `Qwen3-Reranker-0.6B` | 15/18 | 약 700ms | 같음 |
| Laya (`laya-multilingual`) | 11/18 | 약 135ms | 거의 모든 후보에 0.9 이상을 줌 |
| kev 4B, nimble 9B (Jev류 판단 모델) | 중단 | 2~7초 | 지연이 hook 예산(800ms)을 크게 넘음 |

재순위 모델은 도입하지 않는다. 품질 이득이 작고(+1/18), 지연이 hook 예산을 넘으며, 남은 오주입 문제를 해결하지 못하기 때문이다.

---

## 부록 D. LanceDB 버전 누적 실측 (0.15, 2026-11-04)

사용자 범위로 설치한 뒤, 가장 큰 프로젝트(임베딩 작업 약 4만 개)의 `vectors.lance`가 110GB, 버전 69,608개, 데이터 파일 34,798개가 되어 디스크가 가득 찼다. 0.14까지는 레코드 하나마다 `delete`와 `merge_insert`를 실행해 레코드당 버전이 2개 생겼고, 정리를 하지 않았다.

재현 조건: 1024차원, 청크 1개짜리 레코드, lancedb 0.41, release 빌드.

| 방식 | 레코드 수 | 전체 크기 | 버전 수 | 데이터 파일 | 처리 시간 |
|------|----------:|----------:|--------:|------------:|----------:|
| 0.14 (레코드마다 delete+merge_insert, 정리 없음) | 1,000 | 103MB (`_versions` 97MB, `data` 6MB) | 2,001 | 1,000 | 21.5초 |
| 0.14 | 2,000 | 398MB | 4,001 | 2,000 | 83.3초 |
| 레코드마다 저장, 64건마다 정리 | 4,000 | 16.7MB | 66 | 33 | 18.6초 |
| 64건 묶음 저장, 묶음 10개마다 정리 | 4,000 | 16.5MB | 5 | 4 | 0.2초 |

- 늘어난 것은 데이터가 아니라 버전 매니페스트(`_versions`)이다. 매니페스트는 버전마다 그때의 모든 조각 목록을 담는다. 그래서 전체 크기가 쓰기 횟수의 제곱에 비례한다. 같은 비율로 계산하면 쓰기 7만 번에 약 110GB가 되어 실측과 맞는다.
- Lance는 버전 생성을 끌 수 없다. 앱이 정리(compaction, prune)를 호출해야 한다.
- 0.15 규모 검증(FR-STO-12 시연): 해시 임베더(1024차원), LanceDB, 레코드 2만 개를 64개 묶음과 `optimize_every = 20`으로 처리했다. 큐가 빈 뒤 버전 1개, 데이터 파일 1개, 크기 82.3MB(원시 벡터 81.9MB의 1.00배), 처리 중 최대 버전 19개, 처리 시간 2.6초였다. 0.14 방식은 2천 개에서 이미 398MB, 83초였고, 2만 개면 약 40GB가 될 것으로 추정한다(제곱 비례).
- 이전 버전을 들고 있는 다른 핸들이 있을 때 보존 기간 0으로 prune하면, 그 핸들의 읽기와 쓰기가 지워진 파일을 찾지 못해 실패했다. 그래서 정리는 같은 핸들 안에서 한다(FR-STO-12).

