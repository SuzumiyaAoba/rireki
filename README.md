![rireki app icon](assets/app-icon/readme.svg)

# rireki

A native app for macOS, Linux, and Windows for exploring Claude Code and Codex session history. Built with Rust and GPUI, rireki turns local JSONL logs into timelines, calendars, and usage statistics so you can review what your coding agents did, when they did it, and which files they changed.

The app's interface is currently in Japanese.

## Features

- **Timeline:** Browse messages, tool calls, and subagent activity in separate lanes. Zoom in on individual calls or group activity by tool, turn, or project.
- **Calendar:** Switch between day, week, and month views to see session activity and time spent across projects.
- **Search and filters:** Find sessions by title, directory, or ID, and events by name or summary. Filter by Claude Code, Codex, project, or visible time range.
- **Event details:** Inspect tool inputs, outputs, durations, and results, including errors, rejected calls, and interruptions.
- **File history:** Review edited files, change summaries, and diffs for individual edits.
- **Usage and reliability statistics:** Explore tokens, estimated API costs, turns, sessions, tool calls, and failed, rejected, or interrupted tool calls over 7 days, 30 days, 90 days, or all history. Rank projects and sessions by the selected metric, then open a session in the timeline and detail panel; with the failure metric, the detail panel opens filtered to the failed events. Compare tools by call volume, failure breakdown, and median / P95 duration on a shared log scale.
- **Live updates:** Refresh active logs every three seconds and index completed logs locally for faster startup.
- **Terminal UI:** Use timelines, calendars, statistics, event analysis, file diffs, replay, and MCP from an interactive terminal with `--tui`.
- **Local MCP server:** Start and stop a read-only Streamable HTTP server from either interface so coding agents can search, retrieve, and analyze the same history.

## Build and run

### Nix

On macOS or Linux, use the included flake to build and run the app:

```sh
nix run
nix run . -- --stats
```

To build without launching, run `nix build`; the executable is available at
`result/bin/rireki`. The flake pins Nixpkgs in `flake.lock` and uses `Cargo.lock`
for Rust dependencies. Enable the `nix-command` and `flakes` experimental features
in your Nix configuration if needed, or pass
`--extra-experimental-features 'nix-command flakes'` to these commands.

For development, enter the shell or run Cargo directly in it:

```sh
nix develop
nix develop -c cargo run --features gpui/runtime_shaders
nix develop -c cargo test --lib --no-default-features
```

The shell provides Rust and the native build dependencies. On macOS, add
`--features gpui/runtime_shaders` when building or testing with GUI dependencies
to compile Metal shaders at runtime, without requiring Xcode's Metal compiler.
`nix run` and `nix build` enable this automatically. Linux needs an X11 or Wayland
desktop session and a working Vulkan driver; on non-NixOS distributions, configure
GPU driver access with [nixGL](https://github.com/nix-community/nixGL) if necessary.

### Cargo

You need Rust 1.95 or newer and the native build tools for your OS:

- **macOS:** Xcode Command Line Tools and the Metal shader compiler. The repository selects `/usr/bin/cc` for macOS targets only.
- **Linux:** a C/C++ compiler, CMake, Clang, pkg-config, and the development libraries below. The GUI uses X11 or Wayland and needs a Vulkan-capable GPU and driver.
- **Windows:** the MSVC Rust toolchain, Visual Studio Build Tools with the **Desktop development with C++** workload, CMake, and a Windows SDK containing `fxc.exe`. Build from a Visual Studio developer terminal. The GUI uses Direct3D 11.

The Windows release executable uses the static C runtime and opens without a console window. CLI modes such as `--stats` attach to the launching terminal; piped or redirected output is also supported.

On Ubuntu / Debian, install the Linux build dependencies:

```sh
sudo apt-get install build-essential cmake clang pkg-config libssl-dev \
  libfontconfig1-dev libfreetype6-dev libwayland-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev \
  libvulkan-dev fonts-dejavu-core fonts-noto-cjk
```

The upstream [Linux](https://zed.dev/docs/development/linux) and [Windows](https://zed.dev/docs/development/windows) build guides provide more background on GPUI's native dependencies. For release builds on Windows, GPUI 0.2.2 finds `fxc.exe` on `PATH`, at its default Windows SDK location, or through `GPUI_FXC_PATH`. If needed, set it in PowerShell:

```powershell
$env:GPUI_FXC_PATH = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe'
```

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

Linux and Windows builds produce `target/release/rireki` and `target/release/rireki.exe`, respectively. They use the OS titlebar for moving, minimizing, maximizing, and closing the window. When a Wayland compositor does not provide a titlebar, rireki provides window buttons, a draggable top bar, and resize handles. Linux selects Wayland when `WAYLAND_DISPLAY` is set, otherwise X11 when `DISPLAY` is set. To force X11 in a Wayland session, run with `WAYLAND_DISPLAY=''`.

Trackpad pinch zoom is available on macOS. On every OS, use the zoom buttons, `+` / `-`, or Cmd / Ctrl + scroll to zoom.

## Releases

The **Create release tag** workflow can be run manually from the repository's **Actions** tab. It tags the selected branch's commit and calls the reusable **Release** workflow, which builds binaries for macOS (Apple Silicon and Intel), Linux (x86_64), and Windows (x86_64 MSVC) and publishes them to GitHub Releases. This uses the built-in `GITHUB_TOKEN`; no GitHub App or additional secrets are required. [Reusable workflows are called directly from a job](https://docs.github.com/en/actions/how-tos/reuse-automations/reuse-workflows#calling-a-reusable-workflow), so publishing does not depend on the tag push triggering another workflow.

Release tags use the current date in **Asia/Tokyo** as `YYYY-MM-dd`, without a `v` prefix. Further releases on the same date use `.1`, `.2`, and so on, choosing one more than the highest existing suffix. For example: `2026-10-13`, `2026-10-13.1`, `2026-10-13.2`. Tag creation is serialized across branches to prevent concurrent manual runs from choosing the same version, and [multiple pending runs are queued](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#example-queueing-multiple-pending-runs). Release tags are independent of the package version in `Cargo.toml`.

### Publish a version

1. Commit and push the changes to release. The workflow files must be on the default branch for manual runs to be available.
2. Open **Actions → Create release tag → Run workflow** and select the branch containing that commit.
3. Run the workflow without entering a version. It creates the next date tag, builds all supported release targets, and publishes the release assets.

The release contains these assets, where `<tag>` is the version tag:

- `rireki-<tag>-aarch64-apple-darwin.tar.gz` and `rireki-<tag>-x86_64-apple-darwin.tar.gz`: the executable for each architecture.
- `rireki-<tag>-aarch64-apple-darwin.zip` and `rireki-<tag>-x86_64-apple-darwin.zip`: the icon-equipped `rireki.app` bundle for each architecture.
- `rireki-<tag>-aarch64-apple-darwin.dmg` and `rireki-<tag>-x86_64-apple-darwin.dmg`: disk images containing `rireki.app` and a link to `/Applications`. Open the DMG and drag the app into Applications to install it.
- `rireki-<tag>-x86_64-unknown-linux-gnu.tar.gz`: the Linux executable, built on Ubuntu 22.04. Extract it and run `./rireki` in an X11 or Wayland desktop session. Install the runtime libraries listed below when they are not already provided by your desktop.
- `rireki-<tag>-x86_64-pc-windows-msvc.zip`: the Windows executables. Extract it and run `rireki.exe` for the GUI or `rireki-tui.exe` from a terminal for the TUI.
- `SHA256SUMS`: SHA-256 checksums for all release assets.

Linux releases need glibc 2.35 or newer, a Vulkan driver, and the following runtime packages on Ubuntu / Debian:

```sh
sudo apt-get install libfontconfig1 libfreetype6 libwayland-client0 \
  libxkbcommon0 libxkbcommon-x11-0 libxcb1 libvulkan1 libssl3 \
  fonts-dejavu-core fonts-noto-cjk
```

All four builds and their core tests, plus macOS DMG verification, must succeed before a new release is published. The app bundles are ad-hoc signed; Developer ID signing and notarization are not configured.

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

## Connect coding agents with MCP

Click the **plug icon** near the bottom of the left rail, then **サーバーを起動**
(Start server). The panel shows the running status, endpoint URL, and copy buttons
for the URL and client configuration. The default endpoint is
`http://127.0.0.1:47821/mcp`; you can change the port before starting if it is
already in use. Use **サーバーを停止** (Stop server) to stop it. Closing the panel
keeps the server running; exiting rireki stops it. Startup is always manual.
In the TUI, press `F7`, then `Enter` to start or stop the server; `e` edits the
port, `y` copies the client configuration, and `u` copies the URL.

For a client that accepts `mcpServers` configuration:

```json
{
  "mcpServers": {
    "rireki": {
      "type": "http",
      "url": "http://127.0.0.1:47821/mcp"
    }
  }
}
```

Select **Streamable HTTP** in clients with a connection form. Configuration
formats vary by client; use the displayed URL in the client's HTTP MCP settings.
The server supports MCP protocol versions `2025-03-26`, `2025-06-18`, and
`2025-11-25`, with JSON responses and no persistent SSE stream or session ID.
The transport follows the [MCP Streamable HTTP specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).

| Tool | Purpose |
| --- | --- |
| `list_sessions` | Find sessions by title, project directory, ID, source, or time range. |
| `get_session` | Retrieve session metadata, subagent lanes, parent relationships, and usage totals. |
| `get_events` | Retrieve a session's conversation and tool activity across lanes, optionally including input/output text. |
| `search_events` | Search event names, summaries, and models across history, with source, project, time, kind, and outcome filters. |
| `analyze_history` | Aggregate turns, tool outcomes and durations, tokens, and estimated API costs by tool and model. |

For example, call `list_sessions` with `{"query":"my-project","source":"codex"}`,
then `get_events` with `{"session_id":"<id>","include_text":true,"limit":20}`.
For error analysis, use `analyze_history` with
`{"cwd":"/absolute/project/path","status":"failed"}`.

The server shares all loaded history, independently of the GUI's current search
and filters, and receives live updates as the app loads changed logs. `cwd` is an
exact directory match; `after` and `before` are inclusive Unix seconds. Event
queries match whitespace-separated words against names, summaries, and models,
rather than full input/output bodies. Event responses include zero-based `lane`
and `event_index` values for fetching individual entries. Use `order:"newest"`
to reverse the default chronological event order.

List and event tools return `total`, `next_offset`, and `revision`. Request the
next page with `offset:next_offset` and `expected_revision:revision`; if history
has changed, restart pagination. `limit` defaults to 50 and is capped at 100;
`offset` is capped at 10,000, so narrow the filters for larger result sets.
Bodies are omitted by default. With `include_text:true`, `text_limit` defaults
to 4,000 characters per input/output and can be increased to 20,000. Responses
report truncation and unavailable cached text explicitly. Bodies are the
importer's stored representation, which may already be formatted or truncated;
they are not a raw JSONL export. Analysis covers all matches and returns the
top 100 tools and models, with the full distinct counts.

The server listens only on IPv4 loopback, checks `Host` and `Origin`, and does
not enable CORS. It has no authentication: other local processes can read the
loaded history while it is running. It provides no log mutation or command
execution tools. Historical message bodies are untrusted data, and the server
instructs agents not to follow instructions found in them.

## Terminal UI

Start the terminal interface from an interactive terminal:

```sh
cargo run --release -- --tui
```

On Windows, run the console executable `rireki-tui.exe` included in the release ZIP. From source, use `cargo run --release --bin rireki-tui`. It takes the TUI options directly, without `--tui`. The GUI release executable `rireki.exe` does not support interactive TUI mode because Windows shells do not wait for GUI executables to exit.

To build and run it without GPUI or GPU/windowing dependencies:

```sh
cargo run --release --no-default-features --features tui -- --tui
```

The default build includes both interfaces and opens the GUI when run without arguments. A TUI-only build opens the TUI without arguments. `--stats` and the other CLI commands remain available in all builds.

The terminal screens use the same history, search, detail, analysis, pricing, and statistics core as the GUI. The main screens are:

| Key | Screen | Features |
| --- | --- | --- |
| `F1` | History | Session and event search, source/project filters, all subagent lanes, event ranges, and detail panels |
| `F2` | Timeline | Event lanes or directory groups, automatic/tool/turn/call grouping, zoom, date ranges, activity minimap, and API cost buckets |
| `F3` / `F4` / `F5` | Day / week / month | Activity intervals, session counts, time spent, date navigation, and selection into the timeline or details |
| `F6` | Statistics | 7/30/90 days or all history; tokens, estimated costs, turns, sessions, tools, previous-period comparisons, trends, time of day, sources, models, projects, and tools |
| `F7` | MCP | Start/stop the local server, edit its port, and copy connection details |
| `F8` | Settings | Pane visibility and proportions, source/project filters, active sessions, and display defaults |

The detail panel has six tabs: session overview (`1`), conversation history (`2`), changed files and diffs (`3`), event analysis (`4`), replay (`5`), and event body (`6`). It includes token/cache/reasoning counts, model costs, tool outcomes and durations, parent/child agent navigation, resume-command copying, and the location of the source log. Analysis supports kind, outcome, name, text, and sort filters, plus JSON copying and export. Replay supports pause, speed changes, rewind, and following newly revealed events.

Graphs use terminal cells, bars, sparklines, and tables. The statistics time-of-day view is a weekday/hour heatmap with exact cell and aggregate values; the GUI's circular chart is represented by this grid. Keyboard selection replaces hovering and dragging. On narrow terminals, the focused history pane fills the available space; statistics tables can scroll horizontally.

| Context | Key | Action |
| --- | --- | --- |
| All screens | `s`, `p`, `P` | Cycle source, cycle project, or search and select a project |
| All screens | `Ctrl+F`, `Ctrl+R` | Search history from any screen or refresh logs |
| All screens | `Ctrl+L`, `B` | Toggle active-session filtering or the history sidebar |
| All screens | `Shift+Enter` | Toggle full-screen details |
| All screens | `Alt+[` / `Alt+]` | Go back / forward through viewed selections |
| All screens | `?`, `q` / `Ctrl+C` | Show full keyboard help or quit; `q` remains text during input |
| History | `Tab` / `Shift+Tab` | Move between sessions, events, and details |
| Lists and details | Arrows / `j` / `k`, `PageUp` / `PageDown`, `Home` / `End` | Select, scroll, or move to an edge |
| History | `/`, `Esc`, `r` | Search, cancel/clear search or return, refresh logs |
| History events | `v`, `a` / `I` | Mark a range's start/end in one lane, or open the entire lane |
| Details | `1`–`6`, `Enter` | Switch detail tab or open the selected event, lane, or file |
| Details | `t`, `f`, `n`, `o`, `/`, `0` | Filter kind, outcome, name, sort order, or text; reset filters |
| Details | `[` / `]`, `b`, `d`, `u` | Switch lane, open the parent call, child agent, or preceding user instruction |
| Details | `x`, `z`, `l`, `,` / `.` | Expand text/diffs, toggle raw display, line numbers, or event pages |
| Details | `y` / `Y`, `e`, `c`, `C`, `g` | Copy input/selected content or output, export analysis JSON, copy resume command or session ID, show the log in the file manager |
| Replay | `Space`, `+` / `-`, `R` | Play/pause, change speed, rewind |
| Timeline/calendar | `h` / `l`, `+` / `-`, `t`, `L`, `0` | Move period, zoom, show today, latest activity, or all history |
| Timeline/calendar | `g`, `Alt+1`–`Alt+6` | Enter a date/range or use a recent-period preset |
| Timeline/calendar | `c`, `o`, `a`, `v`, `Space` | Switch event/cost view, lane/directory grouping, event granularity, visible-range filtering, or project collapse |
| Timeline/calendar | `<` / `>`, `z`, `Enter` | Select a block, zoom to it, or open details |
| Calendar | `u`, `Enter` | Toggle project active-time totals and return to the selected project’s activity |
| Statistics | `[` / `]`, `m` / `M`, `Tab` / `Shift+Tab` | Change period, metric, or statistics tab |
| Statistics | `Enter` | Open the selected trend period, filter a project, or inspect a cost bucket's API call |
| Statistics | `h` / `l`, `Alt+Left` / `Alt+Right` | Scroll tables horizontally; in hour/cost views plain arrows select cells |
| Statistics API costs | `+` / `-`, `H` / `L`, `0`, `d`, `v`, `Space` | Zoom, move time range, reset range, aggregate directories, filter visible sessions, expand/collapse |

The date input accepts `YYYY-MM-DD`, inclusive ranges such as `2026-10-01..2026-10-22`, `1h`, `6h`, `24h`, `7d`, `30d`, `90d`, or `all`. Search matches session titles, directories, and IDs, and event names and summaries. Analysis text filters also match model names. History refreshes every three seconds in the background. Detail bodies retain the core's 6,000-character limit per input/output; metadata analysis and file history cover the full selected range, and individual events can be opened beyond the range detail cache's first 300 bodies.

Display settings are saved separately from GUI preferences, in `tui.json` under the application's configuration directory (`RIREKI_CONFIG_DIR` overrides it). Both interfaces use the same model prices from `prices.json`. Clipboard actions use the system clipboard where available, with an OSC 52 terminal fallback. Analysis JSON export asks for a filename and does not overwrite an existing file. These actions copy or save recorded data; copying a resume command does not execute it.

Use `--tui --help` for the terminal options. To browse separate log directories, specify both roots:

```sh
cargo run --no-default-features --features tui -- --tui \
  --claude-root /path/to/claude/projects \
  --codex-root /path/to/codex/sessions
```

Explicit roots are loaded in memory without reading or updating the default index or TUI preferences. Terminal mode requires interactive standard input and output; use `--stats` for redirected output. Exiting restores the original terminal screen and input mode, and stops the MCP server if one was started.

## GUI keyboard shortcuts

`Mod` means **Cmd on macOS** and **Ctrl on Linux / Windows**. The app's hints and shortcut help show the keys for the current OS.

| Shortcut | Action |
| --- | --- |
| `Mod+F` or `Mod+K` | Focus search |
| `Mod+1` | Show the timeline |
| `D`, `W`, `M` | Show the day, week, or month calendar |
| `Mod+4` | Show statistics |
| `T` or `Mod+T` | Go to today |
| `Left` / `Right` | Go to the previous / next period |
| `G` | Choose a period or date |
| `+` / `-` | Zoom in / out |
| `J` / `K` | Select the next / previous event |
| `1`, `2`, `3`, `4` | Open the session, events, files, or analysis detail tab |
| `Mod+[` / `Mod+]` | Navigate back / forward |
| `Mod+B` | Toggle the sidebar |
| `Mod+Shift+Enter` | Toggle fullscreen for the detail panel |
| `Esc` | Dismiss the current panel or search |
| `?` or `Mod+/` | Show keyboard shortcut help |

Single-letter shortcuts are disabled while typing in a text field.

## Local storage and configuration

rireki reads the original session logs and stores its index, cached content, and preferences locally.

| Data | macOS | Linux | Windows |
| --- | --- | --- | --- |
| Session index | `~/Library/Caches/rireki/index.db` | `~/.cache/rireki/index.db` | `%LOCALAPPDATA%\rireki\index.db` |
| Compressed event inputs and outputs | `~/Library/Caches/rireki/texts.db` | `~/.cache/rireki/texts.db` | `%LOCALAPPDATA%\rireki\texts.db` |
| UI preferences | `~/Library/Application Support/rireki/prefs.json` | `~/.config/rireki/prefs.json` | `%APPDATA%\rireki\prefs.json` |
| Model pricing | `~/Library/Application Support/rireki/prices.json` | `~/.config/rireki/prices.json` | `%APPDATA%\rireki\prices.json` |

Linux honors `XDG_CACHE_HOME` and `XDG_CONFIG_HOME` when set. On Windows, `~` in the session log paths denotes the user's home directory. Environment variables can be set with `$env:CODEX_HOME = 'C:\path\to\codex-home'` in PowerShell, and similarly for `RIREKI_CACHE_DIR` and `RIREKI_CONFIG_DIR`.

Use `RIREKI_CACHE_DIR` to change the index and text cache directory, and `RIREKI_CONFIG_DIR` to change the UI preferences directory:

```sh
RIREKI_CACHE_DIR=/path/to/cache \
RIREKI_CONFIG_DIR=/path/to/config \
cargo run --release
```

`RIREKI_CONFIG_DIR` applies to `prefs.json`; `prices.json` remains in the default configuration directory for the OS.

### Cost estimates

Costs are API-equivalent estimates based on token usage recorded in the logs and the app's model pricing table. They do not represent subscription charges or an account's actual bill. Usage records without a known model price are counted as unpriced.

The app creates `prices.json` from its built-in rates when pricing is first used. Edit that file to add models or override rates, then restart the app. Each model entry requires `input` and `output` rates in USD per million tokens and may include `cache_read`, `cache_write`, and `cache_write_1h`. Entries match exact model names and names with numeric suffixes, such as dated model versions; longer matching keys take precedence.

## Development

The **CI** workflow builds the release executable, runs tests with and without GUI dependencies, checks Clippy, and smoke-tests `--stats` on macOS ARM64, Ubuntu 22.04 x86_64, and Windows x86_64 MSVC. These checks do not exercise GPU rendering or desktop input; those still need a desktop session on each OS.

```sh
cargo build
cargo clippy
cargo test
```

The core library handles log parsing, indexing, search, statistics, and detail loading without depending on GPUI. The CLI also builds without GUI dependencies:

```sh
cargo build --no-default-features
cargo run --no-default-features -- --help
cargo run --no-default-features -- --stats
```

This build needs Rust and the native tools used by the core dependencies, but no GPUI windowing or GPU libraries. Running it without a command prints an error; enable `--features tui` to include the terminal interface.

To test the core, CLI, and integration workflows without the GUI dependencies:

```sh
cargo test --locked --no-default-features
cargo test --locked --no-default-features --features tui --all-targets
```

### Shared frontend APIs

Use this package as a library with `default-features = false` when adding another frontend. `history::History` owns the index and store: call `load_cached()` once, then `plan()` and `ingest(tasks)` to process changes. Each batch returns progress, a shared `model::Snapshot`, and the touched session indices. Dropping the ingestion iterator stops before the next batch; a later `plan()` rediscovers unprocessed files. `plan_in()` accepts explicit Claude and Codex log roots, and `History::with_indexer(Indexer::in_memory(true))` supports workflows without a persistent index.

`catalog::Catalog::new(&snapshot, source, cwd)` supplies the session order and project list. Pass the same snapshot and its indices to `search`, `detail::DetailLoader`, `statistics`, and `mcp`. Keep a snapshot alive while displaying its results so live updates do not mix session versions. The existing `index::ViewLoader` name remains available as an alias for `History`.

Frontends own their event loops, background execution, polling, selection, and rendering. `history::POLL_INTERVAL` is the shared three-second polling interval. The GUI uses `History::process()` one batch at a time on GPUI's executor; a synchronous CLI or TUI worker can iterate over `History::ingest()` directly. See the `history` module documentation and `tests/history.rs` for complete loading and query examples.

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
