# 설정

[← README](../../README.ko.md) · [English](../en/configuration.md)

설정 파일은 `~/.hippo/config.toml`(데이터 루트의 `config.toml`)입니다. 앱 창의 [설정 탭](usage.md#설정-탭)에서 고치는 것을 권장합니다. 설정 탭은 바꾼 키만 저장하고, 잘못된 값은 쓰지 않습니다.

## 목차

1. [기본 규칙](#1-기본-규칙)
2. [전체 키](#2-전체-키)
3. [자동 주입 기본값을 정한 방법](#3-자동-주입-기본값을-정한-방법)
4. [임베딩 모델이나 벡터 DB 바꾸기](#임베딩-모델이나-벡터-db-바꾸기)

---

## 1. 기본 규칙

- 모든 키는 생략할 수 있습니다. 파일이 없으면 모두 기본값입니다.
- 알 수 없는 키는 오류로 처리합니다. 오타를 바로 알 수 있습니다.
- 설정을 바꾼 뒤에는 데몬을 다시 시작해야 합니다. 설정 탭은 필요할 때 **데몬 다시 시작** 버튼을 보여 줍니다.
- 다음 키는 다시 시작하지 않아도 바로 적용됩니다: `[ui] language`, `[ui] theme`, `[ui] start_tab`, `[inject] enabled`, `[capture] enabled`.

## 2. 전체 키

아래 값은 모두 기본값입니다(`[[redact.patterns]]`와 `transcripts_dir`는 예시).

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
enabled = true                     # false면 주입 일시정지. 다음 프롬프트부터 적용(데몬 재시작 불필요)

[capture]
enabled = true                     # false면 저장 일시정지. 재생하면 열린 세션은 밀린 기록도 저장

[index]
vector_index_threshold = 100000    # 벡터 수가 이 값에 이르면 ANN 인덱스를 만듦
job_batch = 64
optimize_every = 20                # 벡터 저장소를 이만큼 수정하면 작은 파일을 합치고 이전 버전을 지움(0이면 큐가 빌 때만)

[daemon]
autostart = true                   # 데몬이 없으면 hook과 CLI가 띄움
idle_timeout_secs = 1800           # 요청 없이 이 시간(초)이 지나면 종료 (0이면 종료하지 않음)
max_open_projects = 8              # 동시에 열어 둘 프로젝트 벡터 저장소 수
prevent_sleep = true               # 처리할 작업이 남은 동안 Mac의 유휴 잠자기를 막음 (덮개를 닫으면 잠듦)

[project]
git_timeout_ms = 1000              # git 실행 시간 제한. 넘으면 git이 아닌 폴더로 봄

[import]
transcripts_dir = "/Users/me/.claude/projects"   # 과거 transcript를 찾는 디렉터리. 기본값은 $HOME/.claude/projects (`~`는 펼치지 않음)

[ui]
language = "en"                    # CLI와 GUI 문구의 언어: en(기본) 또는 ko. Claude에게 주는 텍스트와 --help는 항상 영어
theme = "system"                   # GUI 테마: system(기본, macOS 설정을 따름), light, dark
start_tab = "about"                # 창을 열 때의 탭: about(기본), status, records, projects, search, import,
                                   # claude_setup, settings, last(마지막으로 본 탭)

[[redact.patterns]]                # 추가 비밀값 패턴 (정규식)
kind = "internal"
pattern = 'HCI-\d{6}'
```

`start_tab = "last"`이면 창은 데이터 루트의 `gui-last-tab` 파일에 기록한 탭을 엽니다. 이 파일은 탭을 바꿀 때마다 갱신합니다. 설치하지 않은 상태에서 시작 탭이 **정보**나 **Claude 적용**이 아니면 **Claude 적용** 탭을 엽니다.

## 3. 자동 주입 기본값을 정한 방법

`inject.threshold`, `min_prompt_chars`, `query_prefix`의 기본값은 실측으로 정했습니다.

- 이 프로젝트의 과거 기록으로 평가셋을 만들었습니다. 정답이 있는 질문 60개와 기억이 필요 없는 프롬프트 30개입니다.
- 질의 지시문 없이 0.40을 쓸 때와 비교했습니다. 정답 포함률은 그대로(53/60)이고, 잘못된 주입은 28/30에서 16/30으로 줄었습니다(SRS 부록 C).

조정 방법:

- 잘못된 주입을 더 줄이려면 `threshold`를 0.55~0.60으로 올립니다. 대신 필요한 기억도 일부 빠집니다.
- 임베딩 모델을 바꾸면 `query_prefix`를 그 모델이 권장하는 질의 형식으로 바꿉니다.
- 값을 바꾸기 전에 [주입 미리 보기](usage.md#이-프로젝트에-질의)로 결과를 확인하십시오.

---

## 임베딩 모델이나 벡터 DB 바꾸기

벡터 저장소는 SQLite 원본에서 파생된 색인이므로 언제든 다시 만들 수 있습니다.

1. 설정 탭(또는 `config.toml`)에서 모델, 차원, 백엔드를 바꿉니다.
2. 데몬을 다시 시작합니다. 상태 탭의 호환성이 `mismatch`가 됩니다. 그동안에도 키워드 검색은 동작합니다.
3. [프로젝트 탭](usage.md#프로젝트-탭)에서 **재색인**을 누릅니다(`hippo reindex`). 백그라운드에서 모든 프로젝트를 차례로 새 세대에 다시 색인합니다.
4. 끝나면 프로젝트마다 `ACTIVE` 포인터가 원자적으로 바뀝니다. 이전 세대는 그 프로젝트를 다음에 열 때 지웁니다.

- 한 프로젝트만 다시 색인하려면 범위를 `project`(현재 폴더의 프로젝트)나 경로로 둡니다.
- 재색인 중에는 기존 색인으로 검색합니다. 그동안 새로 들어온 반복은 큐에서 기다렸다가 전환 뒤 새 색인에 들어가므로, 잃어버리지 않습니다.
- 재색인 중에 재색인을 다시 요청하면 거부합니다.
- 한 프로젝트의 재색인이 실패해도 다음 프로젝트를 계속 처리합니다. 실패한 프로젝트는 상태의 `reindex: failed: …`에 나옵니다.
- 벡터 디렉터리는 `projects/<id>/vectors/<이름>/<세대>/`이고, `projects/<id>/vectors/<이름>/ACTIVE`가 사용 중인 세대를 가리킵니다.

완전히 새로운 백엔드를 추가하는 방법은 [개발과 기여](development.md#새-백엔드-추가하기)를 보십시오.
