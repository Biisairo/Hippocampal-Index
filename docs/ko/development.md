# 개발과 기여

[← README](../../README.ko.md) · [English](../en/development.md)

## 목차

1. [작업 순서: 스펙 먼저](#1-작업-순서-스펙-먼저)
2. [개발 환경](#2-개발-환경)
3. [빌드와 테스트](#3-빌드와-테스트)
4. [소스 구성](#4-소스-구성)
5. [새 백엔드 추가하기](#새-백엔드-추가하기)
6. [기여하기](#6-기여하기)

---

## 1. 작업 순서: 스펙 먼저

[`docs/SRS.md`](../SRS.md)(ISO/IEC/IEEE 29148)가 단일 기준입니다. 요구사항마다 ID(예: `FR-EXT-7`)가 있고, 코드 주석이 이 ID를 가리킵니다.

작업 순서는 다음과 같습니다.

1. SRS를 고칩니다.
2. 실패하는 테스트를 씁니다.
3. 구현합니다.
4. 전체 테스트를 통과시킵니다.

SRS와 코드가 다르면 SRS부터 고칩니다.

## 2. 개발 환경

| 항목 | 사용한 버전 | 비고 |
|------|-------------|------|
| macOS | Darwin (Apple Silicon) | 1차 대상. Unix socket과 launchd를 씁니다 |
| Rust | 1.95 이상 (edition 2024) | |
| `protoc` | libprotoc 36 이상 | LanceDB 빌드에 필요합니다. `brew install protobuf` |
| Ollama | 0.40 이상 | 실측 테스트에만 필요합니다 |

## 3. 빌드와 테스트

```bash
cargo build                    # debug
cargo build --release          # release (LanceDB 때문에 처음에는 약 4분)
scripts/bundle-app.sh          # release 빌드로 target/hippo.app을 만듦(ad-hoc 서명)
cargo test                     # 결정론적 테스트 전체
cargo test -- --ignored        # 로컬 Ollama를 쓰는 실측 테스트
cargo clippy --all-targets     # 경고가 없어야 함
cargo fmt
```

빌드한 실행 파일로 바로 창을 열 수 있습니다: `./target/release/hippo gui`. 설치하지 않은 실행 파일로는 `install`과 `gui`만 쓸 수 있습니다([CLI 참고](cli.md#실행-파일과-데이터-루트)).

### fixture

- `tests/fixtures/`의 fixture는 실제 transcript에서 `user`와 `assistant` 줄만 남긴 것입니다.
- `tests/fixtures/hooks/`에는 실측한 hook 입력이 있습니다.
- 새로 만들 때는 `~/.claude` 설정을 건드리지 않습니다. 이벤트마다 hook 입력을 파일로 기록하는 격리 설정 파일을 따로 만들고, `claude -p --settings <그 파일>`로 실행합니다. 그다음 transcript에서 `user`와 `assistant` 줄만 남기고, 로컬 경로를 `/Users/user`처럼 바꿉니다.

## 4. 소스 구성

| 모듈 | 역할 |
|------|------|
| `main.rs` | CLI 진입점. 실행 파일 위치에서 데이터 루트를 정함 |
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
| `awake.rs` | 처리할 작업이 있는 동안 시스템 유휴 잠자기 막기 |
| `install.rs`, `setup.rs` | 설정 파일 병합, 실행 파일 복사, hook JSON, launchd plist |
| `layout.rs` | 데이터 루트 판정, PATH 링크, Claude 폴더 기록, 설치 상태, 완전 삭제 |
| `config.rs` | `config.toml`과 데이터 경로 |
| `config_edit.rs` | GUI 설정 편집: 키 목록, 바꾼 키만 고치는 저장(`toml_edit`), 재시작·재색인 필요 판정 |
| `commands.rs` | CLI와 GUI가 함께 쓰는 동작: 설치·제거·완전 삭제·멈추기, 가져오기 결과 줄, 데몬 요청 |
| `i18n.rs`, `fsutil.rs` | 출력 언어, 소유자 전용 파일과 디렉터리 |
| `gui/` | `hippo gui`: `state.rs`(화면과 분리한 로직, 테스트 대상), `app.rs`(egui 화면), `widgets.rs`(공통 위젯), `text.rs`(설정 키 설명), `theme.rs`(라이트·다크 테마), `images.rs`(아이콘, 로고, 캐릭터, README 정리), `picker.rs`(폴더 선택 창) |

## 새 백엔드 추가하기

1. `Embedder`(`src/embedder.rs`)나 `VectorStore`(`src/vector.rs`)를 구현합니다.
2. `tests/common/contract.rs`의 공통 계약 테스트(`embedder_contract` 또는 `vector_store_contract`)를 새 백엔드에 돌립니다. `tests/lance_test.rs`가 예시입니다.
3. `src/backends.rs`(`build_embedder` 또는 `vector_source`)에 이름을 추가합니다.
4. `config.toml`에서 선택하고, 데몬을 다시 시작한 뒤 재색인합니다.

수집, 검색, 데몬 코드는 바꾸지 않아도 됩니다.

## 6. 기여하기

버그 제보, 기능 제안, 코드 기여를 모두 환영합니다. 이슈와 PR은 한국어나 영어로 쓰면 됩니다.

### 이슈

- 버그: 재현 순서, 기대한 동작과 실제 동작, 상태 탭 화면이나 `hippo status` 출력, `~/.hippo/logs/`의 관련 오류 줄, macOS·Claude Code·Ollama 버전을 적어 주십시오.
- 로그, 출력, 스크린샷에 대화 내용, 개인 경로, 비밀값이 있으면 지우거나 가린 뒤 올려 주십시오.
- 기능 제안은 어떤 상황에서 왜 필요한지를 먼저 적어 주십시오.

### PR

1. 저장소를 fork하고 브랜치를 만듭니다.
2. 동작을 바꾸는 변경이면 [1장](#1-작업-순서-스펙-먼저)의 순서를 따릅니다: `docs/SRS.md` 수정 → 실패하는 테스트 → 구현.
3. 올리기 전에 다음이 모두 통과해야 합니다.

   ```bash
   cargo test
   cargo clippy --all-targets   # 경고 0
   cargo fmt --check
   ```

4. 사용자에게 보이는 동작이 바뀌면 영어 문서(`README.md`, `docs/en/`)와 한국어 문서(`README.ko.md`, `docs/ko/`)를 함께 고칩니다. 화면이 바뀌면 `docs/images/gui/`의 스크린샷도 바꿉니다.
5. 커밋 메시지는 `종류: 요약` 형식으로 씁니다. 종류는 `feat`, `fix`, `docs`, `test`, `refactor`, `chore`입니다. 요구사항을 바꿨으면 SRS 버전을 함께 적습니다(예: `fix: … (SRS 0.21)`).

### 지켜 주실 것

- 실제 대화, transcript, 개인 경로, 비밀값을 저장소에 넣지 않습니다. fixture를 새로 만들 때는 [3장의 방법](#fixture)을 쓰고 경로를 익명화합니다. 스크린샷의 대화 내용과 경로는 가립니다.
- 테스트는 결정론적이어야 합니다. 실제 Ollama가 필요한 테스트는 `#[ignore]`로 둡니다.
- 테스트와 개발 중에는 실제 `~/.claude`와 `~/.hippo`를 바꾸지 않습니다. 실행 파일을 띄우는 테스트는 임시 폴더의 `<루트>/bin/hippo`로 실행합니다(`tests/cli_test.rs`의 `installed_bin`).

## 라이선스

MIT입니다. [LICENSE](../../LICENSE)를 보십시오.
