# Hippocampal Index (`hippo`)

**Long-term memory for Claude Code sessions.**

The name comes from the *hippocampal memory indexing theory* (Teyler & DiScenna, 1986). In this theory, the hippocampus does not store whole memories. It stores an **index** that points to memory traces spread across the cortex. When a cue arrives, the index reactivates the original memory. This project works the same way: the full records stay in SQLite (the "cortex"), the vector store is the index, and your prompt is the cue. The short name and the command are `hippo`.

[한국어](README.ko.md)

`hippo` gives Claude Code a long-term memory of your own sessions. It records every iteration of the agentic loop into a local database. When you send a new prompt, it searches that memory and gives Claude the relevant past context.

All data stays on your machine. Embeddings come from a local Ollama model, and vectors go into an embedded LanceDB. No conversation content leaves the computer.

---

## Contents

1. [Concept](#1-concept)
2. [Requirements](#2-requirements)
3. [Installation](#3-installation)
4. [Usage](#4-usage)
5. [Configuration](#5-configuration)
6. [Changing the embedding model or vector database](#6-changing-the-embedding-model-or-vector-database)
7. [Data layout and security](#7-data-layout-and-security)
8. [Troubleshooting](#8-troubleshooting)
9. [Development: modify and build](#9-development-modify-and-build)
10. [Known limitations](#10-known-limitations)

---

## 1. Concept

### 1.1 The unit of memory: one agentic-loop iteration

When you send one prompt, Claude Code runs an **agentic loop**. Each **iteration** of the loop is one model call followed by the tool calls that the model requested.

```
Your prompt
  → iteration 1: model text + tool_use(Read) → tool_result
  → iteration 2: model text + tool_use(Edit) → tool_result
  → iteration 3: model text only (the loop ends)
```

`hippo` stores **one record per iteration**. The record contains:

- **Input**: your prompt, a background-task completion notice, or a subagent report, if one arrived before this iteration
- **Tool calls**: the name and input of each tool call
- **Response**: the model's text (`thinking` blocks are not stored)
- **Results**: the full tool output, stored separately and deduplicated by content hash
- **User answers**: for tools that ask you something, such as `AskUserQuestion`, the answer you chose (and your note, if any) is added to the iteration record as `[answer]`. Only the answer is added, because the question is already in the tool call. So "what did we decide" can be found and shows up in automatic injection

Tool calls that run `hippo` (`hippo search` and so on) are stored, but they are kept out of the search index. The Bash command is parsed with bash syntax and judged by the commands it actually runs, so `cd x && hippo …`, `H=…/hippo; $H …`, and `$(hippo …)` are caught, while `cargo test -p hippo`, which only uses hippo as an argument, is not. This stops past lookups from coming back in the next search. `hippo show` still shows them in the turn.

### 1.2 How iterations are captured (hooks)

| Iteration type | Hook that triggers capture |
|----------------|----------------------------|
| Iteration with tool calls | `PostToolBatch` (fires once per batch, after all parallel tool calls finish) |
| Last iteration (text only) | `Stop` / `StopFailure` |
| Leftovers at session end | `SessionEnd` |

These hooks run with `async: true`, so they never slow down your session. The hook payload is only a trigger. The content comes from the session transcript (JSONL). `hippo` keeps a safe byte offset per session, so it reads each line once and never records the same iteration twice.

**Live capture starts at the turn the hook reports.** When `hippo` sees a session for the first time, it does not read the transcript from the beginning. It starts at the first line of the turn that the hook payload's `prompt_id` points to. So in these cases the conversation before that turn is not stored:

- Sessions that were already open when you installed the hooks
- Sessions started before hippo was installed and reopened with `claude --resume` or `--continue`
- Sessions branched with `--fork-session` and similar. Branching copies the earlier conversation into a new file.

When you reopen a session that is already being captured, Claude Code appends to the same file, so capture continues after the saved position. To store the earlier history, use `hippo import` (see [4.4](#44-projects-and-importing-past-records)).

Other details:

- **Subagents**: `hippo` skips their internal iterations. A foreground subagent's final report arrives as the result of the `Agent` tool call, so it is stored with that call.
- **Background tasks** (`run_in_background` for Bash or Agent): the start call is stored as *pending*. When the completion notice (`<task-notification>`) appears in the transcript, `hippo` reads the output and links it to the start call. The call and its response become one pair.

### 1.3 Storage model

Memory is stored **separately for each project**. Records of other projects do not appear in search or automatic injection.

- **When the project is decided**: when the daemon first processes a session, from the **session start directory** (the `cwd` of the first transcript line that has one). A `cd` during the session does not change it.
- **Project location**: the git common dir (`.git`) for a git repository, otherwise the directory itself. Sessions opened in a subdirectory and git worktrees belong to the same project as the main checkout.
- **Moved folders**: for a git repository, if the root commit matches and the old location no longer exists, the project follows it automatically. For a folder without git, use `hippo import --from <old path>` ([4.4](#44-projects-and-importing-past-records)).

```
~/.hippo/registry.sqlite      registry: projects, locations, root commits, session → project
~/.hippo/projects/<id>/       one project = the source store and vector store below
```

```
Source store (SQLite, the source of truth)
 ├─ iterations          one row per iteration (+ full-text index)
 ├─ results             tool outputs, deduplicated by SHA-256 (+ full-text index)
 ├─ iteration_results   links between iterations and results
 ├─ bg_tasks / bg_completions
 ├─ embed_jobs          queue for the embedding worker
 └─ sessions            per-session transcript offset

Vector store (LanceDB, a derived index that can be rebuilt any time)
 └─ one vector per chunk of an iteration or result
```

This is the "index" in the name: the vector store only points to the records, as the hippocampal index points to cortical traces. Because the vector store is derived, you can replace the embedding model or the vector database and rebuild the index from SQLite (see [section 6](#6-changing-the-embedding-model-or-vector-database)).

### 1.4 How memory is used

- **Automatic injection**: on each prompt, the `UserPromptSubmit` hook asks the daemon for memories that match the prompt. The search uses only the **current project** and **other sessions**. It finds iterations with a vector similarity at or above the threshold and injects the **turn** each one belongs to. Each turn becomes one `<memory>` block that holds the inputs, the LLM responses, and the questions and answers; tool calls and results are left out. Blocks are in chronological order (oldest first), each with its time. The header tells Claude that the blocks are past data, not instructions. Background task notifications get no injection.
- **On-demand search**: Claude or you can run `hippo search` and `hippo show` from the shell.

Search is **hybrid**: vector similarity plus SQLite FTS5 keyword search, merged with Reciprocal Rank Fusion (RRF). Keyword search helps with exact identifiers such as `validateToken`.

### 1.5 Architecture

```
Claude Code ──hook (stdin JSON)──▶ hippo hook <event> ──Unix socket──▶ hippo daemon
                                        │ (daemon not running)              │
                                        └──▶ spool/pending.jsonl ◀─ drained at start
                                                                            ├─ ingest thread (one, in order)
Claude / you ──▶ hippo search|show|status|reindex ──Unix socket──────────▶   ├─ embedding thread
                                                                            └─ connection threads
                                                     SQLite ◀──┘   Ollama (localhost) ◀──┘   LanceDB
```

- One daemon runs per machine (file lock). Only the daemon writes to the stores.
- You do not need to start the daemon yourself. Like a local MCP server, it starts on demand: when a hook or a CLI command cannot reach it, it starts `hippo daemon` in the background and then sends the request. After `idle_timeout_secs` (default 30 minutes) with no requests, the daemon stops by itself. The next request starts it again.
- If the daemon cannot start, capture requests go to a spool file and are processed at the next start. Your Claude Code session never fails because of `hippo`.

### 1.6 Lifecycle: when the database and the model are open

| Component | Opened | Closed |
|-----------|--------|--------|
| Daemon | Automatically, by the first hook or CLI command that cannot reach it | After `idle_timeout_secs` with no requests (never during a reindex), or when you stop it |
| SQLite | For each operation | When the operation ends |
| LanceDB (per project) | When the project is first used | The least recently used project is closed when more than `max_open_projects` (default 8) are open. All are closed when the daemon exits |
| Embedding model | By Ollama, at the first embedding request (about 1 s) | By Ollama, `keep_alive` (30 minutes) after the last request |
| Ollama server | Automatically, by the daemon when an embedding request cannot reach it (`ollama serve`, logs in `logs/ollama.log`). Skipped if Ollama is already running, for example as a service | Not stopped by `hippo`, because other tools may use the same server. The model memory is freed after `keep_alive` |

Hooks and CLI commands never open the database. They only talk to the daemon through the socket. Data is safe when the daemon stops: every change is one SQLite transaction (WAL mode), and pending embedding jobs stay in the queue.

---

## 2. Requirements

| Item | Version used | Notes |
|------|--------------|-------|
| macOS | Darwin (Apple Silicon) | Primary target. Uses Unix sockets and launchd. |
| Rust | 1.95+ (edition 2024) | For building |
| `protoc` | libprotoc 36+ | Needed to build LanceDB |
| Ollama | 0.40+ | Runs the embedding model locally |
| Embedding model | `qwen3-embedding:0.6b` | Default. Korean, English, and code. 32K context. |
| Claude Code | 2.1.316 | The hook payload and transcript format were verified with this version. |

---

## 3. Installation

### 3.1 Build

```bash
brew install protobuf            # if protoc is missing or broken: brew reinstall protobuf
git clone git@github.com:Biisairo/Hippocampal-Index.git hippo && cd hippo
cargo build --release
```

### 3.2 Where the binary lives

`hippo install` **copies itself to `~/.hippo/bin/hippo`** in the data root and uses that path in the hook commands and the Claude guide. The daemon also starts from this copy. So you can install straight from the build directory, and the hooks keep working after `cargo clean` or after you move the repository.

```bash
./target/release/hippo install --scope user --dry-run
```

- The hook commands and the Claude guide write the path with `~`, as in `~/.hippo/bin/hippo`, so shared settings files do not reveal your account name. (The launchd plist does not go through a shell, so it uses the absolute path.)
- After you rebuild, run `install` again to copy the new binary. If the content is the same, nothing is copied.
- The copy uses APFS file cloning, so it takes almost no extra disk space.
- `~/.hippo/bin/installs` records which settings files you installed into. When `uninstall` removes the last one, the copy is removed too.

In the examples below, commands before installing use `./target/release/hippo`, and commands after installing use `~/.hippo/bin/hippo`.

### 3.3 Ollama and the embedding model

```bash
brew install ollama
ollama pull qwen3-embedding:0.6b   # needs a running server once: ollama serve (or brew services start ollama)
```

You do not need to keep Ollama running. When `hippo` cannot reach it, the daemon starts `ollama serve` by itself (`[embedder] autostart = true`). If you prefer, run it as a service: `brew services start ollama`. `hippo` never downloads models for you.

Ollama listens on `127.0.0.1:11434` by default. `hippo` refuses any embedder URL that is not localhost.

### 3.4 Run the daemon (optional)

You can skip this step. Hooks and CLI commands start the daemon automatically when it is not running (`[daemon] autostart = true`), and it stops after 30 minutes without requests. Start it yourself only if you want to watch it or keep it running all the time.

Option A: run it in a terminal (good for a first test).

```bash
~/.hippo/bin/hippo daemon
```

Option B: register it with launchd so it starts at login.

```bash
mkdir -p ~/.hippo/logs && chmod 700 ~/.hippo ~/.hippo/logs
~/.hippo/bin/hippo launchd-plist > ~/Library/LaunchAgents/com.hippo.daemon.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.hippo.daemon.plist
```

launchd restarts the daemon only after a crash, not after an idle exit. To stop it: `launchctl bootout gui/$(id -u)/com.hippo.daemon`.

Check the daemon:

```bash
~/.hippo/bin/hippo status
```

### 3.5 Register the hooks

`hippo install` merges the hooks into a Claude Code settings file. It changes only `hippo` hook entries and keeps your other hooks and settings. **Preview first:**

```bash
# One project only: <project>/.claude/settings.local.json (personal file, not committed)
./target/release/hippo install --scope project --project ~/work/my-app --dry-run
./target/release/hippo install --scope project --project ~/work/my-app

# All sessions: ~/.claude/settings.json
./target/release/hippo install --scope user --dry-run
./target/release/hippo install --scope user
```

What `install` does:

- It prints a diff. With `--dry-run`, it writes nothing.
- Before it writes, it saves a backup: `<file>.hippo-backup-<unix time>`.
- It writes atomically and keeps the file permissions.
- If you run it again, nothing changes ("No change").
- It refuses to write through a symbolic link, and it does not touch a settings file that is not valid JSON.
- The file is rewritten as 2-space-indented JSON. Key order and values are kept, but the original formatting may change. The backup keeps the original.
- It adds an English guide block to a memory file, so Claude knows it can run `hippo search` and `hippo show`. The commands use the absolute path of the hippo executable you installed with, so `hippo` does not need to be on your PATH. `--scope user` uses `~/.claude/CLAUDE.md`, and `--scope project` uses `<project>/CLAUDE.local.md` (the personal file the official docs recommend). The block sits between `<!-- hippo:begin -->` and `<!-- hippo:end -->`. Claude Code strips these markers before they reach the context, so they cost no tokens. Text outside the block is kept. If `CLAUDE.local.md` is not ignored by git, it prints a warning but does not change `.gitignore`.

You can also print the hook block and add it by hand: `hippo hooks-config`. `uninstall` removes only the hook entries and the guide block. A settings file or memory file that held only hippo content is deleted without a backup, and a `.claude/` directory left empty by that is removed. A file that keeps other content is backed up before it is changed.

Start a new Claude Code session after you install the hooks.

### 3.6 Uninstall

```bash
~/.hippo/bin/hippo uninstall --scope user          # or --scope project --project <path>
launchctl bootout gui/$(id -u)/com.hippo.daemon
rm ~/Library/LaunchAgents/com.hippo.daemon.plist
rm -rf ~/.hippo                          # deletes all stored memory
```

---

## 4. Usage

After installation, capture and injection run automatically. You can also use these commands:

### 4.1 Search

```bash
hippo search "token expiry bug"                         # iterations of the current directory's project
hippo search "validateToken" --scope all --kind both    # all projects, iterations and results
hippo search "deploy" --scope ~/work/my-app --limit 5    # the project that contains this path
```

| Option | Values | Default |
|--------|--------|---------|
| `--scope` | `project` (the current directory's project), `all`, or a path | `project` |
| `--kind` | `iteration`, `result`, `both` | `iteration` |
| `--limit` | number | `10` |

Each hit shows the RRF score, the vector similarity (`sim=-` means a keyword-only hit), the `prompt_id`, and a preview. For a result hit, the output lists the prompts that produced it. `--scope all` searches each project, merges the lists with RRF, and labels each hit with `project=<id>`. If the current directory belongs to no project, the output is "No project".

### 4.2 Show a whole turn

```bash
hippo show <prompt_id>                  # all iterations of that prompt, in order
hippo show <prompt_id> --with-results   # also print the full tool outputs
hippo show <prompt_id> --scope project  # look only in the current project
```

Without `--scope`, `show` looks in the current project first, then in the other projects, most recently used first. The output includes the completion records of background tasks.

### 4.3 Status

```bash
hippo status
```

```
spool_pending: 0                  # requests captured while the daemon was down
embed_model: qwen3-embedding:0.6b
reindex: idle                     # idle | running | done | failed: <project id>: <reason>
projects: 2
[3f9c…] /Users/me/work/app/.git
  iterations: 1520  results: 980  embed_jobs: 0  vectors: 2610  compatibility: compatible
[a71d…] /Users/me/notes  (location missing)
  iterations: 40  results: 12  embed_jobs: 0  vectors: 52  compatibility: compatible
```

`embed_jobs` counts iterations and results still waiting for embedding. If jobs were processed in the last 5 minutes, the rate and the estimated time left are shown too, for example `embed_jobs: 601 (120/min, about 6 min left)`. `compatibility` is `empty`, `compatible`, or `mismatch`. `(location missing)` means the location no longer exists. If a project needs a reindex, `status` tells you.

### 4.4 Projects and importing past records

```bash
hippo project list                       # id, kind (git/dir), location, iterations, last activity (local time YYYY-MM-DD HH:MM:SS)
hippo import                             # past sessions of the current folder (subfolders and worktrees too) into the current project
hippo import --from ~/old/app            # sessions you ran at an old path into the current folder's project
hippo import --from <project id> --to ~/work/app
hippo import --wait                      # after importing, show progress until embedding is done
```

`import` reads the past transcripts that Claude Code keeps (`~/.claude/projects/*/*.jsonl`) and adds their records to the project of the `--to` folder. An argument you leave out is the current folder. Subagent transcripts are not read.

- **Which sessions**: sessions whose start path is the `--from` path or below it. The path does not need to exist any more. If `--from` is a git repository, sessions opened in its worktrees are included too. Older sessions are processed first.
- **If `--from` already has a project**: that project is first moved into the `--to` project. If `--to` has no project, only the location changes. Otherwise the records are merged and the old project is deleted. When merging, vectors are copied if the embedding model is the same, and embedded again if it is not. The old location is not kept, so later sessions go to the current folder's project without any extra mapping.
- **Sessions already captured live**: only the part before the point where live capture started is filled in. Iterations that already exist are not stored again.
- **The iteration at the end of a transcript**: stored if every tool call has its result. If a tool call has no result yet, it is held. A held iteration is processed when the session is reopened or when you run `import` again.
- **Copies from branched sessions**: an iteration is skipped if an iteration with the same `message.id` already exists under another session.
- **Sessions fixed to another project**: skipped and reported as "other project".

Progress is printed one line per session:

```
Candidate sessions: 120
[1/120] d767e162 imported  iterations 42  pending embeddings 310
[2/120] 2f74fd43 imported  iterations 3  pending embeddings 318
...
Total: 120 sessions; imported 118, unchanged 2, other project 0, errors 0. Stored 2410 iterations, skipped 101 fork copies.
```

You can stop with Ctrl-C at any time. Run the same command again and finished sessions pass as "unchanged" while the rest continue. The daemon keeps working on the remaining embedding jobs. It always handles live capture and its embeddings before import work, so your current conversation is still stored and searchable while an import runs.

### 4.5 Let Claude use the memory

Automatic injection gives Claude only the best few matches. For more, Claude can call the CLI. `hippo install` puts a guide block that tells Claude how into the memory file (see [3.5](#35-register-the-hooks)), so you do not need to write it yourself.

---

## 5. Configuration

File: `~/.hippo/config.toml`. Every key is optional. Unknown keys are an error, so typos are caught. Restart the daemon after you change it. Only `[ui] language` takes effect from the next command without a restart.

```toml
[embedder]
backend = "ollama"                 # ollama | hash (test only)
model = "qwen3-embedding:0.6b"
dimension = 1024                   # Qwen3-Embedding supports 32–4096
max_input_tokens = 32000
url = "http://127.0.0.1:11434"     # must be localhost, 127.0.0.1, or ::1
keep_alive = "30m"                 # keeps the model loaded to avoid a ~1 s cold start
autostart = true                   # start `ollama serve` when the server is not reachable
ollama_command = "ollama"          # path of the ollama executable

[vector]
backend = "lancedb"                # lancedb | memory (test only)
name = "default"                   # directory: projects/<id>/vectors/<name>/

[chunking]
max_tokens = 1000                  # chunk limit = min(model limit, this) × chars_per_token
chars_per_token = 1.0
overlap_chars = 200                # characters shared by neighboring chunks; must be below the chunk limit

[embed_text]
max_tool_input_chars = 500         # tool-input JSON is cut here in the embedded text only ([answer] is not cut)
answer_tools = ["AskUserQuestion", "ExitPlanMode"]   # tools whose result (your answer) is added as [answer]

[inject]
threshold = 0.40                   # minimum vector similarity for automatic injection
top_k = 3                          # number of turns to inject
total_chars = 12000                # total injection limit; a turn that does not fit is skipped whole, never cut
budget_ms = 800                    # time budget for the UserPromptSubmit hook
cold_budget_ms = 2500              # budget when the hook has to start the daemon (cold start); at least budget_ms

[index]
vector_index_threshold = 100000    # build an ANN index when the vector count reaches this
job_batch = 64

[daemon]
autostart = true                   # hooks and CLI start the daemon when it is not running
idle_timeout_secs = 1800           # stop after this many seconds without requests (0 = never)
max_open_projects = 8              # project vector stores kept open at the same time

[project]
git_timeout_ms = 1000              # time limit for git; if exceeded, the folder is treated as non-git

[import]
transcripts_dir = "/Users/me/.claude/projects"   # where past transcripts are found. Default: $HOME/.claude/projects (`~` is not expanded)

[ui]
language = "en"                    # CLI output language: en (default) or ko. Text for Claude and --help are always English

[[redact.patterns]]                # extra secret patterns (regex)
kind = "internal"
pattern = 'HCI-\d{6}'
```

**`inject.threshold` is a tuning value, not a final number.** In a small test with `qwen3-embedding:0.6b`, relevant memories scored 0.46–0.68 and unrelated ones 0.21–0.30. Change it based on your own data.

---

## 6. Changing the embedding model or vector database

The vector store is a derived index, so you can switch at any time:

```
1. Edit config.toml (for example, a new model, dimension, or backend)
2. Restart the daemon       → status shows "mismatch". Keyword search still works.
3. hippo reindex             → rebuilds every project, one after another, in the background into a new generation
4. When it finishes         → each project's ACTIVE pointer switches atomically. Old generations are removed when that project is next opened.
```

To reindex one project only, use `hippo reindex --scope project` (the current directory's project) or `hippo reindex --scope <path>`.

- During a reindex, search uses the old store. New iterations wait in the queue and go into the new store after the switch, so nothing is lost.
- A second `hippo reindex` while one is running is refused.
- If one project fails, the reindex continues with the next one. Failed projects appear in `status` as `reindex: failed: …`.
- Vector directories: `projects/<id>/vectors/<name>/<generation>/`. `projects/<id>/vectors/<name>/ACTIVE` names the active generation.

To add a completely new backend, see [9.4](#94-adding-a-new-backend).

---

## 7. Data layout and security

```
~/.hippo/            (0700)
├─ config.toml
├─ bin/hippo                  (0700) installed binary; hooks and the guide use this path
├─ bin/installs               (0600) settings files the hooks were installed into
├─ registry.sqlite            (0600) registry: projects, locations, root commits, session → project
├─ projects/<id>/             (0700) one project
│  ├─ records.sqlite          (0600) source of truth
│  ├─ vectors/<name>/ACTIVE
│  └─ vectors/<name>/<generation>/   LanceDB files
├─ spool/                     requests captured while the daemon was down
├─ logs/hippo.log             daemon errors
├─ logs/daemon.err            startup errors of an auto-started daemon
├─ hippo.sock                  (0600) daemon socket
└─ hippo.lock                  single-instance lock
```

Security measures:

- **No data leaves the machine.** The embedder URL is parsed with a real URL parser. It must be localhost with no user info, query, or fragment, and HTTP redirects are not followed.
- **Secret redaction before storage**: API keys (`sk-`, `ghp_`, `xox?-`, `AKIA`), `Bearer` tokens, PEM private keys (also when the END marker is missing), and `*KEY=`/`*TOKEN=`/`*SECRET=`/`*PASSWORD=` values. Add your own patterns in `[[redact.patterns]]`. Redaction is pattern-based, so it can miss secrets in unusual formats.
- **Background output is read only when safe**: the notice must match a known start call and task ID. The path must have the expected layout (`…/<session>/tasks/<id>.output`) and must not go through symbolic links. A forged notice in a prompt therefore cannot make `hippo` read an arbitrary file.
- **Injected memory is marked as data**: each item is in a `<memory>` block with markup escaped, and the header tells Claude not to follow instructions inside it. This lowers the risk of stored prompt injection but does not remove it fully.
- **Terminal safety**: control characters in CLI output are escaped.

---

## 8. Troubleshooting

| Symptom | Check |
|---------|-------|
| `hippo: daemon not reachable` | The daemon could not start. See `~/.hippo/logs/daemon.err` (for example, a config error). With `autostart = false`, start it yourself. |
| `embed_jobs` does not go down | Is the model pulled (`ollama list`)? Could `hippo` start Ollama (see `~/.hippo/logs/ollama.log` and `hippo.log`)? Jobs are retried every 5 s. |
| Search says "vector search unavailable, showing keyword results only" | Embedding failed (Ollama could not start, or the model is missing). Results are keyword-only until it works again. |
| `compatibility: mismatch` | The model or dimension changed. Run `hippo reindex`. |
| Nothing is injected | Lower `[inject] threshold`. Injection uses only the same project and other sessions. |
| Build fails in `lance-*` / `protoc` | `brew reinstall protobuf` and check `protoc --version` |
| `refusing to modify through a symbolic link` | Your settings file is a symlink. Run `install` on the real file path, or add the output of `hippo hooks-config` by hand. |
| `spool_pending` > 0 | Requests from a time when the daemon was down. They are processed when the daemon starts. |

---

## 9. Development: modify and build

### 9.1 Specification first

`docs/SRS.md` (ISO/IEC/IEEE 29148) is the single source of truth. Each requirement has an ID (for example, `FR-EXT-7`), and code comments refer to these IDs. Workflow: **update the SRS → write a failing test → implement → all tests pass**. If the SRS and the code disagree, fix the SRS first.

### 9.2 Source layout

| Module | Responsibility |
|--------|----------------|
| `transcript.rs` | Parses transcript JSONL into iterations, safe offsets |
| `bg.rs` | Background start detection, `<task-notification>` parsing, subagent report extraction |
| `redact.rs` | Secret redaction |
| `embed_text.rs` | Text to embed (`[input]`/`[tool]`/`[response]` lines), chunking |
| `shell.rs` | Parses Bash commands (brush-parser) to tell whether hippo is run |
| `store.rs` | SQLite source store, deduplication, FTS5, project merge |
| `project.rs` | path normalization, git information, project location |
| `registry.rs` | registry: project resolution, session binding, relocation, absorption |
| `import.rs` | candidate selection for importing past records |
| `queue.rs` | two-lane queues that take live work before import work |
| `embedder.rs`, `vector.rs` | `Embedder` and `VectorStore` traits |
| `ollama.rs`, `lance.rs` | Real backends |
| `testing.rs` | `HashEmbedder`, `MemoryVectorStore` (deterministic test backends) |
| `backends.rs` | Builds a backend from the config. **The only file to change when you add a backend.** |
| `indexer.rs` | Embedding queue, reindex, compatibility check |
| `generations.rs` | Vector store generations and the `ACTIVE` pointer |
| `ingest.rs` | Transcript → redact → store pipeline |
| `search.rs` | Hybrid search, injection, `search`/`show` output |
| `daemon.rs`, `client.rs`, `protocol.rs`, `spool.rs` | Daemon, hook client, socket protocol, spool |
| `install.rs`, `setup.rs` | Settings merge, hook JSON, launchd plist |
| `config.rs` | `config.toml` and data paths |

### 9.3 Build and test

```bash
cargo build                    # debug
cargo build --release          # release (~4 min the first time, because of LanceDB)
cargo test                     # all deterministic tests
cargo test -- --ignored        # live test against the local Ollama
cargo clippy --all-targets     # must have no warnings
cargo fmt
```

Test fixtures in `tests/fixtures/` are real transcripts, reduced to the `user` and `assistant` lines. `tests/fixtures/hooks/` holds recorded hook inputs. To make new ones, do not touch your `~/.claude` settings. Write a separate settings file whose hooks save each hook input to a file, and run `claude -p --settings <that file>`. Then keep only the `user` and `assistant` lines of the transcript and replace local paths with something like `/Users/user`.

### 9.4 Adding a new backend

1. Implement `Embedder` (`src/embedder.rs`) or `VectorStore` (`src/vector.rs`).
2. Run the shared contract test from `tests/common/contract.rs` against it (`embedder_contract` or `vector_store_contract`), like `tests/lance_test.rs` does.
3. Add a name for it in `src/backends.rs` (`build_embedder` or `vector_source`).
4. Select it in `config.toml`, restart the daemon, and run `hippo reindex`.

Ingest, search, and the daemon do not change.

---

## 10. Known limitations

- Automatic injection quality depends on `inject.threshold`. Tune it with real data (SRS OPEN-5).
- Two-character Korean words (for example, "버그") are not found by keyword search (the FTS5 trigram tokenizer needs 3 or more characters). Vector search covers them.
- Not yet checked in interactive sessions: whether a hook fires after a background notice (OPEN-1), the Esc interrupt (OPEN-2), compaction (OPEN-3), and whether the last line is written before `Stop` fires (OPEN-6).
- Hooks always use the default data root (`~/.hippo`). `--root` is not written into hook commands.
- When the daemon stops with a signal, the socket file stays. It is removed at the next start.
- There is no retention policy yet. Memory grows until you delete it.
- Results (tool outputs) are deduplicated only within a project. The same output in two projects is stored twice.
- A moved folder without git is not followed automatically. Use `hippo import --from <old path>`.
- Live capture stores from the turn the hook reports. Use `hippo import` for the history before it.
- For past sessions whose old path is gone, `hippo` cannot tell which project they belong to. Give the old path with `--from`.
- Data in the single-store layout of v0.1 (`~/.hippo/records.sqlite`) is not migrated to the new layout.
- macOS only for now.
