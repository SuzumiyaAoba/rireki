![rireki app icon](assets/app-icon/readme.svg)

# rireki

A native macOS app for exploring Claude Code and Codex session history. Built with Rust and GPUI, rireki turns local JSONL logs into timelines, calendars, and usage statistics so you can review what your coding agents did, when they did it, and which files they changed.

The app's interface is currently in Japanese.

## Features

- **Timeline:** Browse messages, tool calls, and subagent activity in separate lanes. Zoom in on individual calls or group activity by tool, turn, or project.
- **Calendar:** Switch between day, week, and month views to see session activity and time spent across projects.
- **Search and filters:** Find sessions by title, directory, or ID, and events by name or summary. Filter by Claude Code, Codex, project, or visible time range.
- **Event details:** Inspect tool inputs, outputs, durations, and results, including errors, rejected calls, and interruptions.
- **File history:** Review edited files, change summaries, and diffs for individual edits.
- **Usage and analysis:** Explore token usage, estimated API costs, tool activity, and trends over 7 days, 30 days, 90 days, or all history.
- **Live updates:** Refresh active logs every three seconds and index completed logs locally for faster startup.

## Build and run

You need macOS, a Rust toolchain that supports edition 2024, and the Xcode Command Line Tools. The repository uses the macOS compiler at `/usr/bin/cc`.

From the repository root:

```sh
cargo run --release
```

For development:

```sh
cargo run
```

To build a macOS app bundle with its icon:

```sh
scripts/bundle.sh
open target/release/rireki.app
```

The bundle script builds the release binary and writes `target/release/rireki.app`.

## Releases

The **Create release tag** workflow can be run manually from the repository's **Actions** tab. It tags the selected branch's commit and calls the reusable **Release** workflow, which builds macOS binaries for Apple Silicon and Intel and publishes them to GitHub Releases. This uses the built-in `GITHUB_TOKEN`; no GitHub App or additional secrets are required. [Reusable workflows are called directly from a job](https://docs.github.com/en/actions/how-tos/reuse-automations/reuse-workflows#calling-a-reusable-workflow), so publishing does not depend on the tag push triggering another workflow.

Release tags use the current date in **Asia/Tokyo** as `YYYY-MM-dd`, without a `v` prefix. Further releases on the same date use `.1`, `.2`, and so on, choosing one more than the highest existing suffix. For example: `2026-10-13`, `2026-10-13.1`, `2026-10-13.2`. Tag creation is serialized across branches to prevent concurrent manual runs from choosing the same version, and [multiple pending runs are queued](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#example-queueing-multiple-pending-runs). Release tags are independent of the package version in `Cargo.toml`.

### Publish a version

1. Commit and push the changes to release. The workflow files must be on the default branch for manual runs to be available.
2. Open **Actions → Create release tag → Run workflow** and select the branch containing that commit.
3. Run the workflow without entering a version. It creates the next date tag, builds both architectures, and publishes the release assets.

The release contains these assets, where `<tag>` is the version tag:

- `rireki-<tag>-aarch64-apple-darwin.tar.gz` and `rireki-<tag>-x86_64-apple-darwin.tar.gz`: the executable for each architecture.
- `rireki-<tag>-aarch64-apple-darwin.zip` and `rireki-<tag>-x86_64-apple-darwin.zip`: the icon-equipped `rireki.app` bundle for each architecture.
- `rireki-<tag>-aarch64-apple-darwin.dmg` and `rireki-<tag>-x86_64-apple-darwin.dmg`: disk images containing `rireki.app` and a link to `/Applications`. Open the DMG and drag the app into Applications to install it.
- `SHA256SUMS`: SHA-256 checksums for all six assets.

Both builds, their core tests, and DMG verification must succeed before a new release is published. The app bundles are ad-hoc signed; Developer ID signing and notarization are not configured.

The **Release** workflow also builds assets when a date tag is pushed or a release is published through GitHub's UI. To retry a build or replace existing assets, run **Actions → Release → Run workflow** with an existing date tag. It builds that tag's commit and preserves the release's existing description. Running **Create release tag** again creates a new release tag instead.

## Session logs

rireki discovers existing logs automatically at startup. Create a session in Claude Code or Codex first if you have no history yet.

| Source | Location |
| --- | --- |
| Claude Code | `~/.claude/projects/*/*.jsonl` |
| Claude Code subagents | `<session>/subagents/agent-*.jsonl` within each project, plus legacy `agent-*.jsonl` files |
| Codex | `$CODEX_HOME/sessions`, recursively; defaults to `~/.codex/sessions` |

To use a different Codex home directory:

```sh
CODEX_HOME=/path/to/codex-home cargo run --release
```

Select a source or project in the sidebar, switch between timeline and calendar views, and click an event or session to open its details. The detail panel includes session summaries, events, file history, and event analysis.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Cmd+F` or `Cmd+K` | Focus search |
| `Cmd+1` | Show the timeline |
| `D`, `W`, `M` | Show the day, week, or month calendar |
| `Cmd+4` | Show statistics |
| `T` or `Cmd+T` | Go to today |
| `Left` / `Right` | Go to the previous / next period |
| `G` | Choose a period or date |
| `+` / `-` | Zoom in / out |
| `J` / `K` | Select the next / previous event |
| `1`, `2`, `3`, `4` | Open the session, events, files, or analysis detail tab |
| `Cmd+[` / `Cmd+]` | Navigate back / forward |
| `Cmd+B` | Toggle the sidebar |
| `Cmd+Shift+Enter` | Toggle fullscreen for the detail panel |
| `Esc` | Dismiss the current panel or search |
| `?` or `Cmd+/` | Show keyboard shortcut help |

Single-letter shortcuts are disabled while typing in a text field.

## Local storage and configuration

rireki reads the original session logs and stores its index, cached content, and preferences locally.

| Data | Default macOS location |
| --- | --- |
| Session index | `~/Library/Caches/rireki/index.db` |
| Compressed event inputs and outputs | `~/Library/Caches/rireki/texts.db` |
| UI preferences | `~/Library/Application Support/rireki/prefs.json` |
| Model pricing | `~/Library/Application Support/rireki/prices.json` |

Use `RIREKI_CACHE_DIR` to change the index and text cache directory, and `RIREKI_CONFIG_DIR` to change the UI preferences directory:

```sh
RIREKI_CACHE_DIR=/path/to/cache \
RIREKI_CONFIG_DIR=/path/to/config \
cargo run --release
```

`RIREKI_CONFIG_DIR` applies to `prefs.json`; `prices.json` remains in the default application support directory.

### Cost estimates

Costs are API-equivalent estimates based on token usage recorded in the logs and the app's model pricing table. They do not represent subscription charges or an account's actual bill. Usage records without a known model price are counted as unpriced.

The app creates `prices.json` from its built-in rates when pricing is first used. Edit that file to add models or override rates, then restart the app. Each model entry requires `input` and `output` rates in USD per million tokens and may include `cache_read`, `cache_write`, and `cache_write_1h`. Entries match exact model names and names with numeric suffixes, such as dated model versions; longer matching keys take precedence.

## Development

```sh
cargo build
cargo clippy
cargo test
```

The core library handles log parsing, indexing, search, statistics, and detail loading without depending on GPUI. To test it without the GUI dependencies:

```sh
cargo test --lib --no-default-features
```

To load the local index, ingest changed logs, and print session, event, and tool-call counts without opening a window:

```sh
cargo run --release -- --stats
```

The application also includes performance benchmarks that emit JSON Lines:

```sh
cargo run --release -- --bench
cargo run --release -- --bench-search /path/to/index.db
```

See [the performance notes](docs/perf.md) for benchmark details and implementation constraints. Those notes are in Japanese.
