# CLI 참고

[← README](../../README.ko.md) · [English](../en/cli.md)

앱 창으로 하는 일은 모두 터미널 명령으로도 할 수 있습니다. Claude도 이 명령으로 기억을 찾습니다. 앱 창의 사용법은 [GUI 사용법](usage.md)을 보십시오.

## 목차

- [명령 목록](#명령-목록)
- [실행 파일과 데이터 루트](#실행-파일과-데이터-루트)
- [`install`](#install) · [`uninstall`과 `purge`](#uninstall과-purge)
- [`search`](#search) · [`show`](#show)
- [`status`와 `stop`](#status와-stop)
- [`project`](#project) · [`import`](#import) · [`forget`](#forget)
- [`reindex`](#reindex)
- [`daemon`과 launchd](#daemon과-launchd)
- [`hooks-config`](#hooks-config)
- [출력 언어](#출력-언어)

---

## 명령 목록

| 명령 | 하는 일 | GUI에서 |
|------|---------|---------|
| `gui` | 앱 창을 엽니다 | — |
| `install` | 실행 파일을 설치하고 Claude Code에 hook과 안내 블록을 넣고 PATH 링크를 만듭니다 | Claude 적용 → 적용 |
| `uninstall` | hook, 안내 블록, PATH 링크를 지웁니다. 실행 파일과 기록은 남습니다 | Claude 적용 → 해제 |
| `purge` | 해제한 뒤 데이터 루트와 모든 기록을 지웁니다 | Claude 적용 → 완전 삭제 |
| `search` | 기억을 검색합니다 | 검색 |
| `show` | 턴 하나의 반복을 순서대로 보여 줍니다 | 검색 → 턴 보기, 기록 |
| `status` | 데몬 상태와 프로젝트별 통계를 보여 줍니다 | 상태 |
| `stop` | 데몬과 `hippo`가 띄운 Ollama를 멈춥니다 | 상태 → 멈추기 |
| `project list` / `project remove` | 프로젝트 목록 / 프로젝트 삭제 | 프로젝트, 기록 → 프로젝트 삭제 |
| `import` | 과거 세션 기록을 가져옵니다 | 가져오기 |
| `forget` | 세션 하나나 턴 하나를 지웁니다 | 기록 → 삭제 |
| `reindex` | 벡터 색인을 다시 만듭니다 | 프로젝트 → 재색인 |
| `daemon` | 데몬을 포그라운드로 실행합니다 | — |
| `hooks-config` | `settings.json`에 넣을 hook 설정을 출력합니다 | — |
| `launchd-plist` | launchd plist를 출력합니다 | — |
| `hook` | Claude Code hook 진입점. 직접 실행하지 않습니다 | — |

`hippo <명령> --help`로 명령마다의 옵션을 볼 수 있습니다. `--help`는 항상 영어입니다.

---

## 실행 파일과 데이터 루트

- 설치는 실행 파일을 **데이터 루트의 `bin/hippo`(기본 `~/.hippo/bin/hippo`)로 복사**합니다. hook, Claude 안내문, 데몬은 모두 이 복사본을 씁니다.
- **데이터 루트는 실행 파일 위치로 정합니다.** `<루트>/bin/hippo`로 실행하면 `<루트>`가 데이터 루트입니다. `--root` 옵션은 없습니다. 다른 데이터 루트를 쓰려면 `install --data-dir <경로>`로 그 루트에 설치합니다.
- 설치하지 않은 실행 파일(앱 안의 실행 파일, `target/release/hippo`)로는 `install`과 `gui`만 쓸 수 있습니다.
- 설치는 `~/.local/bin/hippo`에 설치 경로를 가리키는 링크를 만듭니다. `~/.local/bin`이 PATH에 있으면 `hippo`를 바로 쓸 수 있습니다. PATH에 없으면 경고만 출력하고 셸 설정은 바꾸지 않습니다. 다른 곳을 가리키는 `~/.local/bin/hippo`가 이미 있으면 아무것도 바꾸지 않고 오류로 끝납니다.
- hook 명령과 Claude 안내문에는 경로를 `~/.hippo/bin/hippo`처럼 `~`로 적습니다. 생성된 설정 파일이 공유되어도 계정 이름이 드러나지 않습니다. launchd plist는 셸을 거치지 않으므로 절대 경로를 씁니다.
- 복사는 APFS 파일 복제를 쓰므로 디스크를 거의 더 쓰지 않습니다.

아래 예시의 `hippo`는 설치한 실행 파일입니다. PATH 링크가 없으면 `~/.hippo/bin/hippo`로 바꿔 읽으십시오.

---

## `install`

```bash
./target/release/hippo install --dry-run     # 바뀔 내용만 출력
./target/release/hippo install

# 데이터 루트나 Claude 폴더를 바꿀 때
./target/release/hippo install --data-dir ~/hippo-data --claude-dir ~/work/my-app/.claude
```

| 옵션 | 기본값 |
|------|--------|
| `--data-dir` | 이 실행 파일의 데이터 루트, 없으면 `~/.hippo` |
| `--claude-dir` | 설치할 때 기록한 Claude 폴더, 없으면 `~/.claude` |
| `--dry-run` | 쓰지 않고 바뀔 내용만 출력 |

`install`이 하는 일:

- 변경 내용(diff)을 출력합니다. `--dry-run`이면 아무것도 쓰지 않습니다.
- `<Claude 폴더>/settings.json`에 hook을 병합합니다. `hippo` hook 항목만 바꾸고, 다른 hook과 설정은 그대로 둡니다.
- 쓰기 전에 원본을 `<파일>.hippo-backup-<유닉스 시각>`으로 백업합니다. 원자적으로 교체하고, 파일 권한을 유지합니다.
- 다시 실행해도 결과가 같으면 아무것도 바꾸지 않습니다("변경 없음").
- 심볼릭 링크를 거쳐서는 쓰지 않고, JSON이 깨진 설정 파일은 건드리지 않습니다.
- 파일은 2칸 들여쓰기 JSON으로 다시 씁니다. 키 순서와 값은 유지되지만, 원래 형식은 달라질 수 있습니다. 원본은 백업에 남습니다.
- `<Claude 폴더>/CLAUDE.md`에 Claude가 `hippo search`와 `hippo show`를 쓸 수 있다는 영어 안내 블록을 넣습니다. 명령은 설치 경로로 적으므로 `hippo`가 PATH에 없어도 됩니다. 블록은 `<!-- hippo:begin -->`과 `<!-- hippo:end -->` 사이에 있고, 이 표식은 Claude Code가 컨텍스트에 넣기 전에 지우므로 토큰을 쓰지 않습니다. 블록 밖의 내용은 그대로 둡니다.
- 사용한 Claude 폴더를 `<데이터 루트>/bin/claude-dir`에 기록합니다. 다음 `install`, `uninstall`, `purge`와 GUI는 `--claude-dir`이 없으면 이 값을 씁니다.
- 실행 파일 내용이 같으면 복사하지 않습니다. 실행 파일을 바꿨으면 이전 데몬을 내리고, 다음 요청이 새 실행 파일로 데몬을 띄웁니다.

hook을 등록한 뒤에는 Claude Code 세션을 새로 시작하십시오.

## `uninstall`과 `purge`

```bash
hippo uninstall     # hook, 안내 블록, PATH 링크만 지웁니다. 기록은 남습니다
hippo purge         # uninstall 뒤 데이터 루트를 통째로 지웁니다. 저장된 기억이 모두 지워집니다
```

- `uninstall`은 hippo 내용만 있던 설정 파일과 메모리 파일을 백업 없이 지우고, 그래서 비게 된 `.claude/` 디렉터리도 지웁니다. 다른 내용이 남는 파일은 백업한 뒤 고칩니다. 설치 뒤 바로 제거하면 메모리 파일은 바이트 단위로 원래대로 돌아갑니다.
- `purge`는 지울 경로를 보여 주고 확인을 받습니다(`--yes`로 건너뜁니다). 데몬과 `hippo`가 띄운 Ollama 서버를 내린 뒤 지웁니다. 데몬이 재색인 중이면 지우지 않습니다.

---

## `search`

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

- 결과에는 RRF 점수, 벡터 유사도(`sim=-`이면 키워드로만 찾은 결과), `prompt_id`, 미리 보기가 나옵니다.
- 결과(도구 출력)를 찾은 경우에는 그 결과를 만든 프롬프트 목록도 함께 나옵니다.
- `--scope all`은 프로젝트마다 검색한 뒤 RRF로 합치고, 결과마다 `project=<id>`를 붙입니다.
- 현재 디렉터리가 어느 프로젝트에도 속하지 않으면 "No project"(한국어 설정이면 "프로젝트 없음")가 나옵니다.

## `show`

```bash
hippo show <prompt_id>                  # 해당 프롬프트의 반복을 순서대로
hippo show <prompt_id> --with-results   # 도구 출력 원문까지
hippo show <prompt_id> --scope project  # 현재 프로젝트에서만 찾기
```

`--scope`가 없으면 현재 프로젝트를 먼저 찾고, 없으면 나머지 프로젝트를 최근에 쓴 순서로 찾습니다. 백그라운드 작업의 완료 기록도 함께 나옵니다.

---

## `status`와 `stop`

```bash
hippo status
hippo stop       # 데몬과 hippo가 띄운 Ollama를 지금 멈춤. 다음 hook이나 명령이 데몬을 다시 띄움
```

`status` 출력 예(`[ui] language = "ko"`):

```
spool_pending: 0                  # 데몬이 꺼져 있을 때 쌓인 요청
embed_model: qwen3-embedding:0.6b
reindex: idle                     # idle | running | done | failed: <프로젝트 id>: <사유>
projects: 2
[3f9c…] /Users/me/work/app/.git
  iterations: 1520  results: 980  embed_jobs: 0  vectors: 2610  compatibility: compatible
  inject (7d): requests 120, injected 41 (34%), no match 52, skipped 27, timeout 0, error 0; agent: opened 3, search 8, show 5
[a71d…] /Users/me/notes  (위치 없음)
  iterations: 40  results: 12  embed_jobs: 0  vectors: 52  compatibility: compatible
  inject (7d): requests 0, injected 0, no match 0, skipped 0, timeout 0, error 0; agent: opened 0, search 0, show 0
```

- `embed_jobs`는 임베딩을 기다리는 반복과 결과의 수입니다. 대기가 있으면 실시간 작업과 가져오기 작업으로 나눠 보여 주고, 최근 5분 동안 처리한 작업이 있으면 속도와 남은 예상 시간도 나옵니다. 예: `embed_jobs: 601 [실시간 1, 가져오기 600] (분당 120건, 약 6분 남음)`.
- `inject (7d)`는 최근 7일의 자동 주입을 셉니다. 확인한 프롬프트 수와, 그중 주입한 수, 후보가 없던 수, 건너뛴 수(너무 짧음, 작업 완료 알림 등), 시간 초과, 오류입니다. `agent:`는 같은 기간에 에이전트가 실행한 `hippo` 명령의 수입니다. `opened`는 주입받은 턴을 `hippo show`로 열어 본 경우입니다.
- `compatibility`는 `empty`, `compatible`, `mismatch` 중 하나입니다. 재색인이 필요한 프로젝트가 있으면 `status`가 알려 줍니다.

---

## `project`

```bash
hippo project list                          # id, 종류(git/dir), 위치, 반복 수, 마지막 활동 시각(로컬 시간)
hippo project remove <프로젝트 id 또는 경로>  # 프로젝트 전체 삭제. 확인 방식은 forget과 같음
```

## `import`

```bash
hippo import                             # 현재 폴더(하위 폴더와 worktree 포함)의 과거 세션을 현재 프로젝트로
hippo import --from ~/old/app            # 이전 경로에서 진행한 세션을 현재 폴더의 프로젝트로
hippo import --from <프로젝트 id> --to ~/work/app
hippo import --wait                      # 가져온 뒤 임베딩이 끝날 때까지 진행률을 보여 줌
hippo import --dry-run                   # 후보 세션과 처리 내용만 보여 주고 아무것도 바꾸지 않음
```

`import`는 Claude Code가 남긴 과거 transcript(`~/.claude/projects/*/*.jsonl`)를 읽어 `--to` 폴더의 프로젝트에 기록을 추가합니다. 생략한 인자는 현재 폴더입니다. 서브에이전트 transcript는 읽지 않습니다.

- **가져올 세션**: 세션 시작 경로가 `--from` 경로이거나 그 아래인 세션입니다. 경로가 지금 없어도 됩니다. `--from`이 git 저장소이면 그 저장소의 worktree에서 연 세션도 포함합니다. 오래된 세션부터 처리합니다.
- **`--from`의 프로젝트가 이미 있으면**: 먼저 그 프로젝트를 `--to`의 프로젝트로 옮깁니다. `--to`에 프로젝트가 없으면 위치만 바꾸고, 있으면 기록을 병합한 뒤 이전 프로젝트를 지웁니다. 병합할 때 임베딩 모델이 같으면 벡터를 복사하고, 다르면 다시 임베딩합니다. 이전 위치는 남기지 않으므로, 이후 세션은 따로 연결하지 않아도 현재 위치의 프로젝트에 저장됩니다.
- **이미 실시간으로 저장 중인 세션**: 실시간 수집이 시작된 위치 앞부분만 채웁니다. 이미 있는 반복은 다시 저장하지 않습니다.
- **transcript 끝의 반복**: 도구 호출 결과가 모두 있으면 저장하고, 결과가 빠진 도구 호출이 있으면 보류합니다. 보류한 반복은 그 세션이 다시 열리거나 `import`를 다시 실행하면 이어서 처리합니다.
- **분기한 세션의 복사본**: 같은 `message.id`의 반복이 다른 세션으로 이미 있으면 건너뜁니다.
- **다른 프로젝트에 고정된 세션**: 건너뛰고 "다른 프로젝트"로 알려 줍니다.

`--dry-run`은 먼저 할 일(프로젝트 생성, 위치 변경, 병합)을 보여 줍니다. 그다음 후보 세션마다 한 줄씩 시작 시각, 세션 id, 가져오기 상태, 파일 크기, 시작 경로, 첫 프롬프트를 출력합니다. 첫 프롬프트는 비밀값을 가리고 앞 80자만 보여 줍니다. 가져오기 상태는 `새 세션`, `실시간만`, `가져옴`, `다른 프로젝트` 중 하나입니다.

진행률은 세션마다 한 줄씩 출력합니다.

```
후보 세션 120개
[1/120] d767e162 가져옴 반복 42  남은 임베딩 310
[2/120] 2f74fd43 가져옴 반복 3  남은 임베딩 318
...
합계: 세션 120개 중 가져옴 118, 변경 없음 2, 다른 프로젝트 0, 오류 0. 반복 2410개 저장, fork 복사본 101개 건너뜀.
```

중간에 Ctrl-C로 멈춰도 됩니다. 같은 명령을 다시 실행하면 끝난 세션은 "변경 없음"으로 지나가고 남은 세션부터 이어서 처리합니다. 남은 임베딩 작업은 데몬이 계속 처리합니다.

## `forget`

```bash
hippo forget --session <session_id>        # 세션 하나
hippo forget --prompt <prompt_id>          # 턴 하나: 프로젝트 안 모든 세션에서 그 prompt id의 반복 전부
```

명령은 먼저 지울 내용(프로젝트 id와 위치, 세션·턴·반복·결과 수)을 출력합니다. 그다음 `지울까요? [y/N]`를 묻고, `y`를 입력했을 때만 지웁니다.

- `--dry-run`: 미리 보기만 출력하고 끝납니다.
- `--yes`: 묻지 않고 지웁니다. 표준 입력이 터미널이 아니고(스크립트, 파이프) `--yes`도 없으면 아무것도 지우지 않고 종료 코드 1로 끝납니다.
- `--scope project|<경로>`: 생략하면 `show`처럼 현재 프로젝트에서 먼저 찾고, 없으면 나머지 프로젝트에서 찾습니다. 맞는 대상이 없으면 `대상 없음`을 출력합니다.

지우는 규칙은 [GUI 사용법의 기록 지우기](usage.md#기록-지우기)와 같습니다.

---

## `reindex`

```bash
hippo reindex                     # 모든 프로젝트
hippo reindex --scope project     # 현재 디렉터리의 프로젝트
hippo reindex --scope <경로>
```

임베딩 모델이나 벡터 DB를 바꾼 뒤에 씁니다. 자세한 내용은 [설정](configuration.md#임베딩-모델이나-벡터-db-바꾸기)을 보십시오.

---

## `daemon`과 launchd

데몬을 직접 띄울 필요는 없습니다. hook, CLI 명령, 앱이 필요할 때 띄우고(`[daemon] autostart = true`), 요청 없이 30분이 지나면 데몬이 스스로 내려갑니다. 동작을 지켜보거나 항상 띄워 두고 싶을 때만 직접 실행하십시오.

터미널에서 실행합니다. 처음 시험해 볼 때 적합합니다.

```bash
hippo daemon
```

launchd에 등록해 로그인할 때 자동으로 시작합니다.

```bash
mkdir -p ~/.hippo/logs && chmod 700 ~/.hippo ~/.hippo/logs
hippo launchd-plist > ~/Library/LaunchAgents/com.hippo.daemon.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.hippo.daemon.plist
```

launchd는 데몬이 비정상 종료했을 때만 다시 띄우고, 유휴 종료 뒤에는 다시 띄우지 않습니다. 멈출 때는 `launchctl bootout gui/$(id -u)/com.hippo.daemon`을 실행합니다.

## `hooks-config`

`install` 대신 hook 블록을 직접 넣고 싶을 때 씁니다. 출력한 JSON을 `settings.json`의 `hooks`에 넣습니다.

```bash
hippo hooks-config
```

---

## 출력 언어

CLI 출력은 `[ui] language`(`en` 기본, `ko`)를 따릅니다. Claude에게 주는 텍스트와 `--help`는 항상 영어입니다. 이 문서의 출력 예는 `ko` 기준입니다.
