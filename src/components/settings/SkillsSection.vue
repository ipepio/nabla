<script setup lang="ts">
import { onMounted, ref } from "vue";
import { PencilIcon, PlusIcon, SparklesIcon, Trash2Icon } from "@lucide/vue";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  commandErrorMessage,
  deleteSkill,
  listSkills,
  saveSkill,
} from "@/contracts/commands";
import type { SkillEntry } from "@/contracts/types";
import { t } from "@/lib/i18n";

const skills = ref<SkillEntry[]>([]);
const loading = ref(false);
const saving = ref(false);
const error = ref<string | null>(null);

const showForm = ref(false);
const editingName = ref<string | null>(null);
const draftName = ref("");
const draftDescription = ref("");
const draftPrompt = ref("");

async function loadSkills(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    skills.value = await listSkills(null);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    loading.value = false;
  }
}

function openCreateForm(): void {
  editingName.value = null;
  draftName.value = "";
  draftDescription.value = "";
  draftPrompt.value = "";
  showForm.value = true;
}

function openEditForm(skill: SkillEntry): void {
  editingName.value = skill.name;
  draftName.value = skill.name;
  draftDescription.value = skill.description;
  draftPrompt.value = skill.prompt;
  showForm.value = true;
}

function closeForm(): void {
  showForm.value = false;
  editingName.value = null;
}

async function handleSaveSkill(): Promise<void> {
  const name = draftName.value.trim();
  const prompt = draftPrompt.value.trim();
  if (!name || !prompt || saving.value) return;
  saving.value = true;
  error.value = null;
  try {
    const saved = await saveSkill({
      name,
      description: draftDescription.value.trim(),
      prompt,
    });
    if (editingName.value && editingName.value !== saved.name) {
      await deleteSkill(editingName.value).catch(() => {});
    }
    closeForm();
    await loadSkills();
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
}

async function handleDeleteSkill(skill: SkillEntry): Promise<void> {
  error.value = null;
  try {
    await deleteSkill(skill.name);
    skills.value = skills.value.filter((s) => s.id !== skill.id);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

onMounted(() => {
  void loadSkills();
});
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div class="min-w-0">
        <h3 class="text-xs font-semibold text-foreground">
          {{ t("settings.skills.title") }}
        </h3>
        <p class="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">
          {{ t("settings.skills.description") }}
        </p>
      </div>
      <Button
        variant="outline"
        size="sm"
        class="h-7 gap-1.5 text-xs"
        @click="openCreateForm"
      >
        <PlusIcon class="size-3.5" />
        {{ t("settings.skills.add") }}
      </Button>
    </div>

    <Alert v-if="error" variant="destructive">
      <AlertDescription class="text-xs">{{ error }}</AlertDescription>
    </Alert>

    <!-- Create / Edit skill form -->
    <section
      v-if="showForm"
      class="space-y-3 rounded-xl border border-border bg-secondary/40 p-4"
    >
      <div class="grid gap-3 sm:grid-cols-2">
        <div class="space-y-1">
          <label class="text-[11px] font-medium text-foreground">
            {{ t("settings.skills.field.name") }}
          </label>
          <Input
            v-model="draftName"
            class="h-8 font-mono text-xs"
            :placeholder="t('settings.skills.field.namePlaceholder')"
          />
        </div>
        <div class="space-y-1">
          <label class="text-[11px] font-medium text-foreground">
            {{ t("settings.skills.field.description") }}
          </label>
          <Input
            v-model="draftDescription"
            class="h-8 text-xs"
            :placeholder="t('settings.skills.field.descriptionPlaceholder')"
          />
        </div>
      </div>

      <div class="space-y-1">
        <label class="text-[11px] font-medium text-foreground">
          {{ t("settings.skills.field.prompt") }}
        </label>
        <textarea
          v-model="draftPrompt"
          rows="5"
          :placeholder="t('settings.skills.field.promptPlaceholder')"
          class="w-full rounded-lg border border-border bg-background px-3 py-2 font-mono text-xs leading-relaxed text-foreground outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
        />
      </div>

      <div class="flex justify-end gap-2">
        <Button variant="ghost" size="sm" class="h-7 text-xs" @click="closeForm">
          {{ t("common.cancel") }}
        </Button>
        <Button
          variant="default"
          size="sm"
          class="h-7 text-xs"
          :disabled="saving || !draftName.trim() || !draftPrompt.trim()"
          @click="handleSaveSkill"
        >
          {{ t("settings.skills.save") }}
        </Button>
      </div>
    </section>

    <div v-if="loading && skills.length === 0" class="py-6 text-center text-xs text-muted-foreground">
      {{ t("common.loading") }}
    </div>

    <div
      v-else-if="skills.length === 0"
      class="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border py-10 text-center"
    >
      <SparklesIcon class="size-5 text-muted-foreground/60" aria-hidden="true" />
      <p class="text-xs font-medium text-foreground">
        {{ t("settings.skills.emptyTitle") }}
      </p>
      <p class="max-w-sm text-[11px] text-muted-foreground">
        {{ t("settings.skills.emptyDescription") }}
      </p>
    </div>

    <div v-else class="space-y-2">
      <article
        v-for="skill in skills"
        :key="skill.id"
        class="group rounded-xl border border-border bg-secondary/35 p-3.5 transition-colors hover:bg-secondary/55"
      >
        <div class="flex items-start justify-between gap-2">
          <div class="flex min-w-0 flex-wrap items-center gap-2">
            <span class="rounded-md border border-primary/30 bg-primary/10 px-2 py-0.5 font-mono text-xs font-semibold text-primary">
              /{{ skill.name }}
            </span>
            <span class="text-xs text-muted-foreground">
              {{ skill.description }}
            </span>
          </div>
          <div class="flex shrink-0 items-center gap-0.5 opacity-80 group-hover:opacity-100">
            <Button
              variant="ghost"
              size="icon-xs"
              :aria-label="t('settings.providers.edit')"
              @click="openEditForm(skill)"
            >
              <PencilIcon class="size-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon-xs"
              class="text-muted-foreground hover:text-destructive"
              :aria-label="t('common.delete')"
              @click="handleDeleteSkill(skill)"
            >
              <Trash2Icon class="size-3.5" />
            </Button>
          </div>
        </div>
        <pre class="mt-2 max-h-24 overflow-hidden whitespace-pre-wrap rounded-lg border border-border/60 bg-background/60 p-2 font-mono text-[11px] leading-relaxed text-foreground/85">{{ skill.prompt }}</pre>
      </article>
    </div>
  </div>
</template>
