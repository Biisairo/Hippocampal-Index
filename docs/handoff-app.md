# 인계: 앱(.app) 배포(SRS 0.21) 이후 — push와 재설치·재임베딩

이 문서는 다음 세션의 착수 문서입니다. 이번 세션의 지시는 아래 "착수 블록"이 전부입니다.

---

## 착수 블록

1. `git status`를 실행합니다.
   - 작업 트리는 깨끗해야 합니다.
   - `main`은 `origin/main`보다 앞서 있습니다(push 전). 마지막 커밋은 앱 배포 구현(SRS 0.21)입니다.
2. 다음 문서를 읽습니다.
   - 이 문서 전체
   - `docs/handoff-delete-records.md`의 4장(값을 치르고 배운 것), 6장(규약)
3. 다음을 사용자에게 **하나씩 먼저 묻고** 진행합니다.
   1. **push**: `origin/main`으로 push할지.
   2. **재설치**: 지금 이 장비에는 hippo가 설치되어 있지 않습니다(`~/.hippo` 없음, `~/.claude`에 hippo hook 없음). 앱(`scripts/bundle-app.sh` → `target/hippo.app`)의 GUI로 설치할지, `target/release/hippo install`로 설치할지 묻습니다. 설치는 `~/.claude`를 바꾸므로 반드시 사용자 확인 뒤에 합니다.
   3. **재임베딩**: 설치한 뒤 프로젝트 10개를 `~/.hippo/bin/hippo import --from <경로> --to <경로>`로 가져옵니다. 대상 경로는 사용자에게 받습니다.
4. 결정이 필요하면 `AskUserQuestion`으로 하나씩 묻습니다. 커밋과 push는 사용자 확인 후에만 합니다. 커밋할 때는 `git add -A`를 쓰지 말고 파일을 지정합니다.
5. **`docs/handoff-laya.md`는 건드리지 않습니다.** 사용자가 직접 정리하기로 했습니다.

---

## 1. 이번 세션에서 한 일 (SRS 0.21)

### 설치 구조가 바뀌었습니다
| 항목 | 이전 | 지금 |
|------|------|------|
| 데이터 루트 | `--root`, 없으면 `~/.hippo` | 실행 파일의 실제 경로 `<루트>/bin/hippo`로만 정함. `--root` 없음(FR-ROOT-1) |
| 설치하지 않은 실행 파일 | 모든 명령 가능 | `install`, `gui`, 도움말만 가능. hook은 종료 코드 0으로 바로 끝남(FR-INS-14) |
| install | `--scope user\|project [--project]` | `[--data-dir] [--claude-dir] [--dry-run]`. `<Claude 폴더>/settings.json`과 `CLAUDE.md`. 프로젝트 범위(`CLAUDE.local.md`)는 없앰 |
| PATH | 없음 | `~/.local/bin/hippo` 링크. 다른 것이 있으면 아무것도 바꾸지 않고 오류(FR-INS-11) |
| uninstall | 마지막 설치면 `bin/hippo` 삭제(`bin/installs`) | hook, 안내 블록, 링크만 지움. 실행 파일과 기록은 남김(FR-INS-9) |
| 데이터 삭제 | `rm -rf ~/.hippo` | `hippo purge`: uninstall → 확인 → 데몬·Ollama 멈춤 → 루트 삭제(FR-INS-13) |
| 데몬 자동 시작 | 자기 실행 파일 | 항상 `<루트>/bin/hippo`(FR-DMN-7) |
| 실행 파일 교체 | 수동으로 데몬 재시작 | install이 내용을 바꾸면 그 루트의 데몬에 `shutdown`(FR-INS-8) |

- Claude 폴더는 `<루트>/bin/claude-dir`에 기록하고, `--claude-dir`이 없을 때 기본값으로 씁니다(FR-INS-12).
- hippo가 띄운 Ollama의 pid는 `<루트>/ollama.pid`에 기록합니다. 멈출 때는 프로세스 이름이 `ollama`인지 확인합니다(FR-EMB-4).

### 새로 넣은 것
- **앱**: `scripts/bundle-app.sh`가 `target/hippo.app`을 만듭니다(Info.plist, `.icns`, ad-hoc 서명). 앱을 열면 GUI만 뜨고 아무것도 설치하지 않습니다(FR-APP-1~5, IF-CLI-14).
- **GUI 데이터 루트**: 자기 루트 → PATH 링크의 루트 → `~/.hippo` 순서로 고릅니다. Claude 적용 탭에서 데이터 루트와 Claude 폴더를 바꿀 수 있습니다. 설치 상태(`설치 필요`, `업데이트 필요`, `설치됨`)가 `설치됨`이 아니면 Claude 적용 탭과 정보 탭만 쓸 수 있습니다(FR-GUI-22, 23). **완전 삭제** 버튼이 있습니다.
- **수동 중지**: `hippo stop`과 상태 탭의 **데몬과 Ollama 멈추기**. 멈춘 뒤 GUI는 폴링도 멈추고 **다시 시작**을 기다립니다(FR-DMN-12, IF-CLI-15, FR-GUI-3).
- **일시정지**: `[inject] enabled`, `[capture] enabled`(기본 true)와 상태 탭의 **일시정지/재생** 버튼. hook이 설정을 읽어 바로 끝냅니다. 데몬 재시작이 필요 없습니다. 저장을 재생하면 열린 세션은 밀린 기록도 저장합니다(사용자 결정: "그냥 일시정지")(FR-CAP-8, FR-RET-12).

### 검증
- 테스트 447개 통과, 실패 0, 무시 2. clippy 경고 0, fmt 통과.
- 번들: `plutil -lint` OK, `codesign --verify` 통과.
- 화면으로 확인한 것:
  - Finder 방식(`open`)으로 앱을 열면 "설치 안 됨" 안내와 함께 Claude 적용·정보 탭만 켜지고, `~/.hippo`와 `~/.local/bin/hippo`가 생기지 않음.
  - 격리 HOME에 앱 실행 파일로 설치한 뒤: PATH 링크로 데이터 루트를 찾음, 데몬이 설치 경로로 뜸, 저장 일시정지가 그 키만 씀, 멈추기 뒤 데몬이 다시 뜨지 않음. 그 뒤 `purge`로 정리함.
- **화면으로 확인하지 않은 것**: GUI의 적용·해제·완전 삭제 버튼(실제 `~/.claude`를 바꾸므로 라이브러리 테스트와 CLI 테스트로만 확인), `업데이트 필요` 상태.
- GUI 완전 삭제 결함(코드 검토로 찾음): 삭제하는 동안에도 상태 폴링이 데몬 자동 시작을 시도해, 지운 루트에 `logs/`가 다시 생기거나 데몬이 다시 뜰 수 있었습니다. Claude 적용 탭의 작업 중에는 폴링하지 않고, 완전 삭제는 진행 중인 상태 요청이 끝난 뒤 시작해 끝나면 멈춤 상태로 둡니다(FR-GUI-10, `should_poll`, `purge_may_start`).
- 커밋 작성자: 이 저장소에는 로컬 git 설정(`biisairo`)을 넣었습니다. 전역 설정이 회사 이메일이라, 로컬 설정이 없으면 커밋에 회사 이메일이 들어갑니다.

## 2. 남은 것
- 같은 데이터 루트로 여러 Claude 폴더에 설치하면, 기록은 마지막 폴더 하나만 남습니다. `purge`는 그 폴더만 해제합니다(SRS 결정, README 10장).
- TDD 순서: 이번 구현은 테스트를 먼저 썼지만, 일부는 구현과 같은 묶음으로 넣어 실패를 따로 확인하지 않았습니다.
- SRS 상태 칸은 "검토 대기"입니다.

## 3. 값을 치르고 배운 것
- **macOS에는 `timeout` 명령이 없습니다.** `timeout 600 cargo build | grep …`은 명령을 찾지 못해 아무것도 출력하지 않습니다. 빌드가 통과한 것으로 착각하기 쉽습니다. 도구의 timeout 인자를 씁니다.
- 긴 Markdown 표 줄은 Edit보다 `python3 -X utf8` 스크립트로 줄 단위로 바꾸는 편이 안전합니다. 스크립트 첫 줄에 `# -*- coding: utf-8 -*-`를 둡니다(없을 때 한 번 `Non-UTF-8 code` 오류가 났습니다). 바꾸기 전에 대상 문자열이 정확히 한 번 나오는지 `assert`로 확인합니다.
- release 빌드는 약 10분 걸립니다. `scripts/bundle-app.sh`는 백그라운드로 돌립니다.
- egui 화면 조작: 상태 탭 버튼은 `button n of UI element 2 of group 1 of window "hippo"`로 누를 수 있습니다. 창 번호는 CGWindowList에서 소유자 이름 `hippo`, layer 0인 창을 찾습니다.
- 에이전트 응답이 안전 분류기에 막히면, 막힌 작업은 표현을 바꿔서도 다시 하지 않습니다. 이번 세션에서도 두 번 막혔습니다. 사용자에게 알리고 다른 방법을 제안합니다.
- 사용자는 작업 도중에 요구를 더합니다(이번에는 수동 중지, 일시정지). 새 요구는 SRS에 먼저 넣고, 결정이 필요한 것만 하나씩 묻습니다.
