<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { BrainIcon, PlusIcon, SearchIcon, Trash2Icon } from "@lucide/vue";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  commandErrorMessage,
  memoryDelete,
  memoryList,
  memorySave,
} from "@/contracts/commands";
import type { MemoryEntry } from "@/contracts/types";
import { t, type MessageKey } from "@/lib/i18n";

const MEMORY_KINDS: { value: string; labelKey: MessageKey }[] = [
  { value: "preference", labelKey: "settings.memory.kind.preference" },
  { value: "fact", labelKey: "settings.memory.kind.fact" },
  { value: "decision", labelKey: "settings.memory.kind.decision" },
  { value: "project", labelKey: "settings.memory.kind.project" },
  { value: "note", labelKey: "settings.memory.kind.note" },
];

const entries = ref<MemoryEntry[]>([]);
const searchQuery = ref("");
const loading = ref(false);
const saving = ref(false);
const error = ref<string | null>(null);

const showForm = ref(false);
const draftTitle = ref("");
const draftKind = ref("preference");
const draftTags = ref("");
const draftContent = ref("");

let searchTimer: number | null = null;

async function loadMemories(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    const q = searchQuery.value.trim();
    entries.value = await memoryList(q === "" ? null : q, null, 80);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    loading.value = false;
  }
}

watch(searchQuery, () => {
  if (searchTimer !== null) window.clearTimeout(searchTimer);
  searchTimer = window.setTimeout(() => {
    searchTimer = null;
    void loadMemories();
  }, 180);
});

async function handleAddMemory(): Promise<void> {
  const title = draftTitle.value.trim();
  const content = draftContent.value.trim();
  if (!title || !content || saving.value) return;
  saving.value = true;
  error.value = null;
  try {
    await memorySave({
      title,
      content,
      kind: draftKind.value,
      tags: draftTags.value.trim() || null,
      scope: "global",
    });
    draftTitle.value = "";
    draftTags.value = "";
    draftContent.value = "";
    showForm.value = false;
    await loadMemories();
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
}

async function handleDelete(id: number): Promise<void> {
  error.value = null;
  try {
    await memoryDelete(id);
    entries.value = entries.value.filter((item) => item.id !== id);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

function kindLabel(kind: string): string {
  const match = MEMORY_KINDS.find((k) => k.value === kind);
  return match ? t(match.labelKey) : kind;
}

function formatDate(unixSecs: number): string {
  if (!unixSecs) return "";
  try {
    return new Date(unixSecs * 1000).toLocaleDateString();
  } catch {
    return "";
  }
}

onMounted(() => {
  void loadMemories();
});

onBeforeUnmount(() => {
  if (searchTimer !== null) window.clearTimeout(searchTimer);
});
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div class="min-w-0">
        <h3 class="text-xs font-semibold text-foreground">
          {{ t("settings.memory.title") }}
        </h3>
        <p class="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">
          {{ t("settings.memory.description") }}
        </p>
      </div>
      <Button
        variant="outline"
        size="sm"
        class="h-7 gap-1.5 text-xs"
        @click="showForm = !showForm"
      >
        <PlusIcon class="size-3.5" />
        {{ t("settings.memory.add") }}
      </Button>
    </div>

    <Alert v-if="error" variant="destructive">
      <AlertDescription class="text-xs">{{ error }}</AlertDescription>
    </Alert>

    <!-- Inline add memory form -->
    <section
      v-if="showForm"
      class="space-y-3 rounded-xl border border-border bg-secondary/40 p-4"
    >
      <div class="grid gap-3 sm:grid-cols-2">
        <div class="space-y-1">
          <label class="text-[11px] font-medium text-foreground">
            {{ t("settings.memory.field.title") }}
          </label>
          <Input
            v-model="draftTitle"
            class="h-8 text-xs"
            :placeholder="t('settings.memory.field.titlePlaceholder')"
          />
        </div>
        <div class="space-y-1">
          <label class="text-[11px] font-medium text-foreground">
            {{ t("settings.memory.field.kind") }}
          </label>
          <Select v-model="draftKind">
            <SelectTrigger class="h-8 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="item in MEMORY_KINDS"
                :key="item.value"
                :value="item.value"
                class="text-xs"
              >
                {{ t(item.labelKey) }}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      <div class="space-y-1">
        <label class="text-[11px] font-medium text-foreground">
          {{ t("settings.memory.field.tags") }}
        </label>
        <Input
          v-model="draftTags"
          class="h-8 text-xs"
          :placeholder="t('settings.memory.field.tagsPlaceholder')"
        />
      </div>

      <div class="space-y-1">
        <label class="text-[11px] font-medium text-foreground">
          {{ t("settings.memory.field.content") }}
        </label>
        <textarea
          v-model="draftContent"
          rows="3"
          :placeholder="t('settings.memory.field.contentPlaceholder')"
          class="w-full rounded-lg border border-border bg-background px-3 py-2 text-xs text-foreground outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
        />
      </div>

      <div class="flex justify-end gap-2">
        <Button variant="ghost" size="sm" class="h-7 text-xs" @click="showForm = false">
          {{ t("common.cancel") }}
        </Button>
        <Button
          variant="default"
          size="sm"
          class="h-7 text-xs"
          :disabled="saving || !draftTitle.trim() || !draftContent.trim()"
          @click="handleAddMemory"
        >
          {{ t("settings.memory.save") }}
        </Button>
      </div>
    </section>

    <!-- Search input -->
    <div class="relative">
      <SearchIcon
        class="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground"
        aria-hidden="true"
      />
      <Input
        v-model="searchQuery"
        class="h-8 pl-8 text-xs"
        :placeholder="t('settings.memory.searchPlaceholder')"
      />
    </div>

    <!-- Memories list -->
    <div v-if="loading && entries.length === 0" class="py-6 text-center text-xs text-muted-foreground">
      {{ t("common.loading") }}
    </div>

    <div
      v-else-if="entries.length === 0"
      class="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border py-10 text-center"
    >
      <BrainIcon class="size-5 text-muted-foreground/60" aria-hidden="true" />
      <p class="text-xs font-medium text-foreground">
        {{ t("settings.memory.emptyTitle") }}
      </p>
      <p class="max-w-sm text-[11px] text-muted-foreground">
        {{ t("settings.memory.emptyDescription") }}
      </p>
    </div>

    <div v-else class="space-y-2">
      <article
        v-for="item in entries"
        :key="item.id"
        class="group rounded-xl border border-border bg-secondary/35 p-3.5 transition-colors hover:bg-secondary/55"
      >
        <div class="flex items-start justify-between gap-2">
          <div class="flex min-w-0 flex-wrap items-center gap-1.5">
            <h4 class="text-xs font-semibold text-foreground">
              {{ item.title }}
            </h4>
            <span class="rounded-full border border-primary/30 bg-primary/10 px-2 py-0.5 text-[10px] font-medium text-primary">
              {{ kindLabel(item.kind) }}
            </span>
            <span
              v-if="item.scope !== 'global'"
              class="rounded-full border border-border bg-card px-2 py-0.5 font-mono text-[10px] text-muted-foreground"
            >
              {{ item.scope }}
            </span>
          </div>
          <Button
            variant="ghost"
            size="icon-xs"
            class="shrink-0 text-muted-foreground opacity-70 hover:text-destructive group-hover:opacity-100"
            :aria-label="t('common.delete')"
            @click="handleDelete(item.id)"
          >
            <Trash2Icon class="size-3.5" />
          </Button>
        </div>

        <p class="mt-1.5 whitespace-pre-wrap text-xs leading-relaxed text-foreground/90">
          {{ item.content }}
        </p>

        <div class="mt-2 flex flex-wrap items-center gap-2 text-[10px] text-muted-foreground">
          <span v-if="item.tags" class="font-mono">{{ item.tags }}</span>
          <span v-if="item.updatedAt" class="ms-auto">{{ formatDate(item.updatedAt) }}</span>
        </div>
      </article>
    </div>
  </div>
</template>
