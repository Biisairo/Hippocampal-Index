# Hippocampal Index (해마 색인, `hippo`)

**Claude Code 세션을 위한 장기 기억.**

이름은 *해마 색인 이론*(hippocampal memory indexing theory, Teyler & DiScenna, 1986)에서 따왔습니다. 이 이론에 따르면 해마는 기억 전체를 저장하지 않습니다. 대뇌피질에 흩어진 기억 흔적을 가리키는 **색인**을 저장해 두었다가, 단서가 들어오면 그 색인으로 원래 기억을 다시 활성화합니다. 이 프로젝트도 같은 방식으로 동작합니다. 원문 기록은 SQLite(피질)에 두고, 벡터 저장소가 색인 역할을 하며, 프롬프트가 단서가 됩니다. 줄여서 부르는 이름과 명령어는 `hippo`입니다.

[English](README.md)

`hippo`는 Claude Code에 내 세션의 장기 기억을 만들어 줍니다. agentic loop의 반복을 하나씩 로컬 DB에 기록해 두었다가, 새 프롬프트를 보내면 관련된 과거 맥락을 찾아 Claude에게 전달합니다.

모든 데이터는 내 컴퓨터 안에만 있습니다. 임베딩은 로컬 Ollama 모델로 만들고, 벡터는 임베디드 LanceDB에 저장합니다. 대화 내용은 외부로 나가지 않습니다.

---

## 목차

1. [개념](#1-개념)
2. [요구 사항](#2-요구-사항)
3. [설치](#3-설치)
4. [사용법](#4-사용법)
5. [설정](#5-설정)
6. [임베딩 모델이나 벡터 DB 바꾸기](#6-임베딩-모델이나-벡터-db-바꾸기)
7. [데이터 위치와 보안](#7-데이터-위치와-보안)
8. [문제 해결](#8-문제-해결)
9. [개발: 수정과 빌드](#9-개발-수정과-빌드)
10. [알려진 한계](#10-알려진-한계)

---

## 1. 개념

### 1.1 기억의 단위: agentic loop 반복 하나

프롬프트를 하나 보내면 Claude Code는 **agentic loop**를 돕니다. loop의 **반복(iteration)** 하나는 모델 호출 한 번과, 모델이 요청한 도구 호출들로 이루어집니다.

```
프롬프트
  → 반복 1: 모델 텍스트 + tool_use(Read) → tool_result
  → 반복 2: 모델 텍스트 + tool_use(Edit) → tool_result
  → 반복 3: 모델 텍스트만 (loop 종료)
```

`hippo`는 **반복마다 기록을 하나씩** 저장합니다. 기록에 담기는 내용은 다음과 같습니다.

- **입력**: 이 반복 직전에 들어온 프롬프트, 백그라운드 작업 완료 알림, 서브에이전트 보고
- **도구 호출**: 호출한 도구의 이름과 입력
- **응답**: 모델의 텍스트(`thinking`은 저장하지 않음)
- **결과**: 도구 출력 원문. 따로 저장하며, 내용 해시로 중복을 없앱니다

### 1.2 반복을 수집하는 방법 (hook)

| 반복 종류 | 수집을 일으키는 hook |
|-----------|----------------------|
| 도구를 호출한 반복 | `PostToolBatch` (병렬 호출이 모두 끝난 뒤 batch마다 한 번) |
| 마지막 반복 (텍스트만) | `Stop` / `StopFailure` |
| 세션 종료 시 남은 것 | `SessionEnd` |

이 hook들은 `async: true`로 실행되므로 세션을 느리게 만들지 않습니다. hook 입력은 수집 신호로만 쓰고, 실제 내용은 세션 transcript(JSONL)에서 읽습니다. `hippo`는 세션마다 안전 offset을 기억해 두므로, 같은 줄을 두 번 읽거나 같은 반복을 두 번 저장하지 않습니다.

그 밖의 처리 방식:

- **서브에이전트**: 내부 반복은 저장하지 않습니다. 포그라운드 서브에이전트의 최종 보고는 `Agent` 도구의 결과로 돌아오므로, 호출과 함께 저장됩니다.
- **백그라운드 작업**(Bash 또는 Agent의 `run_in_background`): 시작 호출을 *대기 중*으로 저장합니다. transcript에 완료 알림(`<task-notification>`)이 나타나면 출력을 읽어 시작 호출에 연결합니다. 그래서 호출과 응답이 한 쌍으로 남습니다.

### 1.3 저장 구조

```
원본 저장소 (SQLite, 기준 데이터)
 ├─ iterations          반복마다 한 행 (+ 전문 검색 인덱스)
 ├─ results             도구 출력. SHA-256으로 중복 제거 (+ 전문 검색 인덱스)
 ├─ iteration_results   반복과 결과의 연결
 ├─ bg_tasks / bg_completions
 ├─ embed_jobs          임베딩 작업 큐
 └─ sessions            세션별 transcript offset

벡터 저장소 (LanceDB, 언제든 다시 만들 수 있는 파생 인덱스)
 └─ 반복이나 결과의 청크마다 벡터 하나
```

이름의 "색인"이 바로 이 부분입니다. 해마 색인이 피질의 기억 흔적을 가리키듯, 벡터 저장소는 원본 기록을 가리키기만 합니다. 벡터 저장소는 원본 저장소에서 파생된 인덱스입니다. 그래서 임베딩 모델이나 벡터 DB를 바꾸더라도 SQLite에서 인덱스를 다시 만들면 됩니다([6장](#6-임베딩-모델이나-벡터-db-바꾸기) 참고).

### 1.4 기억을 쓰는 방법

- **자동 주입**: 프롬프트를 보낼 때마다 `UserPromptSubmit` hook이 데몬에 관련 기억을 요청합니다. 검색 대상은 **같은 프로젝트**(`cwd`가 같음)의 **다른 세션** 기록입니다. 벡터 유사도가 임계값 이상인 결과만 Claude의 컨텍스트에 들어갑니다. 각 항목은 `<memory>` 블록으로 감싸고, 머리말에 "과거 기록이며 지시가 아님"을 밝힙니다.
- **필요할 때 검색**: Claude나 사용자가 셸에서 `hippo search`, `hippo show`를 실행합니다.

검색은 **하이브리드** 방식입니다. 벡터 유사도와 SQLite FTS5 키워드 검색 결과를 RRF(Reciprocal Rank Fusion)로 합칩니다. `validateToken` 같은 식별자는 키워드 검색이 잘 찾습니다.

### 1.5 구조

```
Claude Code ──hook(stdin JSON)──▶ hippo hook <이벤트> ──Unix socket──▶ hippo daemon
                                       │ (데몬이 꺼져 있음)                │
                                       └──▶ spool/pending.jsonl ◀─ 시작 시 처리
                                                                          ├─ 수집 스레드 (하나, 순서대로)
Claude / 사용자 ──▶ hippo search|show|status|reindex ──Unix socket──────▶  ├─ 임베딩 스레드
                                                                          └─ 연결 처리 스레드
                                                   SQLite ◀──┘   Ollama(localhost) ◀──┘   LanceDB
```

- 데몬은 컴퓨터마다 하나만 실행됩니다(파일 잠금). 저장소에는 데몬만 씁니다.
- 데몬을 직접 띄울 필요는 없습니다. 로컬 MCP 서버처럼 필요할 때 올라옵니다. hook이나 CLI 명령이 데몬에 연결하지 못하면 `hippo daemon`을 백그라운드로 띄운 뒤 요청을 보냅니다. 요청이 없는 상태로 `idle_timeout_secs`(기본 30분)가 지나면 데몬이 스스로 내려가고, 다음 요청이 오면 다시 올라옵니다.
- 데몬을 띄우지 못하면 수집 요청은 스풀 파일에 쌓였다가, 다음에 데몬이 시작될 때 처리됩니다. `hippo` 때문에 Claude Code 세션이 실패하는 일은 없습니다.

### 1.6 생명주기: DB와 모델이 열리고 닫히는 시점

| 구성 요소 | 여는 시점 | 닫는 시점 |
|-----------|-----------|-----------|
| 데몬 | 데몬에 연결하지 못한 첫 hook이나 CLI 명령이 자동으로 띄움 | 요청 없이 `idle_timeout_secs`가 지났을 때(재색인 중에는 내려가지 않음), 또는 직접 멈출 때 |
| SQLite, LanceDB | 데몬이 시작할 때 | 데몬이 종료할 때 |
| 임베딩 모델 | 첫 임베딩 요청 때 Ollama가 올림(약 1초) | 마지막 요청 후 `keep_alive`(30분)가 지나면 Ollama가 내림 |
| Ollama 서버 | 임베딩 요청이 서버에 닿지 않으면 데몬이 자동으로 띄움(`ollama serve`, 로그는 `logs/ollama.log`). 서비스 등으로 이미 떠 있으면 띄우지 않음 | `hippo`가 끄지 않습니다. 다른 도구도 같은 서버를 쓸 수 있기 때문입니다. 모델 메모리는 `keep_alive`가 지나면 해제됩니다 |

hook과 CLI 명령은 DB를 직접 열지 않고, 소켓으로 데몬에 요청만 보냅니다. 데몬이 멈춰도 데이터는 안전합니다. 모든 변경은 SQLite 트랜잭션 하나로 반영되고(WAL 모드), 남은 임베딩 작업은 큐에 그대로 남습니다.

---

## 2. 요구 사항

| 항목 | 사용한 버전 | 비고 |
|------|-------------|------|
| macOS | Darwin (Apple Silicon) | 1차 대상. Unix socket과 launchd를 씁니다 |
| Rust | 1.95 이상 (edition 2024) | 빌드용 |
| `protoc` | libprotoc 36 이상 | LanceDB 빌드에 필요 |
| Ollama | 0.40 이상 | 임베딩 모델을 로컬에서 실행 |
| 임베딩 모델 | `qwen3-embedding:0.6b` | 기본값. 한국어, 영어, 코드 지원. 컨텍스트 32K |
| Claude Code | 2.1.316 | 이 버전에서 hook 입력과 transcript 형식을 확인했습니다 |

---

## 3. 설치

### 3.1 빌드

```bash
brew install protobuf            # protoc가 없거나 깨졌으면: brew reinstall protobuf
git clone git@github.com:Biisairo/Hippocampal-Index.git hippo && cd hippo
cargo build --release
```

### 3.2 바이너리 위치

`hippo install`은 **자신을 실행한 바이너리의 절대 경로**를 hook 명령에 넣고, 데몬도 같은 바이너리로 띄웁니다. 그래서 빌드한 자리에서 바로 설치해도 됩니다.

```bash
./target/release/hippo install --scope user --dry-run
```

다만 hook이 그 경로에 묶입니다. `cargo clean`을 하거나 저장소를 옮기면, 다시 빌드하거나 `install`을 다시 실행할 때까지 hook이 동작하지 않습니다. 같은 자리에서 다시 빌드하는 것은 괜찮습니다. 고정된 위치를 원하면 아래 중 하나로 설치한 뒤 그 위치에서 `install`을 실행하십시오.

```bash
cargo install --path .            # → ~/.cargo/bin/hippo
# 또는
mkdir -p ~/.local/bin && cp target/release/hippo ~/.local/bin/hippo
```

아래 예시는 `~/.local/bin/hippo`를 씁니다. 실제 바이너리 경로로 바꿔서 쓰십시오.

### 3.3 Ollama와 임베딩 모델

```bash
brew install ollama
ollama pull qwen3-embedding:0.6b   # 처음 한 번은 서버가 떠 있어야 합니다: ollama serve (또는 brew services start ollama)
```

Ollama를 계속 띄워 둘 필요는 없습니다. `hippo`가 서버에 연결하지 못하면 데몬이 `ollama serve`를 직접 띄웁니다(`[embedder] autostart = true`). 원하면 서비스로 실행해도 됩니다: `brew services start ollama`. 모델은 `hippo`가 대신 내려받지 않습니다.

Ollama는 기본으로 `127.0.0.1:11434`에서만 요청을 받습니다. `hippo`는 localhost가 아닌 임베더 URL을 거부합니다.

### 3.4 데몬 실행 (선택)

이 단계는 건너뛰어도 됩니다. 데몬이 없으면 hook과 CLI 명령이 자동으로 띄우고(`[daemon] autostart = true`), 30분 동안 요청이 없으면 데몬이 스스로 내려갑니다. 동작을 지켜보고 싶거나 항상 띄워 두고 싶을 때만 직접 실행하십시오.

방법 A: 터미널에서 실행합니다. 처음 시험해 볼 때 적합합니다.

```bash
~/.local/bin/hippo daemon
```

방법 B: launchd에 등록해 로그인할 때 자동으로 시작합니다.

```bash
mkdir -p ~/.hippo/logs && chmod 700 ~/.hippo ~/.hippo/logs
~/.local/bin/hippo launchd-plist > ~/Library/LaunchAgents/com.hippo.daemon.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.hippo.daemon.plist
```

launchd는 데몬이 비정상 종료했을 때만 다시 띄우고, 유휴 종료 뒤에는 다시 띄우지 않습니다. 멈출 때는 `launchctl bootout gui/$(id -u)/com.hippo.daemon`을 실행합니다.

데몬 상태 확인:

```bash
~/.local/bin/hippo status
```

### 3.5 hook 등록

`hippo install`은 Claude Code 설정 파일에 hook을 병합합니다. `hippo` hook 항목만 바꾸고, 다른 hook과 설정은 그대로 둡니다. **먼저 미리보기로 확인하십시오.**

```bash
# 특정 프로젝트만: <프로젝트>/.claude/settings.local.json (개인 파일, git에 올라가지 않음)
~/.local/bin/hippo install --scope project --project ~/work/my-app --dry-run
~/.local/bin/hippo install --scope project --project ~/work/my-app

# 모든 세션: ~/.claude/settings.json
~/.local/bin/hippo install --scope user --dry-run
~/.local/bin/hippo install --scope user
```

`install`이 하는 일:

- 변경 내용(diff)을 출력합니다. `--dry-run`이면 아무것도 쓰지 않습니다.
- 쓰기 전에 원본을 `<파일>.hippo-backup-<유닉스 시각>`으로 백업합니다.
- 원자적으로 교체하고, 파일 권한을 유지합니다.
- 다시 실행해도 결과가 같으면 아무것도 바꾸지 않습니다("변경 없음").
- 심볼릭 링크를 거쳐서는 쓰지 않고, JSON이 깨진 설정 파일은 건드리지 않습니다.
- 파일은 2칸 들여쓰기 JSON으로 다시 쓰입니다. 키 순서와 값은 유지되지만, 원래 형식은 달라질 수 있습니다. 원본은 백업에 남습니다.

hook 블록만 출력해 직접 넣을 수도 있습니다: `hippo hooks-config`.

hook을 등록한 뒤에는 Claude Code 세션을 새로 시작하십시오.

### 3.6 제거

```bash
~/.local/bin/hippo uninstall --scope user          # 또는 --scope project --project <경로>
launchctl bootout gui/$(id -u)/com.hippo.daemon
rm ~/Library/LaunchAgents/com.hippo.daemon.plist
rm -rf ~/.hippo                          # 저장된 기억이 모두 지워집니다
```

---

## 4. 사용법

설치가 끝나면 수집과 자동 주입은 알아서 동작합니다. 필요하면 아래 명령을 씁니다.

### 4.1 검색

```bash
hippo search "토큰 만료 버그"                           # 현재 디렉터리 프로젝트의 반복
hippo search "validateToken" --scope all --kind both    # 모든 프로젝트의 반복과 결과
hippo search "배포" --scope ~/work/my-app --limit 5
```

| 옵션 | 값 | 기본값 |
|------|----|--------|
| `--scope` | `cwd`, `all`, 또는 프로젝트 경로 | `cwd` |
| `--kind` | `iteration`, `result`, `both` | `iteration` |
| `--limit` | 숫자 | `10` |

검색 결과에는 RRF 점수, 벡터 유사도(`sim=-`이면 키워드로만 찾은 결과), `prompt_id`, 미리보기가 나옵니다. 결과(도구 출력)를 찾은 경우에는 그 결과를 만든 프롬프트 목록도 함께 나옵니다.

### 4.2 한 턴 전체 보기

```bash
hippo show <prompt_id>                  # 해당 프롬프트의 반복을 순서대로
hippo show <prompt_id> --with-results   # 도구 출력 원문까지
```

백그라운드 작업의 완료 기록도 함께 나옵니다.

### 4.3 상태

```bash
hippo status
```

```
iterations: 1520
results: 980
embed_jobs: 0          # 임베딩을 기다리는 반복과 결과
spool_pending: 0       # 데몬이 꺼져 있을 때 쌓인 요청
vectors: 2610
embed_model: qwen3-embedding:0.6b
compatibility: compatible   # empty | compatible | mismatch
reindex: idle               # idle | running | done | failed: <사유>
```

재색인이 필요하면 `status`가 알려 줍니다.

### 4.4 Claude가 기억을 쓰게 하기

자동 주입은 가장 잘 맞는 몇 개만 전달합니다. 더 찾아봐야 할 때는 Claude가 CLI를 직접 호출할 수 있습니다. `CLAUDE.md`에 다음과 같은 줄을 넣어 두면 됩니다.

```
과거 세션을 찾아봐야 하면 Bash로 `hippo search "<질의>"` 또는 `hippo show <prompt_id>`를 실행한다.
```

---

## 5. 설정

파일: `~/.hippo/config.toml`. 모든 키는 생략할 수 있습니다. 알 수 없는 키는 오류로 처리하므로 오타를 바로 알 수 있습니다. 설정을 바꾼 뒤에는 데몬을 다시 시작하십시오.

```toml
[embedder]
backend = "ollama"                 # ollama | hash (테스트용)
model = "qwen3-embedding:0.6b"
dimension = 1024                   # Qwen3-Embedding은 32~4096 지원
max_input_tokens = 32000
url = "http://127.0.0.1:11434"     # localhost, 127.0.0.1, ::1만 허용
keep_alive = "30m"                 # 모델을 메모리에 유지해 첫 호출의 약 1초 지연을 피함
autostart = true                   # 서버에 연결할 수 없으면 `ollama serve`를 띄움
ollama_command = "ollama"          # ollama 실행 파일 경로

[vector]
backend = "lancedb"                # lancedb | memory (테스트용)
name = "default"                   # 디렉터리: vectors/<name>/

[chunking]
max_tokens = 8000                  # 청크 한도 = min(모델 한도, 이 값) × chars_per_token
chars_per_token = 1.0

[embed_text]
max_tool_input_chars = 500         # 임베딩 텍스트에서만 도구 입력 JSON을 이 길이로 자름

[inject]
threshold = 0.40                   # 자동 주입의 최소 벡터 유사도
top_k = 3
per_item_chars = 800
total_chars = 4000
budget_ms = 800                    # UserPromptSubmit hook의 시간 예산

[index]
vector_index_threshold = 100000    # 벡터 수가 이 값에 이르면 ANN 인덱스를 만듦
job_batch = 64

[daemon]
autostart = true                   # 데몬이 없으면 hook과 CLI가 띄움
idle_timeout_secs = 1800           # 요청 없이 이 시간(초)이 지나면 종료 (0이면 종료하지 않음)

[[redact.patterns]]                # 추가 비밀값 패턴 (정규식)
kind = "internal"
pattern = 'HCI-\d{6}'
```

**`inject.threshold`는 확정값이 아니라 조정용 값입니다.** `qwen3-embedding:0.6b`로 작게 시험했을 때 관련 기억은 0.46~0.68, 무관한 기억은 0.21~0.30이었습니다. 실제 데이터를 보며 조정하십시오.

---

## 6. 임베딩 모델이나 벡터 DB 바꾸기

벡터 저장소는 파생 인덱스이므로 언제든 바꿀 수 있습니다.

```
1. config.toml 수정 (예: 새 모델, 차원, 백엔드)
2. 데몬 재시작     → status가 "mismatch"를 표시합니다. 키워드 검색은 계속 동작합니다
3. hippo reindex   → 백그라운드에서 새 세대에 전체를 다시 색인합니다
4. 완료되면        → ACTIVE 포인터가 원자적으로 바뀝니다. 이전 세대는 다음 시작 때 지워집니다
```

- 재색인 중에는 기존 저장소로 검색합니다. 그동안 새로 들어온 반복은 큐에서 기다렸다가 전환 후 새 저장소에 들어가므로, 유실되지 않습니다.
- 재색인 중에 `hippo reindex`를 다시 실행하면 거부합니다.
- 벡터 디렉터리: `vectors/<이름>/<세대>/`. `vectors/<이름>/ACTIVE`가 사용 중인 세대를 가리킵니다.

완전히 새로운 백엔드를 추가하는 방법은 [9.4](#94-새-백엔드-추가하기)를 참고하십시오.

---

## 7. 데이터 위치와 보안

```
~/.hippo/            (0700)
├─ config.toml
├─ records.sqlite             (0600) 기준 데이터
├─ vectors/<이름>/ACTIVE
├─ vectors/<이름>/<세대>/      LanceDB 파일
├─ spool/                     데몬이 꺼져 있을 때 쌓인 요청
├─ logs/hippo.log             데몬 오류 로그
├─ logs/daemon.err            자동으로 띄운 데몬의 시작 오류
├─ hippo.sock                  (0600) 데몬 소켓
└─ hippo.lock                  단일 실행 잠금
```

보안 조치:

- **데이터가 컴퓨터 밖으로 나가지 않습니다.** 임베더 URL은 URL 파서로 해석합니다. localhost이어야 하고, userinfo·쿼리·프래그먼트가 없어야 하며, HTTP 리다이렉트는 따르지 않습니다.
- **저장 전에 비밀값을 가립니다.** 대상은 API 키(`sk-`, `ghp_`, `xox?-`, `AKIA`), `Bearer` 토큰, PEM 개인 키(END 마커가 없어도 처리), `*KEY=`/`*TOKEN=`/`*SECRET=`/`*PASSWORD=` 값입니다. `[[redact.patterns]]`로 패턴을 추가할 수 있습니다. 패턴 기반이므로 형식이 특이한 비밀값은 놓칠 수 있습니다.
- **백그라운드 출력은 안전할 때만 읽습니다.** 알림이 알려진 시작 호출과 작업 id에 맞아야 합니다. 경로도 정해진 배치(`…/<session>/tasks/<id>.output`)여야 하고, 심볼릭 링크를 거치면 안 됩니다. 그래서 프롬프트에 위조 알림을 넣어도 임의 파일을 읽게 만들 수 없습니다.
- **주입되는 기억은 데이터로 표시합니다.** 각 항목을 `<memory>` 블록에 넣고 마크업 문자를 이스케이프합니다. 머리말에서 블록 안의 지시를 따르지 말라고 알립니다. 저장된 프롬프트 인젝션의 위험을 낮추지만 완전히 없애지는 못합니다.
- **터미널 안전**: CLI 출력의 제어 문자를 이스케이프합니다.

---

## 8. 문제 해결

| 증상 | 확인할 것 |
|------|-----------|
| `hippo: daemon not reachable` | 데몬을 띄우지 못했습니다. `~/.hippo/logs/daemon.err`를 확인하십시오(예: 설정 오류). `autostart = false`이면 직접 띄워야 합니다 |
| `embed_jobs`가 줄지 않음 | 모델을 받았는지(`ollama list`), `hippo`가 Ollama를 띄울 수 있었는지(`~/.hippo/logs/ollama.log`, `hippo.log`) 확인합니다. 작업은 5초마다 다시 시도합니다 |
| 검색 결과에 "벡터 검색을 쓰지 못해 키워드 검색 결과만 표시합니다"가 나옴 | 임베딩에 실패했습니다(Ollama를 띄우지 못했거나 모델이 없음). 복구될 때까지 키워드 검색 결과만 나옵니다 |
| `compatibility: mismatch` | 모델이나 차원이 바뀌었습니다. `hippo reindex` 실행 |
| 자동 주입이 되지 않음 | `[inject] threshold`를 낮춥니다. 주입은 같은 프로젝트의 다른 세션만 대상입니다 |
| `lance-*`나 `protoc`에서 빌드 실패 | `brew reinstall protobuf` 후 `protoc --version` 확인 |
| `refusing to modify through a symbolic link` | 설정 파일이 심볼릭 링크입니다. 실제 파일 경로에 `install`을 실행하거나, `hippo hooks-config` 출력을 직접 넣으십시오 |
| `spool_pending`이 0보다 큼 | 데몬이 꺼져 있던 동안의 요청입니다. 데몬이 시작되면 처리됩니다 |

---

## 9. 개발: 수정과 빌드

### 9.1 스펙 먼저

`docs/SRS.md`(ISO/IEC/IEEE 29148)가 단일 기준입니다. 요구사항마다 ID(예: `FR-EXT-7`)가 있고, 코드 주석이 이 ID를 가리킵니다. 작업 순서는 **SRS 수정 → 실패하는 테스트 작성 → 구현 → 전체 테스트 통과**입니다. SRS와 코드가 다르면 SRS부터 고칩니다.

### 9.2 소스 구성

| 모듈 | 역할 |
|------|------|
| `transcript.rs` | transcript JSONL을 반복으로 해석, 안전 offset |
| `bg.rs` | 백그라운드 시작 감지, `<task-notification>` 해석, 서브에이전트 보고 추출 |
| `redact.rs` | 비밀값 마스킹 |
| `embed_text.rs` | 임베딩할 텍스트(`[입력]`/`[툴]`/`[응답]` 줄) 생성, 청크 분할 |
| `store.rs` | SQLite 원본 저장소, 중복 제거, FTS5 |
| `embedder.rs`, `vector.rs` | `Embedder`, `VectorStore` 트레이트 |
| `ollama.rs`, `lance.rs` | 실제 백엔드 |
| `testing.rs` | `HashEmbedder`, `MemoryVectorStore` (결정론적 테스트용 백엔드) |
| `backends.rs` | 설정에 따라 백엔드 생성. **백엔드를 추가할 때 바꾸는 유일한 파일** |
| `indexer.rs` | 임베딩 큐 처리, 재색인, 호환성 판정 |
| `generations.rs` | 벡터 저장소 세대와 `ACTIVE` 포인터 |
| `ingest.rs` | transcript → 마스킹 → 저장 파이프라인 |
| `search.rs` | 하이브리드 검색, 자동 주입, `search`/`show` 출력 |
| `daemon.rs`, `client.rs`, `protocol.rs`, `spool.rs` | 데몬, hook 클라이언트, 소켓 프로토콜, 스풀 |
| `install.rs`, `setup.rs` | 설정 파일 병합, hook JSON, launchd plist |
| `config.rs` | `config.toml`과 데이터 경로 |

### 9.3 빌드와 테스트

```bash
cargo build                    # debug
cargo build --release          # release (LanceDB 때문에 처음에는 약 4분)
cargo test                     # 결정론적 테스트 전체
cargo test -- --ignored        # 로컬 Ollama를 쓰는 실측 테스트
cargo clippy --all-targets     # 경고가 없어야 함
cargo fmt
```

`tests/fixtures/`의 fixture는 실제 transcript에서 `user`와 `assistant` 줄만 남긴 것입니다. 새로 만들려면 설정 파일을 건드리지 않고 hook을 기록한 뒤(`claude -p --settings probe/settings.json ...`), `python3 probe/make_fixtures.py tests/fixtures`를 실행합니다(스크립트의 세션 id를 고쳐서 사용).

### 9.4 새 백엔드 추가하기

1. `Embedder`(`src/embedder.rs`)나 `VectorStore`(`src/vector.rs`)를 구현합니다.
2. `tests/common/contract.rs`의 공통 계약 테스트(`embedder_contract` 또는 `vector_store_contract`)를 새 백엔드에 돌립니다. `tests/lance_test.rs`가 예시입니다.
3. `src/backends.rs`(`build_embedder` 또는 `vector_source`)에 이름을 추가합니다.
4. `config.toml`에서 선택하고, 데몬을 재시작한 뒤 `hippo reindex`를 실행합니다.

수집, 검색, 데몬 코드는 바꾸지 않아도 됩니다.

---

## 10. 알려진 한계

- 자동 주입의 품질은 `inject.threshold`에 달려 있습니다. 실제 데이터로 조정해야 합니다(SRS OPEN-5).
- "버그"처럼 2자인 한국어 단어는 키워드 검색으로 찾을 수 없습니다(FTS5 trigram은 3자 이상 필요). 이런 단어는 벡터 검색이 담당합니다.
- 대화형 세션에서 아직 확인하지 못한 것: 백그라운드 알림 뒤 hook 실행 여부(OPEN-1), Esc 중단(OPEN-2), compaction(OPEN-3), `Stop` 실행 시점에 마지막 줄이 기록되어 있는지(OPEN-6).
- hook은 항상 기본 데이터 루트(`~/.hippo`)를 씁니다. `--root`는 hook 명령에 들어가지 않습니다.
- 데몬이 시그널로 종료되면 소켓 파일이 남습니다. 다음 시작 때 지워집니다.
- 아직 보존 기간 정책이 없습니다. 직접 지우기 전까지 기억이 계속 쌓입니다.
- 지금은 macOS만 지원합니다.
