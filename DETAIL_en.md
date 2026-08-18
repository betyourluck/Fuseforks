[日本語](DETAIL.md) | **English**

# Outcasts Fuseforks — Details

[README](README_en.md) covers what the app does and how to build it. This document
covers **how it is put together** — directories, the concurrency model, the screen,
the bundled tools, the LLM wire layer, and the operational specifics.

The design decisions themselves live in [`data_contract.yaml`](data_contract.yaml)
(the domain contract) and [`specs/`](specs); this document is the way in.

---

## Directory Structure

```text
Fuseforks/
├── Cargo.toml                       Cargo workspace (resolver 3 / edition 2024)
├── data_contract.yaml               Registry of domain nouns (change types here first)
├── failures.md                      Registry of pitfalls encountered (symptom → root cause → remedy → generalization)
├── fuseforks_icon.png               Source image for the app icon (regenerate with the steps below)
├── specs/                           Specifications (filed → reviewed → rev iterated → Phase split for implementation)
│
├── crates/
│   └── fuseforks-core/                  ★ The core. Zero dependency on GUI layer
│       ├── src/
│       │   ├── lib.rs               Public API and dependency direction declarations
│       │   ├── model.rs             Domain nouns (AgentId / AgentSpec / ModelTemplate …)
│       │   ├── error.rs             CoreError and ErrorPayload for UI
│       │   ├── event.rs             CoreEvent (state changes pushed via broadcast)
│       │   ├── world.rs             Registry. Synchronous pure data structure (no locks)
│       │   ├── config_store.rs      I/O for SKILL.md / Memory.md / Construct.md / icon.webp / Ordinance.md and world.json
│       │   ├── orchestrator/        ★ Lifecycle and message routing (Tokio)
│       │   │   ├── mod.rs           Types, Shared, and the routing skeleton (agent_loop / deliver)
│       │   │   ├── bootstrap.rs     Startup and restore (nothing auto-starts)
│       │   │   ├── lifecycle.rs     Village composition (servants, roles, templates)
│       │   │   ├── runtime.rs       Running and entry points (start / stop / interrupt / send)
│       │   │   ├── settings.rs      Settings and resource access (budget, language, names, MCP, ordinance)
│       │   │   ├── sessions.rs      Switching conversations (new / resume / fork / summarize)
│       │   │   ├── schedules.rs     Schedule firing (ticker, pre-check, delivery)
│       │   │   ├── turn.rs          Running a turn (phases 1-8; **kept whole as the core file** — see the note below)
│       │   │   ├── delegation.rs    Delegation and handoff (ask / plan / transfer)
│       │   │   └── context.rs       Context that goes into the prompt (public square log, presence)
│       │   ├── compute.rs           ★ CPU-bound processing and Tokio↔Rayon bridging
│       │   ├── schedule.rs          Schedule types and firing rules (pure functions. time and timezone as args)
│       │   ├── schedule_probe.rs    Schedule pre-check (pure functions: judgement, appendix, approval key. Spec 28)
│       │   ├── process.rs           Spawning and awaiting a child process (shared by the run tool and pre-checks)
│       │   ├── doc_index.rs         Markdown heading index (pure functions; the PageIndex idea)
│       │   ├── room_log.rs          Plaza-log pure mechanics (visibility predicate / ID resolution / display-ID lengthening)
│       │   ├── attachment.rs        Attachments: validation, storage, GC (pure mechanics; kind decided by magic bytes)
│       │   ├── secret.rs            Secret storage (OS credential store / in-memory for tests)
│       │   ├── tool.rs              ★ AgentTool / ToolRegistry (MCP reception point)
│       │   ├── tools/memory.rs      Built-in tool: remember (appends to Memory.md)
│       │   ├── tools/fs.rs          Built-in tool: grep / fd / diff (read-only. restricted to work folder)
│       │   ├── tools/edit.rs        Built-in tool: sd / yq (write. preview default + diff required)
│       │   ├── tools/file.rs        Built-in tool: file (create / move / copy / trash. new files only here)
│       │   ├── tools/rag.rs         Built-in tool: rag (heading index over declared reference folders. read-only)
│       │   └── llm/
│       │       ├── mod.rs           LlmBackend / BackendFactory / EchoBackend
│       │       ├── canonical.rs     Provider-neutral types
│       │       ├── wire.rs          Provider raw JSON (single source of truth)
│       │       ├── openai_compat.rs OpenAI-compatible adapter (encode/decode pure functions)
│       │       ├── anthropic.rs     Anthropic Messages API adapter
│       │       ├── gemini.rs        Gemini native adapter (Google Search grounding)
│       │       ├── xai_responses.rs xAI Responses adapter (Grok Live Search)
│       │       ├── openai_responses.rs OpenAI Responses adapter (thinking summary, web search)
│       │       ├── responses_input.rs  input list shared by both Responses wires
│       │       ├── client.rs        HTTP core (URL / headers / retry)
│       │       └── error.rs         LlmError (retry decision axis)
│       ├── tests/orchestrator.rs    Integration tests (no network required)
│       └── tests/external_ask.rs    Integration tests: requests from external LLMs (Spec 25)
│
└── apps/
    └── gui-tauri/                   ★ The shell. Depends on fuseforks-core
        ├── src-tauri/src/
        │   ├── lib.rs               Window launch and IPC command registration
        │   ├── state.rs             Orchestrator assembly + event relay
        │   ├── commands.rs          IPC commands (thin forwarding layer)
        │   ├── mcp_server.rs        The door for external LLMs (HTTP + token; Spec 25)
        │   └── probe_approvals.rs   Whether a pre-check may run on this machine (Spec 28)
        └── src/
            ├── types.ts             Mirror of Rust types (hand-synced contract)
            ├── lib/ipc.ts           Typed invoke wrapper
            ├── lib/attachment.ts    Attachment pure functions (kind detection / scaling math / base64)
            ├── lib/carries.ts       Which wire carries which kind (screen-side copy, for warnings)
            ├── lib/pathComplete.ts  `@` path completion (trigger detection / ranking / commit)
            ├── lib/scheduleProbe.ts Pre-check display rules (pure functions; returns dictionary keys)
            ├── workers/imageConvert.ts   Image → WebP conversion WebWorker (keeps the main thread free)
            ├── assets/fonts/        Bundled fonts (never fetched from an external CDN)
            ├── locales/ja.json / en.json        UI text dictionaries (key-set parity enforced by test)
            ├── composables/useOrchestrator.ts   Single store
            ├── composables/useUiSettings.ts     This-screen settings (stored on the device)
            ├── composables/useChatClear.ts      Clearing the chat view (display only, per conversation)
            ├── App.vue              3-pane grid
            └── components/
                ├── AgentList.vue / AgentCard.vue      Left: agent list
                ├── TopologyMap.vue                    Center-top: Kizuna (the servant ties)
                ├── PlanWavePane.vue                   Center-bottom: Work Status tab (plan execution trace)
                ├── BlackboardPane.vue / BottomPaneTabs.vue   Center-bottom: Blackboard tab (shared working notes)
                ├── ChatPanel.vue / ChatInput.vue      Right: conversation (speech bubbles)
                ├── GroundingNote.vue                  Grounding provenance attached to utterances
                ├── AgentSettingsDialog.vue / MarkdownEditor.vue   Modal: settings
                ├── ModelTemplateDialog.vue            Modal: model templates
                ├── BatchWorkDirDialog.vue             Modal: change work folders together (from the list footer)
                ├── OrdinanceDialog.vue / McpDialog.vue / ScheduleDialog.vue   Modal: ordinance / MCP / schedule
                ├── SettingsDialog.vue / SessionDialog.vue    Modal: system settings / conversation list
                ├── RoleDialog.vue                     Modal: roles (servant templates)
                ├── CommandApprovalDialog.vue          Modal: command approval (waiting `pending` requests)
                ├── StatsView.vue                      Full-screen: stats (replaces the three panes wholesale; Spec 39)
                ├── TitleBar.vue                       Custom title bar (Ordinance, Roles, MCP, Commands, Schedule, System Settings)
                ├── StatusBar.vue                      Bottom: MCP server listening state, **the stats entry point**, date, time (same format as the diagnostic log), and version
                └── PaneSplitter.vue / ErrorBoundary.vue / ToastHost.vue / ConfirmHost.vue
```

> **`turn.rs` is deliberately kept whole as the core file** (owner's call,
> 2026-08-11). It was 2,444 lines at the time of the split, but the inside is
> already divided into per-phase functions (`handle_message` / `build_prompt` /
> `present_tools` / `run_turn` / `CallRunner` / `dispatch_outcome`), so the
> cognitive load was already paid down by the function split. Splitting the file
> now would buy only a smaller line count and cost more `pub(super)`.
> **The trigger for splitting it later is not the line count** — it is whether
> `run_turn` and `CallRunner` start changing together more than five times, or
> whether review starts scrolling back and forth inside this one file.

## Crate Separation Guarantee

Dependencies flow in only one direction.

```text
apps/gui-tauri  ──depends on──▶  crates/fuseforks-core
```

The absence of `tauri` in `crates/fuseforks-core/Cargo.toml` mechanically guarantees this separation.
Notifications sent to the GUI merely stream `CoreEvent` into a `broadcast` channel; the core layer remains entirely unaware of whether the receiver is Tauri or test code. Consequently, **the entire pipeline can be verified without launching the GUI**.

---

## Concurrency Model — The Roles of Rayon and Tokio

| Task | Runtime | Reason |
|---|---|---|
| Agent execution, LLM invocation, and message delivery | **Tokio** | I/O-bound. Prevents blocking threads during idle waits. |
| Log token aggregation | **Rayon** | CPU-bound. Maximizes throughput using all available cores. |

The bridge is established via `compute::spawn_rayon` using a `oneshot` channel, ensuring neither side blocks.

> **Never run agent execution on Rayon.** Since Rayon's thread pool is fixed to the physical core count, waiting for network responses there would allocate one thread per agent, capping maximum concurrent execution to the core count (the 9th agent stalls on an 8-core machine).
> The requirement "do not block the UI" is still fully satisfied with this workload division.

---

## Screen Layout

| Position | Content | Reason for Permanence |
|---|---|---|
| Left | Agent list (status, uptime, tokens, startup). The header is the **create** side (model registration, add); the **footer is the operate-on-many side** (change work folders together) | Always visible |
| Upper Center | Kizuna | Always visible |
| Lower Center | Tabs: **Blackboard** (shared working notes) / **Work Status** (execution traces of `plan`, [Spec 08](specs/08_plan-wave-pane.md)) | Always visible (collapsible down to 80px via splitter) |
| Right | Chat (speech bubble format). Below the input box, a button to **clear the view** (**display only — the conversation stays**) | Always visible |
| Bottom | Status bar (**MCP server listening state**, date and time, version) | Always visible (a 22px strip) |
| Modal | Agent settings + configuration file editing (via the settings button on agent cards) | **Opened occasionally** |
| Modal | Model template management (from the agent list header) | Opened occasionally |
| Modal | Role list, add, edit, and delete (from "Roles" in the title bar, [Spec 14](specs/14_role-label.md)) | Opened occasionally |
| Modal | Command approval (from "Commands" in the title bar, [Spec 20](specs/20_command-approval.md)) | Opened occasionally |
| Modal | Schedule list, addition, deletion, pre-checks and approvals (from "Schedule" in the title bar) | Opened occasionally |
| Modal | System settings (from "System Settings" in the title bar, [Spec 13](specs/13_settings-dialog.md)) | Opened occasionally |
| Modal | Conversation list, forking, and export (from "Conversations" in the chat pane, [Spec 12](specs/12_session-persistence.md)) | Opened occasionally |
| **Full screen** | **Stats** (the bar chart icon to the left of the clock, in the bottom bar, toggles it with the three panes; [Spec 39](specs/39_stats-view.md)) — what this village has paid, by conversation × **servant × model** × how each turn ended (**a servant that switched models gets one row per model** — prices differ per model, so folding them together makes any cost estimate wrong). **Hovering the cache-rate cell shows the input breakdown** (cache read / cache write / fresh; [Spec 40](specs/40_cache-write-accounting.md) — **no extra column**: a ninth column in an eight-column table makes horizontal scrolling permanent). **In a village with registered rates, two lines of `≈ $` appear at the top** ([Spec 41](specs/41_model-pricing.md): the amount and the rate date, then the coverage). **Coverage has two axes** (rows and tokens) because **either one alone misleads** — a real run showed `5/7 rows · 99.9% of tokens`: by rows it looks a third short, by tokens it looks complete. **Not a modal: it replaces the three panes wholesale** (they are only hidden — a half-typed message and the current selection survive). The title bar and the status bar stay, but **the title bar's dialog entries are disabled while it is open** (the window controls are not) | Opened occasionally |

Configuration is excluded from persistent panes because **occasionally opened items consume screen area meant for items that are always watched**.

**Buttons are referenced by name, not by glyph.** This README used to say "via ⚙", and when the
icons were replaced with SVGs, only the prose was left behind. Emoji are font-dependent — their
shape and size vary per environment — and they do not inherit `currentColor`, so they cannot follow
the theme ([Spec 13](specs/13_settings-dialog.md) rev3 D8 made "no emoji for permanent elements" a
mechanism). From here on, the ledgers point at a button's **label and location**: the description
stays true even when the artwork changes.

**The on-screen term is "servant"; the domain vocabulary is "agent"** (2026-07-31). Only user-facing strings follow the setting's fiction. Types, fields, IPC commands, event names, and crate names (`AgentId` / `AgentSpec` / `create_agent` / `fuseforks-core`, …), as well as the prose in this README, `data_contract.yaml`, and `failures.md`, stay on "agent". The name may change at any time, but renaming a type means changing Rust, TypeScript, and the ledgers in lockstep — **do not bind what changes easily and what changes with difficulty to the same word**. The rule of record is `vocabulary` in [data_contract.yaml](data_contract.yaml).

**The upper-center pane, "Kizuna" (絆 — bonds between servants), shows who can speak to whom** (renamed 2026-08-05; formerly "village map"). The name comes from the *Kizuna* system in KOEI TECMO's *Romance of the Three Kingdoms XIII*: as there, **the ties themselves are the mechanism, not decoration**. Without a tie an utterance does not arrive; draw one and it does. The pane keeps the name `Kizuna` in English too — *ties* and *connections* are used as common nouns in prose, but **the name on screen is Kizuna**.

**There are two ways to draw a tie** (both human operations — humans draw the lines):

| Entry point | Operation | Applied |
|---|---|---|
| Servant list | **Drag a card onto a node in Kizuna** (2026-08-06) | Immediately |
| Servant settings | Connection checkboxes | On save |

~~| Kizuna | Drag between node handles |~~ — **this row was wrong** (removed
2026-08-13). Nodes were drawn through an overridden node slot that contained no
handles at all, so **dragging between handles on the map never existed**. The
`@connect` handler was there but unreachable.

Ties are **directed** (arrow at the end); a bidirectional pair is drawn as a single line with arrows on both ends. A card drop starts from the card's side — the dropped-on servant becomes someone the card's servant can delegate to. Dropping onto a servant that already has the reverse tie makes it bidirectional. Dropping onto an already-connected servant or onto yourself makes the node pulse briefly — "it arrived but nothing was drawn" (success is signaled by the line itself appearing; no toast). While dragging, the cursor turns to `copy` over the map.

**Only the label changed; the types did not** — `TopologyEdge` / `topologyPositions` / `TopologyMap.vue` / `list_topology` are untouched, following the same discipline as "servant vs. agent" above: do not bind what changes easily to what changes with difficulty. (The wave pane set the precedent when its display name became "Work status" while its contract stayed put.)

Node coordinates moved by hand in Kizuna are saved to `topologyPositions` in `world.json` and restored after a restart (2026-07-31). The truth of the topology is only "which edges exist," so coordinates are kept in a separate field rather than mixed into `AgentSpec` — moving a node does not change the agent definition. The UI auto-arranges only unplaced nodes in a ring. Coordinates for IDs that no longer exist are dropped when an agent is deleted and when `world.json` is loaded.

**Startup is split into two axes** (2026-07-31). The card toggle determines
"whether to include in batch startup" (persistent, stored in `world.json`), while the standby button to its right controls the actual start/stop state of that specific instance. Pressing ▶ in the header wakes all targeted agents together, and once all targets are running, the icon changes to ■ (batch stop).

Originally, a single toggle served both purposes, making it impossible to express *"stopped right now, but I want to wake this lineup next"* — users were forced to press them one by one every time.
Note that ▶ does not trigger automatic startup; no agents run when the application first opens (avoiding a design where opening the app immediately incurs charges). When pressed in a mixed state, **startup takes priority** — stopping everything right then would abruptly kill active conversations.

### Startup

The window appears **first**. Orchestrator initialization involves connecting to MCP servers (launching child processes of external commands), which can take over 10 seconds — previously, `setup` used `block_on`, preventing any window from rendering during that time.
Initialization now runs in the background, displaying a blocking screen with a CSS spinner reading "Initializing..." until completion. The frontend polls the `boot_status` command and waits; only when it turns `ready` does it begin invoking other commands (since `AppState` is managed the exact moment initialization succeeds, calling it earlier would fail due to unregistered state).
Initialization failures surface visibly on top of the overlay with a reason provided — making it visually distinguishable whether one should wait or if something is broken.

Chat displays conversations in speech bubbles separating speakers by side, grouping consecutive messages under a single avatar and name.
**The name and icon on your own rows come from "System Settings" > "General" > "User"**
([Spec 19](specs/19_user-identity.md)); unset, they fall back to "You" and a circle with your initial.
However, **destinations are never dropped** — since this is an orchestration screen, "who sent it to whom" is essential information, so destinations and hops remain outside the speech bubble. Information is never discarded merely to mimic casual chat.

**Tool executions and notices from the venue** (start, stop, role change, interruption,
budget exhaustion, summarisation) **are not drawn as bubbles but as thin single lines.**
Neither is anyone's utterance — they are *records of events*, and giving them the weight
of a bubble makes the conversation itself harder to read. Whether something is a notice
**is decided by its destination**: addressed from the venue to you it is a record, while
addressed from the venue to a servant (a schedule firing) it is a request that really
arrives, so it keeps its bubble.

**In-flight turns can be interrupted mid-way** ([Spec 10](specs/10_turn-interrupt.md)).
The "■ Stop" button next to a typing bubble cuts that agent's current turn; "Stop all turns" in the header cuts every in-flight turn in the village. What gets cut is the **turn**, not the agent — it stays running, conversation and history survive, and the next request is handled normally. Interrupting a facilitator also stops only the worker tasks spawned by its plan wave (unrelated requests the same workers were handling in parallel are untouched). Detection happens at round boundaries, so the turn stops as soon as the in-flight LLM call or tool finishes — a "stop requested…" indicator covers the gap. The fact of the interruption is recorded as a single System line in the conversation log.

**A new conversation can be started via "New Chat"** (header button, with confirmation).
This clears only the conversation log and individual agent histories from the screen, preserving operational status, cumulative statistics, long-term memory (`Memory.md`), and individual MCP connections — switching a "conversation" rather than an "agent". This reflects a token philosophy of avoiding continuous charges for old contexts whenever topics switch. It is intentional specification if a response currently being processed appears once immediately afterward (logging utterances as facts that occurred).
**When a conversation gets long, press "Summarize and continue"** (Spec 12). Each **running** servant's model is called once to summarize its own memory, which shortens every subsequent prompt (stopped servants are skipped — there are no further turns to shorten, so their tokens are not spent; start one and press again to summarize it). **It never runs on its own** — summarizing is an LLM call, i.e. tokens, and it competes with the token budget ceiling, so it runs only when someone presses the button knowing the cost. The original exchanges are not deleted (they remain readable via export).

**The previous conversation is kept on disk rather than discarded** (Spec 12). The adjacent "Conversations" button reopens it, and also offers **forking**, **exporting** to JSONL, and **deleting**. **Forking means "go back to just before this request"**: the chosen request's text is put back into the input box. Edit it and send, and everything up to that point stays the same while you try a different way of asking (the original conversation is untouched). Reopening and forking are refused while a turn is in flight, because the answer would land in a different conversation — stop it first with "■ Stop".

**Plaza logs can be opted out per agent** (Settings → "Conversation Context" → "Can hear plaza conversations").
By default, every agent receives the latest 12 messages × 200 characters of "conversations exchanged in the plaza" each turn. Excluding roles that do not require shared context eliminates that fixed overhead. **This is a receiver-side setting only**; opting out does not stop an agent's own utterances from being heard by others (it is a cost feature, not a privacy feature).

**When an utterance is cut at 200 characters, the log says so and gives the original length.** Passing a bare `…` lets an agent read it as "the speaker finished there" and treat the excerpt as the whole utterance. **Each clipped line now starts with an utterance ID, and passing that ID to the `room_log` tool returns the full text verbatim** ([Spec 22](specs/22_room-log-pull.md)). The previous guidance — `ask` the original speaker — spent the speaker's turn and tokens and returned a **retelling** rather than the original; the tool reads the conversation log directly. Only utterances longer than 20,000 characters are truncated, with `ask` remaining as the fallback for the remainder. The tool is offered only to agents that hear the plaza log (opting out removes both the excerpts and the tool). **A user's message addressed to someone else stays unreadable even with its ID** — "recipients outside the address must not even know the message exists" holds on the pull path too.

**Agents are aware of each other's operational status** ([Spec 06](specs/06_peer-presence-in-prompt.md)).
While humans could see UI status indicators, agents could not, forcing coordinators to spend tokens on roll-call queries like "try throwing this and see what happens." The prescription has two layers: **roster as authority, notifications as narrative**. The roster occupies a single line in the variable portion of the system prompt (`agent_id (display name) [role]: running`, listed in connection order; the role appears only when one is assigned. Cache stability boundaries are established right before the roster, so cache hits remain unbroken even when status or role changes).
Entries and exits stream into the conversation log in the same format as chat notifications (triggered only on change, preserving order, independent of `hearsRoomLog` — plaza opt-out is a cost feature and must not break delivery correctness). Failures report **only the type**, stating "Stopped due to failure" — the reason (`last_error`) is diagnostic info for the user, and leaking it to other agents would expose internal details outside the conversation.

**Tools executed by agents appear interspersed within the conversation timeline.**
Since tool results disappear inside prompts, viewing only utterances makes it indistinguishable from "side effects occurring silently in the background." They appear as a thin, muted single line stating "Agent executed grep", with failures distinguished by color. They are interleaved because the two are causally linked — the workflow where an agent asked to "investigate" runs grep three times before answering only appears in that sequence. Agent cards also display "Recent Tools" (which represents "what this individual is doing right now" rather than historical logs).

Each utterance includes a **copy button** underneath; clicking it copies the body text (Markdown source prior to rendering) to the clipboard. **Hops (transfer counts) are relegated to a hover over the timestamp** — while important as fuel to prevent infinite loops, they are diagnostic data and unsuited for permanent display.

**During response generation, a "typing..." bubble (three-dot animation) appears.**
The core streams the start and end of utterance processing as paired `agentTyping` events, and the UI displays the bubble only during that window. Because processing including LLM calls can take tens of seconds, its absence would make it impossible to distinguish between "not received" and "thinking".
Note that text bodies are displayed all at once upon completion rather than streamed (SSE support is a candidate for future specs).

**Agent statements are rendered in Markdown** (`lib/markdown.ts`, markdown-it).
Models frequently return Markdown, and raw syntax makes text unreadable. User inputs and system notifications remain plain text — if strings typed by the user were transformed by rendering, verification of sent contents would become impossible. LLM output is treated as untrusted text, utilizing `html: false` from the outset to prevent interpreting raw HTML as tags (and `javascript:` links are caught by markdown-it's default validation). Link clicks are intercepted and opened in an external browser (navigating within the webview would replace the entire app screen).

Input fields follow Kataribe's `ActionInput.vue` (`ChatInput.vue`).

- Begins at `rows="1"` and **grows upward** with each line break (due to the bottom-fixed layout)
- Stops growing at 220px and switches to internal scrolling
- The send button floats **inside** the input field and appears only when content is present (↵ icon)
- Enter to send, Shift+Enter for a line break
- **Alt + ↑↓ moves the selection in the servant list** (works even while typing in the input box)

**Enter during IME conversion does not trigger sending** (`event.isComposing` is checked).
Without this check, unfinished sentences would fly off the moment Japanese conversion is finalized.

Boundaries can be adjusted using two splitters (double-click restores defaults, arrow keys also move them).
Dimensions are saved in `localStorage`. Since this is a display preference rather than orchestrator state, it is not mixed into `world.json`.

---

## UI Synchronization Rules

The truth resides in the core, and the `state` of `useOrchestrator` is merely its projection. To prevent projections from drifting, only two rules are established.

1. **Mutation-related IPCs must be wrapped with `mutate()`, and regardless of success or failure, always re-read from the core.** Previously, we used an approach where each call determined whether "re-synchronization is needed here," and only the paths where that decision was omitted (such as connection updates and reordering) were left with stale displays. It is more correct not to make them the subject of judgment. Use `guard()` for reference-based operations.

2. **Screens with drafts being edited must be recreated with the core's values after saving.** After saving and closing the form, the saved result would disappear from the screen, making it look as though it was "not reflected." Stay on that item even after saving, and display the values received by the core as they are (`reseedDraft` in `ModelTemplateDialog`).

Do not substitute values locally with the assumption that "it should be like this." It causes discrepancies when the core makes a different judgment (for example, the source of retrieval after key deletion is `Unset` rather than `NotRequired`).

---

## Attachments ([Spec 23](specs/23_image-attachment.md) = images / [Spec 36](specs/36_multimodal-attachments.md) = audio, video, PDF)

**Paste** a file into the input box (Ctrl+V) or **pick** one with the clip button on
the left, and the addressed servant sees or hears it and answers. Drag-and-drop is
deliberately not an entry point — Tauri intercepts in-page drops (and Ctrl+V is the main
path for screenshots anyway).

**One attachment per utterance.** There are four kinds — **image / audio / video / PDF** —
and **the kind is always decided by the magic bytes of the content** (neither the file
name's extension nor its MIME type is read; both can be rewritten, and trusting them lets
content and kind disagree all the way to the wire).

| Kind | Accepted formats | Limit | Conversion |
|---|---|---|---|
| Image | anything the browser can decode (png / jpeg / gif / webp …) | 10 MB source → 2 MB converted | **UI scales to 1568px long edge and converts to WebP** |
| Audio | mp3 / wav | 10 MB | none |
| Video | mp4 | 12 MB | none |
| PDF | pdf | 10 MB | none |

Conversion runs in a WebWorker, so picking a large image never freezes the screen.
**Only images are converted** — bundling audio/video transcoders would change the
dependency footprint by an order of magnitude (the ffmpeg family is not available in pure
Rust), so unsupported formats are refused at the entrance instead.

Limits are set by **the narrowest wire that carries the kind** (Gemini's 20 MB inline
request total), counting base64 expansion (4/3) and headroom for the prompt.
**Limits do not vary by recipient** — if the same file passed for one servant and failed
for another, the rule would stop being readable from the screen.

### Which combinations travel

**A single predicate** (`Provider::carries`) holds which wire carries which kind, and
**only the send entrance reads it**. The table was decided by measurement (not by HTTP
200 — audio was checked against a spoken passphrase, video against a colour transition,
PDF against a proper noun in the body).

| Wire | Image | Audio | Video | PDF |
|---|---|---|---|---|
| OpenAI-compatible | ✓ | ✓ | — | ✓ |
| Anthropic | ✓ | — | — | ✓ |
| **Gemini native** | ✓ | **✓** | **✓** | ✓ |
| xAI Responses | ✓ | — | — | ✓ |
| OpenAI Responses | ✓ | — | — | ✓ |
| **Meta native** | ✓ | **✓** | **✓** | ✓ |

**Only the two native paths carry audio and video** (Gemini and Meta). Spec 23 had
frozen "do not implement attachments on the native path"; **that premise (images travel
over the compatible endpoint anyway) disappeared from the side of the kinds**, so Spec 36
reversed it. **Meta became the second one in Spec 37**, and **video passed against the
prediction** — reasoning by analogy from OpenAI Responses would have written ✗, but a
payload-less `input_video` came back with a named 400 (`requires video_url or file_id`),
which is what made the probe possible.

**Combinations that cannot travel are warned about on paste, and refused by the entrance
on send** — and when refused, **nothing is stored and no turn starts** (not a single
token is paid). The guidance text is assembled from the table, so changing the table
moves the guidance with it.

**"Can the wire carry it" and "will the model accept it" are different layers.** The
compatible endpoint really does have an audio field, but a model without audio support
returns 400. The first is refused by the entrance; the second surfaces as the provider's
response — **treating a structural impossibility and a runtime refusal in the same layer
makes them indistinguishable on screen.**

### It reaches the model exactly once

**An attachment is sent to the model only on the turn it arrives.** From the second
turn on, the model can no longer see it (**it stays visible on screen**). The input box
says the same thing next to the attachment chip.

This is design, not a limitation, and the reason is cost. History is a sliding window of
the last 8 exchanges, so keeping an attachment would **resend the same file up to 8
times**. The measured magnitudes differ sharply by kind:

| Kind | Per attachment (measured) | If kept for 8 turns |
|---|---|---|
| Image (1568px, 16:9) | 1,792 | 14,336 |
| Audio / video | depends on length and resolution (~1,000 for a 10-second 720p clip) | 8x |
| PDF (9.3 MB) | **~165,000** | **~1.3 million** |

That **breaks the one-to-one correspondence between what you pay and the act of
attaching**. If you want the model to look again, attach it again: an explicit action has
clearer causality than automatic retention.

**Within a turn it is resent every round, but the cache absorbs that.** In the turn that
actually carried the PDF above (3 rounds), `prompt` swelled to 520,323 — yet the uncached
part was 173,564, **exactly one round's worth**. **Read attachment cost as `prompt`
paired with `cached`, never `prompt` alone**, or it looks like you paid a multiple of the
round count.

**The fact that an attachment was there does survive.** Only the content is dropped; the
servant keeps a line saying "one attachment arrived on this turn, and it is not visible
from the next turn on." So when you later ask it to "show that image to X," it can answer
**"I no longer have it — please attach it again, addressed to X."**

Before that line existed, a servant *remembered the description it had written but did
not know an image had ever existed*. Asked to forward it, the recipient received a body
with no explanation. **An image not being forwarded matters less than nobody being able
to tell why**, so the fact alone is kept.

### People who never use it never pay for it

**No tool is added.** An attachment is a kind of input, not a capability, so neither the
tool schemas nor the system prompt grow by a single character. For utterances without an
attachment, the wire output is **byte-for-byte identical** to what it was before this
feature existed (tests pin this for **all six wires**). In a village that never attaches
anything, nothing sent and nothing paid changes.

### Where attachments do not go

| Path | Attachment | What happens instead |
|---|---|---|
| User → servant | **Delivered** (if the wire carries the kind) | Otherwise the entrance refuses — nothing stored, no turn started |
| Servant → servant (`ask` / `plan` / transfer) | **Always** dropped | The delivered body is prefixed with "(the 1 image is not forwarded)" |
| Plaza log (excerpts of others' conversations) | Dropped | Only its existence is noted, as "(1 image)" |

**Forwarding drops the attachment even when the recipient could carry it.** Passing it on
would turn one attachment into N turns' worth of payment and break the one-to-one
correspondence — hence the division of labour: **only the send entrance reads `carries`.**

**Never drop silently** is the rule here. If the recipient cannot diagnose *why* it cannot
see an attachment, a mechanism working correctly is indistinguishable from a broken one.
In practice this helped **recovery as well as diagnosis** — a sender who read the notice
transcribed the picture's contents into text and passed that along instead.

### Where the bodies live, and how long

Images enter neither the conversation log nor the conversation store (`sessions.redb`).
The body lives at `{workspace}/attachments/{uuid}.{ext}`, and the utterance carries only a
reference. **On startup, anything older than 30 days — and, beyond a 500 MB total, the
oldest first — is deleted.** A deleted image becomes a "this image was deleted after its
retention period" placeholder in the conversation pane; showing nothing would make it look
as if the attachment had never happened.

### Provider coverage (measured 2026-08-06)

WebP and JPEG were sent to five endpoints — Anthropic, OpenAI, the Gemini compatibility
layer, xAI, and a Sakura-hosted Qwen3-VL — and **all ten combinations read the image
correctly**. Hence WebP is the default. Some compatibility servers may still lack a WebP
decoder (xAI's own documentation lists only jpg/png), so **on a 400 the request is
re-encoded to JPEG and sent once more.** If both are refused, the screen says this
endpoint does not accept images.

---

## Path Completion in the Input Box ([Spec 24](specs/24_path-completion.md))

Type `@` in the input box and the **files in the addressed servant's work folder**
appear as candidates. Type part of a filename to narrow them down; picking one
inserts **its relative path** into the message.

```text
@24_path      →  @specs/24_path-completion.md
```

**Only the path goes in — the file's contents are not expanded.** The servant reads
it with the `file` tool if it needs to, and pays nothing if it doesn't.

### Why the contents stay out

**Because the two histories have different lifetimes.** A tool result exists only
for the turn it happened in, whereas your own message rides along in the prompt for
**the last 8 exchanges**. Embedding contents in the message would **resend a
10,000-token file up to 8 times**.

Passing just the path means **the existing `file` tool's lifetime already satisfies
that condition** — where image attachments needed a purpose-built "this turn only"
mechanism, this one needs none.

**The cost is stated plainly**: when the contents are needed, one `file` round trip
remains. **What disappears is the *searching*, not the *reading*.** That still pays —
the rounds a servant spends hunting with `fd` or `grep` vanish, and with them the
**full prompt resend that every extra round costs**.

### What never appears as a candidate

The walk shares its rules with `fd` and `grep` (one implementation, not two):

- **Hidden folders** (`.git`, `.github`, anything starting with `.`)
- `node_modules` / `target` / `dist` / `build` / `out` / `vendor`
- Anything past the **20,000-entry** limit (the cut-off is shown in the list)

**Not being offered is not the same as not being readable.** Type
`.github/workflows/build.yml` by hand and the `file` tool reads it. **Completion
offers candidates; it is not a boundary** — the boundary lives on the reading side,
where the work folder's outside is structurally unreachable.

Note that **`.gitignore` is not consulted.** The list above is matched by name, so a
folder that is git-ignored but absent from that list still shows up.

### `@` is not reserved for files

When mentions of servants arrive later (user broadcasts), they will ride on **the
same `@`**. Only files appear today, but the symbol is not pinned to files.

### Enter means three things

| Situation | Enter |
|---|---|
| Mid-conversion in a Japanese IME | **Commits the conversion** (neither sends nor picks) |
| Completion is showing candidates | **Picks the candidate** |
| Otherwise | Sends |

To send while the completion is open, close it with `Esc` first. When no candidate
matches, completion does not capture keys, so Enter sends as usual.

---

## How Conversations End (Two Layers)

Major frameworks (OpenAI Agents SDK / AutoGen / LangGraph) all handle conversation termination through two layers: **semantic termination** and **mechanical limits**. An implementation with only one of the two does not exist.

### Layer 1: Semantic Termination — Decided by the Model

Adopts the rules of the OpenAI Agents SDK. **Text output without tool calls is the final output.**

Provide one `transfer_to_<agent>` tool per connection destination (following the naming convention of the same SDK), with `tool_choice` set to `Auto`.

- Called a tool → Transfer to that partner, and the conversation continues
- **Called multiple `transfer_to_*` tools simultaneously → Deliver in parallel to all destinations** (fan-out). Duplicates to the same destination are collapsed into one via first-come-first-served.
- **Returned only the body text → Conversation ends**. Returns to the user

**User transmissions are limited to a single destination.** Select a single partner in the left pane or in Kizuna. When you want to run multiple partners simultaneously, ask one acting as a facilitator and have them deploy via `ask_*` / `plan` (orchestrator-workers). Broadcasts run everyone's turns in parallel, and everyone responds without seeing anyone else's answers, causing confusion — whereas orchestrator-workers have each agent speak exactly once, preventing duplicates structurally.

### Parallel Delegation — `plan`

Only "scattering in parallel, waiting for all, and bundling" was missing from the toolset.

| Path | Parallelism | Destination of Answers |
|---|---|---|
| `ask_*` (Delegation) | **Serial** (wait one by one) | Returns to the requester |
| Fan-out of `transfer_to_*` | Parallel | **Scatters to the user** (does not converge) |
| `plan` ([Spec 04](specs/04_plan-parallel-delegation.md)) | **Parallel** | **Bundled and returned to the requester** |

There are three schools of thought on **who creates the process** — humans writing statically (Airflow style), high-level models creating dynamically, and emergence (no planning). Since Fuseforks's concept is "few settings and easy to understand," **the path of having humans write DAGs is not adopted**.

What is adopted is **the model creates the plan, and the code guarantees the execution**. When a facilitator declares a wave of "which worker to ask for what," parallel delivery, convergence, timeout, aggregation, and fuel are deterministically handled on the code side. Seeing the wave results and issuing the next wave is again the model's job, and this round-trip serves as a dynamic alternative to multi-stage DAGs. Not having all stages declared in advance is because it breaks down when "the second wave depends on the results of the first wave" (which would merely reinvent cumulative errors of sequential improvisation in planning within the schema).

`plan` is presented **only to agents with 2 or more connection destinations**. Settings like a "facilitator flag" are not added; the topology itself determines the role. Destinations are closed by the `enum` of connection IDs, and non-existent partners cannot be pointed to in principle.

**Parallelism applies to delivery, not execution.** Each agent's inbox is single-channeled and turns are processed serially, so if a worker is busy with another task, it waits accordingly.

#### Wave Pane — Execution Traces of `plan` ([Spec 08](specs/08_plan-wave-pane.md))

The lower section of the central pane depicts the execution of `plan`. Columns = waves, rows = agents, cells = task resolution states (running / reply / transfer / undeliverable / no response / timed out), equivalent to Airflow's Grid. It is **Airflow "style" rather than Airflow** — the preceding stance that humans do not write DAGs remains unchanged, and what is drawn is the **execution trace** of the plan created by the model, not a place to edit. Classification is carried by types rather than word parsing (the core carves it out with `Reply.kind`). Records are process-lifetime in-memory rings (the latest 50 waves), and identification uses `plan_id` (hidden from the model — the bundling phrasing has not changed by a single character). The stderr observation lines `plan wave:` / `plan bundle:` remain as they are.

Agent-initiated broadcasts (where a single body passes the same content to multiple recipients) continue to work. Each such message includes a list of all destinations, and the recipient's prompt contains a note stating, "Everyone has already received this." Without this, each agent decides that "only I have heard this" and conscientiously transfers to connection partners, causing echoes (failures.md #20).

Since the tool cannot read from the fact that not calling means termination, the procedure is explicitly stated via system messages (same intent as `RECOMMENDED_PROMPT_PREFIX` of the same SDK). For servers that do not implement tool calls, the termination marker `[[END]]` is provided (isomorphic to AutoGen v0.2's `is_termination_msg`).

### Layer 2: Mechanical Limits — Safety Net

Each utterance has a `hop`, and upon reaching `max_hops` (default 8), the chain is cut off and notified via `CoreEvent::HopLimitReached`. This has the same position as LangGraph's `recursion_limit`, representing **fuel exhaustion rather than a way to end**.

> At first, only Layer 2 existed, and the design transferred whenever it produced a response. The model had no way to finish. A round trip that merely conveyed one sentence consumed about 1,200 tokens. See [failures.md](failures.md) #11 for details and sources.

#### Token Budget — an automatic ceiling per request ([Spec 11](specs/11_token-budget.md))

While `hop` bounds *depth*, this bounds *spend* — an orthogonal brake. Each request causality (one of your utterances per recipient, or one scheduled firing) gets a budget denominated in effective tokens, shared by every turn that cascades from it via ask / plan / handoff. When it runs out, the turn is cut at the round boundary and a single System line appears in the conversation (the ceiling, and that you can simply ask again). Agents stay running; the next request gets a fresh budget.

- **Effective tokens** = uncached input ×1 + cached input ×0.1 + output ×4. Proportional to real cost rather than raw counts, so a healthy long job with 87–99% cache hits is not falsely cut.
- **One setting**: `tokenBudget` (effective tokens). **Change it from "System Settings" > "Cost Management" in the title bar** ([Spec 13](specs/13_settings-dialog.md)); saving takes effect **from the next request**, with no restart. It lives in `world.json`, so it is stored in the village and travels with it when shared. Freshly created villages get a default of 1,000,000. **Existing villages are not silently changed** — a startup WARN points you to the setting instead.
- **Guidance**: ~6 agents run fine under 1,000,000 (a measured healthy 6-agent request ≈ 250K effective). Villages running 8-stage flows, ~12 agents, or output-heavy code generation should use 2,000,000–3,000,000.
- The remaining balance is never injected into prompts, and there is no automatic retry. The ceiling counts silently and only speaks when exhausted.
- **Near the ceiling, part of a wave may come back without an answer** ([Spec 38](specs/38_budget-reserve.md)). At each round boundary the budget for the next call is **reserved up front**, so an agent can be refused while the balance is not yet zero. That agent's turn is cut and **nothing re-asks on its own**, so the bundle comes back with a gap. If you see gaps, raise the ceiling or fan out to fewer agents.

### Tool Execution Loop

This runs in the same framework as transfers. The model receives transfer and execution tools as one set, and the receiver distinguishes the returned calls.

```text
Call the model
 ├ Called transfer_to_*  -> transfer and end this turn (no result is returned)
 ├ Called an execution tool -> execute it, add the call and result as a pair, then call again
 ├ Called a name never offered -> answer "no such tool" as a result, then call again
 └ Called no tool -> final output; end the conversation
```

The third branch used to be **silently discarded** ([failures.md](failures.md) #47). From the model's side, "I called it and nothing happened," so the body it wrote was delivered instead of the vanished call. Returning the failure as a result lets the model pick a valid name.

The limit is `max_tool_iterations` (default: 12; **it can be overridden per agent through the "Tool Execution Limit" setting**). The initial default was 6, but ordinary research delegation (grep -> narrow down -> read) exhausted it three times across two sessions. A low limit is not a saving: a new request burns the same tokens again without producing the result of the previous effort.

The system also detects **repeated calls**, independently of the count limit, because repeatedly calling the same tool with the same arguments is a real dead end ([failures.md](failures.md) #41, remedy 1). If the **tool name + arguments + result body** match exactly twice, the third invocation is not executed: a short notice is returned instead and `CoreEvent::ToolRepeatBlocked` is emitted.

The criterion is the **result body**, not an error, because built-in tools return failures as `Ok(<error text>)` rather than `Err` as a consequence of the rule that tool failure must not stop a conversation. Counting `is_err` would have detected none of the real loop that burned tokens (twelve failed `sd` rounds in #39).

Counting is **per (tool name + arguments)** and does not require adjacency. The first implementation compared only against the immediately preceding call, and **it never fired in practice**: the model issues two or three calls per round in parallel, so the same re-read appears across rounds and an intervening call breaks the count. (Measured: the same `file` call appeared in rounds 24, 25, and 28, but the `grep` in round 26 broke the count and the third call went through.) When the same call returns a **different** result, the count restarts — an append that progresses, or a state that finally changed, is not a repeat.

**What stops is the single call, not the loop.** One duplicate among parallel calls must not kill work already in progress. The loop is cut only when **every tool call in that round was blocked**, which is the same as saying the round did nothing new. Keeping the notice short is where the saving comes from: adding the same 12,000 characters again would resend them in every subsequent round.

A count limit bounds cost only when each round costs the same; tool loops resend all prior history on every round. This is why the safeguard prevents wasteful rounds rather than simply reducing the permitted count.

When a round cut off by the limit has no text response, the model is called **once more with tool use prohibited** and asked to put its research into prose. Intermediate tool results exist only in that turn; discarding them unbundled makes every "continue" restart research from zero and hit the same limit. If it is still empty, it is replaced by an honest message; **empty responses are never recorded**. An empty utterance is poison that can make the next turn fail with 400 through both history and the wire ([failures.md](failures.md) #29; history and the Anthropic encoder are both defended).

**Calls and results are always added to history as a pair.** Adding only a result makes providers reject it as a result without its corresponding call. The wire formats differ entirely between the two providers (OpenAI-compatible uses `role: "tool"` + `tool_call_id`; Anthropic uses a `tool_result` block in a `user` message), so adapters perform the translation.

Tool failures do not end the conversation. Errors return to the model as strings, and the model reads them to decide what to do next. **Failing an entire turn just because an argument was wrong would end the conversation.** Invocation itself is announced through `CoreEvent::ToolInvoked`; results disappear inside the prompt, so the UI must not leave silent side effects. **That announcement also carries the one-line reason the model wrote** (see "Tool reasons" below).

There are nine built-in tools. External capabilities are added through MCP.

| Tool | What it does | Scope |
|---|---|---|
| `remember` | Append one line to `Memory.md` (self-updating long-term memory) | Calling agent's configuration folder |
| `grep` | Find **lines** matching a regular expression (`path:line number: content`). `count_only: true` returns **counts only**, `include` narrows by **file name**, `context: 1–3` also returns **surrounding lines** | **Work folder only** |
| `fd` | Find files and folders by **name** (relative path list; folders end in `/`) | **Work folder only** |
| `diff` | Compare two files as a unified diff | **Work folder only** |
| `sd` | **Replace** content in a file with a regular expression (editing). `paths` previews diffs across **several files at once** (preview only, up to 20) | **Work folder only** |
| `yq` | Get / set / remove only TOML or JSON values (editing) | **Work folder only** |
| `file` | File and folder operations (`read` / `write` / `append` / `mkdir` / `move` / `copy` / `remove`). [Spec 09](specs/09_file-tool.md) | **Work folder only** |
| `rag` | Query Markdown in **declared reference folders** through a heading index ([Spec 18](specs/18_bundled-doc-index.md)). Declaring a folder offers the tool automatically | **Declared folders only** (read-only; independent of the work folder) |
| `run` | Execute allowed commands ([Spec 15](specs/15_command-execution.md)). **Off by default** | **Unrestricted** (the allowlist is the only enclosure) |


### Tool reasons ([Spec 27](specs/27_tool-call-reason.md))

In the conversation pane, each tool row shows **why** it ran next to **what** ran.
The model writes that line itself, **up to 60 characters** (anything longer is trimmed).

**Nobody guarantees that the line is true.** It was written by the very agent that ran the
tool, and there is no path that verifies it. **What you can confirm is that nothing happened
silently**; you cannot confirm that it did what it said. The former is what this mechanism is
for; the latter is a different problem.

**Two kinds of tools show no reason.**

| No reason | On screen | Why |
|---|---|---|
| MCP-provided (`MCP_DOCKER__fetch` and friends) | **"external tool"** | The schema belongs to the server. Adding our own field and forwarding it makes servers that declare `additionalProperties: false` reject the call |
| `ask` / `plan` / `room_log` | **the row is absent** | Their arguments already appear as messages in the conversation. A reason would put a summary next to text that is already on screen |

**"No reason given" and "external tool" are different states.** The first means the model
did not write one; the second means we never asked. Collapsing them into one blank makes
external tools look like the ones running silently.

**The outcome reads "returned" / "returned an error", never "succeeded" / "failed".**
Built-in tools report failure in the **body** rather than as a return error, so the outcome
marker only means "did the return value carry an error" — **whether the side effect
succeeded is a separate question**, and the screen cannot answer it.

**Villages that never use it still pay.** Unlike attachments or path completion, this is
on for everyone by default. Measured over 969 tool calls across 355 turns in
`concordia.log`, it adds **1.5–2.2% effective tokens on turns that call tools**, and
**zero on turns that call none**.

### Command execution (`run`)

**Only commands matching a per-agent allowlist can run**
([Spec 15](specs/15_command-execution.md)). Configure it under
Agent settings → config files → `run.json`; press "Insert template" when empty.

There are three states: a call matching `allow` runs without approval; a call
matching `deny` is refused **and not recorded** (so a decision you already made
does not reappear every time); a call in neither list is refused and recorded
under `pending`, where **you press approve or reject** in "Commands" on the title bar
([Spec 20](specs/20_command-approval.md)). Every servant's waiting requests gather in
one screen, and the entry shows **the village-wide total**. You never have to open the
JSON and copy lines by hand (direct editing of `run.json` remains available for finer
control).

- **Approving asks what to permit**: this call only (exact match), or any arguments
  after this prefix (trailing `*`). **The narrow one is the default**, and the
  resulting pattern is shown literally so you can check it before pressing.
- **Rejecting writes to `deny`**, not merely removing it from the list — removal alone
  would let the same command queue up again on the next call.
- **There is no "approve all."** Bulk approval skips the decision of *what* to permit,
  which is the substance of allow-listing.
- **Once even one entry is permitted, that servant can run commands from its next
  turn** (before that, the tool is not even offered to it).
- **Pressing a request that has already been pushed out does nothing** (waiting
  requests are capped at 20 per servant, oldest discarded). The screen says so.

Patterns are exact match plus a trailing `*`. `"ruff"` matches **only a call with
no arguments**; write `"ruff *"` to allow any arguments, or `"ruff check *"` to pin
the leading ones. **Forgetting `*` is more dangerous on `deny`**: on `allow` the
command simply fails and you notice, but on `deny` the thing you meant to block
silently keeps passing. There are no mid-pattern wildcards.

**No shell is involved.** Agents write an executable name and an argument array,
and matching runs against that array, so `&&`, `|` and `$(...)` structurally do
not exist — pipes and redirection are unavailable.

**`run` is off by default.** Updating the app never grants command execution;
you must enable it per agent *and* have at least one `allow` pattern.

> **This is not a safety mechanism.** Putting `python *` in `allow` lets that agent
> run arbitrary code. The work-folder limit, trash-only deletion and environment
> scrubbing **do not constrain `run`**, and `deny` cannot enumerate danger
> (blocking `rm` leaves `python -c`). The value of `deny` is **remembering a
> decision you already made**, not stopping hostile input.

### Heading index (`rag`)

**Queries the Markdown in declared folders through the heading hierarchy the
author wrote** ([Spec 18](specs/18_bundled-doc-index.md)). Add folders under
"RAG folders" in agent settings and the `rag` tool is **offered automatically** —
there is no checkbox; **the declaration itself is the switch** (remove every
folder and the tool disappears). It exists for material that belongs to no work
folder — standards, specifications, papers — and forms an axis independent of
the work folder.

Three ops: `outline` (folder listing plus each file's heading tree) →
`search` (matching lines **with the section path they belong to**) →
`read` (the body of one section, addressed by heading). The intended shape is
to locate by structure before reading anything in full.

- **No vector database, no embedding model.** The variable that matters is not
  how you retrieve but **how you cut** — at headings a human placed, not at
  boundaries a splitter guessed (the PageIndex idea; this mechanism replaced and
  removed the old bundled RAG, whose `HashEmbedder` index was permanently empty)
- **It only works on documents whose headings carry real structure.** On rough
  notes or generated Markdown it is barely better than `grep`
- **Read-only.** There is no write path. Declared folders may lie outside the
  work folder — a reference pile like `D:\ManualeRAG` can be pointed at directly
- **If you run a serious personal knowledge base, MCP is the primary road.**
  Notion and Obsidian connect naturally over MCP; the built-in index is not a
  substitute — it is here because this product decided to treat "who knows what"
  as a first-class axis

> **The declaration is an enclosure, not a safety mechanism.** A declared folder
> is readable in its entirety by that agent. Under prompt injection, what was
> read can travel via `ask` / `plan` to **every reachable agent and each of
> their model providers**. Do not declare folders containing secrets.

`grep` / `fd` / `diff` are built in because these are the tools coding agents use most often, and they are dramatically cheaper and faster than reading entire files. Token efficiency is one of this product's primary concerns. MCP filesystem servers also support search, but it matters that these work in every environment without an external process.

For `grep`, **the cap applies to what is displayed, not to what is counted**. Even when matches exceed 100, the total returned is the real total, along with a per-file breakdown. (Returning the displayed count as the total would force a second search just to learn how many matches exist.) When only the count is needed, pass `count_only: true` to omit the matching lines.

**`include` narrows the search by file name** ([Spec 16](specs/16_grep-precision.md)). When the same term appears across `.md`, `.rs`, and `.yaml`, the 100-match budget fills up with prose and the code matches fall off the end. **It takes a regular expression, not a glob** — write `\.rs$`, not `*.rs`. Every pattern in `grep` / `fd` / `sd` is a Rust regular expression, and mixing two pattern languages inside one tool guarantees they get confused; a glob-shaped `include` fails to compile and comes back with a message naming the mistake, so it never silently returns nothing. **It narrows which files are read, not which are walked** — when the 20,000-file scan limit is hit, only `path` fixes that; `include` does not.

**`context: 1–3` also returns the lines around each match.** A matching line is prefixed `path:line number:` while surrounding lines use `path:line number-`, so **the two are distinguishable by the separator** — without that, context lines get reported as matches. This removes a whole round trip that would otherwise re-read the file just to see what surrounds a hit.

`fd` matches **only names** (the final path component). Matching against whole relative paths would pull in every descendant of a matching directory and fill the list with noise. Matching is case-insensitive by default: name searches inherently have varied spelling, and exact matching by default creates unnecessary miss-and-retry cycles. This is deliberately the reverse of `grep`.

**The search scope is closed to each agent's work folder** (`AgentSpec.workDir`, specified in the Settings dialog — type a path directly, or pick one through the native folder dialog with the adjacent "Browse…" button; typing stays available because a village opened on another machine needs a way to repair the path). An agent can receive prompt injection, so the range it can read is also the range it can leak. If unset, tools only explain that it is not configured and read nothing. The boundary is enforced through **prefix matching after canonicalization**, not string checks; symlinks are not followed (`..` checks alone cannot prevent an escape through a symlink).

Output is always bounded (100 matches, 240 characters per line, 12,000 characters total, and 2 MiB per file). Truncation is never silent: the result reports how many items were dropped. Hidden directories and conventional build outputs (`node_modules` / `target`, etc.) are not scanned.

**Every truncation also names what to do instead.** Reporting only the dropped amount leaves the model with one remaining option — repeating the identical call — which the repeat guard then blocks, taking the whole turn down with it ([failures.md](failures.md) #44, hit on a real run against a 61,891-character ledger). Where no argument can fetch the remainder, as with `file read`, the message must also state that **re-reading returns the same range**; naming an alternative alone leaves room for "maybe calling it again returns the rest."

### Narrowing What Is Presented (Token Cost Is the Differentiator)

**The schema of an unpresented tool is a fixed cost every turn**, and all agents spend from one person's wallet. Giving every capability to every agent is not a feature but waste. The default should be to give only the tools needed; that is this product's differentiator from large orchestration systems.

- The "Built-in Tools" checkboxes in agent settings select what each agent sees (`AgentSpec.enabledTools`). `null` follows the default (the default set is presented; new tools that join the default set appear automatically, and `run` sits outside it); an explicit selection presents only the needed tools and does not grow automatically.
- **If no work folder is configured, the six file tools are not presented regardless of selection**. Do not pay schema cost for tools that can only answer "not configured."
- **`rag` is outside this mechanism.** It has no checkbox; its presentation is decided solely by the "RAG folders" declaration (only a human can write the declaration, so the declaration itself is the opt-in — two switches for one intent create the trap of enabling one and wondering why nothing appears, which a real run hit on day one).
- Removing `remember` stops **only writing**. `Memory.md` still enters the prompt; to remove that injection, empty the file instead of duplicating control mechanisms whose effects cannot be distinguished.

Tools return **relative paths only**, so the agent is given the work folder's real path in its system prompt. Without it, models invent absolute paths for their explanations (a real instance described a nonexistent path as its work location). Missing decision material is filled by information, not prohibition.

### Changing Work Folders Together ([Spec 29](specs/29_batch-workdir.md))

When you point the whole village at another project, "Change work folders together"
in the **agent list footer** sets the work folder of every checked servant in one go.
**Running servants need not be stopped** — they pick it up from their next message.

- The list shows each servant's **current value** before the change, so you never
  overwrite without knowing what each one was pointing at
- **Whether the folder exists is not checked here.** As with the single-agent
  setting, the enclosure is not a check at save time but the boundary applied
  **when a tool runs**. A nonexistent path saves fine, and the tool says so by name
  when it is used
- If one servant fails the rest continue, and **the result names each one**
  ("7 changed / 1 failed (agent_5: …)")
- It doubles as **the first thing you do with a village someone shared with you**:
  work folders are absolute paths, so a shared village points everyone at paths
  that do not exist on your machine — this fixes all of them at once
- **Recently used folders** appear as clickable entries (up to 8). A path is
  remembered only when **an apply succeeded for at least one servant** — typing
  it or picking it in the browser does not. **Folders currently in use are not
  mixed in**: those are already listed just above, so the history covers
  "nobody points there now, but I will go back"

### Safety Boundary for Editing Tools (`sd` / `yq`)

Writing broadens the damage class from disclosure to **tampering**, so it has a stricter contract than read tools ([Spec 01](specs/01_sd-yq-write-tools.md) / `write_tools_contract` in `data_contract.yaml`).

- **Two-stage execution**: preview is the default; it returns a diff of the result and **does not write**. `apply: true` performs the write and must still return the applied diff. **No path writes silently**; every change remains in the conversation log as a diff.
- Writes whose diff exceeds 12,000 characters are **rejected**, not truncated. A truncated diff breaks the contract of knowing what changed.
- **Writes touch one file per invocation**; no creating new files; identical text or values are not written.
- **`sd` accepts `paths` to preview diffs across several files at once** ([Spec 17](specs/17_batch-sd-preview.md), up to 20). This is **preview only — writes still go one file at a time**. What "one file per invocation" protects is the blast radius of a single injection, so widening a read-only preview does not weaken it. Editing ten files drops from ten previews plus ten applies to **one preview plus ten applies**. When the output limit is reached, **no diff is ever cut mid-way**; the batch simply shows fewer files and reports how many matches remain.
- `yq` supports TOML and JSON only. It preserves comments, key ordering, and formatting while changing only values (including TOML end-of-line comments). `set` accepts scalars only, and rejects type destruction such as setting a table, setting a datetime, or automatically creating intermediate keys. **YAML is unsupported**: the candidate yaml-edit library was rejected in a PoC because it lost comments, types, and structure (see Spec 01 Phase 4).

### File and Folder Operations (`file`)

While `sd` / `yq` **partially edit existing files**, `file` operates on **the existence of files and folders** ([Spec 09](specs/09_file-tool.md)). It is one tool with a closed `op` enum: `read` / `write` / `mkdir` / `move` / `copy` / `remove`. Separate tools per operation would add a schema cost for each operation every turn and scatter the model's choices.

- **Start long artifacts with `write`, then extend them with `append`.** Output tokens per response are limited, and `write` carries the **entire file** every time. `read` -> `write` initially seemed sufficient, but measurement showed the expression is impossible: splitting 800 lines into eight 100-line stages still requires the final `write` to emit all 800 lines in one response, so the ceiling does not move (and total output becomes 4.5 times larger). It is not merely expensive; the split cannot work. That is the basis for `append` ([failures.md](failures.md) #40 / Spec 09 Notes 3).
- **Only this tool can create new files.** The trigger was a real agent spinning its wheels: asked to translate, it had no creation method and repeatedly attempted `sd` ([failures.md](failures.md) #39). Along with adding the capability, `sd` / `yq` report "editing only; cannot create a file. Use `file` `write` to create one" when a file is not found.
- **Overwrite has an explicit gate**: `write` to an existing path is rejected unless `overwrite: true` is supplied, and it explains both partial editing (`sd` / `yq`) and overwriting. There is no preview: creation has nothing to damage, and this gate serves the same role for overwrites.
- **Removal moves items to the trash only** (the `trash` crate). There is no permanent-deletion path. If trash is unavailable, it **returns failure** rather than silently falling back to permanent deletion. The work folder itself cannot be removed.
- The boundary is the same enclosure as read tools, but the destination of a new file does not exist, so it cannot be canonicalized. Equivalent strength is maintained in three stages: prefix matching the deepest existing ancestor, rejecting `..` in the remaining components, and not following symlinks (`resolve_creatable`). `move` / `copy` check both ends.

### Prerequisite for Convergence: History

Convergence needs a condition before termination. Agents retain the latest `history_turns` (default 8) exchanges and see their own statements as `assistant`. Without history, every turn is a cold start and the same input produces the same output forever, making convergence impossible in principle ([failures.md](failures.md) #12).

**History is scoped to the conversation** (Spec 12). Close the app and reopen it and the previous conversation's history comes back, which is why the first request after a restart is answered in light of what was said before. Starting or stopping an agent does not clear it. To start over, press "New chat" — that is the one operation that marks a break between conversations.

Cycles in the topology itself are **allowed**. Agents going back and forth is this system's purpose; the two layers above are the proper place to stop it.

---

## LLM Wire Layer

The architecture separates canonical and wire types with adapters. The orchestrator composes only canonical types, while adapters hold all protocol dialect differences. Adding a provider is confined to one adapter file.

Production pitfalls are listed in `llm_wire.invariants` in `data_contract.yaml`. The most consequential are:

- `temperature` is an `Option`. **Omit the key entirely when unset**; newer models do not support it and return 400 when it is sent.
- OpenAI-family `tool_calls[].function.arguments` is a **JSON string**. Parse it exactly once at the decode boundary.
- Every response field has `#[serde(default)]`. Servers claiming compatibility vary widely in practice.
- Retry as an "empty inference response" only when the body is empty, `tool_calls` is empty, and `finish == length`.
- Preserve the **raw** data on parse failure; it is the material for returning the rejection reason and requesting regeneration.

### Prompt Cache

On the native Anthropic path, `cache_control` marks **two** boundaries: the end of **tool definitions + the stable part of the system prompt**, and the end of the **conversation history**. In a multi-agent system that sends the same system prompt across agents and turns, this difference maps directly to operating cost.

The decision uses **estimated token count, not character count**, and **includes tool definitions**. Initially it considered only the stable system prompt at 4,000 characters, causing all five Japanese-configured agents to miss the threshold and making the cache never work ([failures.md](failures.md) #33).

The history boundary was added later. Before it, **the tool loop resent the entire history and every tool result at full price on each round** — 1,826,109 of one turn's 2,052,314 input tokens ([failures.md](failures.md) #42). A stable prefix is by definition the small part that does not change, so guarding only that caps the savings at the small part. **Cost is dominated by the part that grows**, and that side needs a boundary too. The wrap-up call still writes rather than reads, because changing `tool_choice` drops the history-layer cache.

**The TTL is one hour.** The default five minutes suits chat speed, but expires on every turn when users read and think before responding. Since writes cost more than ten times reads, **a longer TTL is beneficial whenever it removes one unnecessary write**.

Its effect is shown in the UI: the card's "N% of input cached" is the metric, and **0% uses a warning color**. Input is the denominator because output cannot be cached in principle; using total tokens would make 100% unattainable.

> Do not put state-varying content in the stable portion. Both destination running state and the set of presented tools split the cache the moment they change. **Presentation stays static; state stays dynamic.**

**And keep the system slot for stable content only.** Adapters lift every system-role message out of the array — **wherever it sits** — and concatenate them into one system prompt. Anything that changes per turn (retrieved references, the room log, presence notices) therefore occupies the head of the prefix regardless of ordering, so moving it later in the array changes nothing. It has to stop being a system message and travel with the current turn instead.

This was a real hole: Gemini agents ran at 0% for days ([failures.md](failures.md) #45). It broke the moment another servant spoke, which means it failed precisely when the app was used as a village. Every provider receives the same assembled messages, so the Anthropic path falls over the same way once the conditions match.

---

The providers are: OpenAI-compatible, Anthropic native, **Gemini native**, and
**xAI native (Responses)** ([Spec 31](specs/31_grok-live-search.md)).

Gemini also works through its OpenAI-compatible endpoint, which is sufficient for function calling alone. The native path (`/models/{model}:generateContent` + `x-goog-api-key`) is needed only for **Google Search grounding**, because the compatibility layer rejects `google_search` with `400 Invalid tool type`.

It is intentional that `generativelanguage.googleapis.com` is **not automatically classified as Gemini**. Existing templates use that base URL through the compatibility path; changing the classification would silently move agents whose settings were untouched onto a different wire protocol.

### Google Search Grounding

Checking "Google Grounding" in a model template makes that model verify with search before replying. It **can be used with function calls**, so enabling grounding does not disable delegation through `transfer_to_*` or built-in tools.

> **The referenced URLs are not returned to us.** Responses contain search queries and a link to Google Search, but not the URLs of articles the model read. Without stating this fact, models produce **strings shaped like citations** when asked for sources. In production, this appeared in the especially confusing form of a mix of real URLs and 404s ([failures.md](failures.md) #31).
>
> Therefore, the system prompt for a grounded agent automatically says: "URLs do not reach you; you can instead state search terms and the publishing organization." The key is a **notice of missing information rather than a prohibition**. Saying only "do not write URLs" collapses when the user asks for "URLs of sources." This is the same remedy as disclosing the real work-folder path: fill missing decision material with information.

For utterances where grounding occurred, **search terms and sources are shown outside the speech bubble**. They are facts observed by this application, not the model's statement, so they do not share the body area. When zero sources are returned, the field remains and says "No sources were returned." **Emptiness itself establishes that no sources are available**; silently collapsing it would make users believe URLs in the text are sources.

Provenance flows **only to the presentation layer**. It never returns to the model. Grounding occurs in the current turn and sources arrive together with its answer, so putting them in the next prompt would only describe sources for the previous topic. Presenting a prior turn's URL as evidence for the current turn is a new form of misattribution, merely replacing fabrication with a different error ([Spec 05](specs/05_gemini-native-and-grounding.md) Notes 9).

**Observed result (2026-07-29, Jamie / `gemini-3.6-flash`)**: when asked to research a television program currently on air, two `queries` arrived and **`sources` was empty**. No source URLs were returned. The model did not invent URLs and named publishers instead (Nippon TV official site / Wikipedia / TVer), so the notice worked as intended. This is one observation, so it does not establish that URLs are impossible in principle; **the design simply does not assume they are available**.

Schemas for built-in and MCP tools are filtered by adapters to retain **only keys Gemini accepts**. Gemini's `parameters` are a subset of OpenAPI 3.0, not JSON Schema; sending `$schema` or `additionalProperties` returns 400. This is an allowlist, not a denylist, because **MCP tool schemas are written by connected servers and cannot be constrained internally**.

---

### Grok Live Search ([Spec 31](specs/31_grok-live-search.md))

Set the model template's **protocol to "xAI native (Responses)"** and the
**Live Search (web)** and **Live Search (X)** switches appear under
"Vendor-specific skills". A model with either checked searches before it answers.

**Using a dedicated wire at `/v1/responses` is a constraint, not a preference.**
The older `search_parameters` on `/v1/chat/completions` now returns **HTTP 410
Gone**, with the server itself naming its successor API (measured 2026-08-09).
Function calling alone works fine on the OpenAI-compatible endpoint, so existing
Grok agents **keep running on the compatible path until you switch the protocol
explicitly** — it is never auto-detected, so a village you did not touch never
silently moves to another wire (the same discipline as Gemini).

**web and X are separate switches.** They are separate tools with separate
billing counters and separate response item types; folding them into one would
mean a village that only wants web search also opens the X surface.

> **Sources come back the opposite way from Google Search.** Gemini returns no
> source URLs; Grok **does** — for X search they are the post URLs themselves,
> so a human can check whether they exist.
>
> But what comes back is the fact that **something was posted, not that it is
> true**. Search result text is injected into the prompt on xAI's side, so
> **the village's tool layer never sees it** — and since anyone can post on X,
> this is also a path for an attacker's text to enter a prompt. The remedy is
> operational rather than mechanical: **do not let the agent that fetched
> something judge whether it is true** (the ordinance's verification step
> applies unchanged).

Provenance uses the same container as Google Search, and the **engine name is
read from the record** (a fixed string would make anything grounded by another
engine claim to be "Google Search"). X posts carry a post ID in the URL, so
**sources flow horizontally rather than one per line, distinguished by the X
mark and the tail of the ID** — one search returns dozens (45 and 77 in the
field), and stacking them vertically fills the pane with provenance alone.
**The count comes first, and nothing is truncated.**

**A turn that searches costs an order of magnitude more input tokens**, because
results are injected into the prompt: 98,213 in one measured turn, of which
62,720 were cached. In a village with a small token limit
([Spec 11](specs/11_token-budget.md)) one search can exhaust it, so the measured
figure sits next to the checkbox.

**The thinking summary is received and shown** ([Spec 33](specs/33_thinking-summary.md) /
[Spec 34](specs/34_openai-responses.md)).
It sits below the bubble in the chat pane, collapsed, in a **frame separate from
the grounding record** — sources are verifiable external pointers while a summary
is an unverifiable internal claim, so putting them in one frame would lend the
latter the credibility of the former. It is **collapsed by default** (measurements
reach 3,700 characters, and it comes back in English). Turns with no summary show
no frame at all.

**Three of the four providers return nothing unless asked.** Only xAI returns it by
default; Anthropic needs `thinking.display: "summarized"`, Gemini needs
`thinkingConfig.includeThoughts`, and **OpenAI needs `reasoning.summary: "detailed"`** —
`auto` returns not a single character (measured: 0 vs 522). **Thinking happens and is
billed either way**, so
these fields change only how it is returned. **Anthropic is asked only on
5-generation models** (older ones reject it with a 400, and they do not think by default).

**Fidelity differs by an order of magnitude between providers** (measured): xAI and
Anthropic return under 10% of their thinking, while **Gemini returns 75–85%**. The
label is "thinking summary" everywhere (an understatement for Gemini, but it errs
in the safe direction — it never claims more than what came back).

> **"Fidelity" may not be a meaningful reading for OpenAI.** One measurement showed
> 131 thinking tokens against a 1,308-character summary (over 300 tokens of English).
> A summary extracted from thinking cannot exceed it, so **it may be generated
> separately**. This is a single observation and is not being treated as settled.

**The token count spent on thinking is also measured**
([Spec 32](specs/32_thinking-reception.md)) — via `reasoning=` on the `turn`
line and a field on `Usage`. It is available for **all four providers: Gemini,
xAI, OpenAI-compatible, and Anthropic**.

**A 0 from Anthropic does not mean "thinking was never switched on".**
claude-sonnet-5 **thinks by default, with nothing requested**. In one
measurement it spent **all 2,048 output tokens on thinking and returned not a
single character of text** — that is what a turn that costs money and comes
back empty actually is, and the amount is now readable as a number.

The reason for measuring it: **most of what was paid for never reached the
screen.** In one measurement (`grok-4.5`), 1,494 of 1,497 output tokens went to
thinking and **the visible answer was four characters long**.

Receiving the thinking **text** split into two further specs — receiving and
displaying the summary (Spec 33), and the OpenAI Responses wire (Spec 34).
**Both have landed.**

---

### The OpenAI Responses Wire ([Spec 34](specs/34_openai-responses.md))

Setting a model template's **protocol to "OpenAI native (Responses)"** adds
**web search** and **Pro reasoning mode** under "provider skills".

**Three things need this path, and none of them are reachable over
`/v1/chat/completions`.**

1. **The text of the thinking summary** — the compatible endpoint returns
   the count but never the body
2. **Web search** — `web_search` on general gpt-5 models is Responses-only
3. **Thinking itself** — the compatible endpoint refuses to combine function
   tools with reasoning, so this app **switched thinking off whenever tools were
   offered**, running a reasoning model with its reasoning killed. The error that
   refuses the combination names this wire as the way out.

The third one produced a matched pair on real hardware. **Same agent, 110 seconds
apart**: Responses at `rounds=5 reasoning=131` (thinking while calling tools six
times), compatible at `rounds=1 reasoning=0`.

**Switching costs two things.** Temperature is rejected by the provider, and image
attachments only travel over the compatible endpoint. **Both are stated in the
model registration screen** — dropping them silently reads as "I configured it and
it does nothing", which is worse than being refused (both fail, but only one is
legible).

**Existing gpt-\* templates change nothing until switched** (there is no
auto-detection). Left alone, nothing on screen would reveal that new capabilities
exist, so **a one-line hint sits directly under the protocol selector**. The hint
also looks at the model name, but **the wire is selected by provider alone** —
guidance and judgement are kept apart.

**Web search adds about 4,400 input tokens to every request, including ones that
never search** (the tool declaration is injected into the prompt); later requests
hit the cache, so the effective cost is roughly a tenth. **Pro reasoning mode
carries its own fixed cost of about 1,500 tokens per request**, against a
published benchmark gain of 23.3% → 28.5% for terra (about level with standard
sol at 28.7%). **Both the accuracy and the fixed cost are stated** — either one
alone is not enough to decide with.


### Where API Keys Live

API keys are stored in the **OS credential store** (Windows Credential Manager / macOS Keychain / freedesktop Secret Service). `ModelTemplate` holds only `credential` (the retrieval source); **no field exists that can contain a secret**. At the type level, there is no path for a secret to enter plaintext `world.json`.

```rust
pub enum CredentialSource {
    Unset,        // Not configured (default). Rejected before sending.
    NotRequired,  // User explicitly declared no authentication is required (local inference server).
    Keyring,      // OS credential store. The key is the template ID.
}
```

`Unset` and `NotRequired` are separate because **"not entered yet" and "not required" are different states**. Combining them makes a template missing a key appear unauthenticated, sends a request without an authentication header to the outside, and turns a local configuration error into a server-side 401. For the same reason, deleting a key returns to `Unset`, not `NotRequired`.

Secrets pass through the process only from `LlmConfig::from_template` to the HTTP headers. They never appear in configuration files, events, error messages, or IPC responses. The UI receives only whether one is registered; **there is no API to read its value**.

> This was rebuilt twice. Initially, `apiKeyEnv` was a `String`, and the only defense was a UI label and warning text. A real key was pasted on the first day of use and persisted in plaintext. The next design required an environment variable name, but that did not fit a desktop GUI: it required terminal work and restart, and on Windows configured variables do not propagate to an already-running process. **Warnings are not controls. It is not enough to make writing impossible; the design must provide a correct place for the user to put the value.**

## Operation

### First Launch

The app works even without an API key. `HttpBackendFactory::echo_on_failure` falls back to an echo response. Silencing it would make it impossible to distinguish a configuration error from an implementation defect.

**The fallback always identifies itself.** When it occurs, a `BackendDegraded` event shows a warning and the response body includes the reason, such as an unregistered key. Degraded backends are not cached, so correcting the cause restores normal operation immediately.

> When the implementation identified itself only as an "echo response," fake replies continued because settings had not reached the backend, and the cause could not be found. **Fallback is allowed, but silent fallback is not.**

To connect to an LLM, create a model template from the agent list header, paste a key in the `API Key` field, and press `Register`. **Everything is completed inside the app.** No terminal operation or restart is needed.

Changing `Protocol` updates the `base URL` default (`https://api.openai.com/v1` for OpenAI-compatible, `https://api.anthropic.com/v1` for Anthropic). A manually entered URL is never overwritten.

### Workspace

Agent settings reside in the OS application-data area.

> **Carrying over a village created before the rename (`Concordia` → `Fuseforks`)**
>
> `{app_data_dir}` is derived from the application identifier, so the rename changes
> where the app looks. **Rename the whole old folder and it opens as before.**
>
> | OS | Old → New |
> |---|---|
> | Windows | `%APPDATA%\jp.outcasts.concordia\` → `%APPDATA%\jp.outcasts.fuseforks\` |
> | macOS | `~/Library/Application Support/jp.outcasts.concordia/` → `.../jp.outcasts.fuseforks/` |
> | Linux | `~/.local/share/jp.outcasts.concordia/` → `.../jp.outcasts.fuseforks/` |
>
> **Do not move `workspace/` alone.** Schedule-probe approvals
> (`probe_approvals.json`) include `workspace/village_id` in their key, so approvals
> left outside the folder all have to be granted again.
>
> **API keys must be re-entered.** The credential-store service name changes too, so
> keys registered under the old name become invisible to the app (they are not
> deleted; remove them with the OS credential manager if unwanted).
>
> **Theme, pane widths, and work-folder history reset to defaults** (the display
> settings are stored under a renamed key). No village content lives there.

```text
{app_data_dir}/workspace/
  world.json                  Agent definitions, model templates, roles, and connection-map coordinates
  fuseforks.log               Diagnostic log (below; rotates one generation to fuseforks.log.old at 8 MB)
  schedules.json              Schedules (time-triggered requests; managed from "Schedule" in the title bar)
  village_id                  This village's identifier (Spec 28; a random value that binds pre-check approvals to this village)
  Ordinance.md                Village ordinance (rules shared by all agents; edit from "Ordinance" in the title bar)
  mcp.json                    Shared MCP server declaration (presented to every agent; edit from "MCP" in the title bar)
  sessions.redb               Conversation store (multiple conversations in one file; Spec 12)
  exports/{session_id}.jsonl  Conversation export destination (written by "Export" in the conversation list)
  attachments/{uuid}.{ext}    Attachment bodies (webp / mp3 / wav / mp4 / pdf; auto-deleted after 30 days / above 500 MB total)
  agents/{agent_id}/
    SKILL.md
    Memory.md
    Construct.md
    mcp.json                  Per-agent MCP (presented only to this agent; edit in the configuration-files tab)
    icon.webp                 Agent icon (only when configured; UI converts and stores it as WebP)
  user/
    icon.webp                 Your icon (only when configured; same handling as above, different location)
  external/
    icon.webp                 External client icon (only when configured; Spec 25; kept separate from yours)
```

**Per-machine settings are the only ones that live outside the workspace.**

```text
{app_data_dir}/mcp_server.json       MCP server enabled/disabled, port, and token (Spec 25)
{app_data_dir}/probe_approvals.json  Whether a schedule's pre-check may run on this machine (Spec 28)
```

What you hand over when sharing a village is the workspace, so **keeping these two outside
is what makes "sharing a village neither opens its door nor runs the commands it carried"
hold**. Only the chosen reception servant lives in `world.json` — who receives is
a per-village question, while enabled/port are per-machine ones.

**The approval file holds hashes only**, never the command text. Writing the text would turn
the file itself into a list of commands that may run on this machine.

### Application Icon

The executable and taskbar icons are the generated files under
`apps/gui-tauri/src-tauri/icons/`; the **source image is `fuseforks_icon.png` at
the repository root**. To regenerate, let the Tauri CLI do it (never export each
size by hand).

```bash
npx tauri icon <square PNG>
```

- **The input must be square.** `fuseforks_icon.png` is 1245×1272, so it was
  padded with transparency to 1272×1272 — **not cropped** (cropping would lose
  27px of artwork)
- The generated `icons/android/` and `icons/ios/` directories **can be deleted**:
  this app is desktop-only, and `bundle.icon` in `tauri.conf.json` references only
  the five desktop entries
- **Do not edit `tauri.conf.json`.** The generated filenames match the existing
  references

**The mark in the in-app title bar is separate** (an SVG written directly in
`TitleBar.vue`). Replacing the icon does not change anything inside the window —
the look of the executable and the look of the interface change for different
reasons, so they are deliberately not coupled.

### Diagnostic Log

Observation lines beginning with `[fuseforks]` go to both stderr and `workspace/fuseforks.log`. Stderr remains only in the terminal that launched `tauri dev`; **nobody could read it until a user pasted it**. Diagnosing a path that used 730,406 input tokens in one turn could not proceed until the log was copied manually (2026-07-31).

The two lines below were **emitted before the rename (2026-07-31)**, so their prefix
is still `[concordia]`. The format itself did not change across the rename.

```text
2026-07-31 04:34:12.481 [concordia] tool: agent=agent round=7 name=file ok=true args_chars=118 body_chars=8304
2026-07-31 04:35:02.117 [concordia] turn: agent=agent hop=0 rounds=19/36 waves=1 stop=- prompt=730406 cached=116334 total=748424
```

A `turn` line carries **two more fields than this example**: a trailing
`reasoning=` (tokens spent on thinking during that turn; Spec 32) and `backend=`
(which wire the turn went over; Spec 34). Older logs have neither.
`reasoning=` is **a share of `total=`**, so the total does not move at all.

`backend=` was added because **adding a wire could not be confirmed on real
hardware**. Switching the protocol changed **not one line of the log** unless a
provider-specific feature such as search was used. An instrument tied to one wire
is only evidence about **the feature**, never about the path.

There are eight line kinds: `turn start` / `turn` (turn start and aggregate; `stop=` records the exit: `-` / `tool_limit` / `repeat:<tool>` / **`failed:<CODE>`**), `turn failed` (the reason a turn died), `tool` (measurement per tool invocation), `tool blocked` (repeat cutoff), `plan wave` / `plan bundle` (wave delivery and convergence), and `schedule` (schedule firing).

The start and failure kinds were added later. The `turn` line existed **only on the success path**, so a turn that died left no line at all; and rounds that call no tool (waiting on the LLM) emit nothing. **"Still in flight" and "died three minutes ago" looked like the same silence.**

**The `turn` line is now emitted for failed turns too** (2026-08-16). A turn whose output hit the token limit with an empty body had been paid for at the provider, yet had no `turn` line and showed up in neither the card totals nor the budget (`failures.md` #103). Now a failed turn still writes one `turn` line with `stop=failed:<CODE>` **in the same columns as a successful one**, and what it paid lands in the card and the budget. `turn failed` stays as the line that carries the reason text — **count `turn` lines only** (a failed turn now produces two lines). Each of the four turn exits writes exactly one usage line: `turn interrupted` for interrupts and `turn budget exhausted` for budget cut-offs. **All four lines end with `model=`** (the template's model name; [Spec 39](specs/39_stats-view.md) — the prefix and the existing columns are unchanged, so earlier greps keep working).

**The same numbers are also kept in `sessions.redb` as `turn` records** (Spec 39; one record per turn, at all four exits). That is what the stats view (the bar chart icon in the bottom bar) reads: `fuseforks.log` rotates at 8 MB with one generation, while the records stay with the conversation. **Conversations from before this version have none** — a conversation without records shows "no records" rather than zeros.

**Records written up to `v0.1.9` carry no `cacheWrite` / `cacheWrite1h`** (added in Spec 40 P3). They read back as 0, but **that 0 means "never recorded", not "nothing was written"** — the screen folds the difference into the "fresh" bucket rather than claiming a breakdown it does not have.

The primary purpose of a `tool` line is **`body_chars`**. Tool results are added to history and resent in every subsequent round, so **the size of one tool result affects input tokens for every round of that turn**. `rounds` and `prompt` in a `turn` line alone could not identify what made the prompt large.

Only these observation lines are logged; **prompt bodies, tool-result bodies, and credentials are not**. Logs are plaintext, readable by anyone opening the workspace, and are treated like `world.json`. This must not break the boundary that secrets belong only in the OS credential store ([failures.md](failures.md) #1).

**The status bar at the bottom of the screen prints the time in this same prefix format** (`2026-08-03 22:15:03`). This is a management tool, so a screenshot needs to carry **which moment it shows**; matching the format means **you can find the corresponding log line by eye from the time in the captured screen** (only the milliseconds are extra on the log side).

This clock alone **does not follow the language setting**. English locale formatting would render `8/3/2026, 10:15:03 PM`, which at once (1) reorders month and day depending on the reader's country, (2) requires reading AM/PM, and (3) no longer lines up with the log. **This is not a missed translation but a deliberate fix for correlation.**

### Per-Agent MCP

`agents/{id}/mcp.json` can declare MCP servers **dedicated** to that agent (the same Claude Desktop `mcpServers` format as the shared file). The motivation is giving every agent a different memory database: even with the same server, distributing one connection target to everyone is wrong when destinations differ.

- **Overridable additive model**: presented tools are "shared + this agent's own." When `{server}__{tool}` has the same name in both, **the per-agent definition wins**. This is the legitimate means of replacing a shared server with an agent-specific target.
- **Connection lifetime matches agent operation**: connect on start and disconnect on stop. Do not keep child processes for stopped agents. Processes are not shared even for the same command, because differing connection targets are the entire motivation.
- Broken JSON is rejected when saving. External edits that break it do not stop startup; the error is readable from the Settings dialog's "Per-Agent MCP" field. Connection state is not persisted, because a state file would falsely retain "connected" after a process exits.
- Per-agent tools cannot be called by other agents, even if they know the name.

### Village Ordinance

Rules stack in three layers: **vendor constitution (model-side, immutable) > village ordinance > individual agent settings**. The ordinance enters the top of every agent's system prompt and applies from the next utterance after saving. Since everyone receives the same document as the rules of the place, it is also a normalization layer that aligns behavior differences between models.

The folder button in the Settings dialog — opened from an agent card's settings button — opens that agent's configuration folder directly.

#### The Village Blackboard — shared working notes

The other tab at the center-bottom is the **Blackboard**. It is literally the `blackboard/` folder inside the shared work folder, written by the agents (with the `file` tool) and by you. The way it is used lives in the village ordinance: one file per agent (`blackboard/<display name>.md`) as a sticky note, and only the coordinator bundles them into `blackboard/まとめ.md`. **There is no path for the GUI to write content**, and nothing is auto-injected into prompts — agents read it when they decide to. No new mechanism was added: rules plus the existing file tool are the whole implementation.

**Deleting can be done from the screen** (the eraser left of the refresh button clears everything; the bin on each note removes one). **The "never write content" line has not moved** — deleting is not writing content under someone's name, and it is **cleanup only a person can do**: notes left by a servant whose work folder has moved sit **somewhere that servant can no longer reach** (the tool's fence does not extend there), so without an exit on screen the only way out is deleting files by hand.

**Notes go to the OS trash; there is no permanent-delete path.** That is why **only the bulk action asks for confirmation and single deletions do not** — piling confirmations onto reversible actions makes the confirmations on irreversible ones read as noise. The bulk confirmation **states the count**, because "everything" alone does not say how much.

### Roles

A **template for creating servants** and a **label visible in the village**
([Spec 14](specs/14_role-label.md)). Managed from "Roles" in the title bar.

Pick a role when creating a servant and it starts with its settings filled in.
Four things are applied: the `Construct.md` body, the model template,
built-in tools, and the tool-call limit. **Connections, the work folder, and
RAG folders are deliberately excluded.** Letting a template draw lines would
break "**lines are drawn by people**," and the work folder and RAG folders are
absolute paths that differ per machine, so sharing a village would ship broken
references (RAG sources were originally on the "applied" side; they moved here
when [Spec 18](specs/18_bundled-doc-index.md) changed their meaning to absolute
paths).

- **Settings are copied; the role name is referenced.** This asymmetry is the core
  of the design. Editing a role later **does not change servants that already
  exist** (we do not create a second layer with the same nature as the ordinance),
  while **renaming a role updates every display**
- **Applying happens only at creation.** You can attach a role to an existing
  servant, but that changes **only the nameplate** — not a single setting is
  overwritten, because overwriting cannot be undone
- **Deleting a role does not break servants.** Their settings were copied, so
  behavior is unchanged; only the badge and the roster label disappear. This is
  the opposite of model templates, which refuse deletion while referenced — and
  it is exactly what the copy approach buys
- **Badge colors come from a closed set of 8.** Lightness and chroma are fixed and
  only the hue varies, so an unreadable color cannot be chosen. Changing a color
  reaches every servant holding that role immediately

**A badge states provenance, not current content.** `Construct.md` and the tools
can be edited after creation, so a servant badged "Researcher" whose insides are
something else can arise through legitimate operations. The badge answers only
"**which template was this made from**" and promises nothing beyond that.

**A servant does not know its own role name. This is by design.** The roster
carries the roles of **others** only; a servant's own role appears nowhere in its
prompt. Persona is carried by the prose in `Construct.md` / `SKILL.md`, and
injecting a role name there **pulls the model toward that word's connotations**
(write "Deputy" and it starts behaving deferentially). A role is a label for
people and for other servants, not material for self-identity.

### Scheduling

From "Schedule" in the title bar, you can register **requests that fire at specified times** ([Spec 07](specs/07_scheduled-tasks.md)). The three forms are "every Thursday at 17:00," "daily at 09:00," and "every 10 minutes"; cron expressions are intentionally not used, since they are unreadable to anyone unfamiliar with them and leave the UI as a free-text input. When fired, the request reaches that agent and the result appears in the conversation pane. The body begins with `[Scheduled: Every Thursday 17:00]`, distinguishing it from human utterances in both the UI and model context.

**Limits (the UI says the same):**

- **Schedules do not run while the app is not open.** This is a desktop app, not a daemon. Missed schedules are not replayed; ringing a 17:00 bell at 23:00 is worse than not ringing it.
- **Interval schedules (every n minutes) run once after resumption.** They do not fan out once for each missed interval.
- Schedules whose destination is stopped are skipped rather than delivered, with one line left in the conversation log.

**Known limitation**: duplicate firing is not completely prevented. A light guard avoids adding work while the previous firing is still running for that agent, but releases the guard when an event is lost. Leaving it closed would silently stop the schedule forever, which is worse than rare duplicate firing. Inbox capacity (64) is the final backpressure.

Multiple instances are mutually exclusive: launching a second Fuseforks brings the existing window to the front and exits the second process. This structurally closes the path where two processes fire the same schedule at once, so it is implemented **before the scheduling mechanism itself**.

#### Checking with a command before asking (pre-check)

For monitoring, "nothing changed" is the common case, yet having a servant do the checking spends tokens every time. At five-minute intervals that is 288 runs a day; if something changes once, **287 of them were wasted**.

A schedule can therefore carry a **pre-check** ([Spec 28](specs/28_schedule-probe.md)). When the time comes, a command runs first, and the request reaches the servant **only if the first line of its output equals the signal you configured**. The decision runs one process and **never calls a model**, so runs that do not match cost no tokens at all.

- **Put the signal on the first line. Any following lines are attached to the request** — what changed arrives as material for the decision, removing the round trip a servant would otherwise spend fetching the same information.
- **The exit code is not used for the decision.** Monitoring scripts normally signal trouble with `exit 1`; deciding on the exit code would make the most natural form never fire.
- Commands **do not go through a shell** (executable plus an argument array). The argument field takes one argument per line.
- Runs that do not match, and runs that fail, **do not appear in the conversation log** (a System line every five minutes would bury real notifications). The latest result is shown in the schedule list.

**Commands that arrive with a shared village never run until you approve them.** Schedules are village content and travel with it, but **approvals are stored on this machine only** (outside the village). A pre-check you wrote yourself in the UI is approved as you save it; anything else is listed as "not approved yet" so you can read the command before approving it.

**This closes the path where a shared village runs commands silently** — it cannot vouch for what an approved command then does (the same property as the `run` allow list).

#### Before and after a firing (new conversation / summary)

Keeping one conversation for a long-running monitor makes the log sent each time grow. Two options are available per schedule:

- **Start a new conversation each time** — the whole village switches to a new conversation. The previous one is kept and can be reopened from the conversation list.
- **Summarise memories when finished** — summarises only the servants involved in that request, never charging the schedule for uninvolved ones.

**The two run in opposite directions, so do not read them as one.** A new conversation *discards* history (keeping it in another conversation and switching away). A summary *stays*, and unlike the sliding window it never falls off — so it is a fixed cost carried by every later turn.

### System Settings

Opened from "System Settings" in the title bar ([Spec 13](specs/13_settings-dialog.md)). Two panes —
a left menu and a right page — where **the left menu is itself the catalog of what can be configured**.
The aim is not to add settings, but to **surface settings that already existed with no place to touch them**.
Three things have since been added into this frame: the theme, your own name and icon, and the MCP server.

| Page | Content |
|---|---|
| General | **User** (your own name and icon) and language (Japanese / English). The language is inferred from the OS on first launch only; never re-inferred afterwards |
| Cost Management | Token limit (the ceiling described under "Token Budget" above). "Limited (value)" or "Unlimited" |
| Integration | **MCP server** (see "Accepting requests from external LLMs" below). Disabled by default |
| User Interface | **Theme** (Dark / Light) and message visibility. The latter now has three: the confirmation for **cutting a tie**, the confirmation **before closing**, and whether **join and leave notices** appear in the chat pane |

- **Your name is both the display name on screen and the name servants read**
  ([Spec 19](specs/19_user-identity.md)). Leave it unset and the screen says "You" while
  servants receive your messages as coming from "ユーザー". Set it and
  **both become the same name and stop following the interface language** — if the same
  person carried a different name on screen and in front of a servant, the conversation
  would stop lining up. **Up to 32 characters**; the characters 【 】 ［ ］ and line breaks
  are rejected, because those delimit the sender in the message a servant receives,
  and one utterance containing it would read as two. **Changing your name does not rewrite
  the old name already recorded in past conversations** — records stay as they were written.
- **Your name and icon are stored in the village**, so they travel with it when shared.
  Whoever receives it can correct the name from this page.
- **The theme switches the moment you pick one** (there is no Save button — appearance is
  chosen by looking at the result, so a setting that waits for a click cannot be chosen).
  **Until you choose, it follows the OS setting and is not persisted.** This is the opposite
  discipline from language, which is inferred once and never re-inferred, and the reason is
  where each lives: **language sits in `world.json` and is burned into the conversation log as
  System lines**, so a later change of interpretation would contradict what was already saved.
  The theme is device-local appearance only, so following the OS until an explicit choice
  harms no one.
- All colours live in one place in `style.css`. Dark is the `@theme` block (the side that
  generates utilities); light overrides the variables from `:root[data-theme="light"]`.
  **Role badges and avatars take only lightness and chroma from the theme** — a badge is used
  as *text* colour, so a light hue is unreadable on a light ground. **Adding a token to only
  one theme leaves exactly the places that use it in the other theme's colour**, so
  `theme.test.ts` checks mechanically that both themes define the same set.
- **There are two storage locations.** Language and token limit are **stored in the village**
  (`world.json`), so they travel with it when shared. Screen settings (theme, confirmations)
  are **stored on the device**, so opening the same village on another PC gives different
  values. Each page states which one applies
- **Unimplemented pages are not listed in the left menu.** Listing something you cannot touch
  would be a lie that shows the impossible as possible
- **Orchestrator internals are not exposed** (history depth, hop limit, public-square-log window, and
  others). This is the substance of "do not add settings"; the line between exposed and hidden is
  frozen as `settings_contract` in `data_contract.yaml`
- **Cutting a tie in Kizuna was the only destructive action with no confirmation** (the other six
  always had one). On by default: cutting a line once is recoverable, but losing one without knowing
  the setting exists is not

Localization covers all three layers ([Spec 35](specs/35_bilingual-core.md) landed the third).
**UI text** and **error text returned by the core** are translated through dictionaries; **what the
core says to the agents (the system-prompt framework, tool descriptions, the sender envelope) exists
in both Japanese and English, and the village's language setting picks which one is sent**. This is
an addition, not a replacement — a Japanese village does not change by a single byte (the discipline
of never altering the bundling text, [Spec 08](specs/08_plan-wave-pane.md), stays intact). The aim
is steering language pull: models tend to reply in whichever language dominates their input, so an
English village gets an English-speaking core. **Village content (the ordinance, Construct, SKILL)
is the user's asset** and the core leaves it alone — an English UI with a Japanese SKILL
occasionally producing Japanese in replies is expected. System lines in the conversation log are
written in **the village's language at the moment of recording** and never retranslated
(retranslating would put the exported JSONL and the screen out of sync; a village that switched
languages keeps its old lines in the old language — an honest record, like the user's own past
messages).

### Accepting requests from external LLMs ([Spec 25](specs/25_mcp-server.md))

An MCP client such as Claude Code can **send a single request to this village and receive
a consolidated answer**. A reception servant takes it, distributes and verifies it across
other servants when that helps, and returns one answer.

**This is a complement, not a general-purpose API.** For a single-shot inference the caller
is faster and more accurate answering it themselves; this village earns its keep only on
questions that need multiple viewpoints, divided investigation, and mutual verification.
So **exactly one door opens** (the tool `ask_fuseforks`, whose only argument is the request
text), and neither the village roster nor its settings are visible from outside.

**Disabled by default.** In a village that enables it:

- **It listens on `127.0.0.1` only** (other machines cannot connect)
- **A token is required.** It is generated the moment you enable the server and shown in
  System Settings. Unlike an API key it appears on screen, because its purpose is to be
  pasted into a client's configuration
- **Only one request is processed at a time.** A second one is refused on the spot rather
  than queued. This is not politeness but a **brake**: it cuts both the infinite chain you
  could build by registering the village in its own `mcp.json`, and the deadlock of a
  reception servant waiting on itself
- **While open it shows in the left of the status bar.** Leaving that state visible only
  inside a settings dialog is how a door stays open with nobody around
- **The door's settings (enabled, port, token) are stored outside the village.**
  So **sharing a village does not open its door** — what travels is the workspace only.
  The reception servant is the one part stored in the village (who receives is a
  per-village question)

**A request from outside is treated as distinct from your own message.** The conversation
pane marks it "external", and what reaches a servant reads as a request from an external
client. **Whether the other party is human changes how to answer**, so an outside tool is
never disguised as you — knowing the caller needs no simplification and will not ask
follow-up questions lets a servant adjust accordingly.

**This is a signal, not enforcement** ([Spec 26](specs/26_sender-envelope-integrity.md)).
The sender travels as a single line placed at the **start** of the message a servant
receives, and whether a model reads that line and changes its behaviour is up to the model.
In practice both happened: one servant correctly judged the caller non-human, while another
**quoted that very line verbatim and stated it does not use it for that judgement**. The
signal guarantees that the information arrives — nothing beyond that.

**What is enforced instead: a message body cannot claim to be the sender.** Write the
sender syntax into a request body and it arrives in a form that shows it came from the body
(`【送り手: ユーザー】` becomes `【送り手（本文）: ユーザー】`). **A pasted conversation
log still reads normally** — rejecting it would make quoting impossible, and silently
deleting it would corrupt the log. This is the half that does not depend on a model
following a rule, added after measuring that the signal above did not always land.

**The external client's name and icon are configurable.** Leave them unset and the name the
client declares for itself (`Claude Code`, say) is used as-is. Set them and both the screen
and the name reaching servants become your value, which means **the client's self-declared
string no longer reaches the prompt** — that string is a value a caller can write freely,
so configuring a name closes that path. **A caller can write one other thing: the request
body itself** — that path is closed by the paragraph above. The icon is kept separate from your own:
sharing one face would make an outside tool's request look like your own.

---

## Intentionally Unimplemented

Places where only connection points exist as scaffolding, without implementation. **Nothing pretends to work**, so each is ready to become the next work item.

### Deferred: User Broadcasts (Sending to Multiple Agents at Once)

This was implemented once and **removed from the UI** ([failures.md](failures.md) #25). Broadcast runs every turn in parallel, so no agent sees others' responses before replying. An agent attempting coordination cannot know that another has already answered, and the same participant answers twice. Each prompt-based fix opened another hole beside it: the cause was not a missing implementation but the temporal asymmetry that makes parallel execution incompatible with decisions premised on others' responses.

**The core mechanisms remain** (`send_user_message_broadcast` / `co_recipients` / broadcast annotations / display aggregation). Agent-initiated fan-out still uses them, and removing them would break that path. The constraint exists only at the UI layer.

It can return under either condition:

1. **A control method that works in parallel is found.** For example: serialize turns, notify recipients who have already replied through an envelope, or give coordination tools only to a facilitator (enforce it structurally).
2. **Expose it under a name for a different use.** "Send the same question independently to everyone and compare answers" (model comparison) is valid and needs no coordination. If restored, it must be a recognizable feature rather than a side effect of destination selection.

| Area | Current state | Next step |
|---|---|---|
| Per-agent MCP test connection | A stopped agent's MCP only shows "not connected" because connections are tied to operation. | At save time, connect once for testing and show only the result (Spec 02 Notes) |
| Long-term memory (Memoria connection) | `Memory.md` (`remember`) works. Multi-layer memory is unconnected, but **the door is open**. | Start according to the policy below |
| Bells and desktop notifications (Spec 07 P4) | Schedule execution is **implemented** (the Scheduling section above), but its result can only reach the conversation pane. The requested "ring a bell" cannot complete without a tool to make sound. | Task-tray indicator and toast notifications (user decision, 2026-07-30). **Implement separately as one UI task**, because it belongs to the same layer as tray residency. |
| Variable store (Airflow Variables/XCom equivalent, user request, 2026-07-30) | Two alternatives are working: `Memory.md` (prose, per agent, persistent) and bundle text (handoff between waves). | Structured values keyed by name. This conflicts with the "few settings" concept, so review should make that tension its main battleground when specified. |
| Conversation persistence and session management (user requests, 2026-07-30 / 2026-08-02) | **Implemented** ([Spec 12](specs/12_session-persistence.md) is done). Close and reopen the app and the previous conversation comes back, with agents answering in light of what was said before. "Conversations" in the chat pane header lists saved conversations: reopen, fork (go back to just before a request, with its text returned to the input box), export, delete. "New chat" now opens a new conversation instead of discarding the current one. "Summarize and continue" shortens subsequent prompts for running servants (manual only — it costs tokens, so it never runs on its own). **Conversations are stored on disk in plain text.** | Store everything in a single `{workspace}/sessions.redb`, keep several conversations, and switch between them. **The original hypothesis "histories are candidates to die" was refuted**: this village has two layers of history (the village conversation log and each agent's own prompt history), and **neither can be reconstructed from the other**. Persisting only the conversation log yields a screen that looks correct while every agent starts amnesiac. |

### Long-Term Memory Policy (Decided, Awaiting Connection)

Short-term memory (conversation context) and long-term memory are **separate layers**. They are not mixed.

| Layer | Lifetime | Purpose | Current state |
|---|---|---|---|
| Conversation context | One session through New Chat | Retain the immediately preceding exchange | Implemented (`history_turns` exchanges + plaza log; reset by New Chat) |
| `Memory.md` | Permanent | Lightweight self-description written by the agent | Implemented (`remember` tool) |
| Memoria (multi-layer memory) | Permanent | Recall semantic / episodic / procedural memory | Unconnected (below) |

**All prerequisites for connection are ready** ("an MCP client mechanism is needed" was once the first work item, but both shared and per-agent MCP are now implemented). Memoria requires no code change: add one Memoria server entry to the target agent's `agents/{id}/mcp.json`.

Launch policy (decided 2026-07-30):

1. **Keep the database completely separate from Neo's (the developer's) database.** Sharing mixes agent memory with developer memory and contaminates both recalls.
2. **Do not copy and branch Neo's database.** Development memories are noise in agent operation, bloat recall payloads and token cost every turn, and the memory "I was Neo" becomes a seedbed for confabulation.
3. **Plant seeds through selective migration**: write most procedures and practices to the file registries `SKILL.md` / `Construct.md`, not Memoria. Write only seeds of experiential knowledge as a small number of distilled entries in the new database. Initial memory may be sparse; an agent's memory should grow from its own experience.
4. Specify the database path as an **absolute path**. A cwd-dependent NotFound is a known pitfall already encountered on the Memoria side.

