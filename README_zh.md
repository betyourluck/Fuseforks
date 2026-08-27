[English](README.md) | [日本語](README_jp.md) | **中文**

# <img src="images/logo.webp" alt="Outcasts Fuseforks Logo" width="28" /> Outcasts Fuseforks

  [![Tauri](https://img.shields.io/badge/Tauri-2.0-orange?style=for-the-badge&logo=tauri&logoColor=white)](https://v2.tauri.app/)
  [![Vue](https://img.shields.io/badge/Vue.js-3.0-4FC08D?style=for-the-badge&logo=vue.js&logoColor=white)](https://vuejs.org)
  [![TypeScript](https://img.shields.io/badge/TypeScript-Strict-3178C6?style=for-the-badge&logo=typescript&logoColor=white)](https://www.typescriptlang.org)
  [![Rust](https://img.shields.io/badge/Rust-Backend-000000?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)

**亲手在本地饲养一个 AI 智能体村落。**

Outcasts Fuseforks 是一款让多个 AI 智能体相互协作、对话的
多智能体编排桌面应用。
创建智能体、连接它们、与它们对话，村落便会开始运转——
委派、分工、汇总，时机一到便会自动工作。
这一切都呈现在一个三栏的单一界面中。

![Outcasts Fuseforks Japanese Light](images/fuseforks.webp)

![Outcasts Fuseforks English Dark](images/fuseforks_en.webp)

Rust（`fuseforks-core`）+ Tauri v2 + Vue 3 + Bun。应用内的显示名称为「Fuseforks」。

## 能做什么

| | |
|---|---|
| 🏘️ **组建村庄** | 创建智能体并用羁绊连接。**仆从的羁绊**本身就是控制面板 |
| 🤝 **委派与合并** | 主持者通过 `ask` 询问、`plan` 并行分派给工作者并进行汇总。加入**计划确认**（可选・默认 OFF）后，可以在发出前让人编辑计划。等待答复的时间可以设置（默认 600 秒），委派的循环会立即被拒绝，无需等待 |
| ⏰ **日程** | 通过「每周四 17:00」「每 10 分钟」让请求按时间触发。不写 cron 表达式 |
| 🔎 **前置判定** | 触发时先运行命令，**仅当输出与信号一致时才**发起请求。不一致的次数不消耗任何 token。分发给村庄的命令未经批准不会运行 |
| 🔌 **MCP** | 可以直接**粘贴** Claude Desktop 的 `mcp.json`。公共 + 按智能体区分 |
| 🔍 **接地（Grounding）** | Gemini 的 Google 搜索、Grok 的 Live Search（web / X）、OpenAI 与 Meta 的 web 搜索，以及 Perplexity 的 web・金融・人物搜索与 URL 抓取。**明确区分显示搜索到的事实与未返回来源的事实** |
| 🧠 **思考摘要** | 将模型思考内容的摘要折叠到与答案**不同的框**中。来源是可验证的指向，摘要是不可验证的申报，因此不混在一起 |
| 🛠️ **自带工具** | `remember` / `grep` / `fd` / `diff` / `sd` / `yq` / `file` / `rag` / `run`。文件类工具无法结构化地读取工作文件夹之外的内容（例外是读取已声明文件夹的 `rag`，以及围栏为允许列表的 `run`） |
| 🪧 **工具的用途** | 使用工具时，**出于何种目的使用**会以一行显示在对话中。是**模型的自报**，不是审计记录 |
| 🎚️ **转移的可行性** | 对主持者设定为不持有「移交对话」的工具。**委派（询问并接收答复）与分派仍保留**，因此答复不会偏离而返回给使用者 |
| 🗣️ **广场日志** | 能听到他人对话的村庄。也有不听的自由（作为成本设置） |
| 📎 **路径补全** | 在输入栏输入 `@` 时，工作文件夹中的文件会作为候选项出现。**进入的只有路径**，消除了仆从寻找的循环 |
| 🖼️ **附件（图像・音频・视频・PDF）** | 粘贴到输入栏或选择后，目标仆从会看・听并答复。**仅在该回合传递**（避免在滑动窗口中重发）。**对无法传递的连接端在粘贴时发出警告，并拒绝发送** |
| 🏛️ **村庄条例** | 进入所有人提示词最顶部的通用规则。用于统一模型间宪法差异的正规化层 |
| 🎭 **职位** | 仆从的雏形。选择创建后即载入设置，在列表和地图上显示彩色徽章 |
| 📁 **工作文件夹的批量切换** | 将整个村庄转向另一项目时，可一次更改所有被勾选者的工作文件夹。**即使运行中也从下一次发言起生效** |
| 💾 **对话的保存** | 关闭后重新打开可从上次继续。可切换多个对话，并能从中间分支 |
| 📊 **统计** | 该村庄支付了多少，按 对话 × **仆从 × 模型** × 回合的结束方式来读取（切换模型的个体其行会分裂）。单位是 token（与预算相同权重的有效 token）。**注册每个模型的单价后，也会显示出大致的金额（`≈ $`）**（[Spec 41](specs/41_model-pricing.md)。单价通过「获取」按钮或手动输入。**没有单价的模型会从合计中排除，并会在画面上显示被排除**）。失败的回合也会计入支付。**「全部对话」显示以结算日为界的一个月数据，并可追溯到上个月**（结算日位于系统设置 &gt; 成本管理。默认是月末。「全期间」也可选择 — [Spec 42](specs/42_stats-period.md)）。**记录从此版本开始**（更早的对话无法计数） |
| ⚙️ **系统设置** | 自己的称呼与图标・语言（画面与发给仆从的指示都会切换）・token 限制・确认对话框。**左侧菜单是可供设置项目的清单** |

连接端是 OpenAI 兼容 / Anthropic / Gemini / xAI / OpenAI / Meta / Perplexity 的原生方式。**base URL 是自由的**，因此
也能直接连接到 Ollama、LM Studio 等本地 LLM 的接口。

## 思想 — 玩具外表下的真实

本应用的预设用户是**工程师的业余爱好**。我们不追求业务编排基础
设施 — 业务计算中有人工成本，能够成立“与其让人确认不如让 AI 运转”，
但个人眼中 API 费用的显眼程度超过了自己的时间。这种不对称性
无法从应用侧改变。

然而，**正因为是面向业余爱好，内部才必须是真实的**。如果只是简单的群聊工具，
甚至无法吸引工程师的兴趣。早期的 Linux 虽被 Solaris 用户视为玩具，
但其内核是真正的 Unix，因此才具备带回家使用的价值 — 我们追求的正是这种形态。

因此，设计分为两层且纪律不同：

- **核心（`fuseforks-core` / `data_contract.yaml` / 触发规则）具备业务品质。**
  冻结契约后再进行实现，测试遵循先看红后变绿。
  对 GUI 的依赖为零（机械化保证。仅靠此 Crate 即可无头运行）
- **外壳（村庄・角色・三窗格）是业余爱好的体验。**
  “配置少且易懂”是差异化轴心，绝不让用户去攀爬 cron 表达式或 YAML 的壁垒

两边都半途而废是唯一的失败形态。绝不为了可爱而放松契约。
绝不为了摆出业务面孔而增加配置。

## 构建

所需条件：**Rust 1.85 以上**（edition 2024）、**[Bun](https://bun.sh)**、
并满足各操作系统的 Tauri v2 前置要求（Windows 需要 WebView2，Linux 需要 WebKitGTK，macOS 需要 Xcode CLT）。

```bash
cd apps/gui-tauri && bun install
```

以开发模式启动（支持 HMR）：

```bash
cd apps/gui-tauri && bun run tauri dev
```

构建发布版本。安装程序将输出到 `target/release/bundle/`：

```bash
cd apps/gui-tauri && bun run tauri build
```

测试与代码检查：

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd apps/gui-tauri && bun run test
```

> **应用保持运行时执行 `cargo test --workspace` 会失败** —
> 因为无法替换可执行文件。仅测试核心时，即使应用正在运行，`cargo test -p fuseforks-core`
> 也可以通过。

推送 `v*.*` 标签后，GitHub Actions 会运行面向 3 种操作系统的发布构建
（[`.github/workflows/build.yml`](.github/workflows/build.yml)）。
普通提交不会触发构建。**macOS 构建仅面向 Apple Silicon**，
不支持 Intel Mac。

## 技术栈

**核心（`crates/fuseforks-core`）**

- **Rust** 2024 edition — 编排、触发规则、工具、LLM wire 层
- **Tokio**（I/O 与并发轮次）+ **Rayon**（CPU 侧）
- **redb** — 持久化会话。纯 Rust 实现，不依赖 C
- **keyring** — 将 API 密钥保存到操作系统的凭据存储中，不保存于配置文件
- **rmcp** — MCP 客户端（连接外部工具）与服务器（接收外部请求）
- **完全不依赖 GUI。** 仅凭此 crate 即可无头运行，并通过机制保证这一点

**外壳（`apps/gui-tauri`）**

- **Tauri v2** + **Vue 3** + **TypeScript** + **Vite**
- **Tailwind CSS v4** — 配色集中在一个位置，支持浅色 / 深色模式
- **v-network-graph** — 代理之间的连接（中央上方的地图。由于使用 SVG，节点采用 Vue 插槽）
- **CodeMirror 6** — 用于编辑条例、职位和设置
- **vue-i18n** — 日语 / 英语
- 测试使用 **vitest**，包管理器使用 **Bun**

> 上方提到的两份文档是以日语编写的。

## 进阶阅读

| | |
|---|---|
| [DETAIL_en.md](DETAIL_en.md) | 目录结构、并发模型、画面布局、工具安全边界、LLM 通信层、运行 |
| [data_contract.yaml](data_contract.yaml) | 领域契约。**优先于实现** |
| [specs/](specs) | 规格说明。经过起草、评审后按阶段实现 |
| [failures.md](failures.md) | 踩过的坑（症状 → 根本原因 → 对策 → 总结） |
| [PRIVACY_en.md](PRIVACY_en.md) | 隐私政策（**开发者不会收到任何数据**） |

> 上方的两份文档是以日语编写的。

## 许可证

**MPL-2.0** ([LICENSE](LICENSE)). 为什么选择该许可证（2026-08-05）：

- **改进应当回馈** — 如果您分发修改了**本分发版中文件的**版本，则必须公开**这些文件**的源码。更好的 Fuseforks 将回归最初的村庄。
- **义务止于文件边界** — 仅包含 Fuseforks 的更大作品（Larger Work）可以**根据您自己的条款**进行分发（§3.3），而您自己编写的文件从一开始就不在范围内。
- **私有修改保持私有** — 在自己的机器上使用修改后的副本无需承担任何公开义务。发布义务仅在进行分发时触发。
- 欢迎社区共同开发。在 MPL-2.0 下接受 Pull Request。
