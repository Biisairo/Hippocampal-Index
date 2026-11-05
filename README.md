<p align="center"><img src="assets/banner.png" alt="Hippocampal Index" width="720"></p>

# Hippocampal Index (`hippo`)

**Long-term memory for Claude Code sessions.**

[한국어](README.ko.md)

`hippo` gives Claude Code a long-term memory of your own sessions. It records every iteration of the agentic loop into a local database. When you send a new prompt, it searches that memory and gives Claude the relevant past context. You install and manage it from the window of a macOS app (`hippo.app`).

All data stays on your machine. Embeddings come from a local Ollama model, and vectors go into an embedded LanceDB. No conversation content leaves the computer.

---

## Name and concept

The name comes from the *hippocampal memory indexing theory* (Teyler & DiScenna, 1986). In this theory, the hippocampus does not store whole memories. It stores an **index** that points to memory traces spread across the cortex. When a cue arrives, the index reactivates the original memory.

`hippo` works the same way.

| Brain | `hippo` |
|-------|---------|
| Memory traces in the cortex | Full records stored in SQLite |
| The hippocampal index | A vector store that points to those records |
| A cue | The prompt you just sent |

The short name and the command are `hippo`.

---

## At a glance

### 1. It records

Each time an iteration of the agentic loop ends, a Claude Code hook tells the daemon. The daemon reads the session transcript, masks secrets, stores the iteration, and makes embeddings in the background. The hooks run asynchronously, so they do not slow down your session.

<p align="center"><img src="docs/images/capture.svg" alt="How iterations are captured: hooks, the daemon, the writer thread and the embedding thread" width="880"></p>

### 2. It stores per project

Memory is stored separately for each project (a git repository or a folder). The full records stay in SQLite. The vector store is an index derived from them, so when you change the embedding model you only rebuild the index.

<p align="center"><img src="docs/images/storage.svg" alt="Where memory is stored: the ~/.hippo data root, per-project SQLite records and LanceDB vectors" width="880"></p>

### 3. It recalls

On every prompt, it finds related turns in other sessions of the same project and injects them as `<memory>` blocks. Search combines vector similarity and keyword search (FTS5). When Claude needs more, it runs `hippo search` and `hippo show` itself.

<p align="center"><img src="docs/images/retrieval.svg" alt="How memory is found and injected" width="880"></p>

---

## Quick start

1. Install Ollama and pull the embedding model: `brew install ollama && ollama pull qwen3-embedding:0.6b`
2. Download the app from [Releases](https://github.com/Biisairo/Hippocampal-Index/releases) and open it.
3. In the app's **Claude setup** tab, press **Preview**, then **Apply**.
4. Start a new Claude Code session.

For the full steps and the things to watch for (such as removing the quarantine attribute), see [Getting started](docs/en/getting-started.md).

---

## Documentation

| Document | Contents |
|----------|----------|
| [Getting started](docs/en/getting-started.md) | Requirements, installing the app, applying it to Claude, updating, removing |
| [Using the app](docs/en/usage.md) | Each tab of the app window (with screenshots), deleting records, pausing |
| [CLI reference](docs/en/cli.md) | Every terminal command: `search`, `show`, `import`, `forget`, `install`, and more |
| [Configuration](docs/en/configuration.md) | Every key in `config.toml`, changing the embedding model or vector DB |
| [Architecture](docs/en/architecture.md) | The unit of memory, capture, storage layout, injection rules, daemon lifecycle, data location and security |
| [Troubleshooting](docs/en/troubleshooting.md) | What to check for each symptom, known limitations |
| [Development and contributing](docs/en/development.md) | Spec-first workflow, source layout, build and test, adding a backend, issues and PRs |
| [Requirements specification (SRS)](docs/SRS.md) | The reference for all behavior (ISO/IEC/IEEE 29148, in Korean) |

---

## License

MIT. See [LICENSE](LICENSE).
