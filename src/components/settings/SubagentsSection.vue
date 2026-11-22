<script setup lang="ts">
/**
 * Subagent profiles pane (S2, S4 and S6 of `odd/tasks/agent-profiles.md`).
 *
 * A sibling of Providers and MCP: a master switch, the profile list, one editor
 * per profile and the five templates offered on demand.
 *
 * Three decisions shape it:
 *
 * - **Nothing is active by default (S3).** The master switch and every profile's
 *   own switch ship off, a fresh install has no profiles at all, and no template
 *   is added without a click. The pane never enables anything on the human's
 *   behalf: it offers, the human turns on and saves.
 *
 * - **An empty list while the switch is on is the built-in templates (S1 of
 *   `agent-profiles-defaults.md`).** With the master switch on and nothing saved,
 *   the five templates apply enabled, so delegation really runs. The list stays
 *   empty and editable — nothing is written into `agents.json` on the human's
 *   behalf — but the pane says out loud that the defaults are what a child runs
 *   with, because an empty-looking list beside spawning children would misdescribe
 *   the app. The moment one profile is saved, that list is authoritative and the
 *   defaults stop applying.
 *
 * - **A change applies to the next session.** The gate is read at spawn time, so
 *   the master switch cannot reach the pi process already running — the line under
 *   the switch says exactly that instead of implying a live reload.
 *
 * - **One draft, one write.** Everything here edits the draft in
 *   `src/lib/agent-profiles.ts` and nothing else; `saveAgentsConfig` is called by
 *   the Save button and by nothing else. Rust validates the whole store before it
 *   writes and answers with the sentence that says which profile is wrong, and
 *   that sentence is rendered verbatim: the pane never rewrites it into "please
 *   check your profiles", because the human needs the profile name and the field.
 *
 * The draft lives at module scope for a structural reason: the settings dialog
 * renders its panes through a `v-if` chain, so switching sections unmounts this
 * component. Local state would lose every edit on a section switch; the module
 * keeps them, and `loadAgentsConfig` refuses to clobber a dirty draft.
 *
 * The list stays readable with the master switch off — dimmed and inert, not
 * hidden: the human configured those profiles and needs to see what is there
 * before turning the gate on.
 *
 * Models are the accounts' own catalogue, read through the shared
 * `provider-models` store the composer's picker and the provider editor already
 * read, so a profile can only name a model this account lists. Tool names come
 * from the catalogue in `src/lib/agent-profiles.ts` (the names the permission gate
 * and the subagent extension use), and efforts are pi's seven thinking levels from
 * `src/lib/preferences.ts` — there is no second list of either.
 *
 * No `watch`: every change goes through an explicit handler, and dirtiness is a
 * computed over the draft and the last file answer.
 */
import { computed, onMounted, ref } from "vue";
import { PlusIcon } from "@lucide/vue";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { t } from "@/lib/i18n";
import {
  AGENT_PROFILE_TEMPLATES,
  AGENT_TOOLS,
  addAgentProfile,
  agentsConfig,
  agentsConfigDirty,
  agentsConfigLoading,
  agentsConfigReadError,
  agentsConfigSaveError,
  agentsConfigSaved,
  agentsConfigSaving,
  blankProfile,
  discardAgentsChanges,
  hasProfileNamed,
  loadAgentsConfig,
  orderedTools,
  removeAgentProfile,
  saveAgentsDraft,
  setSubagentsEnabled,
  setSubagentsModelDefault,
  updateAgentProfile,
  type AgentProfileTemplate,
} from "@/lib/agent-profiles";
import { loadProviderModels, providerModels } from "@/lib/provider-models";
import { THINKING_LEVELS } from "@/lib/preferences";
import type { AgentProfile } from "@/contracts/types";

/** The draft the pane edits, `null` only before the first read. */
const draft = computed(() => agentsConfig.value);

/** S3: the gate. Off, nothing of this section is offered to the model. */
const masterEnabled = computed<boolean>(() => draft.value?.enabled ?? false);

/** The profiles in file order; the list and the rows are positional because that
 * is how the file keeps them and how the editor addresses them. */
const profiles = computed<AgentProfile[]>(() => draft.value?.profiles ?? []);

/**
 * S1: the fallback is in effect — the master switch is on and nothing is stored,
 * so the five built-in templates are what a child runs with. The list stays empty
 * and editable; this flag only changes what the pane SAYS about it.
 */
const usingDefaults = computed<boolean>(
  () => masterEnabled.value && profiles.value.length === 0,
);

/** The first read is in flight and there is nothing to paint yet. */
const loading = computed(
  () => agentsConfigLoading.value && agentsConfig.value === null,
);

/** The profile the editor is open on, `null` when it is closed. */
const editorIndex = ref<number | null>(null);
const editorOpen = ref(false);

/** The templates are being offered. Closed until the human asks for them. */
const templatesOpen = ref(false);

/** The profile on screen in the editor, or `null` when the index is gone. */
const editing = computed<AgentProfile | null>(() => {
  const index = editorIndex.value;
  if (index === null) return null;
  return profiles.value[index] ?? null;
});

/**
 * Tools a stored profile lists that this build's catalogue does not know. They
 * are kept (the file holds them and the extension honours them), shown here so an
 * edit does not pretend they are not there, and never added by this pane.
 */
const unknownTools = computed<string[]>(() => {
  const known: readonly string[] = AGENT_TOOLS.map((tool) => tool.id);
  return (editing.value?.tools ?? []).filter((tool) => !known.includes(tool));
});

// ── the model catalogue, from the accounts this app is signed in to ─────────

interface ModelOption {
  /** The value a profile stores: `provider/model`. */
  value: string;
  label: string;
}

interface ModelGroup {
  provider: string;
  name: string;
  options: ModelOption[];
}

/**
 * The same answer the composer's picker renders: every provider with the models
 * pi would really use. `enabled` is pi's usability mark and `hidden` is Nabla's
 * display preference, so a model marked either way is not offered here — picking
 * one would write a profile that fails at spawn time.
 */
const modelGroups = computed<ModelGroup[]>(() =>
  providerModels.value
    .map((entry) => ({
      provider: entry.provider,
      name: entry.name.trim() === "" ? entry.provider : entry.name,
      options: entry.models
        .filter((model) => model.enabled && model.hidden !== true)
        .map((model) => ({
          value: `${entry.provider}/${model.id}`,
          label: model.name.trim() === "" ? model.id : model.name,
        })),
    }))
    .filter((group) => group.options.length > 0),
);

/** One flat lookup of what can be picked, for the "is it in the catalogue" check. */
const knownModels = computed<Set<string>>(() => {
  const values = new Set<string>();
  for (const group of modelGroups.value) {
    for (const option of group.options) {
      values.add(option.value);
      // A bare model id is a legal stored value (S6): it resolves against the
      // first provider that lists it. Keeping the ids here too is what lets the
      // pane recognize one instead of calling it unknown.
      values.add(option.value.slice(group.provider.length + 1));
    }
  }
  return values;
});

/**
 * The Select value that stands for "nothing chosen". reka-ui refuses an empty
 * string as an item value on purpose (the empty string is how a Select clears its
 * selection), so the two optional fields use a sentinel that no model id or
 * thinking level can be, and map it back to `null` on the way into the draft.
 */
const NO_CHOICE = "__none";

/** The editor's model field as a Select value: empty means "no model of its own". */
const editingModel = computed<string>(() => {
  const model = editing.value?.model?.trim() ?? "";
  return model === "" ? NO_CHOICE : model;
});

/**
 * S6: the editor warns when the stored model is not one this account offers.
 * Nothing is rewritten or blocked — Rust only checks the shape of a model id, and
 * the extension is what refuses an unreachable one at run time — so the honest
 * move is to say so here, before the human finds out from a failed run.
 */
const editingModelUnknown = computed<boolean>(() => {
  const model = editing.value?.model?.trim() ?? "";
  return model !== "" && !knownModels.value.has(model);
});

const editingEffort = computed<string>(
  () => editing.value?.effort ?? NO_CHOICE,
);

/** The default model, as a Select value: `NO_CHOICE` means "the session's model". */
const defaultModel = computed<string>(() => {
  const value = draft.value?.modelDefault?.trim() ?? "";
  return value === "" ? NO_CHOICE : value;
});

/**
 * S6, second level: the default model a *new* run would fall back to, when that
 * value is no longer in the catalogue (a provider removed, a model hidden). Shown
 * as its own row so the selector can show it instead of a blank field that looks
 * like "nothing is set".
 */
const defaultModelUnknown = computed<boolean>(() => {
  const value = defaultModel.value;
  return value !== NO_CHOICE && !knownModels.value.has(value);
});

// ── handlers ────────────────────────────────────────────────────────────────

/** S3: the master switch, on the draft. Off means no delegation at all. */
function handleMasterToggle(next: boolean): void {
  setSubagentsEnabled(next);
}

/** S6: the fallback model. `NO_CHOICE` is a real answer ("the session's model"). */
function handleDefaultModel(value: string): void {
  setSubagentsModelDefault(value === NO_CHOICE ? null : value);
}

/** One profile's own switch. Disabled while the gate is closed, see the row. */
function handleProfileToggle(index: number, next: boolean): void {
  updateAgentProfile(index, { enabled: next });
}

/** Opens the editor of one profile. */
function openEditor(index: number): void {
  editorIndex.value = index;
  editorOpen.value = true;
}

/**
 * S2/S4: a blank profile, written by a click and by nothing else, with its editor
 * open so the human fills the two fields Rust requires. It arrives disabled.
 */
function handleNewProfile(): void {
  const index = addAgentProfile(blankProfile());
  if (index === null) return;
  openEditor(index);
}

/**
 * S2: one template, fully written and disabled, added by an explicit click. A name
 * already in the list is refused here (the button is disabled) instead of creating
 * a pair the save would reject as a duplicate.
 */
function handleAddTemplate(template: AgentProfileTemplate): void {
  if (hasProfileNamed(template.profile.name)) return;
  const index = addAgentProfile(template.profile);
  if (index === null) return;
  openEditor(index);
}

/** Removes the profile on screen from the draft. Nothing is deleted on disk. */
function handleRemoveProfile(): void {
  const index = editorIndex.value;
  if (index === null) return;
  removeAgentProfile(index);
  closeEditor();
}

function closeEditor(): void {
  editorOpen.value = false;
  editorIndex.value = null;
}

/**
 * The editor's own open state. Closing clears the index, so a removed profile can
 * never be painted from a stale position; opening only happens through
 * `openEditor`.
 */
function handleEditorOpenChange(next: boolean): void {
  if (next) {
    editorOpen.value = true;
    return;
  }
  closeEditor();
}

/** One field of the profile on screen, through the store's single patch path. */
function patchEditing(patch: Partial<AgentProfile>): void {
  const index = editorIndex.value;
  if (index === null) return;
  updateAgentProfile(index, patch);
}

/** The name, from the field. Committed as typed: it is a draft, not a file write. */
function handleName(value: string | number): void {
  patchEditing({ name: String(value) });
}

/** The description: the field the model reads when it decides to delegate (S7). */
function handleDescription(value: string | number): void {
  patchEditing({ description: String(value) });
}

/** The child's whole system prompt. */
function handleInstructions(value: string | number): void {
  patchEditing({ instructions: String(value) });
}

/**
 * One tool checkbox. The store keeps the list in catalogue order, and a name this
 * build does not know survives the round trip untouched.
 */
function handleTool(toolId: string, next: boolean): void {
  const profile = editing.value;
  if (profile === null) return;
  const tools = next
    ? [...profile.tools, toolId]
    : profile.tools.filter((tool) => tool !== toolId);
  patchEditing({ tools });
}

function handleModel(value: string): void {
  patchEditing({ model: value === NO_CHOICE ? null : value });
}

function handleEffort(value: string): void {
  patchEditing({ effort: value === NO_CHOICE ? null : value });
}

/** The profile's own switch, inside the editor. */
function handleEditingEnabled(next: boolean): void {
  patchEditing({ enabled: next });
}

/** The one write. Rust validates first; its refusal lands in the error line. */
function handleSave(): void {
  void saveAgentsDraft();
}

function handleDiscard(): void {
  void discardAgentsChanges();
}

// The pane mounts when the section is selected, which is also when a hand edit of
// `agents.json` should be picked up. The read is skipped while the draft is dirty;
// the catalogue is read through the shared store, so this costs one call at most.
onMounted(() => {
  void loadAgentsConfig();
  void loadProviderModels();
});
</script>

<template>
  <div class="space-y-6 py-4">
    <div v-if="loading" class="text-sm text-muted-foreground">
      {{ t("common.loading") }}
    </div>

    <template v-else>
      <!-- The read failed: an unparseable file, or a data directory the app cannot
           read. The off state is still rendered below, so the human can write a
           valid store from here instead of being locked out. -->
      <Alert v-if="agentsConfigReadError" variant="destructive">
        <AlertDescription>
          {{
            t("settings.subagents.loadError", {
              message: agentsConfigReadError,
            })
          }}
        </AlertDescription>
      </Alert>

      <section class="space-y-3">
        <div class="min-w-0">
          <h3 class="text-sm font-medium">{{ t("settings.subagents.title") }}</h3>
          <p class="mt-0.5 text-xs text-muted-foreground">
            {{ t("settings.subagents.description") }}
          </p>
        </div>

        <!-- S3/S4: the gate. Off, the extension is never loaded, so the delegation
             tools do not exist for the model. What a child is — same process, at
             most four, no questions, no further delegation — is said here, and the
             next-session line is stated instead of implied. -->
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0">
            <p class="text-sm">{{ t("settings.subagents.enable") }}</p>
            <p class="mt-0.5 text-xs text-muted-foreground">
              {{ t("settings.subagents.enable.description") }}
            </p>
          </div>
          <button
            type="button"
            role="switch"
            :aria-checked="masterEnabled"
            :aria-label="t('settings.subagents.enable')"
            :class="[
              'relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
              masterEnabled ? 'bg-primary' : 'bg-input',
            ]"
            @click="handleMasterToggle(!masterEnabled)"
          >
            <span
              :class="[
                'size-4 rounded-full bg-background shadow-sm transition-transform',
                masterEnabled ? 'translate-x-4.5' : 'translate-x-0.5',
              ]"
              aria-hidden="true"
            />
          </button>
        </div>

        <p class="text-xs text-muted-foreground">
          {{ t("settings.subagents.nextSession") }}
        </p>

        <!-- S6, the second level of the resolution order: the profile's own model,
             then this default, then the session's model. -->
        <div class="flex flex-wrap items-start justify-between gap-3">
          <div class="min-w-0">
            <p class="text-sm">{{ t("settings.subagents.modelDefault") }}</p>
            <p class="mt-0.5 text-xs text-muted-foreground">
              {{ t("settings.subagents.modelDefault.description") }}
            </p>
            <p
              v-if="defaultModelUnknown"
              class="mt-0.5 text-xs text-muted-foreground"
            >
              {{ t("settings.subagents.editor.model.unknown") }}
            </p>
          </div>
          <Select
            :model-value="defaultModel"
            @update:model-value="handleDefaultModel(String($event))"
          >
            <SelectTrigger
              class="w-56"
              :aria-label="t('settings.subagents.modelDefault')"
            >
              <SelectValue
                :placeholder="t('settings.subagents.modelDefault.inherit')"
              />
            </SelectTrigger>
            <SelectContent>
              <SelectItem :value="NO_CHOICE">
                {{ t("settings.subagents.modelDefault.inherit") }}
              </SelectItem>
              <!-- A stored model this account no longer lists keeps its own row,
                   named raw: the field says what the file holds instead of
                   silently looking empty. -->
              <SelectItem v-if="defaultModelUnknown" :value="defaultModel">
                <span class="font-mono text-xs">{{ defaultModel }}</span>
              </SelectItem>
              <SelectGroup
                v-for="group in modelGroups"
                :key="group.provider"
              >
                <SelectLabel>{{ group.name }}</SelectLabel>
                <SelectItem
                  v-for="option in group.options"
                  :key="option.value"
                  :value="option.value"
                >
                  {{ option.label }}
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </div>
      </section>

      <section class="space-y-2">
        <div class="min-w-0">
          <h3 class="text-sm font-medium">
            {{ t("settings.subagents.profiles") }}
          </h3>
          <p class="mt-0.5 text-xs text-muted-foreground">
            {{ t("settings.subagents.profiles.description") }}
          </p>
        </div>

        <!-- S2/S8: with the gate closed the list is dimmed and inert, never
             hidden: the human needs to see what is configured. -->
        <p v-if="!masterEnabled" class="text-xs text-muted-foreground">
          {{ t("settings.subagents.gateClosed") }}
        </p>

        <ul v-if="profiles.length" class="space-y-1">
          <li
            v-for="(profile, index) in profiles"
            :key="index"
            :class="[
              'rounded-md border border-border px-2 py-1.5 transition-opacity',
              masterEnabled ? '' : 'opacity-60',
            ]"
          >
            <div class="flex items-start justify-between gap-3">
              <div class="min-w-0">
                <p class="flex items-baseline gap-2">
                  <span class="min-w-0 truncate text-sm font-medium">
                    {{
                      profile.name.trim() === ""
                        ? t("settings.subagents.editor.newTitle")
                        : profile.name
                    }}
                  </span>
                  <!-- The row states its own status in words: a dimmed list with
                       only switches would leave the state unreadable. -->
                  <span class="shrink-0 text-xs text-muted-foreground">
                    {{
                      profile.enabled
                        ? t("settings.subagents.row.enabled")
                        : t("settings.subagents.row.disabled")
                    }}
                  </span>
                </p>
                <p
                  class="mt-0.5 truncate text-xs text-muted-foreground"
                  :title="profile.description"
                >
                  {{ profile.description }}
                </p>
                <div class="mt-1 flex flex-wrap items-center gap-1">
                  <Badge
                    v-for="tool in orderedTools(profile.tools)"
                    :key="tool"
                    variant="outline"
                    class="font-mono text-xs"
                  >
                    {{ tool }}
                  </Badge>
                  <span
                    v-if="profile.tools.length === 0"
                    class="text-xs text-muted-foreground"
                  >
                    {{ t("settings.subagents.row.noTools") }}
                  </span>
                </div>
                <p class="mt-1 flex flex-wrap gap-x-2 text-xs text-muted-foreground">
                  <span>
                    {{ t("settings.subagents.editor.model") }}:
                    <span class="font-mono">
                      {{
                        profile.model ??
                        t("settings.subagents.row.modelDefault")
                      }}
                    </span>
                  </span>
                  <span>
                    {{ t("settings.subagents.editor.effort") }}:
                    {{ profile.effort ?? t("settings.subagents.row.effortDefault") }}
                  </span>
                </p>
              </div>

              <div class="flex shrink-0 items-center gap-1">
                <button
                  type="button"
                  role="switch"
                  :aria-checked="profile.enabled"
                  :aria-label="
                    t('settings.subagents.row.enable', { name: profile.name })
                  "
                  :disabled="!masterEnabled || agentsConfigSaving"
                  :class="[
                    'relative inline-flex h-5 w-9 shrink-0 items-center rounded-full outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed',
                    profile.enabled && masterEnabled ? 'bg-primary' : 'bg-input',
                  ]"
                  @click="handleProfileToggle(index, !profile.enabled)"
                >
                  <span
                    :class="[
                      'size-4 rounded-full bg-background shadow-sm transition-transform',
                      profile.enabled && masterEnabled
                        ? 'translate-x-4.5'
                        : 'translate-x-0.5',
                    ]"
                    aria-hidden="true"
                  />
                </button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  :disabled="agentsConfigSaving"
                  @click="openEditor(index)"
                >
                  {{ t("settings.subagents.edit") }}
                </Button>
              </div>
            </div>
          </li>
        </ul>
        <!-- S1 of `agent-profiles-defaults.md`: an empty list with the switch on is
             not "nothing configured" — the five built-in templates apply, enabled.
             Saying "no profile yet" here would misdescribe what the children run
             with, so the empty state names the defaults instead. Nothing is added
             to the list on the human's behalf: it stays empty and editable. -->
        <p v-else class="text-xs text-muted-foreground">
          {{
            usingDefaults
              ? t("settings.subagents.profiles.defaultsActive")
              : t("settings.subagents.profiles.empty")
          }}
        </p>

        <div class="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            :disabled="agentsConfigSaving"
            @click="handleNewProfile"
          >
            <PlusIcon data-icon="inline-start" />
            {{ t("settings.subagents.add") }}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            :disabled="agentsConfigSaving"
            :aria-expanded="templatesOpen"
            @click="templatesOpen = !templatesOpen"
          >
            {{
              templatesOpen
                ? t("settings.subagents.templates.close")
                : t("settings.subagents.templates.open")
            }}
          </Button>
        </div>

        <!-- S2: the templates, under demand. Each arrives fully written and
             disabled; the click adds it to the draft, and the enable switch is
             still the human's. Nothing is added by rendering this block. -->
        <div v-if="templatesOpen" class="space-y-2">
          <div class="min-w-0">
            <h4 class="text-sm font-medium">
              {{ t("settings.subagents.templates.title") }}
            </h4>
            <p class="mt-0.5 text-xs text-muted-foreground">
              {{ t("settings.subagents.templates.description") }}
            </p>
          </div>

          <ul class="space-y-1 rounded-md border border-border p-1">
            <li
              v-for="template in AGENT_PROFILE_TEMPLATES"
              :key="template.id"
              class="flex items-start justify-between gap-3 rounded-sm px-1.5 py-1"
            >
              <div class="min-w-0">
                <p class="flex flex-wrap items-baseline gap-2">
                  <span class="text-sm font-medium">
                    {{ template.profile.name }}
                  </span>
                  <span class="text-xs text-muted-foreground">
                    {{ t(template.purposeKey) }}
                  </span>
                </p>
                <p
                  class="mt-0.5 truncate text-xs text-muted-foreground"
                  :title="template.profile.description"
                >
                  {{ template.profile.description }}
                </p>
                <div class="mt-1 flex flex-wrap items-center gap-1">
                  <Badge
                    v-for="tool in template.profile.tools"
                    :key="tool"
                    variant="outline"
                    class="font-mono text-xs"
                  >
                    {{ tool }}
                  </Badge>
                  <span class="text-xs text-muted-foreground">
                    {{ template.profile.effort }} ·
                    {{ t("settings.subagents.templates.model") }}
                  </span>
                </div>
              </div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                class="shrink-0"
                :disabled="
                  agentsConfigSaving || hasProfileNamed(template.profile.name)
                "
                @click="handleAddTemplate(template)"
              >
                {{
                  hasProfileNamed(template.profile.name)
                    ? t("settings.subagents.templates.added")
                    : t("settings.subagents.templates.add", {
                        name: template.profile.name,
                      })
                }}
              </Button>
            </li>
          </ul>
        </div>
      </section>

      <!-- Rust's refusal, verbatim: it names the profile and the field. The
           sentence is the whole message, so it is not wrapped in a summary. -->
      <Alert v-if="agentsConfigSaveError" variant="destructive">
        <AlertDescription>{{ agentsConfigSaveError }}</AlertDescription>
      </Alert>

      <div class="flex flex-wrap items-center justify-between gap-3">
        <p class="text-xs text-muted-foreground">
          <template v-if="agentsConfigSaving">
            {{ t("settings.subagents.saving") }}
          </template>
          <template v-else-if="agentsConfigDirty">
            {{ t("settings.subagents.dirty") }}
          </template>
          <template v-else-if="agentsConfigSaved">
            {{ t("settings.subagents.saved") }}
          </template>
          <template v-else>
            {{ t("settings.subagents.nextSession") }}
          </template>
        </p>
        <div class="flex items-center gap-2">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            :disabled="!agentsConfigDirty || agentsConfigSaving"
            @click="handleDiscard"
          >
            {{ t("settings.subagents.discard") }}
          </Button>
          <Button
            type="button"
            size="sm"
            :disabled="!agentsConfigDirty || agentsConfigSaving"
            @click="handleSave"
          >
            {{
              agentsConfigSaving
                ? t("settings.subagents.saving")
                : t("settings.subagents.save")
            }}
          </Button>
        </div>
      </div>
    </template>

    <!-- The editor, as a dialog like the provider one: the list stays a list
         instead of becoming a stack of forms. Every field writes to the draft
         through one patch path, so there is no per-field copy to lose. -->
    <Dialog :open="editorOpen" @update:open="handleEditorOpenChange">
      <DialogContent
        v-if="editing"
        class="max-h-[min(85vh,44rem)] overflow-y-auto sm:max-w-xl"
      >
        <DialogHeader>
          <DialogTitle>
            {{
              editing.name.trim() === ""
                ? t("settings.subagents.editor.newTitle")
                : t("settings.subagents.editor.title", { name: editing.name })
            }}
          </DialogTitle>
          <DialogDescription>
            {{ t("settings.subagents.editor.description") }}
          </DialogDescription>
        </DialogHeader>

        <div class="space-y-4">
          <div class="space-y-1.5">
            <label class="text-sm" for="subagent-profile-name">
              {{ t("settings.subagents.editor.name") }}
            </label>
            <Input
              id="subagent-profile-name"
              :model-value="editing.name"
              type="text"
              autocomplete="off"
              spellcheck="false"
              class="h-8 text-sm"
              :placeholder="t('settings.subagents.editor.name.placeholder')"
              @update:model-value="handleName"
            />
            <p class="text-xs text-muted-foreground">
              {{ t("settings.subagents.editor.name.hint") }}
            </p>
          </div>

          <!-- The field the model reads when it decides whether to delegate, so it
               is the one the pane explains most. -->
          <div class="space-y-1.5">
            <label class="text-sm" for="subagent-profile-description">
              {{ t("settings.subagents.editor.profileDescription") }}
            </label>
            <Textarea
              id="subagent-profile-description"
              :model-value="editing.description"
              rows="3"
              :placeholder="
                t('settings.subagents.editor.profileDescription.placeholder')
              "
              @update:model-value="handleDescription"
            />
            <p class="text-xs text-muted-foreground">
              {{ t("settings.subagents.editor.profileDescription.hint") }}
            </p>
          </div>

          <div class="space-y-1.5">
            <p class="text-sm">{{ t("settings.subagents.editor.tools") }}</p>
            <div
              class="max-h-56 space-y-0.5 overflow-y-auto rounded-md border border-border p-1"
            >
              <label
                v-for="tool in AGENT_TOOLS"
                :key="tool.id"
                class="flex items-start gap-2 rounded-sm px-1.5 py-1 text-sm hover:bg-muted"
              >
                <input
                  type="checkbox"
                  class="mt-0.5 size-4 shrink-0 accent-primary"
                  :checked="editing.tools.includes(tool.id)"
                  @change="
                    handleTool(
                      tool.id,
                      ($event.target as HTMLInputElement).checked,
                    )
                  "
                />
                <span class="min-w-0">
                  <span class="font-mono text-xs">{{ tool.id }}</span>
                  <span class="ml-2 text-xs text-muted-foreground">
                    {{ t(tool.descriptionKey) }}
                  </span>
                </span>
              </label>
            </div>
            <p class="text-xs text-muted-foreground">
              {{ t("settings.subagents.editor.tools.hint") }}
            </p>
            <!-- Names the file carries that this build does not list: kept, shown,
                 never dropped by ticking another box. -->
            <p v-if="unknownTools.length" class="text-xs text-muted-foreground">
              {{ t("settings.subagents.editor.tools.unknown") }}
              <span class="font-mono">{{ unknownTools.join(", ") }}</span>
            </p>
          </div>

          <div class="space-y-1.5">
            <label class="text-sm" for="subagent-profile-model">
              {{ t("settings.subagents.editor.model") }}
            </label>
            <Select
              :model-value="editingModel"
              @update:model-value="handleModel(String($event))"
            >
              <SelectTrigger
                id="subagent-profile-model"
                class="w-full"
                :aria-label="t('settings.subagents.editor.model')"
              >
                <SelectValue
                  :placeholder="t('settings.subagents.row.modelDefault')"
                />
              </SelectTrigger>
              <SelectContent>
                <SelectItem :value="NO_CHOICE">
                  {{ t("settings.subagents.row.modelDefault") }}
                </SelectItem>
                <SelectItem v-if="editingModelUnknown" :value="editingModel">
                  <span class="font-mono text-xs">{{ editingModel }}</span>
                </SelectItem>
                <SelectGroup
                  v-for="group in modelGroups"
                  :key="group.provider"
                >
                  <SelectLabel>{{ group.name }}</SelectLabel>
                  <SelectItem
                    v-for="option in group.options"
                    :key="option.value"
                    :value="option.value"
                  >
                    {{ option.label }}
                  </SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
            <p
              v-if="editingModelUnknown"
              class="text-xs text-muted-foreground"
            >
              {{ t("settings.subagents.editor.model.unknown") }}
            </p>
            <p class="text-xs text-muted-foreground">
              {{ t("settings.subagents.editor.model.hint") }}
            </p>
          </div>

          <div class="space-y-1.5">
            <label class="text-sm" for="subagent-profile-effort">
              {{ t("settings.subagents.editor.effort") }}
            </label>
            <Select
              :model-value="editingEffort"
              @update:model-value="handleEffort(String($event))"
            >
              <SelectTrigger
                id="subagent-profile-effort"
                class="w-full"
                :aria-label="t('settings.subagents.editor.effort')"
              >
                <SelectValue
                  :placeholder="t('settings.subagents.editor.effort.inherit')"
                />
              </SelectTrigger>
              <SelectContent>
                <SelectItem :value="NO_CHOICE">
                  {{ t("settings.subagents.editor.effort.inherit") }}
                </SelectItem>
                <SelectItem
                  v-for="level in THINKING_LEVELS"
                  :key="level"
                  :value="level"
                >
                  {{ level }}
                </SelectItem>
              </SelectContent>
            </Select>
            <p class="text-xs text-muted-foreground">
              {{ t("settings.subagents.editor.effort.hint") }}
            </p>
          </div>

          <div class="space-y-1.5">
            <label class="text-sm" for="subagent-profile-instructions">
              {{ t("settings.subagents.editor.instructions") }}
            </label>
            <Textarea
              id="subagent-profile-instructions"
              :model-value="editing.instructions"
              rows="8"
              @update:model-value="handleInstructions"
            />
            <p class="text-xs text-muted-foreground">
              {{ t("settings.subagents.editor.instructions.hint") }}
            </p>
          </div>

          <div class="flex items-start justify-between gap-4">
            <div class="min-w-0">
              <p class="text-sm">{{ t("settings.subagents.editor.enable") }}</p>
              <p class="mt-0.5 text-xs text-muted-foreground">
                {{ t("settings.subagents.editor.enable.hint") }}
              </p>
            </div>
            <button
              type="button"
              role="switch"
              :aria-checked="editing.enabled"
              :aria-label="t('settings.subagents.editor.enable')"
              :class="[
                'relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                editing.enabled ? 'bg-primary' : 'bg-input',
              ]"
              @click="handleEditingEnabled(!editing.enabled)"
            >
              <span
                :class="[
                  'size-4 rounded-full bg-background shadow-sm transition-transform',
                  editing.enabled ? 'translate-x-4.5' : 'translate-x-0.5',
                ]"
                aria-hidden="true"
              />
            </button>
          </div>
        </div>

        <DialogFooter class="justify-between">
          <div class="flex flex-col items-start gap-1">
            <Button
              type="button"
              variant="outline"
              size="sm"
              :disabled="agentsConfigSaving"
              @click="handleRemoveProfile"
            >
              {{ t("settings.subagents.editor.remove") }}
            </Button>
            <p class="text-xs text-muted-foreground">
              {{ t("settings.subagents.editor.remove.hint") }}
            </p>
          </div>
          <DialogClose as-child>
            <Button type="button" variant="outline">
              {{ t("common.close") }}
            </Button>
          </DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
