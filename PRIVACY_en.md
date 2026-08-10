# Privacy Policy — Outcasts Fuseforks

**Last updated: 2026-08-11**

日本語版: [PRIVACY.md](PRIVACY.md)

---

## Summary

**The developer of Outcasts Fuseforks (the "app") collects no information from users.**

- The app has **no developer-operated server**. There is no endpoint the developer controls.
- It performs **no usage measurement, analytics, or crash reporting**. No such mechanism is built in.
- There is **no account registration**.
- It performs **no update check** (it contacts nothing on startup).
- All data you create is stored **entirely on your own device**.

The app communicates externally **only with destinations you configure yourself**.

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
| `user/icon.webp`, `external/icon.webp` | Icons you set (only if set) |
| `attachments/*.webp` | Images attached to conversations (**auto-deleted after 30 days or 500MB total**) |
| `exports/*.jsonl` | Conversations you explicitly exported |
| `village_id` | A random value identifying this configuration set (it does not identify your device or you) |
| `fuseforks.log` | Diagnostic log (see section 6) |

### Per-device settings (outside the workspace)

| Location | Contents |
|---|---|
| `mcp_server.json` | Enable/disable, port, and access token for the local intake feature described below |
| `probe_approvals.json` | Record of which pre-check commands you allowed to run on this device |

---

## 3. How credentials (API keys) are handled

**API keys are never written to configuration files.**

Keys you enter are stored in your **operating system's credential store**:

- Windows: Credential Manager
- macOS: Keychain
- Linux: freedesktop Secret Service

The service name used for storage is `jp.outcasts.fuseforks`.

The app's configuration files (such as `world.json`) **have no field capable of
holding a key**. Because those files are stored in plain text, the place where a
secret could be written was removed from the structure itself.

Credentials are also never written to the diagnostic log.

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
- Credentials

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
