# Development and contributing

[← README](../../README.md) · [한국어](../ko/development.md)

## Contents

1. [Workflow: spec first](#1-workflow-spec-first)
2. [Development environment](#2-development-environment)
3. [Build and test](#3-build-and-test)
4. [Source layout](#4-source-layout)
5. [Adding a backend](#adding-a-backend)
6. [Contributing](#6-contributing)

---

## 1. Workflow: spec first

[`docs/SRS.md`](../SRS.md) (ISO/IEC/IEEE 29148, in Korean) is the single reference. Every requirement has an ID (such as `FR-EXT-7`), and code comments point to these IDs.

Work in this order:

1. Change the SRS.
2. Write a failing test.
3. Implement.
4. Make all tests pass.

If the SRS and the code disagree, fix the SRS first.

## 2. Development environment

| Item | Version used | Notes |
|------|--------------|-------|
| macOS | Darwin (Apple Silicon) | Primary target. Uses Unix sockets and launchd |
| Rust | 1.95 or later (edition 2024) | |
| `protoc` | libprotoc 36 or later | Needed to build LanceDB. `brew install protobuf` |
| Ollama | 0.40 or later | Needed only for the measured tests |

## 3. Build and test

```bash
cargo build                    # debug
cargo build --release          # release (about 4 minutes the first time, because of LanceDB)
scripts/bundle-app.sh          # makes target/hippo.app from a release build (ad-hoc signed)
cargo test                     # all deterministic tests
cargo test -- --ignored        # measured tests that use a local Ollama
cargo clippy --all-targets     # must have no warnings
cargo fmt
```

You can open the window straight from a build: `./target/release/hippo gui`. A binary that is not installed can run only `install` and `gui` (see the [CLI reference](cli.md#binary-and-data-root)).

### Fixtures

- The fixtures in `tests/fixtures/` are real transcripts reduced to their `user` and `assistant` lines.
- `tests/fixtures/hooks/` holds measured hook inputs.
- To make new ones, do not touch your `~/.claude` settings. Write a separate, isolated settings file that saves each hook input to a file, and run `claude -p --settings <that file>`. Then keep only the `user` and `assistant` lines of the transcript, and replace local paths with ones like `/Users/user`.

## 4. Source layout

| Module | Role |
|--------|------|
| `main.rs` | CLI entry point. Decides the data root from the binary location |
| `transcript.rs` | Parses transcript JSONL into iterations, safe offsets |
| `bg.rs` | Detects background starts, parses `<task-notification>`, extracts subagent reports |
| `redact.rs` | Secret masking |
| `embed_text.rs` | Builds the text to embed (`[input]`/`[tool]`/`[response]` lines), chunking |
| `shell.rs` | Parses Bash commands (brush-parser) to decide whether they run hippo |
| `store.rs` | SQLite record store, deduplication, FTS5, project merge |
| `project.rs` | Path normalization, git information, project location |
| `registry.rs` | Index DB: project resolution, session pinning, location moves, absorbing |
| `import.rs` | Candidate session selection and preview for importing past records |
| `queue.rs` | Two-level queue that takes live jobs before import jobs |
| `embedder.rs`, `vector.rs` | The `Embedder` and `VectorStore` traits |
| `ollama.rs`, `lance.rs` | The real backends |
| `testing.rs` | `HashEmbedder`, `MemoryVectorStore` (deterministic backends for tests) |
| `backends.rs` | Builds backends from the configuration. **The only file to change when adding a backend** |
| `indexer.rs` | Embedding queue, reindex, compatibility |
| `generations.rs` | Vector store generations and the `ACTIVE` pointer |
| `ingest.rs` | transcript → masking → storage pipeline |
| `search.rs` | Hybrid search, automatic injection, `search`/`show` output |
| `daemon.rs`, `client.rs`, `protocol.rs`, `spool.rs` | Daemon, hook client, socket protocol, spool |
| `awake.rs` | Keeps the system from idle sleep while work is pending |
| `install.rs`, `setup.rs` | Settings file merge, binary copy, hook JSON, launchd plist |
| `layout.rs` | Data root resolution, PATH link, Claude folder record, install state, delete everything |
| `config.rs` | `config.toml` and data paths |
| `config_edit.rs` | Settings editing for the app: key list, saving only changed keys (`toml_edit`), restart and reindex checks |
| `commands.rs` | Actions shared by the CLI and the app: install, uninstall, purge, stop, import result lines, daemon requests |
| `i18n.rs`, `fsutil.rs` | Output language, owner-only files and directories |
| `gui/` | `hippo gui`: `state.rs` (logic kept apart from drawing, under test), `app.rs` (egui screens), `widgets.rs` (shared widgets), `text.rs` (setting descriptions), `theme.rs` (light and dark themes), `images.rs` (icon, logo, character, README preparation), `picker.rs` (folder picker) |

## Adding a backend

1. Implement `Embedder` (`src/embedder.rs`) or `VectorStore` (`src/vector.rs`).
2. Run the shared contract test in `tests/common/contract.rs` (`embedder_contract` or `vector_store_contract`) against the new backend. `tests/lance_test.rs` is an example.
3. Add its name in `src/backends.rs` (`build_embedder` or `vector_source`).
4. Select it in `config.toml`, restart the daemon, and reindex.

You do not need to change the capture, search, or daemon code.

## 6. Contributing

Bug reports, feature ideas, and code are all welcome. Issues and PRs can be in Korean or English.

### Issues

- Bugs: include the steps to reproduce, what you expected and what happened, the Status tab or `hippo status` output, related error lines from `~/.hippo/logs/`, and your macOS, Claude Code, and Ollama versions.
- If logs, output, or screenshots contain conversation content, personal paths, or secrets, remove or blur them before posting.
- For feature ideas, describe the situation and why you need it first.

### Pull requests

1. Fork the repository and create a branch.
2. For a change in behavior, follow the order in [section 1](#1-workflow-spec-first): change `docs/SRS.md` → failing test → implementation.
3. Before opening the PR, all of these must pass:

   ```bash
   cargo test
   cargo clippy --all-targets   # zero warnings
   cargo fmt --check
   ```

4. If user-visible behavior changes, update the English docs (`README.md`, `docs/en/`) and the Korean docs (`README.ko.md`, `docs/ko/`) together. If the window changes, update the screenshots in `docs/images/gui/` too.
5. Write commit messages as `type: summary`. Types are `feat`, `fix`, `docs`, `test`, `refactor`, `chore`. If you changed requirements, add the SRS version (for example `fix: … (SRS 0.21)`).

### Please keep in mind

- Do not commit real conversations, transcripts, personal paths, or secrets. For new fixtures, use the method in [section 3](#fixtures) and anonymize paths. Blur conversation content and paths in screenshots.
- Tests must be deterministic. Tests that need a real Ollama are marked `#[ignore]`.
- During tests and development, do not change your real `~/.claude` and `~/.hippo`. Tests that run the binary use `<root>/bin/hippo` in a temporary folder (`installed_bin` in `tests/cli_test.rs`).

## License

MIT. See [LICENSE](../../LICENSE).
