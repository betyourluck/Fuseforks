<script setup lang="ts">
/**
 * 登録済みコマンドの編集（Spec 15 P4）。
 *
 * **ここは安全機構の設定画面ではない。** 囲いは登録ただ 1 つで、
 * 登録した時点でそのコマンドができること全部を許している。画面はそれを隠さない。
 *
 * `RoleDialog.vue` と同じく 1 ファイル 1 責務で切り出してある
 * （システム設定の右ページは器で、CRUD はこちらが持つ）。
 */
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as ipc from "../lib/ipc";
import { formatError } from "../lib/errorText";
import type { CommandRequestView, CommandView } from "../types";

const { t } = useI18n();

const rows = ref<CommandView[]>([]);
const requests = ref<CommandRequestView[]>([]);
const selected = ref<number | null>(null);
const loading = ref(true);
const busy = ref(false);
const error = ref("");
const notes = ref<string[]>([]);

/**
 * 追加引数を許した汎用インタプリタ。**警告を出すだけで登録は止めない。**
 *
 * この一覧は**除外リストではない** — 載っていないものを安全と宣言しないし、
 * 許容の判定に一切関与しない。載っているものに注意を促すだけ。
 */
const INTERPRETERS = ["python", "python3", "node", "sh", "bash", "zsh", "pwsh", "powershell", "ruby", "perl", "deno"];

const current = computed(() => (selected.value === null ? null : rows.value[selected.value]));

/** 汎用インタプリタに追加引数を許している = 実質的に任意コード実行。 */
const interpreterWarning = computed(() => {
  const row = current.value;
  if (!row || !row.allowExtraArgs) return false;
  const stem = row.program.replace(/\\/g, "/").split("/").pop()?.toLowerCase() ?? "";
  return INTERPRETERS.some((name) => stem === name || stem === `${name}.exe`);
});

async function load(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    rows.value = await ipc.listCommands();
    requests.value = await ipc.listCommandRequests();
    selected.value = rows.value.length ? 0 : null;
  } catch (e) {
    error.value = formatError(ipc.toErrorPayload(e));
  } finally {
    loading.value = false;
  }
}

onMounted(load);

function addRow(name = ""): void {
  rows.value.push({
    name,
    description: "",
    program: "",
    args: [],
    allowExtraArgs: false,
    timeoutSecs: 60,
    cwd: null,
    unavailable: null,
  });
  selected.value = rows.value.length - 1;
}

function removeRow(): void {
  if (selected.value === null) return;
  rows.value.splice(selected.value, 1);
  selected.value = rows.value.length ? Math.min(selected.value, rows.value.length - 1) : null;
}

/** 引数は 1 行 1 引数で編集する。空白区切りにするとシェルの引用規則が要る。 */
const argsText = computed({
  get: () => current.value?.args.join("\n") ?? "",
  set: (value: string) => {
    if (current.value) current.value.args = value.split("\n").filter((a) => a.length > 0);
  },
});

const cwdText = computed({
  get: () => current.value?.cwd ?? "",
  set: (value: string) => {
    if (current.value) current.value.cwd = value.trim() || null;
  },
});

/** `PATH` から絶対パスを引いて欄へ入れる。**記録するのは解決後の絶対パス。** */
async function resolveProgram(): Promise<void> {
  const row = current.value;
  if (!row || !row.program.trim()) return;
  busy.value = true;
  try {
    const resolved = await ipc.resolveCommandProgram(row.program.trim());
    if (resolved) row.program = resolved;
    else error.value = t("commands.notFoundOnPath", { name: row.program.trim() });
  } catch (e) {
    error.value = formatError(ipc.toErrorPayload(e));
  } finally {
    busy.value = false;
  }
}

async function save(): Promise<void> {
  busy.value = true;
  error.value = "";
  notes.value = [];
  try {
    notes.value = await ipc.saveCommands(
      rows.value.map(({ unavailable: _drop, ...spec }) => spec),
    );
    await load();
  } catch (e) {
    error.value = formatError(ipc.toErrorPayload(e));
  } finally {
    busy.value = false;
  }
}

/** 要求された名前で登録の下書きを作る。引数は利用者が決める（自動で入れない）。 */
function adoptRequest(request: CommandRequestView): void {
  addRow(request.name);
}

async function dismissRequest(name: string): Promise<void> {
  busy.value = true;
  try {
    await ipc.dismissCommandRequest(name);
    requests.value = requests.value.filter((r) => r.name !== name);
  } catch (e) {
    error.value = formatError(ipc.toErrorPayload(e));
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div>
    <h3 class="mb-1 text-xs font-semibold text-ink">{{ $t("commands.heading") }}</h3>
    <p class="mb-1 text-ink-dim">{{ $t("commands.help") }}</p>
    <p class="mb-3 text-warn">{{ $t("commands.notASafetyMechanism") }}</p>

    <p v-if="loading" class="py-6 text-center text-ink-dim">{{ $t("settings.loading") }}</p>

    <template v-else>
      <!-- エージェントが要求したコマンド -->
      <section v-if="requests.length" class="mb-4 rounded border border-line p-2">
        <p class="mb-1 font-semibold text-ink">{{ $t("commands.requested") }}</p>
        <div
          v-for="request in requests"
          :key="request.name"
          class="flex items-center gap-2 py-0.5"
        >
          <code class="text-accent">{{ request.name }}</code>
          <span class="text-ink-dim">
            {{ request.attemptedExtraArgs.join(" ") }}
            （{{ $t("commands.requestCount", { count: request.count }) }}）
          </span>
          <button class="ml-auto btn-mini" @click="adoptRequest(request)">
            {{ $t("commands.adopt") }}
          </button>
          <button class="btn-mini" :disabled="busy" @click="dismissRequest(request.name)">
            {{ $t("commands.dismiss") }}
          </button>
        </div>
      </section>

      <div class="flex gap-3">
        <!-- 一覧 -->
        <div class="w-40 shrink-0">
          <div class="mb-1 max-h-56 overflow-y-auto rounded border border-line">
            <button
              v-for="(row, index) in rows"
              :key="index"
              class="block w-full truncate px-2 py-1 text-left hover:bg-surface-1"
              :class="{ 'bg-surface-1 text-accent': selected === index }"
              @click="selected = index"
            >
              {{ row.name || $t("commands.untitled") }}
              <span v-if="row.unavailable" class="text-warn">!</span>
            </button>
            <p v-if="!rows.length" class="px-2 py-2 text-ink-dim">{{ $t("commands.none") }}</p>
          </div>
          <button class="btn-mini" @click="addRow()">{{ $t("commands.add") }}</button>
        </div>

        <!-- 編集 -->
        <div v-if="current" class="min-w-0 flex-1 space-y-2">
          <p v-if="current.unavailable" class="rounded border border-warn p-1 text-warn">
            {{ current.unavailable }}
          </p>

          <label class="block">
            <span class="text-ink-dim">{{ $t("commands.name") }}</span>
            <input v-model="current.name" class="input" />
          </label>

          <label class="block">
            <span class="text-ink-dim">{{ $t("commands.description") }}</span>
            <input v-model="current.description" class="input" />
          </label>

          <label class="block">
            <span class="text-ink-dim">{{ $t("commands.program") }}</span>
            <div class="flex gap-1">
              <input v-model="current.program" class="input" />
              <button class="btn-mini shrink-0" :disabled="busy" @click="resolveProgram">
                {{ $t("commands.resolve") }}
              </button>
            </div>
          </label>

          <label class="block">
            <span class="text-ink-dim">{{ $t("commands.args") }}</span>
            <textarea v-model="argsText" rows="3" class="input font-mono" />
          </label>

          <label class="flex items-center gap-2">
            <input v-model="current.allowExtraArgs" type="checkbox" />
            <span>{{ $t("commands.allowExtraArgs") }}</span>
          </label>
          <p v-if="interpreterWarning" class="text-warn">
            {{ $t("commands.interpreterWarning") }}
          </p>

          <label class="block">
            <span class="text-ink-dim">{{ $t("commands.timeoutSecs") }}</span>
            <input v-model.number="current.timeoutSecs" type="number" min="1" max="3600" class="input" />
          </label>

          <label class="block">
            <span class="text-ink-dim">{{ $t("commands.cwd") }}</span>
            <input v-model="cwdText" class="input" :placeholder="$t('commands.cwdPlaceholder')" />
          </label>

          <button class="btn-mini" @click="removeRow">{{ $t("commands.remove") }}</button>
        </div>
      </div>

      <div class="mt-3 flex items-center gap-2">
        <button class="btn" :disabled="busy" @click="save">{{ $t("commands.save") }}</button>
        <span v-if="error" class="text-warn">{{ error }}</span>
      </div>
      <p v-for="note in notes" :key="note" class="mt-1 text-warn">{{ note }}</p>
    </template>
  </div>
</template>
