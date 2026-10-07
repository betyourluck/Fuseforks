# Privacy Policy — Outcasts Fuseforks

**Last updated: 2026-10-07**

日本語版: [PRIVACY.md](PRIVACY.md)

---

## Summary

**The developer of Outcasts Fuseforks (the "app") collects no information from users.**

- The app has **no developer-operated server**, with one exception: the **model price table**
  is fetched, by default, from a static file the developer publishes on GitHub (see below).
- It performs **no usage measurement, analytics, or crash reporting**. No such mechanism is built in.
- There is **no account registration**.
- It performs **no update check** (it contacts nothing on startup).
- All data you create is stored **entirely on your own device**.

The app communicates externally with **only two kinds of destination**: the ones **you configure
yourself**, and the **model price table**.

**About the price-table fetch** (used to fill in per-model rates on screen):

- It happens **only when you press the "fetch" button**. There is **no fetch on startup,
  no fetch when a screen opens, and no periodic check**.
- The default source is a **static file the developer publishes on GitHub**. You can
  **change it, or clear it, from the system settings screen**; cleared, this request never
  happens at all (rates can also be typed in by hand).
- The request is an **HTTP GET only**. It carries **no village data, no conversation, and no
  model names**. The host (GitHub) sees only that the file was requested, plus the
  originating IP address.

---

## 1. Information the developer receives

**None.**

The app runs entirely on your device. The developer receives no information,
including whether you use the app at all.

---

## 2. Information stored on your device

The app stores the following in your device's application data area. None of it
is transmitted anywhere.

### Workspace

| Location | Contents |
|---|---|
| `world.json` | Agent definitions, model connection settings (**excluding credentials**), roles, layout |
| `sessions.redb` | Conversation history |
| `Ordinance.md` | Shared rules you wrote |
| `schedules.json` | Time-triggered request settings |
| `mcp.json` | Declarations of MCP servers to connect to |
| `agents/<id>/` | Per-agent persona, memory, icon, and the list of commands you allowed |
| `judges/<id>/judge.toml` | The questions and rules you wrote for a judge (see 4-4) |
| `user/icon.webp`, `external/icon.webp` | Icons you set (only if set) |
| `attachments/*.webp` | Images attached to conversations (**auto-deleted after 30 days or 500MB total**) |
| `exports/*.jsonl` | Conversations you explicitly exported |
| `village_id` | A random value identifying this configuration set (it does not identify your device or you) |
| `fuseforks.log` | Diagnostic log (see section 6) |
| `.fuseforks.lock` | A marker so that two processes never open the same configuration set at once (**empty**) |

### Per-device settings (outside the workspace)

| Location | Contents |
|---|---|
| `mcp_server.json` | Enable/disable, port, and access token for the local intake feature described below |
| `probe_approvals.json` | Record of which pre-check commands you allowed to run on this device |
| `pricing.json` | The URL the price table is fetched from |
| `jev.json` | Jev settings (see 4-4) — enable/disable, account ID, and strength for tool-result pruning |

**These live outside the village (workspace).** Handing your village to someone else
never puts their copy in a state where it talks to a destination they do not know about.

---

## 3. How credentials (API keys) are handled

**API keys are never written to configuration files.**

Keys you enter are stored in your **operating system's credential store**:

- Windows: Credential Manager
- macOS: Keychain
- Linux: freedesktop Secret Service

The service name used for storage is `jp.outcasts.fuseforks`. Besides model API keys,
this is also where the Cloudflare API token for Jev (see 4-4; shared by tool-result pruning and judges) and the
values that MCP server headers reference as `${secret:NAME}` (see 4-3) are kept.
**None of them can be read back** — the screen only shows whether a key is stored.

The app's configuration files (such as `world.json`) **have no field capable of
holding a key**. Because those files are stored in plain text, the place where a
secret could be written was removed from the structure itself.

Credentials are also never written to the diagnostic log.

### When running without the GUI (`fuseforks-cli`)

The `fuseforks-cli` executable, which you build from source, **sends the same things as the GUI**.
It opens the same configuration set through the same machinery, so no destination is added.

`fuseforks-cli` can also **read API keys from environment variables** instead of the operating system's
credential store (`--secrets env`, for containers and other places without a credential store; the default is
the credential store). If you choose this, **the keys sit in the process environment and the credential store's
protection does not apply** (other programs running as the same user, or tools that record the environment, may
be able to read them). **Which place to use is the choice of whoever runs it.** The app only reads environment
variables and never writes to them. The GUI always uses the credential store only.

### When running in a container (`fuseforks-cli bake`)

If you use `bake`, which makes a copy of a GUI village to run in a container, **the copy of the village (including
the conversations and memory that build up in the container) and the secrets passed as environment variables sit on
the host of whoever runs the container**. The app does not send that copy or those secrets anywhere, and running in a
container sends the same things as the GUI.

`bake` **stops without making a copy** if an MCP server header contains what looks like a key in plain text (the copy
flows into volumes and backups). Rewriting it as a `${secret:NAME}` reference lets it through. If you pass
`--allow-plaintext-headers` instead, the value goes into the copy in plain text. Whether a value looks like a key is
guessed from the header's name; there is no guarantee that every secret is recognised.

---

## 4. Information sent to third parties

The app communicates **only with destinations you configure**. You decide both
where data goes and what goes there.

### 4-1. AI model providers

When you register a model, the following is sent to that provider:

- The requests you type
- Conversation history (a recent window)
- Rules, personas, and memory text you wrote
- Results of tools the agent ran (which may include file contents and search results)
- Images attached to conversations
- Your API key, for authentication

The destination is whatever endpoint you registered — for example Anthropic,
OpenAI, Google, xAI, or any compatible server (including one you run yourself).

**Conversation history can include messages written by other agents.** If you assign
models from different providers to different agents, text written by one provider's model
is sent to another provider. This covers requests and answers between agents, excerpts of
the shared conversation, and copies of messages you picked with `@@` in the input box and
attached to your request.

**When you use "Draft with AI" (drafting assistance), the following is sent to the provider of
the model you picked in that panel.** It is sent only when you press send; opening the panel
sends nothing.

- What you type in the panel, and the questions and drafts exchanged within it
- The text you are editing (including unsaved changes)
- Part of the target's settings — for a SKILL.md / Construct.md draft: the target agent's name,
  its tool names (tool names of connected MCP servers included), the names of the agents it is
  connected to, and the paired file (Construct.md for SKILL.md and vice versa); for a judge draft:
  the IDs, names and roles of the agents it can route to

**Not sent:** memory (Memory.md), the shared rules (ordinance), and the conversation history.
The exchange in the panel is not saved and disappears when you close it (only the token counts
are kept in the conversation store).

**Handling of transmitted data is governed by each provider's own privacy policy.**
The developer of this app does not mediate that traffic and retains none of it.

### 4-2. Search grounding

If you enable the relevant feature, the model provider performs web or X (formerly
Twitter) searches on your behalf. **Search queries are sent to that provider.**
This is disabled by default and must be enabled by you, per model.

### 4-3. MCP servers

If you declare an MCP server, the app connects to it and sends the arguments of
tools the agent invokes. The destination and its behavior depend on the server
you chose.

If you declare a remote MCP server (`"type": "http"`), **the headers you
configured are sent with every request to that destination** (including an
Authorization header, i.e. your access token). A value written directly into a
header is stored in plaintext `mcp.json`, so distributing your workspace distributes
the token with it. If a header value is written as `${secret:NAME}`, the value is read
from the operating system's credential store (see 3) just before connecting and does not
stay in `mcp.json`. If it cannot be found, the app does not connect to that server.

### 4-4. The judgement-only model Jev (tool-result pruning, judges)

**Off by default.** It runs only once you enter your own Cloudflare account ID and
API token. In a village where those are not set, this path does not exist.
Two features use Jev (from TypeSafe AI, reached through Cloudflare Workers AI) and share
the key. Each sends only what is listed under it.

#### Tool-result pruning

When on, long bodies returned by MCP tools have paragraphs unrelated to the current
request dropped, and the judgement is asked of **Jev**, a judgement-only model
(from TypeSafe AI, reached through Cloudflare Workers AI).

**Nothing is sent except when a judgement is actually made.** A call that matches any of
the following is decided entirely on your device and **never leaves it**: the feature is
off, no key is set, the tool is out of scope (`file`, `run`, `rag` — anything but MCP),
the result is under 4,000 characters, the body is JSON that is neither a simple wrapper
nor an object with array fields, the request is under 20 characters, or there are no
paragraphs to score.

**Exactly two things are sent:**

- the **first 2,000 characters of the request** for that turn, and
- the **paragraphs being scored** from the tool's body (paragraphs over 6,000 characters
  are not sent). When the body is array-shaped JSON, what is sent instead of paragraphs is
  **the JSON text of each array element** (key names included).

**Tool names, arguments, conversation history, model names, and village data are never
sent.** The body that is sent may contain whatever a connected MCP server returned
(including memories, mail, or internal documents, if you connected a server that returns
those).

The diagnostic log (section 6) records **counts and character totals only** — not one
character of the body.

#### Judges

A judge decides where a request goes, using questions and rules you wrote. **If you have
created no judges, nothing leaves your device through this path.** Data is sent only when:

- a servant asks a judge to judge (only if you drew a tie from that servant to the judge), or
- you press "Try" in the judge editor.

**Exactly two things are sent:**

- the material to judge — the text the servant passed to the judge (`message`; **it may
  include the text of your request**). For "Try", the sample text you typed
- the **questions** you wrote in `judge.toml` — each question's name and text, the keys and
  descriptions of options, the descriptions of levels, and the true / false conditions

**Rules, destinations, notes (`note`), conversation history, the plaza log, the blackboard,
search results and village data are never sent.** Rules are evaluated on your device. The
diagnostic log (section 6) records only question names, the chosen answers, probabilities and
token counts — not the material judged and not the question text.

---

## 5. Operations performed on your device

Agents perform local operations only within limits you set.

- **File reads and writes** are confined to the **working folder you assign to
  each agent** (plus any folders you explicitly declare for read-only reference).
- **File deletion moves items to the trash only.** There is no permanent-delete path.
- **Command execution** runs only commands matching the **allow list you wrote for
  that agent**. Requests outside it are not executed; they are only recorded as
  pending your approval. Nothing is allowed by default.

---

## 6. Diagnostic log

`fuseforks.log` records operational observations: turn start and end, token counts,
tool names and result sizes, and errors.

**It does not record:**

- Prompt bodies
- Tool result bodies
- The "Draft with AI" exchange and draft text (only character counts and token counts are recorded)
- Credentials

Opening a tool row in the chat shows the arguments passed to that tool and the text it returned.
These are **held only in the app's memory** and are gone when the app exits.
Nothing new is written to the diagnostic log or to the saved conversation for this display.

The log stays on your device and rotates once when it exceeds 8MB. It is never transmitted.

---

## 7. Local intake feature (MCP server)

The app can accept requests from other programs on the same device.

- It is **disabled by default**.
- When enabled, it listens **only on `127.0.0.1` (your own machine)**. It cannot be
  reached from an external network.
- Authentication with an access token is **required**.
- While it is listening, this is shown in the status bar at the bottom of the window.

Its settings (enable/disable, port, token) are stored **outside the workspace**, so
sharing a workspace with someone else does not enable intake on their machine.

---

## 8. Your controls

- **Deleting data**: removing the workspace folder erases everything, including
  conversations, settings, and attached images.
- **Deleting credentials**: remove the entry from your OS credential store, or
  delete it from the app's model registration screen.
- **Stopping transmission**: if you register no model, nothing is sent anywhere.
- **Exporting**: conversations can be exported to JSONL by your own action.

---

## 9. Children

The app is not directed at children. Because the developer collects no information,
no information is collected from children either.

---

## 10. Changes to this policy

If this policy changes, this file is updated and the date above is revised.
The change history is visible in the repository's commit history.

---

## 11. Contact

For privacy inquiries about this app:

- GitHub Issues: https://github.com/betyourluck/Fuseforks/issues

---

## Note: why the developer holds nothing

This app is a tool for connecting your own API keys to models you chose, directly.
The developer is not part of that path. That is a policy, but it is also a
**structural fact**: the app contains no endpoint for sending anything to the developer.
