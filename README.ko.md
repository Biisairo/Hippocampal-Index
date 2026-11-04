<p align="center"><img src="assets/banner.png" alt="Hippocampal Index" width="720"></p>

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
11. [라이선스](#11-라이선스)

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
- **사용자의 답**: `AskUserQuestion`처럼 사용자에게 묻는 도구는 사용자가 고른 답(메모가 있으면 메모도)을 반복 기록에 `[answer]`로 넣습니다. 질문은 이미 도구 호출에 있으므로 답만 넣습니다. 그래서 "무엇을 정했는가"가 검색되고 자동 주입에도 보입니다

`hippo`를 실행한 도구 호출(`hippo search` 등)은 원본에는 저장하지만 검색 색인에는 넣지 않습니다. Bash 명령을 bash 문법으로 파싱해 실제로 실행되는 명령으로 판정하므로, `cd x && hippo …`, `H=…/hippo; $H …`, `$(hippo …)`도 잡고 `cargo test -p hippo`처럼 인자로만 쓴 경우는 잡지 않습니다. 기억을 찾던 장면이 다음 검색에 다시 나오지 않게 하기 위해서입니다. `hippo show`로 턴을 볼 때는 그대로 보입니다.

### 1.2 반복을 수집하는 방법 (hook)

| 반복 종류 | 수집을 일으키는 hook |
|-----------|----------------------|
| 도구를 호출한 반복 | `PostToolBatch` (병렬 호출이 모두 끝난 뒤 batch마다 한 번) |
| 마지막 반복 (텍스트만) | `Stop` / `StopFailure` |
| 세션 종료 시 남은 것 | `SessionEnd` |

이 hook들은 `async: true`로 실행되므로 세션을 느리게 만들지 않습니다. hook 입력은 수집 신호로만 쓰고, 실제 내용은 세션 transcript(JSONL)에서 읽습니다. `hippo`는 세션마다 안전 offset을 기억해 두므로, 같은 줄을 두 번 읽거나 같은 반복을 두 번 저장하지 않습니다.

**실시간 수집은 hook이 알려 준 턴부터 시작합니다.** `hippo`가 처음 보는 세션은 transcript 처음부터 읽지 않습니다. hook 입력의 `prompt_id`가 가리키는 턴의 첫 줄부터 읽습니다. 그래서 다음 세션은 그 턴 앞의 대화를 저장하지 않습니다.

- hook을 설치할 때 이미 열려 있던 세션
- hippo를 설치하기 전에 시작한 세션을 `claude --resume`이나 `--continue`로 다시 연 경우
- `--fork-session` 등으로 분기한 세션. 분기할 때 이전 대화가 새 파일로 복사되기 때문입니다.

이미 수집 중인 세션을 다시 열면, 같은 파일에 이어 쓰므로 저장한 위치 뒤부터 계속 읽습니다. 그 앞의 과거 기록은 `hippo import`로 가져옵니다([4.4](#44-프로젝트와-과거-기록-가져오기) 참고).

그 밖의 처리 방식:

- **서브에이전트**: 내부 반복은 저장하지 않습니다. 포그라운드 서브에이전트의 최종 보고는 `Agent` 도구의 결과로 돌아오므로, 호출과 함께 저장됩니다.
- **백그라운드 작업**(Bash 또는 Agent의 `run_in_background`): 시작 호출을 *대기 중*으로 저장합니다. transcript에 완료 알림(`<task-notification>`)이 나타나면 출력을 읽어 시작 호출에 연결합니다. 그래서 호출과 응답이 한 쌍으로 남습니다.

### 1.3 저장 구조

기억은 **프로젝트마다 따로** 저장합니다. 다른 프로젝트의 기록은 검색과 자동 주입에 섞이지 않습니다.

- **프로젝트를 정하는 시점**: 세션을 처음 처리할 때 **세션 시작 경로**(transcript에서 `cwd`가 있는 첫 줄)로 정합니다. 세션 중에 `cd`해도 바뀌지 않습니다.
- **프로젝트의 위치**: git 저장소면 git common dir(`.git`)이고, 아니면 그 폴더입니다. 하위 디렉터리에서 연 세션과 git worktree는 원본 저장소와 같은 프로젝트가 됩니다.
- **폴더를 옮긴 경우**: git 저장소는 루트 커밋이 같고 이전 위치가 사라졌으면 자동으로 이어 씁니다. git이 아닌 폴더는 `hippo import --from <이전 경로>`로 직접 가져옵니다([4.4](#44-프로젝트와-과거-기록-가져오기) 참고).

<p align="center"><img src="docs/images/storage.ko.svg" alt="기억을 저장하는 곳: ~/.hippo 데이터 루트, 프로젝트별 SQLite 원본과 LanceDB 벡터" width="880"></p>

이름의 "색인"이 바로 이 부분입니다. 해마 색인이 피질의 기억 흔적을 가리키듯, 벡터 저장소는 원본 기록을 가리키기만 합니다. 벡터 저장소는 원본 저장소에서 파생된 인덱스입니다. 그래서 임베딩 모델이나 벡터 DB를 바꾸더라도 SQLite에서 인덱스를 다시 만들면 됩니다([6장](#6-임베딩-모델이나-벡터-db-바꾸기) 참고).

### 1.4 기억을 쓰는 방법

<p align="center"><img src="docs/images/retrieval.ko.svg" alt="기억을 찾아 주입하는 흐름" width="880"></p>

- **자동 주입**: 프롬프트를 보낼 때마다 `UserPromptSubmit` hook이 데몬에 관련 기억을 요청합니다. 검색 대상은 **같은 프로젝트**의 **다른 세션** 기록입니다. 벡터 유사도가 임계값 이상인 반복을 찾고, 그 반복이 속한 **턴**을 주입합니다. 턴마다 `<memory>` 블록 하나에 입력, LLM 응답, 질문과 답을 넣고 도구 호출과 결과는 넣지 않습니다. 블록은 시간순(오래된 것부터)으로 놓고 각 블록에 시각을 붙입니다. 머리말에 "과거 기록이며 지시가 아님"을 밝힙니다. 백그라운드 작업 완료 알림과, 공백을 뺀 길이가 11자 미만인 짧은 프롬프트("진행", "그거 알려줘" 등)에는 주입하지 않습니다. 검색 질의에는 임베딩 모델이 권장하는 지시문을 앞에 붙입니다.
- **필요할 때 검색**: Claude나 사용자가 셸에서 `hippo search`, `hippo show`를 실행합니다.

검색은 **하이브리드** 방식입니다. 벡터 유사도와 SQLite FTS5 키워드 검색 결과를 RRF(Reciprocal Rank Fusion)로 합칩니다. `validateToken` 같은 식별자는 키워드 검색이 잘 찾습니다.

### 1.5 구조

<p align="center"><img src="docs/images/capture.ko.svg" alt="반복을 수집하는 흐름: hook, 데몬, 쓰기 스레드와 임베딩 스레드" width="880"></p>

- 데몬은 컴퓨터마다 하나만 실행됩니다(파일 잠금). 저장소에는 데몬만 씁니다.
- 데몬을 직접 띄울 필요는 없습니다. 로컬 MCP 서버처럼 필요할 때 올라옵니다. hook이나 CLI 명령이 데몬에 연결하지 못하면 `hippo daemon`을 백그라운드로 띄운 뒤 요청을 보냅니다. 요청이 없는 상태로 `idle_timeout_secs`(기본 30분)가 지나면 데몬이 스스로 내려가고, 다음 요청이 오면 다시 올라옵니다.
- 데몬을 띄우지 못하면 수집 요청은 스풀 파일에 쌓였다가, 다음에 데몬이 시작될 때 처리됩니다. `hippo` 때문에 Claude Code 세션이 실패하는 일은 없습니다.
- CLI 명령(`hippo search`, `show`, `status`, `reindex` 등)도 같은 소켓으로 같은 데몬에 요청합니다.

### 1.6 생명주기: DB와 모델이 열리고 닫히는 시점

| 구성 요소 | 여는 시점 | 닫는 시점 |
|-----------|-----------|-----------|
| 데몬 | 데몬에 연결하지 못한 첫 hook이나 CLI 명령이 자동으로 띄움 | 요청 없이 `idle_timeout_secs`가 지났을 때(재색인 중에는 내려가지 않음), 또는 직접 멈출 때 |
| SQLite | 작업마다 엶 | 작업이 끝날 때 |
| LanceDB (프로젝트별) | 그 프로젝트를 처음 쓸 때 | 열어 둔 프로젝트가 `max_open_projects`(기본 8)를 넘으면 가장 오래 쓰지 않은 것부터 닫음. 데몬이 종료할 때 모두 닫음 |
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

`hippo install`은 자신을 데이터 루트의 **`~/.hippo/bin/hippo`로 복사**하고, hook 명령과 Claude 안내문에 이 경로를 씁니다. 데몬도 이 복사본으로 뜹니다. 그래서 빌드한 자리에서 바로 설치해도 되고, 그 뒤에 `cargo clean`을 하거나 저장소를 옮겨도 hook은 계속 동작합니다.

```bash
./target/release/hippo install --scope user --dry-run
```

- hook 명령과 Claude 안내문에는 경로를 `~/.hippo/bin/hippo`처럼 `~`로 적습니다. 생성된 설정 파일이 공유되어도 계정 이름이 드러나지 않습니다. (launchd plist는 셸을 거치지 않으므로 절대 경로를 씁니다.)
- 다시 빌드한 뒤에는 `install`을 다시 실행해야 새 바이너리가 복사됩니다. 내용이 같으면 복사하지 않습니다.
- 복사는 APFS 파일 복제를 쓰므로 디스크를 거의 더 쓰지 않습니다.
- 어느 설정 파일에 설치했는지는 `~/.hippo/bin/installs`에 기록됩니다. `uninstall`로 마지막 설치까지 지우면 복사본도 지웁니다.

아래 예시에서 설치 전 명령은 `./target/release/hippo`, 설치 후 명령은 `~/.hippo/bin/hippo`를 씁니다.

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
~/.hippo/bin/hippo daemon
```

방법 B: launchd에 등록해 로그인할 때 자동으로 시작합니다.

```bash
mkdir -p ~/.hippo/logs && chmod 700 ~/.hippo ~/.hippo/logs
~/.hippo/bin/hippo launchd-plist > ~/Library/LaunchAgents/com.hippo.daemon.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.hippo.daemon.plist
```

launchd는 데몬이 비정상 종료했을 때만 다시 띄우고, 유휴 종료 뒤에는 다시 띄우지 않습니다. 멈출 때는 `launchctl bootout gui/$(id -u)/com.hippo.daemon`을 실행합니다.

데몬 상태 확인:

```bash
~/.hippo/bin/hippo status
```

### 3.5 hook 등록

`hippo install`은 Claude Code 설정 파일에 hook을 병합합니다. `hippo` hook 항목만 바꾸고, 다른 hook과 설정은 그대로 둡니다. **먼저 미리보기로 확인하십시오.**

```bash
# 특정 프로젝트만: <프로젝트>/.claude/settings.local.json (개인 파일, git에 올라가지 않음)
./target/release/hippo install --scope project --project ~/work/my-app --dry-run
./target/release/hippo install --scope project --project ~/work/my-app

# 모든 세션: ~/.claude/settings.json
./target/release/hippo install --scope user --dry-run
./target/release/hippo install --scope user
```

`install`이 하는 일:

- 변경 내용(diff)을 출력합니다. `--dry-run`이면 아무것도 쓰지 않습니다.
- 쓰기 전에 원본을 `<파일>.hippo-backup-<유닉스 시각>`으로 백업합니다.
- 원자적으로 교체하고, 파일 권한을 유지합니다.
- 다시 실행해도 결과가 같으면 아무것도 바꾸지 않습니다("변경 없음").
- 심볼릭 링크를 거쳐서는 쓰지 않고, JSON이 깨진 설정 파일은 건드리지 않습니다.
- 파일은 2칸 들여쓰기 JSON으로 다시 쓰입니다. 키 순서와 값은 유지되지만, 원래 형식은 달라질 수 있습니다. 원본은 백업에 남습니다.
- Claude가 `hippo search`와 `hippo show`를 쓸 수 있다는 영어 안내 블록을 메모리 파일에 넣습니다. 명령은 설치에 쓴 hippo 실행 파일의 절대 경로로 적으므로 `hippo`가 PATH에 없어도 됩니다. `--scope user`는 `~/.claude/CLAUDE.md`, `--scope project`는 `<프로젝트>/CLAUDE.local.md`(공식 문서가 권장하는 개인용 파일)입니다. 블록은 `<!-- hippo:begin -->`과 `<!-- hippo:end -->` 사이에 있고, 이 표식은 Claude Code가 컨텍스트에 넣기 전에 지우므로 토큰을 쓰지 않습니다. 블록 밖의 내용은 그대로 둡니다. `CLAUDE.local.md`가 git에서 무시되지 않으면 경고만 출력하고, `.gitignore`는 바꾸지 않습니다.

hook 블록만 출력해 직접 넣을 수도 있습니다: `hippo hooks-config`. `uninstall`은 hook 항목과 안내 블록만 지웁니다. hippo 내용만 있던 설정 파일과 메모리 파일은 백업 없이 지우고, 그래서 비게 된 `.claude/` 디렉터리도 지웁니다. 다른 내용이 남는 파일은 백업한 뒤 고칩니다.

hook을 등록한 뒤에는 Claude Code 세션을 새로 시작하십시오.

### 3.6 제거

```bash
~/.hippo/bin/hippo uninstall --scope user          # 또는 --scope project --project <경로>
launchctl bootout gui/$(id -u)/com.hippo.daemon
rm ~/Library/LaunchAgents/com.hippo.daemon.plist
rm -rf ~/.hippo                          # 저장된 기억이 모두 지워집니다
```

---

## 4. 사용법

설치가 끝나면 수집과 자동 주입은 알아서 동작합니다. 필요하면 아래 명령을 씁니다.

### 4.1 검색

```bash
hippo search "토큰 만료 버그"                           # 현재 디렉터리가 속한 프로젝트의 반복
hippo search "validateToken" --scope all --kind both    # 모든 프로젝트의 반복과 결과
hippo search "배포" --scope ~/work/my-app --limit 5      # 그 경로가 속한 프로젝트
```

| 옵션 | 값 | 기본값 |
|------|----|--------|
| `--scope` | `project`(현재 디렉터리의 프로젝트), `all`, 또는 경로 | `project` |
| `--kind` | `iteration`, `result`, `both` | `iteration` |
| `--limit` | 숫자 | `10` |

검색 결과에는 RRF 점수, 벡터 유사도(`sim=-`이면 키워드로만 찾은 결과), `prompt_id`, 미리보기가 나옵니다. 결과(도구 출력)를 찾은 경우에는 그 결과를 만든 프롬프트 목록도 함께 나옵니다. `--scope all`은 프로젝트마다 검색한 뒤 RRF로 합치고, 결과마다 `project=<id>`를 붙입니다. 현재 디렉터리가 어느 프로젝트에도 속하지 않으면 "No project"(한국어 설정이면 "프로젝트 없음")가 나옵니다. 이 문서의 출력 예는 `[ui] language = "ko"` 기준입니다.

### 4.2 한 턴 전체 보기

```bash
hippo show <prompt_id>                  # 해당 프롬프트의 반복을 순서대로
hippo show <prompt_id> --with-results   # 도구 출력 원문까지
hippo show <prompt_id> --scope project  # 현재 프로젝트에서만 찾기
```

`--scope`가 없으면 현재 프로젝트를 먼저 찾고, 없으면 나머지 프로젝트를 최근에 쓴 순서로 찾습니다. 백그라운드 작업의 완료 기록도 함께 나옵니다.

### 4.3 상태

```bash
hippo status
```

```
spool_pending: 0                  # 데몬이 꺼져 있을 때 쌓인 요청
embed_model: qwen3-embedding:0.6b
reindex: idle                     # idle | running | done | failed: <프로젝트 id>: <사유>
projects: 2
[3f9c…] /Users/me/work/app/.git
  iterations: 1520  results: 980  embed_jobs: 0  vectors: 2610  compatibility: compatible
[a71d…] /Users/me/notes  (위치 없음)
  iterations: 40  results: 12  embed_jobs: 0  vectors: 52  compatibility: compatible
```

`embed_jobs`는 임베딩을 기다리는 반복과 결과의 수입니다. 최근 5분 동안 처리한 작업이 있으면 `embed_jobs: 601 (분당 120건, 약 6분 남음)`처럼 속도와 남은 예상 시간도 나옵니다. `compatibility`는 `empty`, `compatible`, `mismatch` 중 하나입니다. 재색인이 필요한 프로젝트가 있으면 `status`가 알려 줍니다.

### 4.4 프로젝트와 과거 기록 가져오기

```bash
hippo project list                       # id, 종류(git/dir), 위치, 반복 수, 마지막 활동 시각(로컬 시간 YYYY-MM-DD HH:MM:SS)
hippo import                             # 현재 폴더(하위 폴더와 worktree 포함)의 과거 세션을 현재 프로젝트로
hippo import --from ~/old/app            # 이전 경로에서 진행한 세션을 현재 폴더의 프로젝트로
hippo import --from <프로젝트 id> --to ~/work/app
hippo import --wait                      # 가져온 뒤 임베딩이 끝날 때까지 진행률을 보여 줌
hippo import --dry-run                   # 후보 세션과 처리 내용만 보여 주고 아무것도 바꾸지 않음
```

`import`는 Claude Code가 남긴 과거 transcript(`~/.claude/projects/*/*.jsonl`)를 읽어 `--to` 폴더의 프로젝트에 기록을 추가합니다. 생략한 인자는 현재 폴더입니다. 서브에이전트 transcript는 읽지 않습니다.

- **가져올 세션**: 세션 시작 경로가 `--from` 경로이거나 그 아래인 세션입니다. 경로가 지금 없어도 됩니다. `--from`이 git 저장소이면 그 저장소의 worktree에서 연 세션도 포함합니다. 오래된 세션부터 처리합니다.
- **`--from`의 프로젝트가 이미 있으면**: 먼저 그 프로젝트를 `--to`의 프로젝트로 옮깁니다. `--to`에 프로젝트가 없으면 위치만 바꾸고, 있으면 기록을 병합한 뒤 이전 프로젝트를 지웁니다. 병합할 때 임베딩 모델이 같으면 벡터를 복사하고, 다르면 다시 임베딩합니다. 이전 위치는 남기지 않으므로 이후 세션은 따로 연결하지 않아도 현재 위치의 프로젝트에 저장됩니다.
- **이미 실시간으로 저장 중인 세션**: 실시간 수집이 시작된 위치 앞부분만 채웁니다. 이미 있는 반복은 다시 저장하지 않습니다.
- **transcript 끝의 반복**: 도구 호출 결과가 모두 있으면 저장하고, 결과가 빠진 도구 호출이 있으면 보류합니다. 보류한 반복은 그 세션이 다시 열리거나 `import`를 다시 실행하면 이어서 처리합니다.
- **분기한 세션의 복사본**: 같은 `message.id`의 반복이 다른 세션으로 이미 있으면 건너뜁니다.
- **다른 프로젝트에 고정된 세션**: 건너뛰고 "다른 프로젝트"로 알려 줍니다.

`--dry-run`은 아무것도 바꾸지 않습니다. 먼저 `import`가 할 일(프로젝트 생성, 위치 변경, 병합)을 보여 줍니다. 그다음 후보 세션마다 한 줄씩 시작 시각, 세션 id, 가져오기 상태, 파일 크기, 시작 경로, 첫 프롬프트를 출력합니다. 첫 프롬프트는 비밀값을 가리고 앞 80자만 보여 줍니다. 가져오기 상태는 `새 세션`, `실시간만`(실시간 수집이 시작된 뒤만 저장됨), `가져옴`, `다른 프로젝트` 중 하나입니다.

진행률은 세션마다 한 줄씩 출력합니다. 아래는 `[ui] language = "ko"`일 때의 예입니다(기본은 영어).

```
후보 세션 120개
[1/120] d767e162 가져옴 반복 42  남은 임베딩 310
[2/120] 2f74fd43 가져옴 반복 3  남은 임베딩 318
...
합계: 세션 120개 중 가져옴 118, 변경 없음 2, 다른 프로젝트 0, 오류 0. 반복 2410개 저장, fork 복사본 101개 건너뜀.
```

중간에 Ctrl-C로 멈춰도 됩니다. 같은 명령을 다시 실행하면 끝난 세션은 "변경 없음"으로 지나가고 남은 세션부터 이어서 처리합니다. 남은 임베딩 작업은 데몬이 계속 처리합니다. 데몬은 실시간 수집과 그 임베딩을 가져오기 작업보다 먼저 처리하므로, 가져오는 동안에도 대화 기록은 늦지 않게 저장되고 검색됩니다.

### 4.5 Claude가 기억을 쓰게 하기

자동 주입은 가장 잘 맞는 몇 개만 전달합니다. 더 찾아봐야 할 때는 Claude가 CLI를 직접 호출할 수 있습니다. 그 방법을 알려 주는 안내 블록은 `hippo install`이 메모리 파일에 넣습니다([3.5](#35-hook-등록) 참고). 따로 적을 필요는 없습니다.

### 4.6 GUI

```bash
hippo gui
```

네이티브 창(egui)을 엽니다. 브라우저나 웹뷰는 쓰지 않습니다. 창을 닫으면 명령이 끝납니다. 명령을 실행한 폴더가 모든 요청의 현재 폴더이고, 경로 입력란의 기본값입니다.

| 탭 | 하는 일 |
|----|---------|
| 상태 | 2초마다 데몬 상태를 물어 스풀, 재색인 상태, 프로젝트별 표(임베딩 대기, 처리 속도, 남은 시간)를 보여 줍니다. 창이 열려 있는 동안 데몬은 내려가지 않습니다. |
| 기록 | 프로젝트 하나의 기록을 둘러봅니다. 세션 목록(최신순) → 그 세션의 턴 목록(시간순) → 턴 상세 순서입니다. 목록은 50개씩 나눠 보여 줍니다. 목록 위의 필터에 글자를 넣으면 그 글자가 든 반복이 있는 세션과 턴만 남깁니다(3자 이상은 전문 검색, 그보다 짧으면 단순 포함 비교). 턴마다 임베딩 상태를 `완료` 또는 `대기 n`으로 보여 줍니다. 턴 상세는 `show --with-results`와 같은 내용을 반복별로 나눠 보여 주고, 도구 출력은 접어 둡니다. 목록 위의 질의란에서는 이 프로젝트의 하이브리드 검색(결과를 누르면 그 턴을 엽니다)이나 **주입 미리 보기**를 할 수 있습니다. 주입 미리 보기는 자동 주입과 같은 설정과 규칙으로 계산하되, 세션은 제외하지 않고 시간 예산도 적용하지 않습니다. 후보마다 RRF 순위, 유사도, 판정(`선택`, `임계값 미만`, `FTS 전용`, `같은 턴`, `길이 한도 초과`, `개수 초과`)을 보여 주고, 실제로 넣을 텍스트와 걸린 시간도 함께 보여 줍니다. 주입하지 않는 경우에는 그 사유를 보여 줍니다. 단계마다 **삭제** 버튼이 있고, 누르면 [4.7](#47-기록-지우기)의 미리 보기를 보여 준 뒤 확인을 받습니다. |
| 설정 | `config.toml`의 모든 키를 설명, 기본값 여부와 함께 보여 줍니다. 저장할 때는 바꾼 키만 고치고 직접 쓴 주석은 그대로 둡니다. 기본값으로 되돌린 키는 파일에서 지웁니다. 저장 전에 결과를 검사하며, 잘못된 설정은 쓰지 않습니다. 데몬을 다시 시작해야 하면 **데몬 다시 시작** 버튼이 나타나고, 재색인이 필요한 변경이면 그것도 알려 줍니다. |
| 가져오기 | **미리 보기**는 처리 내용과 후보 세션 표를 보여 줍니다(`--dry-run`과 같음). `새 세션`과 `실시간만`인 세션을 처음에 선택해 둡니다. **선택한 세션 가져오기**는 체크한 세션만 보냅니다. **멈춤**을 누르면 지금 세션을 마친 뒤 멈춥니다. 임베딩 진행은 상태 폴링 값으로 보여 줍니다. |
| 검색 | `search`와 `show`. 반복 결과마다 **기록에서 열기**가 있어, 누르면 기록 탭에서 그 프로젝트와 턴을 엽니다. |
| 프로젝트 | `project list`와 `reindex` |
| Claude 적용 | 사용자 범위나 프로젝트 범위에 hippo가 적용되어 있는지 보여 줍니다. **적용/해제 미리 보기**는 `--dry-run`과 같은 diff를 보여 줍니다. **적용**과 **해제**는 같은 입력으로 미리 보기를 한 뒤에만 누를 수 있고, 확인을 한 번 더 받습니다. |
| 정보 | 버전, README, LICENSE |

창의 문구는 `[ui] language`를 따릅니다. 한글은 macOS 시스템 폰트로 표시합니다. 색은 기본으로 macOS의 라이트·다크 모드를 따릅니다. 한쪽으로 고정하려면 `[ui] theme`을 `light`나 `dark`로 둡니다. 설정 탭에서 테마를 저장하면 바로 바뀝니다.

### 4.7 기록 지우기

```bash
hippo forget --session <session_id>        # 세션 하나
hippo forget --prompt <prompt_id>          # 턴 하나: 프로젝트 안 모든 세션에서 그 prompt id의 반복 전부
hippo project remove <프로젝트 id 또는 경로> # 프로젝트 전체
```

명령은 먼저 지울 내용을 출력합니다. 프로젝트 id와 위치, 그리고 세션·턴·반복·결과 수입니다. 이 반복과 결과의 벡터는 청크 수와 관계없이 모두 지웁니다. 그다음 `지울까요? [y/N]`를 묻고, `y`를 입력했을 때만 지웁니다.

- `--dry-run`을 주면 미리 보기만 출력하고 끝납니다.
- `--yes`를 주면 묻지 않고 지웁니다. 표준 입력이 터미널이 아니고(스크립트, 파이프) `--yes`도 없으면 아무것도 지우지 않고 종료 코드 1로 끝납니다.
- `forget`은 `--scope project|<경로>`를 받습니다. 생략하면 `show`처럼 현재 프로젝트에서 먼저 찾고, 없으면 나머지 프로젝트에서 찾습니다. 맞는 대상이 없으면 `대상 없음`을 출력합니다.
- 결과(도구 출력)는 남은 기록 중 어디에서도 참조하지 않을 때만 지웁니다. 같은 출력을 여러 세션이 함께 쓰는 경우가 있기 때문입니다.
- 세션을 지우면 그 세션을 어디까지 읽었는지도 함께 지웁니다. 아직 열려 있는 세션이면 다음 hook부터 그 턴을 기준으로 다시 저장합니다. 과거 세션도 다시 가져오면 돌아옵니다. 지운 기록은 따로 남기지 않으며, 되살릴 수 없습니다.
- `project remove` 뒤에 같은 폴더에서 새 세션을 열면 새 프로젝트로 쌓입니다.
- 재색인 중에는 지우지 않고 거부합니다. 임베딩 묶음을 처리하는 중이면 그 묶음이 끝날 때까지 기다린 뒤 지웁니다. 긴 도구 출력이 많은 묶음은 수십 초가 걸릴 수 있습니다.

---

## 5. 설정

파일: `~/.hippo/config.toml`. 모든 키는 생략할 수 있습니다. 알 수 없는 키는 오류로 처리하므로 오타를 바로 알 수 있습니다. 설정을 바꾼 뒤에는 데몬을 다시 시작하십시오. `[ui] language`와 `[ui] theme`은 다시 시작하지 않아도 바로 적용됩니다. `hippo gui`의 설정 탭([4.6](#46-gui))에서도 고칠 수 있고, 그 탭에서 데몬을 다시 시작할 수도 있습니다.

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
query_prefix = "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:"
                                   # 검색 질의 앞에만 붙이는 지시문(Qwen3-Embedding 권장값). 비우면 붙이지 않음. 재색인 불필요

[vector]
backend = "lancedb"                # lancedb | memory (테스트용)
name = "default"                   # 디렉터리: projects/<id>/vectors/<name>/

[chunking]
max_tokens = 1000                  # 청크 한도 = min(모델 한도, 이 값) × chars_per_token
chars_per_token = 1.0
overlap_chars = 200                # 이웃 청크가 공유하는 문자 수. 청크 한도보다 작아야 함

[embed_text]
max_tool_input_chars = 500         # 임베딩 텍스트에서만 도구 입력 JSON을 이 길이로 자름 ([answer]는 자르지 않음)
answer_tools = ["AskUserQuestion", "ExitPlanMode"]   # 결과(사용자의 답)를 [answer]로 넣는 도구

[inject]
threshold = 0.50                   # 자동 주입의 최소 벡터 유사도
top_k = 3                          # 주입할 턴 수
total_chars = 12000                # 주입 전체 길이 한도. 넘는 턴은 자르지 않고 통째로 뺌
budget_ms = 800                    # UserPromptSubmit hook의 시간 예산
cold_budget_ms = 2500              # hook이 데몬을 새로 띄울 때(콜드 시작)의 예산. budget_ms 이상
min_prompt_chars = 11              # 앞뒤 공백을 뺀 길이가 이보다 짧은 프롬프트는 주입하지 않음

[index]
vector_index_threshold = 100000    # 벡터 수가 이 값에 이르면 ANN 인덱스를 만듦
job_batch = 64
optimize_every = 20                 # 벡터 저장소를 이만큼 수정하면 작은 파일을 합치고 이전 버전을 지움(0이면 큐가 빌 때만)

[daemon]
autostart = true                   # 데몬이 없으면 hook과 CLI가 띄움
idle_timeout_secs = 1800           # 요청 없이 이 시간(초)이 지나면 종료 (0이면 종료하지 않음)
max_open_projects = 8              # 동시에 열어 둘 프로젝트 벡터 저장소 수

[project]
git_timeout_ms = 1000              # git 실행 시간 제한. 넘으면 git이 아닌 폴더로 봄

[import]
transcripts_dir = "/Users/me/.claude/projects"   # 과거 transcript를 찾는 디렉터리. 기본값은 $HOME/.claude/projects (`~`는 펼치지 않음)

[ui]
language = "en"                    # CLI 출력 언어: en(기본) 또는 ko. Claude에게 주는 텍스트와 --help는 항상 영어
theme = "system"                   # GUI 테마: system(기본, macOS 설정을 따름), light, dark

[[redact.patterns]]                # 추가 비밀값 패턴 (정규식)
kind = "internal"
pattern = 'HCI-\d{6}'
```

**`inject.threshold`, `min_prompt_chars`, `query_prefix`의 기본값은 실측으로 정했습니다.** 이 프로젝트의 과거 기록으로 만든 평가셋(정답 질문 60개, 기억이 필요 없는 프롬프트 30개)에서, 질의 지시문 없이 0.40을 쓸 때보다 정답 포함률은 그대로(53/60)이고 잘못된 주입은 28/30에서 16/30으로 줄었습니다(SRS 부록 C). 잘못된 주입을 더 줄이려면 `threshold`를 0.55~0.60으로 올리십시오. 대신 필요한 기억도 일부 빠집니다. 임베딩 모델을 바꾸면 `query_prefix`를 그 모델이 권장하는 질의 형식으로 바꾸십시오.

---

## 6. 임베딩 모델이나 벡터 DB 바꾸기

벡터 저장소는 파생 인덱스이므로 언제든 바꿀 수 있습니다.

```
1. config.toml 수정 (예: 새 모델, 차원, 백엔드)
2. 데몬 재시작     → status가 "mismatch"를 표시합니다. 키워드 검색은 계속 동작합니다
3. hippo reindex   → 백그라운드에서 모든 프로젝트를 차례로 새 세대에 다시 색인합니다
4. 완료되면        → 프로젝트마다 ACTIVE 포인터가 원자적으로 바뀝니다. 이전 세대는 그 프로젝트를 다음에 열 때 지워집니다
```

한 프로젝트만 다시 색인하려면 `hippo reindex --scope project`(현재 디렉터리의 프로젝트)나 `hippo reindex --scope <경로>`를 씁니다.

- 재색인 중에는 기존 저장소로 검색합니다. 그동안 새로 들어온 반복은 큐에서 기다렸다가 전환 후 새 저장소에 들어가므로, 유실되지 않습니다.
- 재색인 중에 `hippo reindex`를 다시 실행하면 거부합니다.
- 한 프로젝트의 재색인이 실패해도 다음 프로젝트를 계속 처리합니다. 실패한 프로젝트는 `status`의 `reindex: failed: …`에 나옵니다.
- 벡터 디렉터리: `projects/<id>/vectors/<이름>/<세대>/`. `projects/<id>/vectors/<이름>/ACTIVE`가 사용 중인 세대를 가리킵니다.

완전히 새로운 백엔드를 추가하는 방법은 [9.4](#94-새-백엔드-추가하기)를 참고하십시오.

---

## 7. 데이터 위치와 보안

```
~/.hippo/            (0700)
├─ config.toml
├─ bin/hippo                  (0700) 설치한 실행 파일. hook과 안내문이 이 경로를 씀
├─ bin/installs               (0600) hook을 넣은 설정 파일 목록
├─ registry.sqlite            (0600) 목록 DB: 프로젝트, 위치, 루트 커밋, 세션 → 프로젝트
├─ projects/<id>/             (0700) 프로젝트 하나
│  ├─ records.sqlite          (0600) 기준 데이터
│  ├─ vectors/<이름>/ACTIVE
│  └─ vectors/<이름>/<세대>/   LanceDB 파일
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
| 검색 결과에 "vector search unavailable, showing keyword results only"(한국어 설정이면 "벡터 검색을 쓰지 못해 키워드 검색 결과만 표시합니다")가 나옴 | 임베딩에 실패했습니다(Ollama를 띄우지 못했거나 모델이 없음). 복구될 때까지 키워드 검색 결과만 나옵니다 |
| `compatibility: mismatch` | 모델이나 차원이 바뀌었습니다. `hippo reindex` 실행 |
| 자동 주입이 되지 않음 | `[inject] threshold`를 낮춥니다. 주입은 같은 프로젝트의 다른 세션만 대상입니다 |
| `lance-*`나 `protoc`에서 빌드 실패 | `brew reinstall protobuf` 후 `protoc --version` 확인 |
| `refusing to modify through a symbolic link` | 설정 파일이 심볼릭 링크입니다. 실제 파일 경로에 `install`을 실행하거나, `hippo hooks-config` 출력을 직접 넣으십시오 |
| `spool_pending`이 0보다 큼 | 데몬이 꺼져 있던 동안의 요청입니다. 데몬이 시작되면 처리됩니다 |
| 0.14 이하 버전에서 올린 뒤 `vectors/`가 매우 큼(수십 GB) | 0.14 이하 버전은 LanceDB의 버전을 모두 남겼습니다. 업그레이드 뒤 첫 정리에서 지워지지만, 몇 분 걸릴 수 있고 그동안 그 프로젝트의 검색이 기다립니다. 바로 비우려면 `hippo reindex`를 실행하십시오 |

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
| `embed_text.rs` | 임베딩할 텍스트(`[input]`/`[tool]`/`[response]` 줄) 생성, 청크 분할 |
| `shell.rs` | Bash 명령을 파싱(brush-parser)해 hippo 실행 여부 판정 |
| `store.rs` | SQLite 원본 저장소, 중복 제거, FTS5, 프로젝트 병합 |
| `project.rs` | 경로 정규화, git 정보, 프로젝트 위치 |
| `registry.rs` | 목록 DB: 프로젝트 판정, 세션 고정, 위치 변경, 흡수 |
| `import.rs` | 과거 기록 가져오기의 후보 세션 선택과 미리 보기 |
| `queue.rs` | 실시간 작업을 가져오기 작업보다 먼저 꺼내는 두 단계 큐 |
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
| `config_edit.rs` | GUI 설정 편집: 키 목록, 바꾼 키만 고치는 저장(`toml_edit`), 재시작·재색인 필요 판정 |
| `commands.rs` | CLI와 GUI가 함께 쓰는 동작: 설치, 가져오기 결과 줄, 데몬 요청 |
| `gui/` | `hippo gui`: `state.rs`(화면과 분리한 로직, 테스트 대상), `app.rs`(egui 화면), `text.rs`(키 설명), `theme.rs`(라이트·다크 테마), `images.rs`(아이콘, 로고, 캐릭터) |

### 9.3 빌드와 테스트

```bash
cargo build                    # debug
cargo build --release          # release (LanceDB 때문에 처음에는 약 4분)
cargo test                     # 결정론적 테스트 전체
cargo test -- --ignored        # 로컬 Ollama를 쓰는 실측 테스트
cargo clippy --all-targets     # 경고가 없어야 함
cargo fmt
```

`tests/fixtures/`의 fixture는 실제 transcript에서 `user`와 `assistant` 줄만 남긴 것입니다. `tests/fixtures/hooks/`에는 실측한 hook 입력이 있습니다. 새로 만들 때는 `~/.claude` 설정을 건드리지 않습니다. 이벤트마다 hook 입력을 파일로 기록하는 격리 설정 파일을 따로 만들고, `claude -p --settings <그 파일>`로 실행합니다. 그다음 transcript에서 `user`와 `assistant` 줄만 남기고, 로컬 경로를 `/Users/user`처럼 바꿉니다.

### 9.4 새 백엔드 추가하기

1. `Embedder`(`src/embedder.rs`)나 `VectorStore`(`src/vector.rs`)를 구현합니다.
2. `tests/common/contract.rs`의 공통 계약 테스트(`embedder_contract` 또는 `vector_store_contract`)를 새 백엔드에 돌립니다. `tests/lance_test.rs`가 예시입니다.
3. `src/backends.rs`(`build_embedder` 또는 `vector_source`)에 이름을 추가합니다.
4. `config.toml`에서 선택하고, 데몬을 재시작한 뒤 `hippo reindex`를 실행합니다.

수집, 검색, 데몬 코드는 바꾸지 않아도 됩니다.

---

## 10. 알려진 한계

- 자동 주입의 품질은 `inject.threshold`에 달려 있습니다. "방금 주입됐어?"처럼 과거에 같은 말을 한 적이 있는 메타 질문에는 관련 없는 기억이 들어올 수 있습니다(SRS 부록 C).
- "버그"처럼 2자인 한국어 단어는 키워드 검색으로 찾을 수 없습니다(FTS5 trigram은 3자 이상 필요). 이런 단어는 벡터 검색이 담당합니다.
- 대화형 세션에서 아직 확인하지 못한 것: 백그라운드 알림 뒤 hook 실행 여부(OPEN-1), Esc 중단(OPEN-2), compaction(OPEN-3), `Stop` 실행 시점에 마지막 줄이 기록되어 있는지(OPEN-6).
- hook은 항상 기본 데이터 루트(`~/.hippo`)를 씁니다. `--root`는 hook 명령에 들어가지 않습니다.
- 데몬이 시그널로 종료되면 소켓 파일이 남습니다. 다음 시작 때 지워집니다.
- 아직 보존 기간 정책이 없습니다. `forget`이나 `project remove`로 지우기 전까지 기억이 계속 쌓입니다([4.7](#47-기록-지우기) 참고).
- 결과(도구 출력)의 중복 제거는 프로젝트 안에서만 합니다. 같은 출력이 두 프로젝트에 있으면 각각 저장합니다.
- git이 아닌 폴더를 옮기면 자동으로 이어지지 않습니다. `hippo import --from <이전 경로>`로 가져와야 합니다.
- 실시간 수집은 hook이 알려 준 턴부터 저장합니다. 그 앞의 기록은 `hippo import`로 가져와야 합니다.
- 이전 경로가 사라진 과거 세션은 어느 프로젝트의 것인지 자동으로 알 수 없습니다. `--from`으로 이전 경로를 직접 알려 줘야 합니다.
- 단일 저장소 배치(v0.1, `~/.hippo/records.sqlite`)의 데이터는 새 배치로 옮기지 않습니다.
- 지금은 macOS만 지원합니다.

---

## 11. 라이선스

MIT입니다. [LICENSE](LICENSE)를 보십시오.
