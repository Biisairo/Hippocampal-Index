# Troubleshooting

[← README](../../README.md) · [한국어](../ko/troubleshooting.md)

## Contents

1. [What to check for each symptom](#1-what-to-check-for-each-symptom)
2. [Known limitations](#2-known-limitations)

---

## 1. What to check for each symptom

Look at the app's [Status tab](usage.md#status-tab) (or `hippo status`) first. Logs are in `~/.hippo/logs/`.

| Symptom | What to check |
|---------|---------------|
| The app does not open ("is damaged" and similar) | The downloaded app still has the quarantine attribute. Run `xattr -dr com.apple.quarantine hippo.app` ([Getting started](getting-started.md#option-a-download-a-release)) |
| A notice at the top says hippo "is not installed" or "differs" | Press **Preview** and **Apply** in the **Claude setup** tab. Until then, only the About and Claude setup tabs are available |
| The daemon mark at the top right is red, or `hippo: daemon not reachable` | The daemon could not start. Check `~/.hippo/logs/daemon.err` (for example, a configuration error). With `[daemon] autostart = false`, you have to start it yourself |
| The embedding queue (`embed_jobs`) does not go down | Check that the model is pulled (`ollama list`) and that `hippo` could start Ollama (`~/.hippo/logs/ollama.log`, `hippo.log`). Jobs are retried every 5 seconds |
| Search says "vector search unavailable, showing keyword results only" | Embedding failed (Ollama could not start, or the model is missing). Results are keyword-only until it works again |
| Compatibility is `mismatch` | The model or dimension changed. Press **Reindex** in the Projects tab (`hippo reindex`) |
| You cannot tell whether injection works | Look at the **Injection (last 7 days)** table in the Status tab (or the `inject (7d)` line of `hippo status`): requests, injections, reasons for skipping, and how often the agent opened a memory. The result of each prompt is in `~/.hippo/logs/hook.log` |
| Nothing is injected | Check in the Status tab that injection is not paused. Injection looks only at other sessions of the same project. See the reason with the [injection preview](usage.md#query-this-project), and lower `[inject] threshold` if needed |
| New conversations are not recorded | Check that saving is not paused. Check that you started a new Claude Code session after applying the hooks. If `~/.hippo/logs/hook.log` has no `hook … session=<id>` line for that session, the hooks did not run (safe mode, or an Agent SDK session started with `setting_sources=[]`). If it has lines, the `capture … session=<id>` lines of `hippo.log` show how many iterations were saved and why a request was skipped |
| `refusing to modify through a symbolic link` when applying | The settings file is a symbolic link. Choose the Claude folder that holds the real file, or add the output of `hippo hooks-config` yourself |
| Spool pending (`spool_pending`) is above 0 | These are requests from while the daemon was down. They are processed when the daemon starts |
| The build fails in `lance-*` or `protoc` | Run `brew reinstall protobuf`, then check `protoc --version` |
| `vectors/` is very large (tens of GB) after upgrading from 0.14 or earlier | Versions up to 0.14 kept every LanceDB version. The first cleanup after the upgrade removes them, but it can take a few minutes, and search in that project waits meanwhile. To clear it at once, run a reindex |

## 2. Known limitations

- Only macOS is supported for now.
- Injection quality depends on `inject.threshold`. Meta questions you have asked before, such as "was that just injected?", can bring in unrelated memories (SRS appendix C).
- Two-character Korean words such as "버그" cannot be found by keyword search (FTS5 trigram needs at least 3 characters). Vector search covers them.
- Not yet checked in interactive sessions: whether hooks run after a background notification (OPEN-1), Esc interrupts (OPEN-2), compaction (OPEN-3), and whether the last line is written when `Stop` runs (OPEN-6).
- The install record keeps only one Claude folder. If you installed one data root into several Claude folders, remove the others with `hippo uninstall --claude-dir <path>` before deleting everything.
- If the daemon is killed by a signal, the socket file stays. It is removed at the next start.
- There is no retention policy yet. Memory keeps growing until you delete it (see [Deleting records](usage.md#deleting-records)).
- Results (tool output) are deduplicated only within a project. The same output in two projects is stored twice.
- A moved non-git folder does not continue automatically. Import from the old path.
- Live capture starts at the turn the hook reports. Fill in the earlier part with an import.
- For a past session whose old path is gone, `hippo` cannot tell which project it belongs to. Put the old path in From when importing.
- Data in the single-store layout (v0.1, `~/.hippo/records.sqlite`) is not moved to the new layout.
- The About tab does not show the README's images. The copy button of a code block may show an empty box instead of its icon.
