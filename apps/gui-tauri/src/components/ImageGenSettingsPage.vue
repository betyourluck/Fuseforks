<script setup lang="ts">
/**
 * システム設定 ＞ 外部連携 ＞ 画像生成（Spec 76 D8 / P2）。
 *
 * **1 ファイル 1 責務で切り出した**（`SettingsDialog.vue` は既に大きい。`CommandRegistryPane.vue` と同じ判断）。
 * 判断は `lib/imageGenSettings.ts`、ここは配線と DOM だけ。
 *
 * **開いただけでは 1 バイトも外へ出ない** — 読むのは設定だけで、接続の確認は「接続を確かめる」を
 * 押したときだけ（D8。`imageGenSettingsWiring.test.ts` が留める）。保存は押した時点で反映
 * （Jev のページと同じ）。
 */
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as ipc from "../lib/ipc";
import { formatError } from "../lib/errorText";
import { askConfirm } from "../composables/useConfirm";
import {
  IMAGE_ASPECTS,
  IMAGE_RESOLUTIONS,
  PROVIDER_NAMES,
  buildConfig,
  inputsFromSlot,
  probeBlockKey,
  probeMessageKey,
  shownProvider,
  slotDirty,
  type SlotInputs,
} from "../lib/imageGenSettings";
import type {
  ImageAspect,
  ImageGenProbeView,
  ImageGenSettingsView,
  ImageProvider,
  ImageResolution,
} from "../types";

const { t } = useI18n();

const view = ref<ImageGenSettingsView | null>(null);
const inputs = ref<SlotInputs>({ model: "", baseUrl: "", timeout: "" });
const keyInput = ref("");
const busy = ref(false);
const notice = ref("");
const probe = ref<ImageGenProbeView | null>(null);

/** いま表示している接続先の行（未選択なら `null`）。 */
const row = computed(() => shownProvider(view.value));
const dirty = computed(() => slotDirty(view.value, inputs.value));
const probeBlock = computed(() => probeBlockKey(view.value, inputs.value));
const blocked = computed(() => !!view.value?.blocked);

/** 戻り値で画面を置き換え、表示している接続先の入力を保存値から組み直す。 */
function setView(next: ImageGenSettingsView): void {
  view.value = next;
  inputs.value = inputsFromSlot(shownProvider(next)?.slot);
}

onMounted(async () => {
  try {
    setView(await ipc.getImageGenSettings());
  } catch (err) {
    notice.value = formatError(ipc.toErrorPayload(err));
  }
});

async function save(
  next: { provider?: ImageProvider | null; aspect?: ImageAspect; resolution?: ImageResolution } = {},
): Promise<void> {
  if (busy.value || !view.value || blocked.value) return;
  busy.value = true;
  notice.value = "";
  try {
    setView(await ipc.setImageGenSettings(buildConfig(view.value, inputs.value, next)));
    if (next.provider !== undefined) probe.value = null;
    notice.value = t("settings.imageGen.saved");
  } catch (err) {
    notice.value = formatError(ipc.toErrorPayload(err));
  } finally {
    busy.value = false;
  }
}

async function saveKey(): Promise<void> {
  const secret = keyInput.value.trim();
  const provider = row.value?.id;
  if (!secret || !provider || busy.value) return;
  busy.value = true;
  notice.value = "";
  try {
    setView(await ipc.setImageGenKey(provider, secret));
    keyInput.value = "";
    probe.value = null;
    notice.value = t("settings.imageGen.keySaved");
  } catch (err) {
    notice.value = formatError(ipc.toErrorPayload(err));
  } finally {
    busy.value = false;
  }
}

async function clearKey(): Promise<void> {
  const provider = row.value?.id;
  if (!provider || busy.value) return;
  const ok = await askConfirm({
    title: t("settings.imageGen.clearKey"),
    message: t("settings.imageGen.clearKeyConfirm", { provider: PROVIDER_NAMES[provider] }),
    danger: true,
  });
  if (!ok) return;
  busy.value = true;
  notice.value = "";
  try {
    setView(await ipc.clearImageGenKey(provider));
    probe.value = null;
    notice.value = t("settings.imageGen.keyCleared");
  } catch (err) {
    notice.value = formatError(ipc.toErrorPayload(err));
  } finally {
    busy.value = false;
  }
}

/** **画像を作らない GET を 1 本。押したときだけ。** */
async function testConnection(): Promise<void> {
  if (busy.value || probeBlock.value) return;
  busy.value = true;
  notice.value = "";
  probe.value = null;
  try {
    probe.value = await ipc.testImageGen();
  } catch (err) {
    notice.value = formatError(ipc.toErrorPayload(err));
  } finally {
    busy.value = false;
  }
}

function onProviderChange(raw: string): void {
  void save({ provider: raw === "" ? null : (raw as ImageProvider) });
}
</script>

<template>
  <div>
    <h3 class="mb-1 text-sm font-semibold">{{ $t("settings.imageGen.heading") }}</h3>
    <p class="mb-3 text-xs text-ink-dim">{{ $t("settings.imageGen.lead") }}</p>

    <p v-if="view?.blocked" class="mb-3 rounded border border-warn px-2 py-1 text-xs text-warn">
      {{ $t("settings.imageGen.blocked", { reason: view.blocked }) }}
    </p>

    <!-- いま効いているか（Rust の `status` の 1 実装が決めたもの。D8）。 -->
    <p
      v-if="view"
      class="mb-3 text-xs"
      :class="view.active ? 'text-run' : 'text-ink-dim'"
      data-image-gen-status
    >
      {{
        view.active && view.provider
          ? $t("settings.imageGen.activeYes", { provider: PROVIDER_NAMES[view.provider] })
          : $t(`settings.imageGen.inactive.${view.inactiveReason ?? "no_provider"}`)
      }}
    </p>

    <label class="mb-1 block text-xs text-ink-dim">{{ $t("settings.imageGen.providerLabel") }}</label>
    <select
      class="rounded border border-line bg-surface-1 px-2 py-1 text-xs outline-none focus:border-accent disabled:opacity-50"
      :value="view?.provider ?? ''"
      :disabled="busy || blocked || !view"
      @change="onProviderChange(($event.target as HTMLSelectElement).value)"
    >
      <option value="">{{ $t("settings.imageGen.providerNone") }}</option>
      <option v-for="p in view?.providers ?? []" :key="p.id" :value="p.id">
        {{ PROVIDER_NAMES[p.id] }}
      </option>
    </select>
    <p class="mt-1 text-xs text-ink-dim">{{ $t("settings.imageGen.providerHint") }}</p>

    <template v-if="row">
      <label class="mb-1 mt-3 block text-xs text-ink-dim">
        {{ $t("settings.imageGen.keyLabel", { provider: PROVIDER_NAMES[row.id] }) }}
      </label>
      <div class="flex items-center gap-2">
        <input
          v-model="keyInput"
          type="password"
          spellcheck="false"
          autocomplete="off"
          :placeholder="
            row.hasKey ? $t('settings.imageGen.keyPlaceholderSet') : $t('settings.imageGen.keyPlaceholder')
          "
          class="flex-1 rounded border border-line bg-surface-0 px-2 py-1 font-mono text-xs outline-none focus:border-accent"
        />
        <button
          type="button"
          class="rounded border border-line px-3 py-1 text-xs hover:border-accent disabled:opacity-50"
          :disabled="!keyInput.trim() || busy"
          @click="saveKey"
        >
          {{ $t("settings.imageGen.saveKey") }}
        </button>
        <button
          v-if="row.hasKey"
          type="button"
          class="rounded border border-line px-3 py-1 text-xs text-warn hover:border-warn disabled:opacity-50"
          :disabled="busy"
          @click="clearKey"
        >
          {{ $t("settings.imageGen.clearKey") }}
        </button>
      </div>
      <p class="mt-1 text-xs text-ink-dim">{{ $t("settings.imageGen.keyHint") }}</p>

      <div class="mt-3 flex flex-wrap items-center gap-2">
        <button
          type="button"
          class="rounded border border-line px-3 py-1 text-xs hover:border-accent disabled:opacity-50"
          :disabled="busy || probeBlock !== null"
          :title="probeBlock ? $t(probeBlock) : ''"
          data-image-gen-test
          @click="testConnection"
        >
          {{ $t("settings.imageGen.test") }}
        </button>
        <span v-if="probeBlock" class="text-xs text-ink-dim">{{ $t(probeBlock) }}</span>
        <span
          v-else-if="probe"
          class="text-xs"
          :class="probe.outcome === 'ok' ? 'text-run' : 'text-warn'"
        >
          {{ $t(probeMessageKey(probe.outcome), { ms: probe.elapsedMs ?? "-", status: probe.status ?? "-" }) }}
        </span>
      </div>
      <p
        v-if="probe?.detail"
        class="mt-1 break-all rounded bg-surface-0 px-2 py-1 font-mono text-[11px] text-ink-dim"
      >
        {{ probe.detail }}
      </p>
      <p class="mt-1 text-xs text-ink-dim">{{ $t("settings.imageGen.testHint") }}</p>

      <h4 class="mb-1 mt-5 text-xs font-semibold text-ink">
        {{ $t("settings.imageGen.slotHeading", { provider: PROVIDER_NAMES[row.id] }) }}
      </h4>
      <div class="rounded border border-line bg-surface-0 p-3">
        <label class="mb-1 block text-xs text-ink-dim">{{ $t("settings.imageGen.modelLabel") }}</label>
        <input
          v-model="inputs.model"
          type="text"
          spellcheck="false"
          :disabled="blocked"
          :placeholder="row.defaultModel"
          class="w-full rounded border border-line bg-surface-1 px-2 py-1 font-mono text-xs outline-none focus:border-accent disabled:opacity-50"
        />
        <label class="mb-1 mt-2 block text-xs text-ink-dim">{{ $t("settings.imageGen.baseUrlLabel") }}</label>
        <input
          v-model="inputs.baseUrl"
          type="url"
          spellcheck="false"
          :disabled="blocked"
          :placeholder="row.defaultBaseUrl"
          class="w-full rounded border border-line bg-surface-1 px-2 py-1 font-mono text-xs outline-none focus:border-accent disabled:opacity-50"
        />
        <label class="mb-1 mt-2 block text-xs text-ink-dim">{{ $t("settings.imageGen.timeoutLabel") }}</label>
        <input
          v-model="inputs.timeout"
          type="text"
          inputmode="numeric"
          spellcheck="false"
          :disabled="blocked"
          :placeholder="String(row.defaultTimeoutSecs)"
          class="w-28 rounded border border-line bg-surface-1 px-2 py-1 font-mono text-xs outline-none focus:border-accent disabled:opacity-50"
        />
        <p class="mt-1 text-xs text-ink-dim">{{ $t("settings.imageGen.slotHint") }}</p>
        <button
          type="button"
          class="mt-2 rounded border border-line px-3 py-1 text-xs hover:border-accent disabled:opacity-50"
          :disabled="!dirty || busy || blocked"
          @click="save()"
        >
          {{ $t("common.save") }}
        </button>
      </div>
    </template>

    <h4 class="mb-1 mt-5 text-xs font-semibold text-ink">{{ $t("settings.imageGen.defaultsHeading") }}</h4>
    <div class="flex flex-wrap items-end gap-4">
      <div>
        <label class="mb-1 block text-xs text-ink-dim">{{ $t("settings.imageGen.aspectLabel") }}</label>
        <select
          class="rounded border border-line bg-surface-1 px-2 py-1 text-xs outline-none focus:border-accent disabled:opacity-50"
          :value="view?.aspect ?? 'square'"
          :disabled="busy || blocked || !view"
          @change="save({ aspect: ($event.target as HTMLSelectElement).value as ImageAspect })"
        >
          <option v-for="a in IMAGE_ASPECTS" :key="a" :value="a">
            {{ $t(`settings.imageGen.aspect.${a}`) }}
          </option>
        </select>
      </div>
      <div>
        <label class="mb-1 block text-xs text-ink-dim">{{ $t("settings.imageGen.resolutionLabel") }}</label>
        <!-- Meta は解像度が効かない（`size` が縦横比しか決めない）。値は保存したまま押せなくする（D8）。 -->
        <select
          class="rounded border border-line bg-surface-1 px-2 py-1 text-xs outline-none focus:border-accent disabled:opacity-50"
          :value="view?.resolution ?? 'standard'"
          :disabled="busy || blocked || !view || row?.honorsResolution === false"
          data-image-gen-resolution
          @change="save({ resolution: ($event.target as HTMLSelectElement).value as ImageResolution })"
        >
          <option v-for="r in IMAGE_RESOLUTIONS" :key="r" :value="r">
            {{ $t(`settings.imageGen.resolution.${r}`) }}
          </option>
        </select>
      </div>
    </div>
    <p v-if="row?.honorsResolution === false" class="mt-1 text-xs text-ink-dim">
      {{ $t("settings.imageGen.resolutionMeta") }}
    </p>
    <p class="mt-1 text-xs text-ink-dim">{{ $t("settings.imageGen.defaultsHint") }}</p>

    <span v-if="notice" class="mt-3 block text-xs text-ink-dim">{{ notice }}</span>
    <p class="mt-4 text-xs text-ink-dim">{{ $t("settings.imageGen.costNote") }}</p>
    <p class="mt-1 text-xs text-ink-dim">{{ $t("settings.imageGen.privacy") }}</p>
    <p class="mt-1 text-xs text-ink-dim">{{ $t("settings.imageGen.deviceNote") }}</p>
  </div>
</template>
