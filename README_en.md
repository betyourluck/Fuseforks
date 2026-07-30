# Outcasts Concordia

**Run a village of AI agents on your desktop.**

Outcasts Concordia is a desktop application for multi-agent orchestration
where multiple AI agents interact and communicate with each other.
Create agents, connect them, and start talking — the village comes to life.
Delegate, divide work, bundle responses, and let agents work automatically when it's time.
You see it all in a single 3-pane screen.

Rust (`agent-core`) + Tauri v2 + Vue 3 + Bun. In-app display name is "Concordia".

## What It Can Do

| | |
|---|---|
| 🏘️ **Build a Village** | Create agents and connect them with lines. The connection map is your control panel |
| 🤝 **Delegation and Merging** | The moderator `ask`s, `plan`s parallel work to agents, and bundles the results |
| ⏰ **Scheduling** | Requests fire at specific times: "every Thursday at 17:00" or "every 10 minutes". No need to write cron syntax |
| 🔌 **MCP** | **Paste Claude Desktop's `mcp.json` directly**. Shared + per-agent |
| 🔍 **Grounding** | Supports Gemini's Google Search grounding. Displays facts retrieved from search separately from facts with missing sources |
| 🛠️ **Built-in Tools** | `remember` / `grep` / `fd` / `diff` / `sd` / `yq` / `file`. Can only structurally read within the work folder |
| 🗣️ **Village Square Log** | A village where you hear others' conversations. You also have the freedom not to listen (as a cost setting) |
| 🏛️ **Village Ordinance** | Common rules that go at the top of every agent's prompt. A normalization layer that equalizes constitutional differences between models |

The connection target is OpenAI-compatible / Anthropic / Gemini. **The base URL is flexible**,
so it connects directly to local LLM endpoints like Ollama or LM Studio.

## Philosophy — Something Real in the Shape of a Toy

This app assumes its users are **engineers with hobbies**. We do not aim to be
an orchestration platform for business — in business there is a cost calculation of labor, and
"running it through AI rather than asking a person to verify" makes sense. But for individuals,
API costs stand out more than one's own time. That asymmetry cannot be moved from the app side.

However, **precisely because it is for hobbies, the substance must be real**. If it were just
a simple group chat tool, engineers would never adopt it as a hobby. Early Linux was dismissed
as a toy by Solaris users, but it was worth taking home because the substance was real Unix —
we aim for that same shape.

Therefore, the design enforces two tiers with different disciplines:

- **The Core (`agent-core` / `data_contract.yaml` / firing rules) is production-grade.**
  Freeze the contract before implementing, and write tests red-first.
  Zero dependency on the GUI (guaranteed mechanically; this crate alone runs headless).
- **The Shell (village, characters, 3 panes) is the hobby experience.**
  "Few settings and easy to understand" is the differentiator. We don't make users climb the walls
  of cron syntax or YAML.

Both cannot be compromised. Don't soften the contract for cuteness.
Don't add settings just to look professional.

## Directory Structure

This project adopts a multi-crate workspace structure to strictly separate responsibilities between the core logic, UI layer, and terminal interface.

```
.
├── Cargo.toml          # Workspace configuration
├── core                # Core logic & LLM state management
├── tui                 # Ratatui-based TUI application
└── gui                 # egui-based GUI application (planned / stub)
```

- **`core/`**: Implements conversation state machines, context building, thread execution via Tokio, and high-performance processing via Rayon. It has no dependency on UI frameworks.
- **`tui/`**: A rich terminal user interface built with `ratatui` and `crossterm`. It handles input events, rendering loop, and event dispatching.
- **`gui/`**: Reserved for a graphical user interface using `egui`.

---

## Crate Isolation Guarantees

To maintain a clean architectural boundary, the codebase strictly enforces the following constraints:

1. **`core` knows nothing about UI**: The `core` crate depends solely on foundational libraries (like `tokio`, `rayon`, `serde`, `reqwest`) and contains zero references to `ratatui`, `egui`, or terminal/window APIs.
2. **UI handles rendering only**: `tui` and `gui` function purely as presentation and input adapters. All state mutations and asynchronous workflows are delegated to `core` APIs.
3. **Uni-directional dependency flow**: Dependencies flow strictly from UI (`tui`/`gui`) down to `core`. Circular dependencies between crates are prohibited at the compiler level.

---

## Concurrency Model — Division of Labor between Rayon and Tokio

The application leverages both `Tokio` and `Rayon` concurrently, mapping each tool to the workload it handles best:

- **Tokio (Asynchronous I/O)**:
  - Manages network communication with LLM APIs (HTTP requests, streaming SSE/WebSocket responses).
  - Handles asynchronous event loops, user input polling, and timer ticks for the UI.
- **Rayon (CPU-bound Parallelism)**:
  - Handles heavy data processing such as markdown parsing, syntax highlighting, token counting, and text search across conversation histories.
  - Keeps CPU-intensive tasks off the async runtime threads to prevent I/O starvation or UI stuttering.

## Screen Layout

Outcasts Concordia provides a split-pane interface designed for multi-agent collaboration, allowing simultaneous observation and interaction with multiple agents.

```
+-------------------------------------------------------------+
|                     Outcasts Concordia                      |
+------------------------------+------------------------------+
|                              |                              |
|         Agent Pane A         |         Agent Pane B         |
|         (e.g., Zari)         |       (e.g., Robot-kun)      |
|                              |                              |
+------------------------------+------------------------------+
|                     Unified Input / Log                     |
+-------------------------------------------------------------+
```

- **Agent Pane**: Displays real-time thought streams, tool executions, and outputs for each agent.
- **Unified Input**: A shared input line where instructions can be targeted to specific agents or broadcasted to the entire village.

---

### Startup

To launch Outcasts Concordia, ensure Rust is installed and run the following command from the workspace root:

```bash
cargo run --release
```

#### Command-line Options

| Option | Description | Default |
| :--- | :--- | :--- |
| `-c, --config <FILE>` | Specify a custom configuration file | `config.toml` |
| `-v, --verbose` | Enable verbose logging for debugging | `false` |
| `--port <PORT>` | Port for the local web UI backend | `8080` |

Upon startup, the system initializes the agent registry, loads village rules, and establishes connections to LLM providers as defined in the configuration.

## How Conversations End (2 Layers)

When you ask a moderator to delegate work to agents, the conversation follows a strict two-layer structure.
The first layer is your interaction with the moderator.
The second layer is the moderator's interaction with agents — you don't directly see this.

### Layer 1: You ↔ Moderator

You send a request to the moderator. The moderator receives it and decides what to do next.
The moderator then either:

1. **Responds directly to you** — "I've finished the work. Here's the result."
2. **Delegates to agents** — "I need to ask agents about this. Let me work in parallel."

If the moderator chooses delegation, they move to Layer 2.
**You wait until Layer 2 is completely finished.** During this time, you cannot interact with the moderator.

### Layer 2: Moderator ↔ Agents (Parallel Delegation)

The moderator now works with agents. This is where the real magic happens.

The moderator can:

- **`ask`** — "Agent A, what do you think about X?"
- **`plan`** — "Agent A, handle X. Agent B, handle Y. Agent C, handle Z. I'll wait for all three."
- **`transfer_to_*`** — Hand off the conversation to Agent A entirely. "Agent A, take it from here."

When the moderator uses **`plan`**, agents work **in parallel**.
The moderator waits for all results, then bundles them into a single response.
This is the **wave pane** — you see all agents' activities at once on the timeline.

### Layer 2 ends when:

- All agents finish their work, OR
- The moderator decides to stop waiting and return to Layer 1

Once Layer 2 ends, the moderator returns to you with a bundled answer.
**You are now back in Layer 1.**

This structure ensures:
- **No interruptions during parallel work** — once delegation starts, you wait
- **Clear separation of concerns** — moderator logic vs. agent logic
- **Predictable conversation flow** — you always know where you are
