# Configuration

[← README](../../README.md) · [한국어](../ko/configuration.md)

The configuration file is `~/.hippo/config.toml` (`config.toml` in the data root). We recommend editing it in the app's [Settings tab](usage.md#settings-tab). The Settings tab saves only the keys you changed and does not write invalid values.

## Contents

1. [Basic rules](#1-basic-rules)
2. [All keys](#2-all-keys)
3. [How the injection defaults were chosen](#3-how-the-injection-defaults-were-chosen)
4. [Changing the embedding model or vector DB](#changing-the-embedding-model-or-vector-db)

---

## 1. Basic rules

- Every key is optional. Without the file, every key has its default.
- An unknown key is an error, so typos show up at once.
- After changing settings, restart the daemon. The Settings tab shows a **Restart daemon** button when needed.
- These keys apply without a restart: `[ui] language`, `[ui] theme`, `[ui] start_tab`, `[inject] enabled`, `[capture] enabled`.

## 2. All keys

All values below are the defaults (`[[redact.patterns]]` and `transcripts_dir` are examples).

```toml
[embedder]
backend = "ollama"                 # ollama | hash (for tests)
model = "qwen3-embedding:0.6b"
dimension = 1024                   # Qwen3-Embedding supports 32 to 4096
max_input_tokens = 32000
url = "http://127.0.0.1:11434"     # only localhost, 127.0.0.1, ::1
keep_alive = "30m"                 # keeps the model in memory to avoid ~1 s on the first call
autostart = true                   # start `ollama serve` if the server cannot be reached
ollama_command = "ollama"          # path to the ollama binary
query_prefix = "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:"
                                   # instruction put only before search queries (recommended for Qwen3-Embedding).
                                   # Empty means none. No reindex needed

[vector]
backend = "lancedb"                # lancedb | memory (for tests)
name = "default"                   # directory: projects/<id>/vectors/<name>/

[chunking]
max_tokens = 1000                  # chunk limit = min(model limit, this) × chars_per_token
chars_per_token = 1.0
overlap_chars = 200                # characters shared by neighbor chunks; must be smaller than the chunk limit

[embed_text]
max_tool_input_chars = 500         # cuts tool input JSON to this length, in the embedding text only ([answer] is not cut)
answer_tools = ["AskUserQuestion", "ExitPlanMode"]   # tools whose result (the user's answer) goes in as [answer]

[inject]
threshold = 0.50                   # minimum vector similarity for automatic injection
top_k = 3                          # number of turns to inject
total_chars = 8000                 # length limit of the whole injection; a turn over it is dropped whole, not cut
budget_ms = 800                    # time budget of the UserPromptSubmit hook
cold_budget_ms = 2500              # budget when the hook has to start the daemon (cold start); at least budget_ms
min_prompt_chars = 11              # prompts shorter than this (without surrounding spaces) get no injection
enabled = true                     # false pauses injection from the next prompt (no daemon restart)

[capture]
enabled = true                     # false pauses saving; on resume, open sessions also save what they missed

[index]
vector_index_threshold = 100000    # build an ANN index when the vector count reaches this
job_batch = 64
optimize_every = 20                # after this many vector store changes, merge small files and drop old versions
                                   # (0: only when the queue is empty)

[daemon]
autostart = true                   # hooks and the CLI start the daemon if it is not running
idle_timeout_secs = 1800           # exit after this many seconds without requests (0: never)
max_open_projects = 8              # number of project vector stores kept open
prevent_sleep = true               # keep the Mac from idle sleep while work is pending (closing the lid still sleeps)

[project]
git_timeout_ms = 1000              # time limit for git; over it, the folder counts as non-git

[import]
transcripts_dir = "/Users/me/.claude/projects"   # where past transcripts are found
                                                 # default: $HOME/.claude/projects (`~` is not expanded)

[ui]
language = "en"                    # language of CLI and window text: en (default) or ko
                                   # text for Claude and --help are always English
theme = "system"                   # app theme: system (default, follows macOS), light, dark
start_tab = "about"                # tab shown when the window opens: about (default), status, records, projects,
                                   # search, import, claude_setup, settings, last (the tab you saw last)

[[redact.patterns]]                # extra secret patterns (regular expressions)
kind = "internal"
pattern = 'HCI-\d{6}'
```

With `start_tab = "last"`, the window opens the tab recorded in the `gui-last-tab` file in the data root. The file is updated every time you change tabs. When `hippo` is not installed and the start tab is not **About** or **Claude setup**, the window opens **Claude setup**.

## 3. How the injection defaults were chosen

The defaults of `inject.threshold`, `min_prompt_chars`, and `query_prefix` come from measurement.

- An evaluation set was built from this project's own past records: 60 questions with known answers and 30 prompts that need no memory.
- Compared with 0.40 and no query instruction, the hit rate stayed the same (53/60), and wrong injections dropped from 28/30 to 16/30 (SRS appendix C).

How to adjust:

- To cut wrong injections further, raise `threshold` to 0.55–0.60. Some useful memories will be missed too.
- If you change the embedding model, change `query_prefix` to the query format that model recommends.
- Before changing values, check the result with the [injection preview](usage.md#query-this-project).

---

## Changing the embedding model or vector DB

The vector store is an index derived from the SQLite records, so you can rebuild it at any time.

1. Change the model, dimension, or backend in the Settings tab (or `config.toml`).
2. Restart the daemon. The compatibility in the Status tab becomes `mismatch`. Keyword search keeps working meanwhile.
3. Press **Reindex** in the [Projects tab](usage.md#projects-tab) (`hippo reindex`). It reindexes every project into a new generation, one by one, in the background.
4. When done, each project's `ACTIVE` pointer switches atomically. The old generation is deleted the next time that project is opened.

- To reindex one project only, set the scope to `project` (the current folder's project) or a path.
- During a reindex, search uses the old index. Iterations that arrive meanwhile wait in the queue and go into the new index after the switch, so nothing is lost.
- Requesting a reindex while one is running is refused.
- If one project fails, the others still continue. A failed project shows in the status as `reindex: failed: …`.
- The vector directory is `projects/<id>/vectors/<name>/<generation>/`, and `projects/<id>/vectors/<name>/ACTIVE` points to the generation in use.

To add a completely new backend, see [Development and contributing](development.md#adding-a-backend).
