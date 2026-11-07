# CLI reference

[← README](../../README.md) · [한국어](../ko/cli.md)

Everything you do in the app window can also be done with terminal commands. Claude also uses these commands to find memory. For the app window, see [Using the app](usage.md).

## Contents

- [Commands](#commands)
- [Binary and data root](#binary-and-data-root)
- [`install`](#install) · [`uninstall` and `purge`](#uninstall-and-purge)
- [`search`](#search) · [`show`](#show)
- [`status` and `stop`](#status-and-stop)
- [`project`](#project) · [`import`](#import) · [`forget`](#forget)
- [`reindex`](#reindex)
- [`daemon` and launchd](#daemon-and-launchd)
- [`hooks-config`](#hooks-config)
- [Output language](#output-language)

---

## Commands

| Command | What it does | In the app |
|---------|--------------|------------|
| `gui` | Opens the app window | — |
| `install` | Installs the binary, adds hooks and the guide block to Claude Code, and makes the PATH link | Claude setup → Apply |
| `uninstall` | Removes the hooks, the guide block, and the PATH link. The binary and records stay | Claude setup → Remove |
| `purge` | Uninstalls, then deletes the data root with all records | Claude setup → Delete everything |
| `search` | Searches memory | Search |
| `show` | Shows the iterations of one turn in order | Search → Show a turn, Records |
| `status` | Shows the daemon state and per-project statistics | Status |
| `stop` | Stops the daemon and any Ollama server that `hippo` started | Status → Stop |
| `project list` / `project remove` | Lists projects / deletes a project | Projects, Records → Delete project |
| `import` | Imports past session records | Import |
| `forget` | Deletes one session or one turn | Records → Delete |
| `reindex` | Rebuilds vector indexes | Projects → Reindex |
| `daemon` | Runs the daemon in the foreground | — |
| `hooks-config` | Prints the hook settings for `settings.json` | — |
| `launchd-plist` | Prints a launchd plist | — |
| `hook` | The Claude Code hook entry point. Do not run it yourself | — |

`hippo <command> --help` shows the options of each command.

---

## Binary and data root

- Installation **copies the binary to `bin/hippo` in the data root (default `~/.hippo/bin/hippo`)**. The hooks, the Claude guide, and the daemon all use this copy.
- **The data root comes from where the binary is.** Running `<root>/bin/hippo` makes `<root>` the data root. There is no `--root` option. To use another data root, install into it with `install --data-dir <path>`.
- A binary that is not installed (the one inside the app, `target/release/hippo`) can run only `install` and `gui`.
- Installation makes a link at `~/.local/bin/hippo` that points to the installed binary. If `~/.local/bin` is on your PATH, you can run `hippo` directly. If not, it only prints a warning and does not change your shell settings. If `~/.local/bin/hippo` already points somewhere else, it changes nothing and ends with an error.
- Hook commands and the Claude guide write the path with `~`, such as `~/.hippo/bin/hippo`, so a shared settings file does not show your account name. The launchd plist does not go through a shell, so it uses an absolute path.
- The copy uses APFS file cloning, so it uses almost no extra disk space.

In the examples below, `hippo` is the installed binary. Without the PATH link, read it as `~/.hippo/bin/hippo`.

---

## `install`

```bash
./target/release/hippo install --dry-run     # print the changes only
./target/release/hippo install

# to change the data root or the Claude folder
./target/release/hippo install --data-dir ~/hippo-data --claude-dir ~/work/my-app/.claude
```

| Option | Default |
|--------|---------|
| `--data-dir` | This binary's data root, else `~/.hippo` |
| `--claude-dir` | The Claude folder recorded at install, else `~/.claude` |
| `--dry-run` | Print the changes without writing |

What `install` does:

- Prints the changes (a diff). With `--dry-run`, it writes nothing.
- Merges hooks into `<Claude folder>/settings.json`. It changes only the `hippo` hook entries and keeps other hooks and settings.
- Before writing, it backs up the original to `<file>.hippo-backup-<unix time>`. It replaces the file atomically and keeps its permissions.
- Running it again with the same result changes nothing ("No change").
- It does not write through symbolic links, and it does not touch a settings file with broken JSON.
- The file is rewritten as JSON with 2-space indentation. Key order and values are kept, but the original formatting may change. The original stays in the backup.
- Adds an English guide block to `<Claude folder>/CLAUDE.md` that tells Claude it can use `hippo search` and `hippo show`. The commands use the install path, so `hippo` does not need to be on your PATH. The block sits between `<!-- hippo:begin -->` and `<!-- hippo:end -->`. Claude Code strips these markers before adding the file to the context, so they cost no tokens. Text outside the block stays.
- Records the Claude folder it used in `<data root>/bin/claude-dir`. Later `install`, `uninstall`, `purge`, and the app use this value when `--claude-dir` is not given.
- If the binary content is the same, it does not copy. If the binary changed, it stops the old daemon, and the next request starts the daemon with the new binary.

After registering the hooks, start a new Claude Code session.

## `uninstall` and `purge`

```bash
hippo uninstall     # removes only the hooks, the guide block, and the PATH link; records stay
hippo purge         # uninstalls, then deletes the whole data root; all stored memory is deleted
```

- `uninstall` deletes a settings file or memory file that held only hippo content, without a backup, and deletes a `.claude/` directory left empty by that. A file that keeps other content is backed up, then edited. If you remove right after installing, the memory file goes back to the original byte for byte.
- `purge` shows the path it will delete and asks for confirmation (`--yes` skips it). It stops the daemon and any Ollama server that `hippo` started, then deletes. It does not delete while the daemon is reindexing.

---

## `search`

```bash
hippo search "token expiry bug"                         # iterations in the project of the current directory
hippo search "validateToken" --scope all --kind both    # iterations and results in every project
hippo search "deploy" --scope ~/work/my-app --limit 5   # the project that path belongs to
```

| Option | Values | Default |
|--------|--------|---------|
| `--scope` | `project` (the current directory's project), `all`, or a path | `project` |
| `--kind` | `iteration`, `result`, `both` | `iteration` |
| `--limit` | number | `10` |

- Results show the RRF score, vector similarity (`sim=-` means a keyword-only match), `prompt_id`, and a preview.
- For a result (tool output), the prompts that produced it are listed too.
- `--scope all` searches each project, merges with RRF, and adds `project=<id>` to each result.
- If the current directory belongs to no project, it prints "No project".

## `show`

```bash
hippo show <prompt_id>                  # the iterations of that prompt, in order
hippo show <prompt_id> --with-results   # including the raw tool output
hippo show <prompt_id> --scope project  # look only in the current project
```

Without `--scope`, it looks in the current project first, then in the other projects, most recently used first. Completion records of background tasks are shown too.

---

## `status` and `stop`

```bash
hippo status
hippo stop       # stops the daemon and any Ollama that hippo started, now; the next hook or command starts the daemon again
```

Example `status` output:

```
spool_pending: 0                  # requests captured while the daemon was down
embed_model: qwen3-embedding:0.6b
reindex: idle                     # idle | running | done | failed: <project id>: <reason>
projects: 2
[3f9c…] /Users/me/work/app/.git
  iterations: 1520  results: 980  embed_jobs: 0  vectors: 2610  compatibility: compatible
  inject (7d): requests 120, injected 41 (34%), no match 52, skipped 27, timeout 0, error 0; agent: opened 3, search 8, show 5
[a71d…] /Users/me/notes  (location missing)
  iterations: 40  results: 12  embed_jobs: 0  vectors: 52  compatibility: compatible
  inject (7d): requests 0, injected 0, no match 0, skipped 0, timeout 0, error 0; agent: opened 0, search 0, show 0
```

- `embed_jobs` is the number of iterations and results waiting for embedding. When there is a backlog, it is split into live and import (`hippo import`) work. If work was done in the last 5 minutes, the rate and the estimated time left are shown too. For example: `embed_jobs: 601 [live 1, import 600] (120/min, about 6 min left)`.
- `inject (7d)` counts the automatic injections of the last 7 days: how many prompts were checked, injected, had no match, were skipped (too short, a task notification, and so on), timed out, or failed. `agent:` counts the agent's `hippo` calls in the same period. `opened` means it ran `hippo show` on a turn it was given by injection.
- `compatibility` is one of `empty`, `compatible`, `mismatch`. `(location missing)` means the location no longer exists. If a project needs a reindex, `status` says so.

---

## `project`

```bash
hippo project list                          # id, kind (git/dir), location, iteration count, last activity (local time)
hippo project remove <project id or path>   # deletes a whole project; confirmation works like forget
```

## `import`

```bash
hippo import                             # past sessions in the current folder (with subfolders and worktrees) into the current project
hippo import --from ~/old/app            # sessions run at an old path into the current folder's project
hippo import --from <project id> --to ~/work/app
hippo import --wait                      # after importing, show progress until embedding is done
hippo import --dry-run                   # show the candidate sessions and what would happen; change nothing
```

`import` reads the past transcripts that Claude Code keeps (`~/.claude/projects/*/*.jsonl`) and adds records to the project of the `--to` folder. Omitted arguments default to the current folder. Subagent transcripts are not read.

- **Which sessions**: sessions whose start path is the `--from` path or below it. The path does not need to exist now. If `--from` is a git repository, sessions opened in its worktrees are included. Older sessions go first.
- **If a project already exists for `--from`**: it first moves that project to the `--to` project. If `--to` has no project, only the location changes. If it has one, the records are merged and the old project is deleted. When merging, vectors are copied if the embedding model is the same, and re-embedded if not. The old location is not kept, so later sessions go to the project at the current location without extra steps.
- **Sessions already captured live**: only the part before live capture started is filled in. Iterations that already exist are not stored again.
- **Iterations at the end of a transcript**: stored if every tool call has its result. If a tool call has no result yet, the iteration is held back. A held-back iteration continues when the session is opened again or `import` runs again.
- **Copies of forked sessions**: skipped if an iteration with the same `message.id` already exists in another session.
- **Sessions pinned to another project**: skipped and reported as "other project".

`--dry-run` first shows what `import` will do (create a project, move a location, merge). Then it prints one line per candidate session: start time, session id, import status, file size, start path, and first prompt. The first prompt is masked for secrets and cut to 80 characters. The import status is one of `new`, `live-only` (stored only after live capture started), `imported`, `other project`.

Progress is printed one line per session:

```
Candidate sessions: 120
[1/120] d767e162 imported  iterations 42  pending embeddings 310
[2/120] 2f74fd43 imported  iterations 3  pending embeddings 318
...
Total: 120 sessions; imported 118, unchanged 2, other project 0, errors 0. Stored 2410 iterations, skipped 101 fork copies.
```

You can stop with Ctrl-C. Running the same command again passes finished sessions as "unchanged" and continues with the rest. The daemon keeps working on the remaining embeddings.

## `forget`

```bash
hippo forget --session <session_id>        # one session
hippo forget --prompt <prompt_id>          # one turn: every iteration with that prompt id in all sessions of the project
```

The command first prints what will be deleted (project id and location, and the session, turn, iteration, and result counts). Then it asks `Delete? [y/N]` and deletes only if you type `y`.

- `--dry-run`: prints the preview and ends.
- `--yes`: deletes without asking. If standard input is not a terminal (a script or a pipe) and `--yes` is not given, it deletes nothing and exits with code 1.
- `--scope project|<path>`: if omitted, it looks in the current project first and then in the other projects, like `show`. If nothing matches, it prints `Nothing to delete.`

The deletion rules are the same as in [Deleting records](usage.md#deleting-records).

---

## `reindex`

```bash
hippo reindex                     # every project
hippo reindex --scope project     # the project of the current directory
hippo reindex --scope <path>
```

Use it after you change the embedding model or vector DB. See [Configuration](configuration.md#changing-the-embedding-model-or-vector-db).

---

## `daemon` and launchd

You do not need to start the daemon. Hooks, CLI commands, and the app start it when needed (`[daemon] autostart = true`), and it stops by itself after 30 minutes without requests. Start it yourself only if you want to watch it or keep it running.

Run it in a terminal. This is good for a first try:

```bash
hippo daemon
```

Register it with launchd to start at login:

```bash
mkdir -p ~/.hippo/logs && chmod 700 ~/.hippo ~/.hippo/logs
hippo launchd-plist > ~/Library/LaunchAgents/com.hippo.daemon.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.hippo.daemon.plist
```

launchd restarts the daemon only after an abnormal exit, not after an idle exit. To stop it: `launchctl bootout gui/$(id -u)/com.hippo.daemon`.

## `hooks-config`

Use this if you want to add the hook block yourself instead of `install`. Put the printed JSON under `hooks` in `settings.json`.

```bash
hippo hooks-config
```

---

## Output language

CLI output follows `[ui] language` (`en` by default, or `ko`). Text for Claude and `--help` are always English.
