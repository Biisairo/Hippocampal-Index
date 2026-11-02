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

### 1.2 How iterations are captured (hooks)

| Iteration type | Hook that triggers capture |
|----------------|----------------------------|
| Iteration with tool calls | `PostToolBatch` (fires once per batch, after all parallel tool calls finish) |
| Last iteration (text only) | `Stop` / `StopFailure` |
| Leftovers at session end | `SessionEnd` |

These hooks run with `async: true`, so they never slow down your session. The hook payload is only a trigger. The content comes from the session transcript (JSONL). `hippo` keeps a safe byte offset per session, so it reads each line once and never records the same iteration twice.

Other details:

- **Subagents**: `hippo` skips their internal iterations. A foreground subagent's final report arrives as the result of the `Agent` tool call, so it is stored with that call.
- **Background tasks** (`run_in_background` for Bash or Agent): the start call is stored as *pending*. When the completion notice (`<task-notification>`) appears in the transcript, `hippo` reads the output and links it to the start call. The call and its response become one pair.

### 1.3 Storage model

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

- **Automatic injection**: on each prompt, the `UserPromptSubmit` hook asks the daemon for memories that match the prompt. The search uses only the **current project** (same `cwd`) and **other sessions**. Only results with a vector similarity at or above the threshold are added to Claude's context. Each item is wrapped in a `<memory>` block, and the header tells Claude that the block is past data, not an instruction.
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
- If the daemon is down, capture requests go to a spool file and are processed at the next start. Your Claude Code session never fails because of `hippo`.

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

### 3.2 Put the binary in a fixed location

The hook commands contain the **absolute path** of the `hippo` binary that ran `hippo install`. Put the binary in a stable place first.

```bash
mkdir -p ~/.local/bin
cp target/release/hippo ~/.local/bin/hippo
```

Use `~/.local/bin/hippo` for all later steps.

### 3.3 Ollama and the embedding model

```bash
brew install ollama
brew services start ollama       # starts now and at every login
ollama pull qwen3-embedding:0.6b
```

Ollama listens on `127.0.0.1:11434` by default. `hippo` refuses any embedder URL that is not localhost.

### 3.4 Run the daemon

Option A: run it in a terminal (good for a first test).

```bash
~/.local/bin/hippo daemon
```

Option B: register it with launchd so it starts at login.

```bash
mkdir -p ~/.hippo/logs && chmod 700 ~/.hippo ~/.hippo/logs
~/.local/bin/hippo launchd-plist > ~/Library/LaunchAgents/com.hippo.daemon.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.hippo.daemon.plist
```

To stop it: `launchctl bootout gui/$(id -u)/com.hippo.daemon`.

Check the daemon:

```bash
~/.local/bin/hippo status
```

### 3.5 Register the hooks

`hippo install` merges the hooks into a Claude Code settings file. It changes only `hippo` hook entries and keeps your other hooks and settings. **Preview first:**

```bash
# One project only: <project>/.claude/settings.local.json (personal file, not committed)
~/.local/bin/hippo install --scope project --project ~/work/my-app --dry-run
~/.local/bin/hippo install --scope project --project ~/work/my-app

# All sessions: ~/.claude/settings.json
~/.local/bin/hippo install --scope user --dry-run
~/.local/bin/hippo install --scope user
```

What `install` does:

- It prints a diff. With `--dry-run`, it writes nothing.
- Before it writes, it saves a backup: `<file>.hippo-backup-<unix time>`.
- It writes atomically and keeps the file permissions.
- If you run it again, nothing changes ("변경 없음" = no change).
- It refuses to write through a symbolic link, and it does not touch a settings file that is not valid JSON.
- The file is rewritten as 2-space-indented JSON. Key order and values are kept, but the original formatting may change. The backup keeps the original.

You can also print the hook block and add it by hand: `hippo hooks-config`.

Start a new Claude Code session after you install the hooks.

### 3.6 Uninstall

```bash
~/.local/bin/hippo uninstall --scope user          # or --scope project --project <path>
launchctl bootout gui/$(id -u)/com.hippo.daemon
rm ~/Library/LaunchAgents/com.hippo.daemon.plist
rm -rf ~/.hippo                          # deletes all stored memory
```

---

## 4. Usage

After installation, capture and injection run automatically. You can also use these commands:

### 4.1 Search

```bash
hippo search "token expiry bug"                         # current directory's project, iterations
hippo search "validateToken" --scope all --kind both    # all projects, iterations and results
hippo search "deploy" --scope ~/work/my-app --limit 5
```

| Option | Values | Default |
|--------|--------|---------|
| `--scope` | `cwd`, `all`, or a project path | `cwd` |
| `--kind` | `iteration`, `result`, `both` | `iteration` |
| `--limit` | number | `10` |

Each hit shows the RRF score, the vector similarity (`sim=-` means a keyword-only hit), the `prompt_id`, and a preview. For a result hit, the output lists the prompts that produced it.

### 4.2 Show a whole turn

```bash
hippo show <prompt_id>                  # all iterations of that prompt, in order
hippo show <prompt_id> --with-results   # also print the full tool outputs
```

The output includes the completion records of background tasks.

### 4.3 Status

```bash
hippo status
```

```
iterations: 1520
results: 980
embed_jobs: 0          # iterations/results still waiting for embedding
spool_pending: 0       # requests captured while the daemon was down
vectors: 2610
embed_model: qwen3-embedding:0.6b
compatibility: compatible   # empty | compatible | mismatch
reindex: idle               # idle | running | done | failed: <reason>
```

If a reindex is needed, `status` tells you.

### 4.4 Let Claude use the memory

Automatic injection gives Claude only the best few matches. For more, Claude can call the CLI. Add a line like this to your `CLAUDE.md`:

```
To look up past sessions, run `hippo search "<query>"` or `hippo show <prompt_id>` with Bash.
```

---

## 5. Configuration

File: `~/.hippo/config.toml`. Every key is optional. Unknown keys are an error, so typos are caught. Restart the daemon after you change it.

```toml
[embedder]
backend = "ollama"                 # ollama | hash (test only)
model = "qwen3-embedding:0.6b"
dimension = 1024                   # Qwen3-Embedding supports 32–4096
max_input_tokens = 32000
url = "http://127.0.0.1:11434"     # must be localhost, 127.0.0.1, or ::1
keep_alive = "30m"                 # keeps the model loaded to avoid a ~1 s cold start

[vector]
backend = "lancedb"                # lancedb | memory (test only)
name = "default"                   # directory: vectors/<name>/

[chunking]
max_tokens = 8000                  # chunk limit = min(model limit, this) × chars_per_token
chars_per_token = 1.0

[embed_text]
max_tool_input_chars = 500         # tool-input JSON is cut here in the embedded text only

[inject]
threshold = 0.40                   # minimum vector similarity for automatic injection
top_k = 3
per_item_chars = 800
total_chars = 4000
budget_ms = 800                    # time budget for the UserPromptSubmit hook

[index]
vector_index_threshold = 100000    # build an ANN index when the vector count reaches this
job_batch = 64

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
3. hippo reindex             → rebuilds everything in the background into a new generation
4. When it finishes         → the ACTIVE pointer switches atomically. Old generations are removed at the next start.
```

- During a reindex, search uses the old store. New iterations wait in the queue and go into the new store after the switch, so nothing is lost.
- A second `hippo reindex` while one is running is refused.
- Vector directories: `vectors/<name>/<generation>/`. `vectors/<name>/ACTIVE` names the active generation.

To add a completely new backend, see [9.4](#94-adding-a-new-backend).

---

## 7. Data layout and security

```
~/.hippo/            (0700)
├─ config.toml
├─ records.sqlite             (0600) source of truth
├─ vectors/<name>/ACTIVE
├─ vectors/<name>/<generation>/   LanceDB files
├─ spool/                     requests captured while the daemon was down
├─ logs/hippo.log             daemon errors
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
| `hippo: daemon not reachable` | Is the daemon running? `hippo daemon` or the launchd steps |
| `embed_jobs` does not go down | Is Ollama running (`curl http://127.0.0.1:11434/api/version`)? Is the model pulled? See `~/.hippo/logs/hippo.log`. |
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
| `embed_text.rs` | Text to embed (`[입력]`/`[툴]`/`[응답]` lines), chunking |
| `store.rs` | SQLite source store, deduplication, FTS5 |
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

Test fixtures in `tests/fixtures/` are real transcripts, reduced to the `user` and `assistant` lines. To make new ones, record hooks without touching your settings (`claude -p --settings probe/settings.json ...`), then run `python3 probe/make_fixtures.py tests/fixtures` (edit the session IDs in the script).

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
- macOS only for now.
