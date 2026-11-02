# 인계: 프로젝트별 저장공간 분리 · 과거 기록 import 재검토

이 문서는 다음 세션의 착수 문서다. 아래 "착수 블록"이 이번 세션의 지시 전부다.

---

## 착수 블록

1. `git status`가 깨끗한지, `git log -1`이 이 문서를 추가한 커밋인지 확인한다.
2. 이 문서 전체를 읽는다. 그다음 `docs/SRS.md`의 1.3 용어, 2.4 제약, 3.2.2 해석(EXT), 3.2.4 저장(STO), 3.2.5 검색(RET), 3.2.6 교체(SWP), 3.2.7 데몬(DMN), 3.4 데이터(DR), 부록 B를 읽는다. README는 1장(개념)과 7장(데이터 위치)을 읽는다.
3. **과제 A(프로젝트별 저장공간 분리)부터** 시작한다. 구현 전에 아래 "A. 결정할 것"을 사용자와 하나씩 정한다. 질문은 한 번에 하나, 권장안을 함께 낸다.
4. 결정이 끝나면 SRS를 고친다 → 실패하는 테스트를 쓴다 → 구현한다 → 전체 테스트를 통과시킨다.
5. 과제 A가 끝나면 과제 B(과거 기록 import 재검토)로 간다.

---

## 1. 지금 상태

| 항목 | 값 |
|------|----|
| 프로젝트 | Hippocampal Index, 명령어/crate `hippo` |
| 저장소 | `git@bii:Biisairo/Hippocampal-Index.git` (원격 `origin`, 브랜치 `main`) |
| 이 문서 직전 커밋 | `cb9bc85 feat: 데몬·Ollama 자동 시작, 유휴 종료, 검색 FTS 대체` |
| 테스트 | 204개 통과 + `--ignored` 실측 테스트 3개(로컬 Ollama 필요). clippy 경고 0 |
| 사용 환경 | **hook 미설치**(전역 설치했다가 사용자 요청으로 제거). `~/.hippo` 삭제됨. 데몬과 Ollama 정지됨 |
| 설치된 도구 | Homebrew `ollama` 0.40.13, 모델 `qwen3-embedding:0.6b`, `protobuf` 36.2 |

구현이 끝난 기능(SRS ID 기준): 수집(CAP, EXT), 백그라운드 연결(BG), 저장(STO), 교체·재색인(SWP), 검색·자동 주입(RET), 데몬(DMN-1~8), 설치(INS), Ollama 자동 시작(EMB).

---

## 2. 과제 A: 프로젝트별 저장공간 분리

### 배경
지금은 모든 프로젝트가 `~/.hippo/records.sqlite` 하나와 벡터 저장소 하나에 함께 들어간다. 프로젝트 구분은 반복의 `cwd` 컬럼과 검색 필터(`FtsFilter.cwd`, `VectorFilter.cwd`)뿐이다. 사용자가 "섞인다"고 느꼈고, 저장 단위를 프로젝트별로 나누기로 했다.

전역 설치를 했을 때 실제로 다른 프로젝트 세션 2개가 같은 DB에 함께 들어갔다. `hippo search --scope all` 결과에 다른 프로젝트의 Agent 호출 프롬프트가 섞여 나왔다.

### 결정할 것 (사용자와 하나씩)
1. **프로젝트를 식별하는 키**
   - 후보: (a) hook 입력의 `cwd`, (b) `cwd`의 git 최상위 디렉터리, (c) transcript 경로의 Claude 프로젝트 디렉터리 이름(`~/.claude/projects/<slug>/`).
   - 고려할 점: 한 세션 안에서 `cd`로 `cwd`가 바뀔 수 있다. 하위 디렉터리에서 연 세션을 같은 프로젝트로 볼지 정해야 한다. (c)는 Claude Code가 이미 프로젝트 단위로 나눠 둔 값이다. 다만 slug 규칙은 Claude Code 내부 구현이다.
2. **디렉터리 배치**: 예) `~/.hippo/projects/<키>/{records.sqlite, vectors/<이름>/<세대>/}`. 키를 디렉터리 이름으로 안전하게 바꾸는 규칙(해시 또는 이스케이프)과, 사람이 읽을 수 있는 이름을 남길지 정한다.
3. **프로젝트를 넘나드는 검색**(`--scope all`): 프로젝트 저장소들을 차례로 검색해 합칠지(RRF), 기능을 없앨지 정한다.
4. **데몬이 여러 저장소를 다루는 방식**: 필요할 때 열기, 열어 둘 개수 상한(LRU), 프로젝트별 수집 스레드 분리 여부. 현재 쓰기는 수집 스레드 하나가 전담한다(FR-DMN-2, FR-DMN-6).
5. **재색인·상태·show의 단위**: `hippo reindex`, `hippo status`를 프로젝트별로 할지 전체로 할지 정한다. `show <prompt_id>`는 어느 저장소를 볼지 알아야 한다(prompt_id → 프로젝트 찾기).
6. **임베딩 작업 큐**: 저장소마다 큐를 둘지 정한다. 임베딩 스레드가 여러 큐를 공정하게 돌게 해야 한다.

### 영향 범위 (코드)
`config.rs`(Paths), `store.rs`, `ingest.rs`, `daemon.rs`(Shared, 스레드, 요청 처리), `search.rs`(scope), `indexer.rs`, `generations.rs`(vectors base), `backends.rs`(VectorSource), `main.rs`(CLI scope), `protocol.rs`, 관련 테스트. SRS의 DR-1, FR-RET-1/6, IF-CLI-1, FR-SWP-5/9/10, FR-DMN-2/6.

이미 지운 데이터만 있었으므로 **기존 데이터 마이그레이션은 필요 없다.** 이 점을 사용자에게 확인한다.

---

## 3. 과제 B: 과거 기록 import 재검토

### 지금 동작 (실측으로 확인함)
- 세션마다 "어디까지 읽었는지(안전 offset)"를 `sessions` 테이블에 저장한다(FR-EXT-1).
- **처음 보는 세션은 offset이 0이다.** 그래서 설치 후 hook이 처음 실행된 세션은 **transcript를 처음부터** 읽는다. 설치 전에 오간 대화도 함께 들어간다.
- 전역 설치 직후, 이미 열려 있던 세션 3개가 모두 처음부터 수집되었다(반복 566개, 임베딩 작업 약 1,000개).
- **닫힌 과거 세션의 로그**(`~/.claude/projects/*/*.jsonl`, 당시 317개)는 읽지 않는다. hook이 실행되지 않기 때문이다.

### 결정할 것 (사용자와 하나씩)
1. **과거 세션을 다시 열었을 때**(`claude --resume`, `--continue`): 이전 턴들을 처음부터 수집할지, 다시 연 시점 이후만 수집할지 정한다.
   - 먼저 확인할 사실: 재개할 때 Claude Code가 같은 transcript 파일에 이어 쓰는지, 새 session id로 새 파일을 만들고 이전 기록을 복사하는지. 복사한다면 같은 반복이 다른 session id로 두 번 저장된다(반복 id가 `<session_id>:<message_id>`이므로 중복 제거가 안 된다).
   - 확인 방법: 이전 세션에서 쓴 `claude -p --settings <격리 설정>` 방식(`probe/`)으로 hook 입력과 transcript를 기록한다. `~/.claude` 설정은 건드리지 않는다.
2. **닫힌 과거 세션 일괄 가져오기**(`hippo import` 같은 명령)를 만들지, 만든다면 범위를 어떻게 할지 정한다.
   - 범위 후보: 전체, 프로젝트 단위(과제 A의 키와 연결), 날짜 이후.
   - 비용: 0.6b 모델로 임베딩 처리량을 실측하면 초당 약 2~3개였다. 수백 개 세션이면 시간이 오래 걸린다. 진행률 표시와 중단 후 재개가 필요하다.
   - 서브에이전트 transcript(`<session>/subagents/agent-*.jsonl`)는 지금 정책상 저장하지 않는다(서브에이전트 보고만 결과로 저장). import에서도 같은 정책을 따를지 정한다.
3. **설치 직후 이미 열려 있는 세션**: 지금처럼 처음부터 읽을지, 설치 시점 이후만 읽을지 정한다(1번과 같은 결정일 수 있다).

---

## 4. 이 세션이 값을 치르고 배운 것

### Claude Code hook과 transcript (2.1.316 실측, SRS 부록 A)
- `Stop`은 턴 단위, `PostToolBatch`는 반복 단위(병렬 호출 batch가 끝난 뒤 한 번). `TaskCompleted`는 작업 목록 항목용이라 쓸 수 없다.
- 서브에이전트 내부 hook 입력에는 `agent_id`가 있다. 메인 루프와 이것으로 구분한다.
- 병렬 호출의 `tool_use`와 `tool_result`는 transcript에 교차로 기록된다. 결과가 모두 모였다고 반복이 끝난 것이 아니다.
- assistant 라인에는 `promptId`가 없다. 직전 user 라인에서 가져온다.
- 백그라운드 서브에이전트 보고(`<agent-message>`)는 `isMeta: true`다. `<task-notification>`은 아니다.
- 백그라운드 shell 완료에는 hook이 없다. 완료 알림은 transcript에 user 메시지로 들어오고 새 `promptId`로 루프가 돈다.
- 포그라운드 Agent의 tool_result 본문은 안내 문구이고, 보고는 `toolUseResult.handbackReport.text`에 있다(이 환경 기준).
- **전역 설치는 이미 열려 있는 세션에도 즉시 적용된다**(설정을 다시 읽음).

### 개발 중 겪은 함정
- Serena `rename_symbol`은 `src` 안의 참조만 바꾸고 통합 테스트(`tests/`)의 참조는 놓친다. 이름을 바꾼 뒤 `grep`으로 남은 곳을 확인한다.
- rustfmt가 줄바꿈을 바꾼 뒤에는 예전 문자열로 찾아 바꾸는 스크립트가 실패한다. 실제 파일 내용을 먼저 읽는다.
- zsh는 변수 안의 glob을 펼치지 않는다(`FILES="src/*.rs"; cmd $FILES` 실패).
- 실제 바이너리로 데몬을 띄우는 CLI 테스트는 프로세스를 남길 수 있다. 테스트 설정에 `[daemon] autostart = false`나 짧은 `idle_timeout_secs`를 넣는다. 테스트 후 `pgrep -fl "hippo --root"`로 확인한다.
- LanceDB 때문에 release 첫 빌드는 약 4분 걸린다. `protoc`가 필요하다.
- 백그라운드 보안 리뷰 플러그인이 커밋마다 지적을 낸다. 지금까지 반영한 것: PEM 잘린 키, 치환 템플릿 주입, DB/소켓 권한, 주입 블록 이스케이프, 터미널 제어 문자, 위조 알림으로 임의 파일 읽기(경로·작업 id·O_NOFOLLOW), 임베더 URL 우회와 리다이렉트, 설치 시 임시 파일·백업 링크 추적.
- Ollama 첫 호출은 모델 로딩으로 약 1초 걸린다. `keep_alive`로 유지한다.
- 자동 주입 임계값 실측(표본 4개): 관련 0.46~0.68, 무관 0.21~0.30. 기본값 0.40은 임시값이다(OPEN-5).

### 아직 고치지 않은 것
- `~/.hippo/hippo.lock`이 0644로 만들어진다. 0600으로 맞춘다(`daemon.rs`의 잠금 파일 생성부).
- IF-SKL-1(skill 문서)은 미구현이다. README 4.4에 `CLAUDE.md` 안내로 대신했다. 사용자 결정이 필요하다.
- OPEN-1, 2, 3, 6은 대화형 세션에서 아직 확인하지 못했다. OPEN-6(Stop 시점에 마지막 줄이 기록되어 있는가)이 가장 영향이 크다.

---

## 5. 변하지 않는 규약

- **작업 순서**: 규모가 중간 이상이면 인터뷰 → SRS(IEEE 29148) 수정 → 실패하는 테스트 → 구현 → 전체 테스트 통과. SRS와 코드가 다르면 SRS부터 고친다.
- **검증**: `cargo test`(전체), `cargo clippy --all-targets`(경고 0), `cargo fmt`. 시간에 의존하는 테스트는 여러 번 반복해 결과가 일정한지 본다.
- **심볼 이름 변경**은 Serena `rename_symbol`을 쓴다(위 함정 참고).
- **커밋**: 사용자 확인 후에만 한다. 커밋 메시지에 AI 서명(`Co-Authored-By` 등)을 넣지 않는다. 이 저장소의 git 작성자는 `biisairo <rlark1224@naver.com>`으로 설정되어 있다(저장소 로컬 설정).
- **push**: `git push origin main`. 원격은 SSH 호스트 `bii`를 쓴다.
- **개인정보 금지**: 저장소는 GitHub에 있다. 로컬 사용자 경로, 회사 정보, 회사 이메일을 파일과 커밋에 넣지 않는다. 실측 fixture를 새로 만들면 경로를 `/Users/user`처럼 바꾼다. 커밋 전에 `git grep`으로 확인한다.
- **`~/.claude` 설정 파일**은 사용자가 요청할 때 `hippo install`/`uninstall`로만 바꾼다. 실측은 `claude -p --settings <격리 설정 파일>`로 한다.
- **문서**: 한국어 문서는 자연스러운 한국어로 쓴다. 영어 README와 한국어 README를 함께 고친다.

---

## 6. 아직 유효한 사용자 결정

- 기억 단위는 agentic loop 반복 하나(입력 + 도구 호출 + 응답). `thinking`은 저장하지 않는다. tool_result는 원문을 저장하되 결과 저장소에 따로 두고 SHA-256으로 중복을 없앤다.
- 백그라운드 작업(서브에이전트, shell)은 시작 호출과 완료 결과를 한 쌍으로 연결한다. 서브에이전트 내부 반복은 저장하지 않는다.
- 검색은 자동 주입 + CLI. MCP는 쓰지 않는다(사내 규정상 승인 필요).
- 구현 언어 Rust. 임베딩 모델과 벡터 DB는 언제든 바꿀 수 있어야 한다(원본은 SQLite, 벡터 저장소는 파생 인덱스).
- 임계값 같은 수치는 확정하지 않고 설정값으로 둔다.
- 데몬과 Ollama는 필요할 때 자동으로 띄운다. 데몬은 유휴 시간 뒤 스스로 내려가고, Ollama는 hippo가 끄지 않는다.
- 바이너리는 옮기지 않고 빌드 위치에서 설치해도 된다.
- 프로젝트 풀 네임은 Hippocampal Index, 명령어와 줄인 이름은 `hippo`.
