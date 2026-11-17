<script setup lang="ts">
/**
 * Add-server form (S1 of `odd/tasks/tool-drawer-and-mcp.md`).
 *
 * Two ways in, one write out:
 *
 * - the CURATED CATALOGUE at the top (`src/lib/mcp-catalogue.ts`) fills the form.
 *   A template is data, never code: picking one writes values into the fields
 *   below, shows what will run, and stops there. Nothing is installed, and
 *   nothing is submitted — the human still presses "Add server".
 * - the fields themselves, for a server no template covers.
 *
 * The single write is `mcp_add`: Rust turns the draft into `pi mcp add … -- cmd
 * args` (or `--url`), which is what actually edits `mcp.json`. The answer is the
 * refreshed list, so the pane renders what the file now holds.
 *
 * Validation mirrors the rules pi enforces in `mcp.json`, so a typo costs a
 * field instead of a failed write: a server name is letters, digits, `_` and `-`
 * (pi's "Configuration rules"), a stdio `command` is ONE executable and never a
 * shell string, an http endpoint has to be an absolute `http(s)` URL, a header or
 * env row needs a name, and `--bearer-token-env-var` takes a variable NAME. Rust
 * validates again and its rejection lands on this form's one error line, with
 * every typed value kept.
 *
 * Secrets: no field here is prefilled by Rust (there is nothing to prefill — the
 * file's values are never read back), and the values a human types this time are
 * written to pi's plain-text `mcp.json` and then forgotten by the UI.
 */
import { computed, ref } from "vue";
import { PlusIcon, XIcon } from "@lucide/vue";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useI18n } from "@/lib/i18n";
import { MCP_CATALOGUE, type McpCatalogueEntry } from "@/lib/mcp-catalogue";
import { commandErrorMessage, mcpAdd } from "@/contracts/commands";
import type {
  McpAddDraft,
  McpKeyValue,
  McpScope,
  McpServerSummary,
  McpTransport,
} from "@/contracts/types";

const emit = defineEmits<{
  saved: [servers: McpServerSummary[]];
  cancel: [];
}>();

const { t } = useI18n();

/**
 * pi's and Rust's name rule, in one place: 1-64 characters from
 * `[A-Za-z0-9._-]`. Rust validates it again (`mcp::validate_name`), so a name
 * this form accepts is never one the write rejects for its shape.
 */
const SERVER_NAME = /^[A-Za-z0-9._-]+$/;
const NAME_MAX_LENGTH = 64;

/**
 * An env or header name: non-empty and free of `=` and whitespace, which is what
 * `mcp::validate_key` enforces (the `KEY` half of `--env KEY=VALUE`).
 */
const KEY_FORBIDDEN = /[=\s]/;

/** An environment variable NAME, which is what `--bearer-token-env-var` takes. */
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** One editable `key=value` row of the env or the headers block. */
interface PairRow {
  key: string;
  value: string;
}

/** The choices the two selects offer, with the label each one shows. */
const SCOPE_OPTIONS: McpScope[] = ["global", "project"];
const TRANSPORT_OPTIONS: McpTransport[] = ["stdio", "http"];

const name = ref("");
const scope = ref<McpScope>("global");
const transport = ref<McpTransport>("stdio");
/** stdio: the executable. Never a shell string, so it holds no spaces. */
const command = ref("");
/** stdio: one argument per line, because pi's `args` is an array. */
const argsText = ref("");
/** http: the streamable HTTP endpoint. */
const url = ref("");
const envRows = ref<PairRow[]>([]);
const headerRows = ref<PairRow[]>([]);
/** http: the variable pi reads for `Authorization: Bearer ${NAME}`. */
const bearerEnvVar = ref("");

/** The template that last filled this form, `null` when none did. */
const pickedId = ref<string | null>(null);

const saving = ref(false);
const error = ref<string | null>(null);

/** The template behind the "what this template runs" line, if one was picked. */
const pickedEntry = computed<McpCatalogueEntry | null>(() => {
  if (pickedId.value === null) return null;
  return MCP_CATALOGUE.find((entry) => entry.id === pickedId.value) ?? null;
});

/** The arguments the form will send: one trimmed line each, blanks dropped. */
const args = computed<string[]>(() =>
  argsText.value
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== ""),
);

/**
 * A pair list as the draft carries it: rows with no name are dropped (an empty
 * row is not an entry), and a row with a value but no name is caught by
 * `pairError` before this ever runs. The KEY is trimmed because Rust trims it;
 * the value is kept verbatim — it may be `${NAME}`, `!command`, or a credential
 * whose spaces matter.
 */
function filledPairs(rows: PairRow[]): McpKeyValue[] {
  return rows
    .map((row) => ({ key: row.key.trim(), value: row.value }))
    .filter((row) => row.key !== "");
}

const env = computed(() => filledPairs(envRows.value));
const headers = computed(() => filledPairs(headerRows.value));

/** The name is required and has to be one pi accepts. */
const nameValid = computed(() => {
  const value = name.value.trim();
  return (
    value.length > 0 &&
    value.length <= NAME_MAX_LENGTH &&
    SERVER_NAME.test(value)
  );
});

/** A stdio command is one executable: whitespace means it is a shell string. */
const commandValid = computed(() => {
  const value = command.value.trim();
  return value !== "" && !/\s/.test(value);
});

/** An http endpoint is an absolute `http(s)` URL. */
const urlValid = computed(() => {
  const value = url.value.trim();
  if (value === "") return false;
  try {
    const parsed = new URL(value);
    return parsed.protocol === "http:" || parsed.protocol === "https:";
  } catch {
    return false;
  }
});

/** The bearer variable is optional; when present it is a variable name. */
const bearerValid = computed(() => {
  const value = bearerEnvVar.value.trim();
  return value === "" || ENV_NAME.test(value);
});

/**
 * Every row is either empty (dropped) or carries a usable name: no `=` and no
 * whitespace, the two things that would break the `KEY=VALUE` pi is handed.
 */
const pairsValid = computed(() => {
  const rows = transport.value === "stdio" ? envRows.value : headerRows.value;
  return rows.every((row) => {
    const key = row.key.trim();
    if (key === "") return row.value.trim() === "";
    return !KEY_FORBIDDEN.test(key);
  });
});

const canSave = computed(() => {
  if (saving.value) return false;
  if (!nameValid.value || !pairsValid.value) return false;
  return transport.value === "stdio"
    ? commandValid.value
    : urlValid.value && bearerValid.value;
});

/**
 * What pi is about to be told, on one line, updated as the fields change. It is
 * the honest version of the catalogue's note: the note describes the template,
 * this describes the form — including whatever the human edited.
 */
const preview = computed<string>(() => {
  if (transport.value === "stdio") {
    return [command.value.trim(), ...args.value].join(" ").trim();
  }
  return url.value.trim();
});

/** The name problem, shown under the field once something has been typed. */
const nameError = computed<string | null>(() =>
  name.value.trim() === "" || nameValid.value
    ? null
    : t("settings.mcp.form.nameInvalid"),
);

const commandError = computed<string | null>(() =>
  command.value.trim() === "" || commandValid.value
    ? null
    : t("settings.mcp.form.commandInvalid"),
);

const urlError = computed<string | null>(() =>
  url.value.trim() === "" || urlValid.value
    ? null
    : t("settings.mcp.form.urlInvalid"),
);

const bearerError = computed<string | null>(() =>
  bearerValid.value ? null : t("settings.mcp.form.bearerEnvVarInvalid"),
);

const pairError = computed<string | null>(() => {
  if (pairsValid.value) return null;
  const rows = transport.value === "stdio" ? envRows.value : headerRows.value;
  const missing = rows.some(
    (row) => row.key.trim() === "" && row.value.trim() !== "",
  );
  return missing
    ? t("settings.mcp.form.pairKeyMissing")
    : t("settings.mcp.form.pairKeyInvalid");
});

function addRow(rows: PairRow[]) {
  rows.push({ key: "", value: "" });
}

function removeRow(rows: PairRow[], index: number) {
  rows.splice(index, 1);
}

/**
 * Fills the form from one template. It is a one-time copy, not a binding: no
 * watcher re-applies a template afterwards, so every value stays editable. Two
 * things are deliberately NOT touched — the scope (a human choice about where the
 * entry lives) and the typed values of the template's required keys (the form has
 * just replaced them, and inventing a secret is not possible anyway).
 *
 * `id` doubles as the server name, because it is the name the template was
 * checked under and it already satisfies pi's rule.
 */
function applyTemplate(entry: McpCatalogueEntry) {
  pickedId.value = entry.id;
  name.value = entry.id;
  transport.value = entry.transport;
  error.value = null;
  if (entry.transport === "stdio") {
    command.value = entry.command ?? "";
    argsText.value = (entry.args ?? []).join("\n");
    url.value = "";
    headerRows.value = [];
    bearerEnvVar.value = "";
    envRows.value = entry.requiredKeys.map((key) => ({ key, value: "" }));
    return;
  }
  url.value = entry.url ?? "";
  command.value = "";
  argsText.value = "";
  envRows.value = [];
  headerRows.value = [];
  bearerEnvVar.value = "";
  // Required keys of an http template are headers, never `Authorization` (pi
  // owns that through OAuth or `--bearer-token-env-var`), so they are named rows
  // whose value the human fills with `${NAME}` or `!command`.
  headerRows.value = entry.requiredKeys.map((key) => ({ key, value: "" }));
}

/**
 * A transport switch drops the half that no longer applies, so a stray command
 * can never ride along in an http draft (Rust would reject it) and vice-versa.
 */
function handleTransportChange(next: string) {
  transport.value = next === "http" ? "http" : "stdio";
  error.value = null;
}

/**
 * S1: the one write. The draft carries exactly one half — the stdio fields or the
 * http fields — so what is sent matches the transport the form shows. Rust
 * validates it again and answers with the refreshed list; a rejection keeps every
 * typed value and shows Rust's reason.
 */
async function handleSave() {
  if (!canSave.value) return;
  saving.value = true;
  error.value = null;
  const draft: McpAddDraft =
    transport.value === "stdio"
      ? {
          name: name.value.trim(),
          scope: scope.value,
          transport: "stdio",
          command: command.value.trim(),
          args: args.value,
          env: env.value,
          url: null,
          headers: [],
          bearerTokenEnvVar: null,
        }
      : {
          name: name.value.trim(),
          scope: scope.value,
          transport: "http",
          command: null,
          args: [],
          env: [],
          url: url.value.trim(),
          headers: headers.value,
          bearerTokenEnvVar: bearerEnvVar.value.trim() || null,
        };
  try {
    const servers = await mcpAdd(draft);
    emit("saved", servers);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <form
    class="space-y-4 rounded-lg border border-border p-3"
    @submit.prevent="handleSave"
  >
    <div class="flex items-start justify-between gap-3">
      <div class="min-w-0">
        <h4 class="text-sm font-medium">
          {{ t("settings.mcp.form.title") }}
        </h4>
        <p class="mt-0.5 text-xs text-muted-foreground">
          {{ t("settings.mcp.form.description") }}
        </p>
      </div>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        :disabled="saving"
        @click="emit('cancel')"
      >
        {{ t("common.cancel") }}
      </Button>
    </div>

    <!-- The catalogue sits above the fields it fills, so the order reads "pick a
         server, then adjust it". A template never submits: the button only copies
         values and shows what it would run. -->
    <section class="space-y-1.5">
      <div class="min-w-0">
        <h5 class="text-xs font-medium">
          {{ t("settings.mcp.form.catalog.title") }}
        </h5>
        <p class="mt-0.5 text-xs text-muted-foreground">
          {{ t("settings.mcp.form.catalog.description") }}
        </p>
      </div>

      <ul class="space-y-0.5">
        <li v-for="entry in MCP_CATALOGUE" :key="entry.id">
          <button
            type="button"
            :aria-pressed="pickedId === entry.id"
            :class="[
              'w-full rounded-md px-2 py-1.5 text-left outline-hidden transition-colors hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring',
              pickedId === entry.id ? 'bg-muted' : '',
            ]"
            @click="applyTemplate(entry)"
          >
            <span class="flex items-center gap-2">
              <span class="min-w-0 truncate text-sm font-medium">
                {{ entry.label }}
              </span>
              <Badge variant="outline" class="shrink-0">
                {{ entry.transport }}
              </Badge>
            </span>
            <span class="mt-0.5 block text-xs text-muted-foreground">
              {{ entry.description }}
            </span>
          </button>
        </li>
      </ul>
    </section>

    <!-- What this template is, in the template's own words. It describes the
         template, not the form: the live line further down describes the form. -->
    <div
      v-if="pickedEntry"
      class="space-y-1 rounded-md border border-border bg-muted/40 p-2"
    >
      <p class="text-xs">
        {{ pickedEntry.note }}
      </p>
      <p
        v-if="pickedEntry.requiredKeys.length"
        class="text-xs text-muted-foreground"
      >
        {{
          t("settings.mcp.form.catalog.requiredKeys", {
            keys: pickedEntry.requiredKeys.join(", "),
          })
        }}
      </p>
    </div>

    <div class="grid gap-3 sm:grid-cols-3">
      <div class="space-y-1 sm:col-span-1">
        <label for="mcp-name" class="text-xs font-medium">
          {{ t("settings.mcp.form.name") }}
        </label>
        <Input
          id="mcp-name"
          v-model="name"
          autocomplete="off"
          spellcheck="false"
          :aria-invalid="nameError !== null"
          :placeholder="t('settings.mcp.form.namePlaceholder')"
        />
        <p class="text-xs text-destructive" v-if="nameError">
          {{ nameError }}
        </p>
        <p v-else class="text-xs text-muted-foreground">
          {{ t("settings.mcp.form.name.description") }}
        </p>
      </div>

      <div class="space-y-1">
        <span id="mcp-scope-label" class="text-xs font-medium">
          {{ t("settings.mcp.form.scope") }}
        </span>
        <Select v-model="scope">
          <SelectTrigger class="w-full" aria-labelledby="mcp-scope-label">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem
              v-for="option in SCOPE_OPTIONS"
              :key="option"
              :value="option"
            >
              {{ t(`settings.mcp.scope.${option}`) }}
            </SelectItem>
          </SelectContent>
        </Select>
        <p class="text-xs text-muted-foreground">
          {{ t("settings.mcp.form.scope.description") }}
        </p>
      </div>

      <div class="space-y-1">
        <span id="mcp-transport-label" class="text-xs font-medium">
          {{ t("settings.mcp.form.transport") }}
        </span>
        <Select
          :model-value="transport"
          @update:model-value="handleTransportChange(String($event))"
        >
          <SelectTrigger class="w-full" aria-labelledby="mcp-transport-label">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem
              v-for="option in TRANSPORT_OPTIONS"
              :key="option"
              :value="option"
            >
              {{ option }}
            </SelectItem>
          </SelectContent>
        </Select>
        <p class="text-xs text-muted-foreground">
          {{ t("settings.mcp.form.transport.description") }}
        </p>
      </div>
    </div>

    <!-- stdio: one executable plus its arguments, and the environment it needs. -->
    <template v-if="transport === 'stdio'">
      <div class="space-y-1">
        <label for="mcp-command" class="text-xs font-medium">
          {{ t("settings.mcp.form.command") }}
        </label>
        <Input
          id="mcp-command"
          v-model="command"
          autocomplete="off"
          spellcheck="false"
          class="font-mono text-sm"
          :aria-invalid="commandError !== null"
          :placeholder="t('settings.mcp.form.commandPlaceholder')"
        />
        <p v-if="commandError" class="text-xs text-destructive">
          {{ commandError }}
        </p>
        <p v-else class="text-xs text-muted-foreground">
          {{ t("settings.mcp.form.command.description") }}
        </p>
      </div>

      <div class="space-y-1">
        <label for="mcp-args" class="text-xs font-medium">
          {{ t("settings.mcp.form.args") }}
        </label>
        <Textarea
          id="mcp-args"
          v-model="argsText"
          rows="3"
          spellcheck="false"
          class="font-mono text-sm"
          :placeholder="t('settings.mcp.form.argsPlaceholder')"
        />
        <p class="text-xs text-muted-foreground">
          {{ t("settings.mcp.form.args.description") }}
        </p>
      </div>

      <div class="space-y-1.5">
        <div class="flex items-center justify-between gap-2">
          <span class="text-xs font-medium">
            {{ t("settings.mcp.form.env") }}
          </span>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            class="h-6 px-2 text-xs"
            @click="addRow(envRows)"
          >
            <PlusIcon data-icon="inline-start" />
            {{ t("settings.mcp.form.addPair") }}
          </Button>
        </div>
        <p class="text-xs text-muted-foreground">
          {{ t("settings.mcp.form.env.description") }}
        </p>
        <div
          v-for="(row, index) in envRows"
          :key="`env-${index}`"
          class="flex items-center gap-2"
        >
          <Input
            v-model="row.key"
            class="h-8 min-w-0 flex-1 font-mono text-sm"
            autocomplete="off"
            spellcheck="false"
            :aria-label="t('settings.mcp.form.pairKey')"
            :placeholder="t('settings.mcp.form.pairKey')"
          />
          <Input
            v-model="row.value"
            class="h-8 min-w-0 flex-1 font-mono text-sm"
            autocomplete="off"
            spellcheck="false"
            :aria-label="t('settings.mcp.form.pairValue')"
            :placeholder="t('settings.mcp.form.pairValue')"
          />
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            :aria-label="t('settings.mcp.form.removePair')"
            @click="removeRow(envRows, index)"
          >
            <XIcon />
          </Button>
        </div>
        <p v-if="pairError" class="text-xs text-destructive">
          {{ pairError }}
        </p>
      </div>
    </template>

    <!-- http: the endpoint, its headers, and the variable a bearer token lives
         in. pi owns OAuth itself, so there is no token field here. -->
    <template v-else>
      <div class="space-y-1">
        <label for="mcp-url" class="text-xs font-medium">
          {{ t("settings.mcp.form.url") }}
        </label>
        <Input
          id="mcp-url"
          v-model="url"
          autocomplete="off"
          spellcheck="false"
          class="font-mono text-sm"
          :aria-invalid="urlError !== null"
          :placeholder="t('settings.mcp.form.urlPlaceholder')"
        />
        <p v-if="urlError" class="text-xs text-destructive">
          {{ urlError }}
        </p>
        <p v-else class="text-xs text-muted-foreground">
          {{ t("settings.mcp.form.url.description") }}
        </p>
      </div>

      <div class="space-y-1.5">
        <div class="flex items-center justify-between gap-2">
          <span class="text-xs font-medium">
            {{ t("settings.mcp.form.headers") }}
          </span>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            class="h-6 px-2 text-xs"
            @click="addRow(headerRows)"
          >
            <PlusIcon data-icon="inline-start" />
            {{ t("settings.mcp.form.addPair") }}
          </Button>
        </div>
        <p class="text-xs text-muted-foreground">
          {{ t("settings.mcp.form.headers.description") }}
        </p>
        <div
          v-for="(row, index) in headerRows"
          :key="`header-${index}`"
          class="flex items-center gap-2"
        >
          <Input
            v-model="row.key"
            class="h-8 min-w-0 flex-1 font-mono text-sm"
            autocomplete="off"
            spellcheck="false"
            :aria-label="t('settings.mcp.form.pairKey')"
            :placeholder="t('settings.mcp.form.pairKey')"
          />
          <Input
            v-model="row.value"
            class="h-8 min-w-0 flex-1 font-mono text-sm"
            autocomplete="off"
            spellcheck="false"
            :aria-label="t('settings.mcp.form.pairValue')"
            :placeholder="t('settings.mcp.form.pairValue')"
          />
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            :aria-label="t('settings.mcp.form.removePair')"
            @click="removeRow(headerRows, index)"
          >
            <XIcon />
          </Button>
        </div>
        <p v-if="pairError" class="text-xs text-destructive">
          {{ pairError }}
        </p>
      </div>

      <div class="space-y-1">
        <label for="mcp-bearer" class="text-xs font-medium">
          {{ t("settings.mcp.form.bearerEnvVar") }}
        </label>
        <Input
          id="mcp-bearer"
          v-model="bearerEnvVar"
          autocomplete="off"
          spellcheck="false"
          class="font-mono text-sm"
          :aria-invalid="bearerError !== null"
          :placeholder="t('settings.mcp.form.bearerEnvVarPlaceholder')"
        />
        <p v-if="bearerError" class="text-xs text-destructive">
          {{ bearerError }}
        </p>
        <p v-else class="text-xs text-muted-foreground">
          {{ t("settings.mcp.form.bearerEnvVar.description") }}
        </p>
      </div>
    </template>

    <!-- What pi is about to be told, from the form itself. It exists so nothing
         is ever added sight unseen, even when a template was edited. -->
    <p v-if="preview" class="text-xs text-muted-foreground">
      {{ t("settings.mcp.form.preview", { target: preview }) }}
    </p>

    <Alert v-if="error" variant="destructive">
      <AlertDescription>{{ error }}</AlertDescription>
    </Alert>

    <div class="flex flex-wrap items-center justify-between gap-2">
      <p v-if="!canSave && !saving" class="text-xs text-muted-foreground">
        {{ t("settings.mcp.form.required") }}
      </p>
      <span v-else />
      <Button type="submit" size="sm" :disabled="!canSave">
        {{
          saving ? t("settings.mcp.form.saving") : t("settings.mcp.form.save")
        }}
      </Button>
    </div>
  </form>
</template>
