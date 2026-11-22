<script setup lang="ts">
/**
 * Settings dialog (specs S1-S5 of `odd/tasks/settings-dialog.md`, S1/S2/S4 of
 * `odd/tasks/i18n-and-settings-polish.md`).
 *
 * Two panes, like a preferences window: a header with the title and the close
 * control; a left nav with the two sections that have a real backend today
 * (General, Providers); and a scrolling right pane of rows — label and one-line
 * description on the left, control on the right.
 *
 * S1 of `odd/tasks/providers-pane.md`: the second pane is Providers, not Models.
 * It lists the providers pi knows and marks the active one (`settings.provider`);
 * clicking a row persists that choice. The model selector moved out of this
 * dialog: the chat composer owns it and already writes `settings.model`.
 *
 * S5 of the same task: the custom-provider form is gated by
 * `VITE_ENABLE_CUSTOM_PROVIDERS`, a build-time switch like the rest of Nabla's
 * env configuration — and it is ON BY DEFAULT: only an explicit `0`/`false`/`off`
 * hides the block. The subscription accounts block above the list is gated the
 * same way by `VITE_ENABLE_PROVIDER_LOGIN` (S5 of `pi-cli-bridge.md`), read with
 * the same helper and with the same default: a build that sets neither shows
 * both, so a flag nobody exported never hides a working feature.
 *
 * S3/S4 of `pi-cli-bridge.md`: those accounts are pi's own subscription login
 * (device code, live output, pasted code) and logout — see `AccountsSection.vue`.
 * The pane still owns only which provider the next session uses; the account
 * state never rewrites the settings.
 *
 * S3/S4: every configured row also offers the api key of that provider (typed
 * inline, never prefilled, written into pi's plain-text `models.json`) and its
 * removal behind a confirmation. Removing the active provider falls back to the
 * first remaining one; with none left the settings stay untouched and the pane
 * says so.
 *
 * S2 of `providers-pane-refinements.md`: opening this pane is a file read and
 * nothing else. The provider list renders as soon as `list_providers()` answers
 * (milliseconds); the account badges are a background fill owned by
 * `AccountsSection.vue`, cached for the session, and never awaited here.
 *
 * S3 of the same task: the subscription accounts above the list are exactly the
 * four subscription providers and nothing else — a configured endpoint is a
 * Providers row, never an account.
 *
 * S4 of the same task: the models of a provider are no longer a standalone block
 * of the pane. Every row offers "Edit", which opens one dialog with that
 * provider's details and its models editor, so providers stay a list instead of
 * becoming a stack of sections. The login and custom-provider flags keep their
 * meaning.
 *
 * Catalogue models: the pane also reads `list_provider_models`, because two of the
 * things it shows are not in `list_providers`. A provider only pi's catalogue knows
 * — the usual result of signing in to a subscription — gets a row of its own below
 * the configured list, with the two actions that read it (activate, edit) and
 * without the two that write `models.json` (api key, remove), because there is no
 * such entry. The model count of every row is the number the editor will list, so a
 * catalogue an account just populated is visible here instead of behind a stale
 * summary. A finished sign-in re-reads both lists (`AccountsSection` emits it) and
 * bumps `editorRefresh`, so an open editor shows the new rows without reopening.
 *
 * S1 of `i18n-and-settings-polish.md`: the pane has no rule between single rows.
 * Blocks are separated by spacing plus ONE rule, between the accounts block and
 * the provider list; the custom-provider form belongs to the list it adds to, so
 * it needs no rule of its own.
 *
 * S4: the configuration-file action exists exactly once, at the end of the
 * General pane; the header no longer repeats it.
 *
 * S5 of `settings-dialog.md`: the nav lists only the sections the app can drive
 * — permissions, font size, work details and code view have no backend yet, so
 * they are not painted as if they worked. Shortcuts left that list when the zoom
 * chords landed: the General pane shows them read-only, because a chord is not a
 * preference (`src/lib/zoom.ts` owns the listener and the labels). S1 of
 * `tool-drawer-and-mcp.md` added the third one: MCP, a pane of its own rather
 * than a block inside Providers, because servers and providers are two different
 * things pi reads from two different files.
 *
 * The MCP pane mounts only while it is selected (the content is a `v-if`/`v-else`
 * chain), so opening the dialog costs nothing for it: `McpSection.vue` reads
 * pi's MCP configuration on its own mount, and this component knows nothing about
 * that file.
 *
 * S2 of `agent-profiles.md` added the fourth: Subagents, a pane of its own because a
 * profile is not a preference of the chat but a stored definition the agent
 * delegates to. `SubagentsSection.vue` owns its read, its draft and its single
 * write (`save_agents_config`), so this component only decides that it is on
 * screen — the same arrangement as MCP. Its draft lives at module scope in
 * `src/lib/agent-profiles.ts` precisely because this chain unmounts the pane
 * whenever another section is selected.
 *
 * Preferences live in `settings.json`; this component owns the get/save flow
 * (`getSettings`/`saveSettings`) and applies the theme through the shared
 * `useAppearance().setMode`, the only path that touches the palette. Every
 * user-visible string comes from the catalogue in `src/lib/i18n.ts` (S2).
 */
import { computed, defineAsyncComponent, ref, watch, type Ref } from "vue";
import {
  BlocksIcon,
  BotIcon,
  BrainIcon,
  CheckIcon,
  ExternalLinkIcon,
  MonitorIcon,
  MoonIcon,
  PlugIcon,
  PlusIcon,
  SettingsIcon,
  SparklesIcon,
  SunIcon,
  XIcon,
} from "@lucide/vue";
import { Alert, AlertDescription } from "@/components/ui/alert";
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
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { useAppearance } from "@/lib/appearance";
import {
  applyPreferences,
  enabledTools,
  isToolEnabled,
  notificationPreferences,
  preferenceDraftCommit,
  queueMessages,
  setNotificationPreferences,
  setQueueMessages,
  setShowReasoning,
  setToolEnabled,
  setWebSearchCredentials,
  setWebSearchEnabled,
  setWebSearchInstance,
  showReasoning,
  webSearchCredentials,
  webSearchEnabled,
  webSearchInstance,
  type ToolPreferenceId,
} from "@/lib/preferences";
import {
  checkNotificationPermission,
  ensureNotificationPermission,
  type NotificationPermissionState,
} from "@/lib/notifications";
import {
  agentsConfigDirty,
  agentsConfigSaving,
  discardAgentsChanges,
  saveAgentsDraft,
} from "@/lib/agent-profiles";
import { useI18n, type Locale, type MessageKey } from "@/lib/i18n";
import { detectPlatform, zoomShortcutLabels } from "@/lib/zoom";
import {
  invalidateProviderModels,
  loadProviderModels,
  providerModels,
  providerModelsError,
} from "@/lib/provider-models";
import type { ThemeMode } from "@/lib/theme";
const AccountsSection = defineAsyncComponent(() => import("./AccountsSection.vue"));
const McpSection = defineAsyncComponent(() => import("./McpSection.vue"));
const ProviderForm = defineAsyncComponent(() => import("./ProviderForm.vue"));
const ProviderModelsEditor = defineAsyncComponent(
  () => import("./ProviderModelsEditor.vue"),
);
const SubagentsSection = defineAsyncComponent(
  () => import("./SubagentsSection.vue"),
);
const MemorySection = defineAsyncComponent(
  () => import("./MemorySection.vue"),
);
const SkillsSection = defineAsyncComponent(
  () => import("./SkillsSection.vue"),
);
import {
  commandErrorMessage,
  configFilePath,
  deleteProvider,
  getSettings,
  listProviders,
  openConfigFile,
  saveSettings,
  setProviderKey,
} from "@/contracts/commands";
import type { AppSettings, ProviderSummary } from "@/contracts/types";

/** The dialog is open when the drawer's gear button says so. */
const open = defineModel<boolean>("open", { default: false });

/** S1 of `settings-dialog.md`: the sections the app can drive right now. */
type Section = "general" | "providers" | "mcp" | "subagents" | "memory" | "skills";
const section = ref<Section>("general");

/** S2: every row renders through the catalogue, so a locale switch re-renders. */
const { locale, setLocale, t } = useI18n();

/** S3: the three modes, each with the icon its card shows. */
const THEME_OPTIONS: {
  mode: ThemeMode;
  labelKey: MessageKey;
  icon: typeof SunIcon;
}[] = [
  {
    mode: "light",
    labelKey: "settings.general.appearance.light",
    icon: SunIcon,
  },
  { mode: "dark", labelKey: "settings.general.appearance.dark", icon: MoonIcon },
  {
    mode: "system",
    labelKey: "settings.general.appearance.system",
    icon: MonitorIcon,
  },
];

/**
 * S2: the locales the selector offers. The label is each language's own name, so
 * someone who cannot read the current interface language still finds their own.
 */
const LANGUAGE_OPTIONS: { value: Locale; labelKey: MessageKey }[] = [
  { value: "en", labelKey: "settings.general.language.english" },
  { value: "es", labelKey: "settings.general.language.spanish" },
];

/**
 * The zoom chords this build listens to, labelled for the platform it runs on.
 *
 * Read-only on purpose: a chord is not a preference yet, and `src/lib/zoom.ts`
 * owns both the listener and these labels, so this list cannot describe a key the
 * app ignores. The platform is read once, exactly as `App.vue` reads it.
 */
const SHORTCUT_PLATFORM = detectPlatform(navigator.userAgent, navigator.platform);
const ZOOM_SHORTCUT_LABELS = zoomShortcutLabels(SHORTCUT_PLATFORM);
const SHORTCUT_ROWS: { labelKey: MessageKey; keys: string }[] = [
  {
    labelKey: "settings.general.shortcuts.zoomIn",
    keys: ZOOM_SHORTCUT_LABELS.in,
  },
  {
    labelKey: "settings.general.shortcuts.zoomOut",
    keys: ZOOM_SHORTCUT_LABELS.out,
  },
  {
    labelKey: "settings.general.shortcuts.zoomReset",
    keys: ZOOM_SHORTCUT_LABELS.reset,
  },
];

/** Persisted preferences, `null` until the file is read on open. */
const settings = ref<AppSettings | null>(null);
/** S1 of `providers-pane.md`: the providers pi knows, as Rust summarizes them. */
const providers = ref<ProviderSummary[]>([]);
/** S3: the absolute preferences path, resolved by Rust. */
const configPath = ref<string | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
/** A write is in flight, so the controls cannot fire twice. */
const saving = ref(false);
/** Brief confirmation shown in the modal footer right after a save succeeds. */
const savedNotice = ref(false);
let savedNoticeTimer: number | null = null;
const opening = ref(false);
/** S2: the inline custom-provider form is open. */
const showProviderForm = ref(false);
/** S4: the provider whose inline api-key editor is open, `null` when closed. */
const keyTarget = ref<string | null>(null);
/** The key being typed. Always empty when the editor opens: never prefilled. */
const keyDraft = ref("");
/**
 * S3 of `web-search.md`: the two web-search text fields are edited as drafts of
 * their own and written when they are committed, so typing is never a write.
 *
 * A draft, not the shared ref, is what the field is bound to, and that is the fix
 * for the field that wiped itself while typing: `Input` renders a native input
 * with its own internal `v-model` (`src/components/ui/input/Input.vue`), and Vue
 * re-syncs an input's `value` from its props on every patch — so a `:value` bound
 * to a ref that the same keystroke re-renders (through the input's own model) is
 * written back to the DOM mid-typing. Binding the input's own model to a local
 * draft removes the second writer: the only thing that moves the field is what the
 * human types, and the only thing that moves the file is a commit.
 */
const webSearchInstanceDraft = ref("");
const webSearchCredentialsDraft = ref("");
/** A key write is in flight. */
const savingKey = ref(false);
/** The provider the removal confirmation is about, `null` when it is closed. */
const pendingRemoval = ref<ProviderSummary | null>(null);
const removalOpen = ref(false);
/** A removal finished, so a write is in flight. */
const removing = ref(false);
/**
 * A line the pane shows after a removal that left the settings untouched (S1
 * fallback could not happen). Informational, not an error: nothing failed.
 */
const warning = ref<string | null>(null);
/**
 * S4 of `providers-pane-refinements.md`: the provider whose edit dialog is open,
 * by id. One dialog at a time: opening another provider replaces this id.
 */
const editTargetId = ref<string | null>(null);
const editOpen = ref(false);
/**
 * Bumped when pi may have rewritten its cached catalogue behind our back (a
 * finished sign-in). The editor watches it and re-reads its rows, so an open
 * dialog shows the models pi just published without being reopened.
 */
const editorRefresh = ref(0);

/** What the edit dialog shows about one provider, wherever pi keeps it. */
interface EditTarget {
  id: string;
  name: string;
  /** The provider's own API root; `null` when only pi's catalogue knows it. */
  baseUrl: string | null;
  modelCount: number;
  /** `true` when pi's catalogue is its only source: nothing of it is writable. */
  catalogOnly: boolean;
}

/**
 * How many models the editor of this provider will list. `providerModels` is the
 * same read the editor does, so a provider whose catalogue an already signed-in
 * account populated shows the models it really has, not the count `models.json`
 * alone would give.
 */
function modelCountOf(providerId: string, fallback: number): number {
  const entry = providerModels.value.find(
    (item) => item.provider === providerId,
  );
  return entry ? entry.models.length : fallback;
}

/**
 * The providers pi's catalogue knows and `models.json` does not: signing in to a
 * subscription is what makes them exist, and there is no configured entry of
 * theirs to activate, key or remove. They are listed so their models are
 * reachable, and editing them is the only write-free action left.
 */
const catalogOnlyRows = computed(() => {
  const known = new Set(providers.value.map((entry) => entry.id));
  return providerModels.value
    .filter((entry) => !known.has(entry.provider))
    .filter((entry) => entry.models.some((model) => model.source === "catalog"))
    .map((entry) => ({
      id: entry.provider,
      name: entry.name,
      modelCount: entry.models.length,
    }));
});

/**
 * S4: the provider the edit dialog shows, resolved from the pane's own lists on
 * every render. A model write refreshes them, and the dialog's details follow the
 * fresh read instead of a stale copy taken when it opened.
 */
const editTarget = computed<EditTarget | null>(() => {
  const id = editTargetId.value;
  if (id === null) return null;
  const configured = providers.value.find((entry) => entry.id === id);
  if (configured) {
    return {
      id: configured.id,
      name: configured.name,
      baseUrl: configured.baseUrl,
      modelCount: modelCountOf(configured.id, configured.modelCount),
      catalogOnly: false,
    };
  }
  const catalogOnly = catalogOnlyRows.value.find((row) => row.id === id);
  if (!catalogOnly) return null;
  return {
    id: catalogOnly.id,
    name: catalogOnly.name,
    // pi's catalogue carries model data, not an endpoint: there is no URL to
    // show and none was read.
    baseUrl: null,
    modelCount: catalogOnly.modelCount,
    catalogOnly: true,
  };
});

/**
 * S2 of `providers-pane-refinements.md`: the ids pi already has in `models.json`.
 * The accounts block probes these first, so its first wave asks for the providers
 * the human can see configured instead of walking the fixed list from the top.
 * It is the same read the list below renders, so it costs no extra call.
 */
const configuredProviderIds = computed(() =>
  providers.value.map((entry) => entry.id),
);

/**
 * S5 of `providers-pane.md`: build-time switches, read the same way the palette
 * and the hero copy are. ON BY DEFAULT — only an explicit `0`/`false`/`off`
 * (any case, surrounding spaces ignored) turns a section off, so a build without
 * these variables configured shows the features instead of hiding them.
 */
function readFlag(name: string): boolean {
  const value = (import.meta.env as Record<string, string | undefined>)[name];
  const normalized = (value ?? "").trim().toLowerCase();
  return normalized !== "0" && normalized !== "false" && normalized !== "off";
}

/**
 * S5 of `providers-pane.md`: build-time switch, read the same way the palette and
 * the hero copy are. The custom-provider block is the only gated block of this
 * pane.
 */
const customProvidersEnabled = readFlag("VITE_ENABLE_CUSTOM_PROVIDERS");

/**
 * S5 of `pi-cli-bridge.md`: the subscription accounts block, gated by the same
 * `readFlag` rule — on by default, off only for an explicit `0`/`false`/`off`.
 * The check lives here, next to the other switch, so `AccountsSection.vue` never
 * declares a second copy of the semantics.
 */
const providerLoginEnabled = readFlag("VITE_ENABLE_PROVIDER_LOGIN");

// The applied mode, not the stored one: the ring shows what the app is actually
// painting, and both agree because opening the dialog applies the stored mode.
const { mode, setMode } = useAppearance();

/** S1 of `providers-pane.md`: the provider the next session starts with. */
const activeProvider = computed(() => settings.value?.provider ?? "");

async function loadPreferences() {
  loading.value = true;
  error.value = null;
  try {
    const loaded = await getSettings();
    settings.value = loaded;
    // S3 of `settings-dialog.md`: the file may have been edited outside the app,
    // so the stored mode is applied again on every open. S2 of
    // `i18n-and-settings-polish.md`: same rule for the language. S2/S3 of
    // `chat-preferences-and-queue.md`: same rule for the two chat toggles, which
    // live in the shared module so the chat reads the same values.
    setMode(loaded.theme);
    setLocale(loaded.language);
    applyPreferences(loaded);
    // S3 of `web-search.md`: the two text fields of the pane are drafts, so they
    // are seeded from what the file just answered — on every open, which is also
    // how a hand edit of `settings.json` reaches the fields. Nothing else writes
    // them while the pane is open: a keystroke only moves the draft.
    webSearchInstanceDraft.value = webSearchInstance.value;
    webSearchCredentialsDraft.value = webSearchCredentials.value;
  } catch (cause) {
    settings.value = null;
    error.value = commandErrorMessage(cause);
  } finally {
    loading.value = false;
  }
}

/**
 * S1 of `providers-pane.md`: the list is read from pi's own configuration on
 * every open, so a provider added elsewhere shows up instead of going stale.
 */
async function loadProviders() {
  try {
    providers.value = await listProviders();
  } catch (cause) {
    providers.value = [];
    error.value = commandErrorMessage(cause);
  }
}

/**
 * The catalogue is read through the shared store and, when the read fails, its
 * reason is reported on this pane's one error line. The store keeps the previous
 * list on a failure, so the counts and the catalogue-only rows keep rendering
 * instead of collapsing to zero.
 */
async function refreshModels(options: { force?: boolean } = {}): Promise<void> {
  await loadProviderModels(options);
  if (providerModelsError.value !== null) error.value = providerModelsError.value;
}

/**
 * The providers and their models are one read of pi's files: they travel
 * together. This is the path every write takes, so the catalogue is read with
 * `force` — a write must be visible even when a read already answered — and the
 * store's debounce folds the requests of one write into one read.
 */
async function refreshProviderData(): Promise<void> {
  await Promise.all([loadProviders(), refreshModels({ force: true })]);
}

/**
 * A sign-in finished: pi may have written its cached catalogue, so both lists are
 * read again and an open editor is told (it re-reads its own rows). Nothing is
 * guessed from the login itself. The forced catalogue read is also what hands the
 * new models to the chat's picker, which renders the same shared list.
 */
function handleSignedIn() {
  editorRefresh.value += 1;
  void refreshProviderData();
}

/**
 * A model write landed: the counts and the catalogue-only rows may have changed,
 * so both lists are read again. The write already refreshed the shared catalogue
 * (the read the editor does before it emits), and this one keeps the pane's own
 * summary honest — the same two reads a save cost before the catalogue was
 * shared.
 */
function handleModelsChanged() {
  void refreshProviderData();
}

async function loadConfigPath() {
  try {
    configPath.value = await configFilePath();
  } catch (cause) {
    configPath.value = null;
    error.value = commandErrorMessage(cause);
  }
}

/**
 * S3 of `settings-dialog.md`: persists the appearance and applies it
 * immediately. The whole file is re-read first so this write cannot clobber a
 * preference another surface changed in the meantime (`save_settings` replaces
 * the file).
 */
async function handleThemeChange(next: ThemeMode) {
  if (saving.value || mode.value === next) return;
  saving.value = true;
  error.value = null;
  try {
    const current = await getSettings();
    settings.value = await saveSettings({ ...current, theme: next });
    setMode(next);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
}

/**
 * S2 of `chat-preferences-and-queue.md`: the "Show reasoning" switch. The card
 * reads the shared ref, so flipping it hides or shows every reasoning block
 * immediately; the setter persists the choice in the background.
 */
function handleShowReasoning(next: boolean) {
  void setShowReasoning(next);
}

/** S3: the "Queue messages" switch, same pattern as the reasoning one. */
function handleQueueMessages(next: boolean) {
  void setQueueMessages(next);
}

/**
 * S5 of `tool-drawer-and-mcp.md`: the drawer switches, in the order the pane
 * lists them. Files and Changes are read-only views; Terminal and Browser run
 * code, so each of those two carries the warning shown while it is on. Browser
 * has no tab yet (F3), and the switch is still offered because the preference is
 * what F3 will read — the description says so instead of pretending otherwise.
 */
const TOOL_SWITCHES: {
  id: ToolPreferenceId;
  labelKey: MessageKey;
  descriptionKey: MessageKey;
  warningKey?: MessageKey;
}[] = [
  {
    id: "files",
    labelKey: "settings.general.tools.files",
    descriptionKey: "settings.general.tools.files.description",
  },
  {
    id: "changes",
    labelKey: "settings.general.tools.changes",
    descriptionKey: "settings.general.tools.changes.description",
  },
  {
    id: "document",
    labelKey: "settings.general.tools.document",
    descriptionKey: "settings.general.tools.document.description",
  },
  {
    id: "terminal",
    labelKey: "settings.general.tools.terminal",
    descriptionKey: "settings.general.tools.terminal.description",
    warningKey: "settings.general.tools.terminal.warning",
  },
  {
    id: "browser",
    labelKey: "settings.general.tools.browser",
    descriptionKey: "settings.general.tools.browser.description",
    warningKey: "settings.general.tools.browser.warning",
  },
];

/**
 * S5: one switch, one write. The shared setter updates the live list first — the
 * open drawer gains or drops the tab immediately — and persists the normalized
 * list through the same read-merge-save path as every other preference.
 */
function handleToolToggle(id: ToolPreferenceId, next: boolean) {
  void setToolEnabled(id, next);
}

const notificationPermission = ref<NotificationPermissionState>("prompt");

async function refreshNotificationPermission(): Promise<void> {
  notificationPermission.value = await checkNotificationPermission();
}

async function handleNotificationsMasterToggle(next: boolean): Promise<void> {
  if (next) {
    const granted = await ensureNotificationPermission();
    notificationPermission.value = granted ? "granted" : "denied";
    if (!granted) {
      await setNotificationPreferences({ enabled: false });
      return;
    }
    const prefs = notificationPreferences.value;
    const anyTrigger =
      prefs.onTurnFinish || prefs.onQuestion || prefs.onSubagentFinish;
    await setNotificationPreferences({
      enabled: true,
      // When turning notifications on for the first time, pre-select useful defaults
      // so the human immediately sees what will notify them and can toggle each one.
      ...(anyTrigger
        ? {}
        : { onTurnFinish: true, onQuestion: true, onSubagentFinish: false }),
    });
    return;
  }
  await setNotificationPreferences({ enabled: false });
}

function handleNotificationTriggerToggle(
  field: "onTurnFinish" | "onQuestion" | "onSubagentFinish",
  next: boolean,
): void {
  void setNotificationPreferences({ [field]: next });
}

/**
 * S3 of `web-search.md`: the agent's web search switch. The Rust default is off,
 * and the pane says what the toggle does — the query leaves the machine for the
 * instance.
 */
function handleWebSearchToggle(next: boolean) {
  void setWebSearchEnabled(next);
}

/**
 * S3: one text draft of this pane, committed on blur or Enter.
 *
 * `preferenceDraftCommit` answers what the draft is worth (trimmed) or `null` when
 * the file already holds it, so a commit that changes nothing writes nothing. The
 * field is then left showing what was stored: the trimmed value the human meant,
 * never the stray spaces they did not. The write is fire-and-forget like every
 * other preference in this dialog — the shared setter updates the live value first
 * and persists behind it, and a failed write leaves the field as typed.
 */
function commitTextDraft(
  draft: Ref<string>,
  stored: Ref<string>,
  write: (next: string) => Promise<void>,
): void {
  const next = preferenceDraftCommit(draft.value, stored.value);
  if (next === null) {
    draft.value = stored.value;
    return;
  }
  void write(next).then(() => {
    draft.value = stored.value;
  });
}

/** S3: the instance url, written when the field is committed (blur or Enter). */
function commitWebSearchInstance() {
  commitTextDraft(
    webSearchInstanceDraft,
    webSearchInstance,
    setWebSearchInstance,
  );
}

/**
 * The credential, same commit rule as the instance. The field is a password
 * input and starts from the stored value, because a human re-editing a token needs
 * to see what is there to correct it; it is their own file.
 */
function commitWebSearchCredentials() {
  commitTextDraft(
    webSearchCredentialsDraft,
    webSearchCredentials,
    setWebSearchCredentials,
  );
}

/**
 * S1 of `providers-pane.md`: clicking a provider makes it the active one by
 * persisting `settings.provider`.
 *
 * `settings.model` is left exactly as it is: this pane has no catalogue to check
 * it against, and the chat composer owns that choice (L4 of the task doc).
 */
async function handleProviderActivate(providerId: string) {
  if (saving.value || providerId === activeProvider.value) return;
  saving.value = true;
  error.value = null;
  try {
    const current = await getSettings();
    settings.value = await saveSettings({ ...current, provider: providerId });
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
}

/**
 * S4: opens the inline api-key editor for one provider. The field always starts
 * empty — Rust never sends the key, so there is nothing to prefill.
 */
function toggleKeyEditor(entry: ProviderSummary) {
  if (keyTarget.value === entry.id) {
    closeKeyEditor();
    return;
  }
  keyTarget.value = entry.id;
  keyDraft.value = "";
  error.value = null;
}

function closeKeyEditor() {
  keyTarget.value = null;
  keyDraft.value = "";
}

/**
 * S4: stores the typed key through `set_provider_key`, which rewrites
 * `providers.<id>.apiKey` in pi's plain-text file (backup + atomic write). Rust
 * answers with the summary the file now holds, so the row is refreshed from it
 * instead of guessing, and "key set" appears because the file says so.
 *
 * An empty field cannot be submitted (the button is disabled), and the typed key
 * is dropped once the write lands.
 */
async function handleSaveKey() {
  const target = keyTarget.value;
  const key = keyDraft.value.trim();
  if (!target || key === "" || savingKey.value) return;
  savingKey.value = true;
  error.value = null;
  try {
    const summary = await setProviderKey(target, key);
    providers.value = providers.value.map((entry) =>
      entry.id === summary.id ? summary : entry,
    );
    closeKeyEditor();
  } catch (cause) {
    // A rejected key keeps the editor open with what was typed.
    error.value = commandErrorMessage(cause);
  } finally {
    savingKey.value = false;
  }
}

/**
 * S4 of `providers-pane-refinements.md`: opens the edit dialog of one provider.
 * The row keeps only actions; the details and the models of that provider live
 * in the dialog, which is keyboard reachable like every other dialog of the app.
 */
function openEditor(entry: { id: string }) {
  editTargetId.value = entry.id;
  editOpen.value = true;
  error.value = null;
}

/** Opens the removal confirmation for one provider (S1/S3). */
function requestRemove(entry: ProviderSummary) {
  pendingRemoval.value = entry;
  removalOpen.value = true;
  error.value = null;
  warning.value = null;
}

/**
 * S3: removes one provider from pi's configuration after confirmation, then
 * re-reads the list (Rust may have had nothing to remove).
 *
 * S1 fallback: if the removed provider was the active one, the first remaining
 * provider becomes active; with none left, the settings are left untouched and
 * the pane says so instead of writing a provider the file no longer has.
 */
async function confirmRemove() {
  const target = pendingRemoval.value;
  if (!target || removing.value) return;
  removalOpen.value = false;
  pendingRemoval.value = null;
  removing.value = true;
  error.value = null;
  warning.value = null;
  try {
    // An editor left open on the removed row would have no provider to write.
    if (keyTarget.value === target.id) closeKeyEditor();
    // S4: the same goes for the edit dialog of the row that is going away.
    if (editTargetId.value === target.id) {
      editTargetId.value = null;
      editOpen.value = false;
    }
    const removed = await deleteProvider(target.id);
    await refreshProviderData();
    if (!removed) {
      // The entry was already gone from the file: report it instead of
      // pretending the removal happened.
      warning.value = t("settings.providers.remove.missing");
      return;
    }
    if (target.id !== activeProvider.value) return;
    const remaining = providers.value[0];
    if (!remaining) {
      warning.value = t("settings.providers.remove.activeWarning");
      return;
    }
    const current = await getSettings();
    settings.value = await saveSettings({
      ...current,
      provider: remaining.id,
    });
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    removing.value = false;
  }
}

/**
 * S2 of `providers-pane.md`: a saved provider lands in pi's file, so the list is
 * re-read instead of patched locally — Rust is the one that knows the stored
 * name and model count (an empty name falls back to the id there).
 */
function handleProviderSaved(_summary: ProviderSummary) {
  showProviderForm.value = false;
  void refreshProviderData();
}

/**
 * S2 of `i18n-and-settings-polish.md`: persists the language and applies it at
 * once, so the dialog itself switches without a reload. The write lands first:
 * a failed save must not leave the UI in a language the file does not record.
 */
async function handleLanguageChange(next: string) {
  if (saving.value) return;
  saving.value = true;
  error.value = null;
  try {
    const current = await getSettings();
    settings.value = await saveSettings({ ...current, language: next });
    setLocale(next);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
}

/**
 * S2 of `settings-dialog.md`: opens the real preferences file — `settings.json` —
 * with the system's default editor. Rust creates it with the defaults when it
 * does not exist yet, so the action never targets a missing path. S4 of
 * `i18n-and-settings-polish.md` keeps this as the only such action.
 */
async function handleOpenConfigFile() {
  if (opening.value) return;
  opening.value = true;
  error.value = null;
  try {
    await openConfigFile();
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    opening.value = false;
  }
}

/** True when any text draft or subagent draft differs from what is stored. */
const hasUnsavedDrafts = computed(
  () =>
    agentsConfigDirty.value ||
    webSearchInstanceDraft.value.trim() !== webSearchInstance.value ||
    webSearchCredentialsDraft.value.trim() !== webSearchCredentials.value,
);

/**
 * Unified Save action for the persistent modal footer: commits any pending
 * text drafts, writes the current preferences snapshot to `settings.json`, and
 * saves the subagent profiles draft to `agents.json` when dirty.
 */
async function handleSaveAll(): Promise<void> {
  if (saving.value || agentsConfigSaving.value) return;
  saving.value = true;
  error.value = null;
  savedNotice.value = false;
  try {
    const current = await getSettings();
    const nextInstance = webSearchInstanceDraft.value.trim();
    const nextCredentials = webSearchCredentialsDraft.value.trim();
    const saved = await saveSettings({
      ...current,
      theme: mode.value,
      language: locale.value,
      showReasoning: showReasoning.value,
      queueMessages: queueMessages.value,
      enabledTools: [...enabledTools.value],
      webSearchEnabled: webSearchEnabled.value,
      webSearchInstance: nextInstance,
      webSearchCredentials: nextCredentials,
      notifications: { ...notificationPreferences.value },
    });
    settings.value = saved;
    applyPreferences(saved);
    webSearchInstanceDraft.value = webSearchInstance.value;
    webSearchCredentialsDraft.value = webSearchCredentials.value;
    if (agentsConfigDirty.value) {
      await saveAgentsDraft();
    }
    savedNotice.value = true;
    if (savedNoticeTimer !== null) window.clearTimeout(savedNoticeTimer);
    savedNoticeTimer = window.setTimeout(() => {
      savedNotice.value = false;
      savedNoticeTimer = null;
    }, 3000);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    saving.value = false;
  }
}

function handleDiscardAll(): void {
  if (agentsConfigDirty.value) discardAgentsChanges();
  webSearchInstanceDraft.value = webSearchInstance.value;
  webSearchCredentialsDraft.value = webSearchCredentials.value;
}

// Every open re-reads the file, the provider list and the path, so a human edit
// outside the app is picked up instead of showing a stale dialog. The inline form
// starts closed, the way a preferences window does.
watch(open, (isOpen) => {
  if (!isOpen) return;
  section.value = "general";
  savedNotice.value = false;
  showProviderForm.value = false;
  closeKeyEditor();
  pendingRemoval.value = null;
  removalOpen.value = false;
  editTargetId.value = null;
  editOpen.value = false;
  savingKey.value = false;
  removing.value = false;
  warning.value = null;
  error.value = null;
  void loadPreferences();
  void loadProviders();
  void refreshNotificationPermission();
  // The catalogue is re-read on every open — the file may have been edited
  // outside the app — through the shared store, so this pane and the chat's
  // picker always agree. `invalidate` marks it stale and lets the read below (or
  // the one a write is already queued behind) answer, instead of asking for a
  // second read of a file that is already being read.
  invalidateProviderModels();
  void refreshModels();
  void loadConfigPath();
}, { immediate: true });
</script>

<template>
  <Dialog v-model:open="open">
    <!-- The close control is the only header action (S4: the configuration-file
         action lives at the end of the General pane), so the default floating X
         is off. -->
    <DialogContent
      :show-close-button="false"
      class="flex max-h-[min(86vh,46rem)] w-full max-w-3xl flex-col gap-0 overflow-hidden rounded-2xl border border-border bg-card p-0 shadow-2xl sm:max-w-3xl"
    >
      <DialogHeader
        class="flex-row items-center justify-between gap-3 border-b border-border bg-card px-5 py-4"
      >
        <DialogTitle class="text-sm font-semibold text-foreground">
          {{ t("settings.title") }}
        </DialogTitle>
        <DialogClose as-child>
          <Button
            variant="ghost"
            size="icon-sm"
            class="text-muted-foreground hover:bg-accent hover:text-foreground"
            :aria-label="t('settings.close')"
          >
            <XIcon />
          </Button>
        </DialogClose>
      </DialogHeader>

      <div class="flex min-h-0 flex-1">
        <!-- S1: left navigation. Active item uses the drawer's accent tokens so
             it reads as the same kind of selected row as a workspace. -->
        <nav
          :aria-label="t('settings.sections')"
          class="flex w-48 shrink-0 flex-col gap-1 border-r border-sidebar-border bg-sidebar p-3"
        >
          <button
            type="button"
            :aria-current="section === 'general' ? 'page' : undefined"
            :class="[
              'flex w-full items-center gap-2.5 rounded-lg border px-3 py-2 text-left text-xs outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
              section === 'general'
                ? 'border-border bg-sidebar-accent font-semibold text-sidebar-accent-foreground'
                : 'border-transparent text-muted-foreground hover:bg-secondary hover:text-foreground',
            ]"
            @click="section = 'general'"
          >
            <SettingsIcon
              class="size-4 shrink-0"
              :class="section === 'general' ? 'text-primary' : ''"
              aria-hidden="true"
            />
            {{ t("settings.section.general") }}
          </button>
          <button
            type="button"
            :aria-current="section === 'providers' ? 'page' : undefined"
            :class="[
              'flex w-full items-center gap-2.5 rounded-lg border px-3 py-2 text-left text-xs outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
              section === 'providers'
                ? 'border-border bg-sidebar-accent font-semibold text-sidebar-accent-foreground'
                : 'border-transparent text-muted-foreground hover:bg-secondary hover:text-foreground',
            ]"
            @click="section = 'providers'"
          >
            <PlugIcon
              class="size-4 shrink-0"
              :class="section === 'providers' ? 'text-primary' : ''"
              aria-hidden="true"
            />
            {{ t("settings.section.providers") }}
          </button>
          <button
            type="button"
            :aria-current="section === 'mcp' ? 'page' : undefined"
            :class="[
              'flex w-full items-center gap-2.5 rounded-lg border px-3 py-2 text-left text-xs outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
              section === 'mcp'
                ? 'border-border bg-sidebar-accent font-semibold text-sidebar-accent-foreground'
                : 'border-transparent text-muted-foreground hover:bg-secondary hover:text-foreground',
            ]"
            @click="section = 'mcp'"
          >
            <BlocksIcon
              class="size-4 shrink-0"
              :class="section === 'mcp' ? 'text-primary' : ''"
              aria-hidden="true"
            />
            {{ t("settings.section.mcp") }}
          </button>
          <button
            type="button"
            :aria-current="section === 'subagents' ? 'page' : undefined"
            :class="[
              'flex w-full items-center gap-2.5 rounded-lg border px-3 py-2 text-left text-xs outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
              section === 'subagents'
                ? 'border-border bg-sidebar-accent font-semibold text-sidebar-accent-foreground'
                : 'border-transparent text-muted-foreground hover:bg-secondary hover:text-foreground',
            ]"
            @click="section = 'subagents'"
          >
            <BotIcon
              class="size-4 shrink-0"
              :class="section === 'subagents' ? 'text-primary' : ''"
              aria-hidden="true"
            />
            {{ t("settings.section.subagents") }}
          </button>
          <button
            type="button"
            :aria-current="section === 'memory' ? 'page' : undefined"
            :class="[
              'flex w-full items-center gap-2.5 rounded-lg border px-3 py-2 text-left text-xs outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
              section === 'memory'
                ? 'border-border bg-sidebar-accent font-semibold text-sidebar-accent-foreground'
                : 'border-transparent text-muted-foreground hover:bg-secondary hover:text-foreground',
            ]"
            @click="section = 'memory'"
          >
            <BrainIcon
              class="size-4 shrink-0"
              :class="section === 'memory' ? 'text-primary' : ''"
              aria-hidden="true"
            />
            {{ t("settings.section.memory") }}
          </button>
          <button
            type="button"
            :aria-current="section === 'skills' ? 'page' : undefined"
            :class="[
              'flex w-full items-center gap-2.5 rounded-lg border px-3 py-2 text-left text-xs outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
              section === 'skills'
                ? 'border-border bg-sidebar-accent font-semibold text-sidebar-accent-foreground'
                : 'border-transparent text-muted-foreground hover:bg-secondary hover:text-foreground',
            ]"
            @click="section = 'skills'"
          >
            <SparklesIcon
              class="size-4 shrink-0"
              :class="section === 'skills' ? 'text-primary' : ''"
              aria-hidden="true"
            />
            {{ t("settings.section.skills") }}
          </button>
        </nav>

        <!-- S1: the scrolling pane. -->
        <div class="min-h-0 flex-1 overflow-y-auto bg-card px-6 py-5">
          <div v-if="loading" class="py-6 text-xs text-muted-foreground">
            {{ t("settings.loading") }}
          </div>

          <!-- S3 of `settings-dialog.md`: General — appearance, chat, tools,
               web search, language and shortcuts, framed as clean cards. -->
          <div v-else-if="section === 'general'" class="space-y-4">
            <section class="rounded-xl border border-border bg-secondary/40 p-4">
              <div class="flex flex-wrap items-center justify-between gap-4">
                <div class="min-w-0">
                  <h3 class="text-xs font-semibold text-foreground">
                    {{ t("settings.general.appearance") }}
                  </h3>
                  <p class="mt-0.5 text-[11px] text-muted-foreground">
                    {{ t("settings.general.appearance.description") }}
                  </p>
                </div>
                <div class="flex gap-1.5">
                  <button
                    v-for="option in THEME_OPTIONS"
                    :key="option.mode"
                    type="button"
                    :aria-pressed="mode === option.mode"
                    :disabled="saving"
                    :class="[
                      'flex w-16 flex-col items-center gap-1 rounded-lg border py-2 text-[11px] outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50',
                      mode === option.mode
                        ? 'border-primary/45 bg-primary/10 font-semibold text-primary'
                        : 'border-border bg-card text-muted-foreground hover:bg-accent hover:text-foreground',
                    ]"
                    @click="handleThemeChange(option.mode)"
                  >
                    <component :is="option.icon" class="size-3.5" aria-hidden="true" />
                    {{ t(option.labelKey) }}
                  </button>
                </div>
              </div>
            </section>

            <!-- S2/S3 of `chat-preferences-and-queue.md`: the two chat toggles,
                 right under the appearance cards because they are the other half
                 of "how the conversation looks and behaves". One section, two
                 rows: label and short description on the left, switch on the
                 right (the same `role="switch"` control the model editor uses).
                 Both read the live refs from `src/lib/preferences.ts`, so the
                 chat reflects a flip without a reload. -->
            <section class="space-y-3 rounded-xl border border-border bg-secondary/40 p-4">
              <div class="min-w-0">
                <h3 class="text-xs font-semibold text-foreground">
                  {{ t("settings.general.chat") }}
                </h3>
                <p class="mt-0.5 text-[11px] text-muted-foreground">
                  {{ t("settings.general.chat.description") }}
                </p>
              </div>

              <div class="divide-y divide-sidebar-border">
                <div class="flex items-start justify-between gap-4 py-2.5 first:pt-0 last:pb-0">
                  <div class="min-w-0">
                    <p class="text-xs font-medium text-foreground">
                      {{ t("settings.general.showReasoning") }}
                    </p>
                    <p class="mt-0.5 text-[11px] text-muted-foreground">
                      {{ t("settings.general.showReasoning.description") }}
                    </p>
                  </div>
                  <button
                    type="button"
                    role="switch"
                    :aria-checked="showReasoning"
                    :aria-label="t('settings.general.showReasoning')"
                    :class="[
                      'relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full border outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                      showReasoning
                        ? 'border-primary/40 bg-primary/15'
                        : 'border-border bg-accent',
                    ]"
                    @click="handleShowReasoning(!showReasoning)"
                  >
                    <span
                      :class="[
                        'size-3.5 rounded-full transition-transform',
                        showReasoning
                          ? 'translate-x-4.5 bg-primary'
                          : 'translate-x-0.5 bg-muted-foreground',
                      ]"
                      aria-hidden="true"
                    />
                  </button>
                </div>

                <div class="flex items-start justify-between gap-4 py-2.5 first:pt-0 last:pb-0">
                  <div class="min-w-0">
                    <p class="text-xs font-medium text-foreground">
                      {{ t("settings.general.queueMessages") }}
                    </p>
                    <p class="mt-0.5 text-[11px] text-muted-foreground">
                      {{ t("settings.general.queueMessages.description") }}
                    </p>
                  </div>
                  <button
                    type="button"
                    role="switch"
                    :aria-checked="queueMessages"
                    :aria-label="t('settings.general.queueMessages')"
                    :class="[
                      'relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full border outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                      queueMessages
                        ? 'border-primary/40 bg-primary/15'
                        : 'border-border bg-accent',
                    ]"
                    @click="handleQueueMessages(!queueMessages)"
                  >
                    <span
                      :class="[
                        'size-3.5 rounded-full transition-transform',
                        queueMessages
                          ? 'translate-x-4.5 bg-primary'
                          : 'translate-x-0.5 bg-muted-foreground',
                      ]"
                      aria-hidden="true"
                    />
                  </button>
                </div>
              </div>
            </section>

            <!-- S5 of `tool-drawer-and-mcp.md`: the drawer's tools. -->
            <section class="space-y-3 rounded-xl border border-border bg-secondary/40 p-4">
              <div class="min-w-0">
                <h3 class="text-xs font-semibold text-foreground">
                  {{ t("settings.general.tools") }}
                </h3>
                <p class="mt-0.5 text-[11px] text-muted-foreground">
                  {{ t("settings.general.tools.description") }}
                </p>
              </div>

              <div class="divide-y divide-sidebar-border">
                <div
                  v-for="tool in TOOL_SWITCHES"
                  :key="tool.id"
                  class="flex items-start justify-between gap-4 py-2.5 first:pt-0 last:pb-0"
                >
                  <div class="min-w-0">
                    <p class="text-xs font-medium text-foreground">
                      {{ t(tool.labelKey) }}
                    </p>
                    <p class="mt-0.5 text-[11px] text-muted-foreground">
                      {{ t(tool.descriptionKey) }}
                    </p>
                    <p
                      v-if="tool.warningKey && isToolEnabled(tool.id)"
                      class="mt-1 text-[11px] text-warning"
                    >
                      {{ t(tool.warningKey) }}
                    </p>
                  </div>
                  <button
                    type="button"
                    role="switch"
                    :aria-checked="isToolEnabled(tool.id)"
                    :aria-label="t(tool.labelKey)"
                    :class="[
                      'relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full border outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                      isToolEnabled(tool.id)
                        ? 'border-primary/40 bg-primary/15'
                        : 'border-border bg-accent',
                    ]"
                    @click="handleToolToggle(tool.id, !isToolEnabled(tool.id))"
                  >
                    <span
                      :class="[
                        'size-3.5 rounded-full transition-transform',
                        isToolEnabled(tool.id)
                          ? 'translate-x-4.5 bg-primary'
                          : 'translate-x-0.5 bg-muted-foreground',
                      ]"
                      aria-hidden="true"
                    />
                  </button>
                </div>
              </div>
            </section>

            <!-- Configurable OS notifications (opt-in, per-event triggers). -->
            <section class="space-y-3 rounded-xl border border-border bg-secondary/40 p-4">
              <div class="flex items-start justify-between gap-4">
                <div class="min-w-0">
                  <div class="flex items-center gap-2">
                    <h3 class="text-xs font-semibold text-foreground">
                      {{ t("settings.general.notifications") }}
                    </h3>
                    <span
                      v-if="notificationPermission === 'granted'"
                      class="rounded-full border border-primary/30 bg-primary/10 px-2 py-0.5 text-[10px] font-medium text-primary"
                    >
                      {{ t("settings.general.notifications.permissionGranted") }}
                    </span>
                    <span
                      v-else-if="notificationPermission === 'denied'"
                      class="rounded-full border border-destructive/30 bg-destructive/10 px-2 py-0.5 text-[10px] font-medium text-destructive"
                    >
                      {{ t("settings.general.notifications.permissionDenied") }}
                    </span>
                  </div>
                  <p class="mt-0.5 text-[11px] text-muted-foreground">
                    {{ t("settings.general.notifications.description") }}
                  </p>
                </div>
                <button
                  type="button"
                  role="switch"
                  :aria-checked="notificationPreferences.enabled"
                  :aria-label="t('settings.general.notifications')"
                  :class="[
                    'relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full border outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                    notificationPreferences.enabled
                      ? 'border-primary/40 bg-primary/15'
                      : 'border-border bg-accent',
                  ]"
                  @click="handleNotificationsMasterToggle(!notificationPreferences.enabled)"
                >
                  <span
                    :class="[
                      'size-3.5 rounded-full transition-transform',
                      notificationPreferences.enabled
                        ? 'translate-x-4.5 bg-primary'
                        : 'translate-x-0.5 bg-muted-foreground',
                    ]"
                    aria-hidden="true"
                  />
                </button>
              </div>

              <div
                v-if="notificationPreferences.enabled"
                class="divide-y divide-sidebar-border border-t border-sidebar-border pt-2"
              >
                <div class="flex items-start justify-between gap-4 py-2 first:pt-0 last:pb-0">
                  <div class="min-w-0">
                    <p class="text-xs font-medium text-foreground">
                      {{ t("settings.general.notifications.onTurnFinish") }}
                    </p>
                    <p class="mt-0.5 text-[11px] text-muted-foreground">
                      {{ t("settings.general.notifications.onTurnFinish.description") }}
                    </p>
                  </div>
                  <button
                    type="button"
                    role="switch"
                    :aria-checked="notificationPreferences.onTurnFinish"
                    :aria-label="t('settings.general.notifications.onTurnFinish')"
                    :class="[
                      'relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full border outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                      notificationPreferences.onTurnFinish
                        ? 'border-primary/40 bg-primary/15'
                        : 'border-border bg-accent',
                    ]"
                    @click="handleNotificationTriggerToggle('onTurnFinish', !notificationPreferences.onTurnFinish)"
                  >
                    <span
                      :class="[
                        'size-3.5 rounded-full transition-transform',
                        notificationPreferences.onTurnFinish
                          ? 'translate-x-4.5 bg-primary'
                          : 'translate-x-0.5 bg-muted-foreground',
                      ]"
                      aria-hidden="true"
                    />
                  </button>
                </div>

                <div class="flex items-start justify-between gap-4 py-2 first:pt-0 last:pb-0">
                  <div class="min-w-0">
                    <p class="text-xs font-medium text-foreground">
                      {{ t("settings.general.notifications.onQuestion") }}
                    </p>
                    <p class="mt-0.5 text-[11px] text-muted-foreground">
                      {{ t("settings.general.notifications.onQuestion.description") }}
                    </p>
                  </div>
                  <button
                    type="button"
                    role="switch"
                    :aria-checked="notificationPreferences.onQuestion"
                    :aria-label="t('settings.general.notifications.onQuestion')"
                    :class="[
                      'relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full border outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                      notificationPreferences.onQuestion
                        ? 'border-primary/40 bg-primary/15'
                        : 'border-border bg-accent',
                    ]"
                    @click="handleNotificationTriggerToggle('onQuestion', !notificationPreferences.onQuestion)"
                  >
                    <span
                      :class="[
                        'size-3.5 rounded-full transition-transform',
                        notificationPreferences.onQuestion
                          ? 'translate-x-4.5 bg-primary'
                          : 'translate-x-0.5 bg-muted-foreground',
                      ]"
                      aria-hidden="true"
                    />
                  </button>
                </div>

                <div class="flex items-start justify-between gap-4 py-2 first:pt-0 last:pb-0">
                  <div class="min-w-0">
                    <p class="text-xs font-medium text-foreground">
                      {{ t("settings.general.notifications.onSubagentFinish") }}
                    </p>
                    <p class="mt-0.5 text-[11px] text-muted-foreground">
                      {{ t("settings.general.notifications.onSubagentFinish.description") }}
                    </p>
                  </div>
                  <button
                    type="button"
                    role="switch"
                    :aria-checked="notificationPreferences.onSubagentFinish"
                    :aria-label="t('settings.general.notifications.onSubagentFinish')"
                    :class="[
                      'relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full border outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                      notificationPreferences.onSubagentFinish
                        ? 'border-primary/40 bg-primary/15'
                        : 'border-border bg-accent',
                    ]"
                    @click="handleNotificationTriggerToggle('onSubagentFinish', !notificationPreferences.onSubagentFinish)"
                  >
                    <span
                      :class="[
                        'size-3.5 rounded-full transition-transform',
                        notificationPreferences.onSubagentFinish
                          ? 'translate-x-4.5 bg-primary'
                          : 'translate-x-0.5 bg-muted-foreground',
                      ]"
                      aria-hidden="true"
                    />
                  </button>
                </div>
              </div>
            </section>

            <!-- S3 of `web-search.md`: the agent's web search. -->
            <section class="space-y-3 rounded-xl border border-border bg-secondary/40 p-4">
              <div class="min-w-0">
                <h3 class="text-xs font-semibold text-foreground">
                  {{ t("settings.general.webSearch") }}
                </h3>
                <p class="mt-0.5 text-[11px] text-muted-foreground">
                  {{ t("settings.general.webSearch.description") }}
                </p>
              </div>

              <div class="flex items-start justify-between gap-4">
                <div class="min-w-0">
                  <p class="text-xs font-medium text-foreground">
                    {{ t("settings.general.webSearch.enable") }}
                  </p>
                  <p class="mt-0.5 text-[11px] text-muted-foreground">
                    {{ t("settings.general.webSearch.enable.description") }}
                  </p>
                </div>
                <button
                  type="button"
                  role="switch"
                  :aria-checked="webSearchEnabled"
                  :aria-label="t('settings.general.webSearch.enable')"
                  :class="[
                    'relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full border outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                    webSearchEnabled
                      ? 'border-primary/40 bg-primary/15'
                      : 'border-border bg-accent',
                  ]"
                  @click="handleWebSearchToggle(!webSearchEnabled)"
                >
                  <span
                    :class="[
                      'size-3.5 rounded-full transition-transform',
                      webSearchEnabled
                        ? 'translate-x-4.5 bg-primary'
                        : 'translate-x-0.5 bg-muted-foreground',
                    ]"
                    aria-hidden="true"
                  />
                </button>
              </div>

              <div class="space-y-1.5">
                <label
                  class="text-xs font-medium text-foreground"
                  for="settings-web-search-instance"
                >
                  {{ t("settings.general.webSearch.instance") }}
                </label>
                <Input
                  id="settings-web-search-instance"
                  v-model="webSearchInstanceDraft"
                  type="text"
                  inputmode="url"
                  autocomplete="off"
                  spellcheck="false"
                  class="h-8 bg-card text-xs"
                  :placeholder="
                    t('settings.general.webSearch.instance.placeholder')
                  "
                  :aria-label="t('settings.general.webSearch.instance')"
                  @change="commitWebSearchInstance"
                  @keydown.enter="commitWebSearchInstance"
                />
                <p class="text-[11px] text-muted-foreground">
                  {{ t("settings.general.webSearch.instance.hint") }}
                </p>
              </div>

              <div class="space-y-1.5">
                <label
                  class="text-xs font-medium text-foreground"
                  for="settings-web-search-credentials"
                >
                  {{ t("settings.general.webSearch.credentials") }}
                </label>
                <Input
                  id="settings-web-search-credentials"
                  v-model="webSearchCredentialsDraft"
                  type="password"
                  autocomplete="off"
                  spellcheck="false"
                  class="h-8 bg-card text-xs"
                  :placeholder="
                    t('settings.general.webSearch.credentials.placeholder')
                  "
                  :aria-label="t('settings.general.webSearch.credentials')"
                  @change="commitWebSearchCredentials"
                  @keydown.enter="commitWebSearchCredentials"
                />
                <p class="text-[11px] text-muted-foreground">
                  {{ t("settings.general.webSearch.credentials.hint") }}
                </p>
              </div>
            </section>

            <section class="rounded-xl border border-border bg-secondary/40 p-4">
              <div class="flex flex-wrap items-center justify-between gap-4">
                <div class="min-w-0">
                  <h3 class="text-xs font-semibold text-foreground">
                    {{ t("settings.general.language") }}
                  </h3>
                  <p class="mt-0.5 text-[11px] text-muted-foreground">
                    {{ t("settings.general.language.description") }}
                  </p>
                </div>
                <Select
                  :model-value="locale"
                  :disabled="saving"
                  @update:model-value="handleLanguageChange(String($event))"
                >
                  <SelectTrigger
                    class="w-40 bg-card text-xs"
                    :aria-label="t('settings.general.language')"
                  >
                    <SelectValue
                      :placeholder="t('settings.general.language.selectPlaceholder')"
                    />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem
                      v-for="option in LANGUAGE_OPTIONS"
                      :key="option.value"
                      :value="option.value"
                    >
                      {{ t(option.labelKey) }}
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </section>

            <section class="space-y-2.5 rounded-xl border border-border bg-secondary/40 p-4">
              <div>
                <h3 class="text-xs font-semibold text-foreground">
                  {{ t("settings.general.shortcuts") }}
                </h3>
                <p class="mt-0.5 text-[11px] text-muted-foreground">
                  {{ t("settings.general.shortcuts.description") }}
                </p>
              </div>
              <ul class="divide-y divide-sidebar-border rounded-lg border border-border bg-card">
                <li
                  v-for="row in SHORTCUT_ROWS"
                  :key="row.labelKey"
                  class="flex items-center justify-between gap-4 px-3 py-2"
                >
                  <span class="text-xs text-foreground">{{ t(row.labelKey) }}</span>
                  <kbd
                    class="rounded border border-border bg-secondary px-1.5 py-0.5 font-mono text-[10px] font-medium text-muted-foreground"
                  >{{ row.keys }}</kbd>
                </li>
              </ul>
              <p class="text-[11px] text-muted-foreground">
                {{ t("settings.general.shortcuts.zoomScope") }}
              </p>
            </section>

            <!-- S4: the only configuration-file action, at the end of the pane. -->
            <div class="flex justify-end">
              <Button
                variant="outline"
                size="sm"
                class="rounded-lg border-border bg-card text-xs hover:border-primary/40 hover:text-primary"
                :disabled="opening"
                @click="handleOpenConfigFile"
              >
                <ExternalLinkIcon data-icon="inline-start" />
                {{ t("settings.general.configurationFile.open") }}
              </Button>
            </div>
          </div>

          <!-- S3/S4 of `pi-cli-bridge.md`: the subscription accounts, above the
               provider list because signing in is what makes a provider usable.
               S5: the whole block is ON unless the build sets
               `VITE_ENABLE_PROVIDER_LOGIN` to `0`/`false`/`off`.
               S3 of `providers-pane-refinements.md`: this is the ONLY rule of
               the pane — the accounts block and the provider list are the two
               major blocks here, and the custom-provider form belongs to the
               list it adds to. -->
          <div v-else-if="section === 'providers'" class="space-y-6 py-4">
            <template v-if="providerLoginEnabled">
              <AccountsSection
                :configured="configuredProviderIds"
                @signed-in="handleSignedIn"
              />
              <Separator />
            </template>

            <section class="space-y-2">
              <div class="min-w-0">
                <h3 class="text-sm font-medium">
                  {{ t("settings.providers.title") }}
                </h3>
                <p class="mt-0.5 text-xs text-muted-foreground">
                  {{ t("settings.providers.description") }}
                </p>
              </div>

              <!-- S1: the name, url and state are one activate control; the row's
                   other actions are siblings of it (a button inside a button is
                   invalid HTML). The active row is marked by the accent fill, not
                   by a rule: this pane rules only between its major blocks
                   (accounts | providers). -->
              <ul v-if="providers.length" class="space-y-0.5">
                <li
                  v-for="entry in providers"
                  :key="entry.id"
                  :class="[
                    'rounded-md transition-colors',
                    entry.id === activeProvider
                      ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                      : 'hover:bg-muted',
                  ]"
                >
                  <div class="flex items-center gap-1 px-2 py-1.5">
                    <button
                      type="button"
                      :aria-pressed="entry.id === activeProvider"
                      :disabled="saving || removing"
                      class="min-w-0 flex-1 rounded-sm text-left outline-hidden focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
                      @click="handleProviderActivate(entry.id)"
                    >
                      <span class="flex items-center justify-between gap-2">
                        <span class="flex min-w-0 items-baseline gap-2">
                          <span class="min-w-0 truncate text-sm font-medium">
                            {{ entry.name }}
                          </span>
                          <span
                            class="min-w-0 truncate font-mono text-xs text-muted-foreground"
                          >
                            {{ entry.id }}
                          </span>
                        </span>
                        <CheckIcon
                          v-if="entry.id === activeProvider"
                          class="size-4 shrink-0"
                          aria-hidden="true"
                        />
                      </span>
                      <span
                        class="mt-0.5 flex items-center justify-between gap-2 text-xs text-muted-foreground"
                      >
                        <span class="min-w-0 truncate" :title="entry.baseUrl">
                          {{ entry.baseUrl }}
                        </span>
                        <span class="shrink-0">
                          {{
                            t("settings.providers.models", {
                              count: modelCountOf(entry.id, entry.modelCount),
                            })
                          }}
                          ·
                          {{
                            entry.hasApiKey
                              ? t("settings.providers.keySet")
                              : t("settings.providers.noKey")
                          }}
                        </span>
                      </span>
                    </button>

                    <!-- S4 of `providers-pane-refinements.md`: the models of
                         this provider and its details live in one dialog, opened
                         from here. One dialog at a time. -->
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      :disabled="removing"
                      @click="openEditor(entry)"
                    >
                      {{ t("settings.providers.edit") }}
                    </Button>
                    <!-- S4: the api-key action of this row. -->
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      :aria-expanded="keyTarget === entry.id"
                      :disabled="removing"
                      @click="toggleKeyEditor(entry)"
                    >
                      {{ t("settings.providers.apiKey") }}
                    </Button>
                    <!-- S1/S3: removal, behind the pane's confirmation dialog. -->
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      :disabled="removing"
                      @click="requestRemove(entry)"
                    >
                      {{ t("settings.providers.remove") }}
                    </Button>
                  </div>

                  <!-- S4: the key is typed here and never prefilled — Rust has
                       no copy to send back, so the field always starts empty. -->
                  <form
                    v-if="keyTarget === entry.id"
                    class="flex flex-wrap items-center gap-2 px-2 pb-2"
                    @submit.prevent="handleSaveKey"
                  >
                    <Input
                      v-model="keyDraft"
                      type="password"
                      autocomplete="new-password"
                      spellcheck="false"
                      class="h-8 min-w-0 flex-1 text-sm"
                      :aria-label="
                        t('settings.providers.apiKey.label', { id: entry.id })
                      "
                      :placeholder="t('settings.providers.apiKey.placeholder')"
                    />
                    <Button
                      type="submit"
                      size="sm"
                      :disabled="!keyDraft.trim() || savingKey"
                    >
                      {{
                        savingKey
                          ? t("settings.providers.apiKey.saving")
                          : t("settings.providers.apiKey.save")
                      }}
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      :disabled="savingKey"
                      @click="closeKeyEditor"
                    >
                      {{ t("common.cancel") }}
                    </Button>
                    <p class="w-full text-xs text-muted-foreground">
                      {{ t("settings.providers.apiKey.description") }}
                    </p>
                  </form>
                </li>
              </ul>
              <p
                v-if="!providers.length && !catalogOnlyRows.length"
                class="text-xs text-muted-foreground"
              >
                {{ t("settings.providers.empty") }}
              </p>

              <!-- Providers pi's catalogue lists and `models.json` does not: a
                   subscription signed in from the accounts block above. There is
                   no configured entry of theirs to activate, key or remove, so
                   the row offers the one action that reads them — Edit. They are
                   separated from the list above by the row's own label, not by a
                   second rule. -->
              <ul v-if="catalogOnlyRows.length" class="space-y-0.5">
                <li
                  v-for="row in catalogOnlyRows"
                  :key="row.id"
                  :class="[
                    'rounded-md transition-colors',
                    row.id === activeProvider
                      ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                      : 'hover:bg-muted',
                  ]"
                >
                  <div class="flex items-center gap-1 px-2 py-1.5">
                    <button
                      type="button"
                      :aria-pressed="row.id === activeProvider"
                      :disabled="saving || removing"
                      class="min-w-0 flex-1 rounded-sm text-left outline-hidden focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
                      @click="handleProviderActivate(row.id)"
                    >
                      <span class="flex items-center justify-between gap-2">
                        <span class="flex min-w-0 items-baseline gap-2">
                          <span class="min-w-0 truncate text-sm font-medium">
                            {{ row.name }}
                          </span>
                          <span
                            class="min-w-0 truncate font-mono text-xs text-muted-foreground"
                          >
                            {{ row.id }}
                          </span>
                        </span>
                        <CheckIcon
                          v-if="row.id === activeProvider"
                          class="size-4 shrink-0"
                          aria-hidden="true"
                        />
                      </span>
                      <span
                        class="mt-0.5 flex items-center justify-between gap-2 text-xs text-muted-foreground"
                      >
                        <span class="min-w-0 truncate">
                          {{ t("settings.providers.catalog") }}
                        </span>
                        <span class="shrink-0">
                          {{
                            t("settings.providers.models", {
                              count: row.modelCount,
                            })
                          }}
                        </span>
                      </span>
                    </button>

                    <!-- The models of this provider, read-only: pi owns the
                         catalogue they come from. -->
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      :disabled="removing"
                      @click="openEditor(row)"
                    >
                      {{ t("settings.providers.edit") }}
                    </Button>
                  </div>
                </li>
              </ul>

              <!-- Informational line after a removal: nothing failed, so it is
                   not rendered as an error. -->
              <p v-if="warning" class="text-xs text-muted-foreground">
                {{ warning }}
              </p>
            </section>

            <!-- S5: this whole block is ON unless the build sets
                 VITE_ENABLE_CUSTOM_PROVIDERS to `0`/`false`/`off`; with the flag
                 off there is nothing to click and nothing to explain. It adds a
                 provider to the list above, so it sits inside this block without
                 a rule of its own. -->
            <template v-if="customProvidersEnabled">
              <section class="space-y-3">
                <Button
                  v-if="!showProviderForm"
                  type="button"
                  variant="outline"
                  size="sm"
                  @click="showProviderForm = true"
                >
                  <PlusIcon data-icon="inline-start" />
                  {{ t("settings.providers.custom.title") }}
                </Button>
                <ProviderForm
                  v-else
                  @saved="handleProviderSaved"
                  @cancel="showProviderForm = false"
                />
              </section>
            </template>

            <!-- S4 of `providers-pane-refinements.md`: the edit dialog of one
                 provider — its details (read-only: they belong to the file) and
                 the editor of its models. `Dialog` handles focus, Escape and the
                 focus trap, so the surface is keyboard reachable; the target is
                 resolved from the pane's list, so a refreshed model count shows
                 up here without reopening the dialog. -->
            <Dialog v-model:open="editOpen">
              <DialogContent
                v-if="editTarget"
                class="max-h-[min(85vh,44rem)] overflow-y-auto sm:max-w-xl"
              >
                <DialogHeader>
                  <DialogTitle>
                    {{ t("settings.providers.edit.title", { name: editTarget.name }) }}
                  </DialogTitle>
                  <DialogDescription>
                    {{ t("settings.providers.edit.description") }}
                  </DialogDescription>
                </DialogHeader>

                <dl class="grid gap-1.5 text-xs">
                  <div class="flex gap-3">
                    <dt class="w-24 shrink-0 text-muted-foreground">
                      {{ t("settings.providers.edit.id") }}
                    </dt>
                    <dd class="min-w-0 truncate font-mono">{{ editTarget.id }}</dd>
                  </div>
                  <div v-if="editTarget.baseUrl !== null" class="flex gap-3">
                    <dt class="w-24 shrink-0 text-muted-foreground">
                      {{ t("settings.providers.edit.baseUrl") }}
                    </dt>
                    <dd
                      class="min-w-0 truncate font-mono"
                      :title="editTarget.baseUrl"
                    >
                      {{ editTarget.baseUrl }}
                    </dd>
                  </div>
                  <div class="flex gap-3">
                    <dt class="w-24 shrink-0 text-muted-foreground">
                      {{ t("settings.providers.edit.models") }}
                    </dt>
                    <dd>{{ editTarget.modelCount }}</dd>
                  </div>
                </dl>

                <Separator />

                <ProviderModelsEditor
                  :provider="editTarget"
                  :catalog-only="editTarget.catalogOnly"
                  :refresh="editorRefresh"
                  @changed="handleModelsChanged"
                />

                <DialogFooter>
                  <DialogClose as-child>
                    <Button variant="outline">
                      {{ t("common.close") }}
                    </Button>
                  </DialogClose>
                </DialogFooter>
              </DialogContent>
            </Dialog>

            <!-- S1/S3: one confirmation dialog for the whole list, the same
                 pattern the session rows use: the trigger only says which
                 provider it is about. -->
            <Dialog v-model:open="removalOpen">
              <DialogContent>
                <DialogHeader>
                  <DialogTitle>
                    {{ t("settings.providers.remove.title") }}
                  </DialogTitle>
                  <DialogDescription>
                    {{
                      t("settings.providers.remove.description", {
                        id: pendingRemoval?.id ?? "",
                      })
                    }}
                  </DialogDescription>
                </DialogHeader>
                <DialogFooter>
                  <DialogClose as-child>
                    <Button variant="outline">
                      {{ t("common.cancel") }}
                    </Button>
                  </DialogClose>
                  <Button
                    variant="destructive"
                    :disabled="removing"
                    @click="confirmRemove"
                  >
                    {{ t("settings.providers.remove") }}
                  </Button>
                </DialogFooter>
              </DialogContent>
            </Dialog>
          </div>

          <!-- S1 of `tool-drawer-and-mcp.md`: the MCP pane, a sibling of
               Providers because servers and providers are two different things
               pi reads from two different files. It owns its own read/write
               cycle (list, add, remove, login/logout) and its own error line, so
               this component only decides that it is on screen. -->
          <McpSection v-else-if="section === 'mcp'" />

          <!-- S2 of `agent-profiles.md`: the Subagents pane. Same arrangement as
               MCP — it owns its read, its draft and its one write — and it is a
               section rather than a block inside General because a profile is a
               stored definition, not a preference of the chat. -->
          <SubagentsSection v-else-if="section === 'subagents'" />

          <MemorySection v-else-if="section === 'memory'" />

          <SkillsSection v-else />

          <Alert v-if="error" variant="destructive" class="mt-4">
            <AlertDescription>{{ error }}</AlertDescription>
          </Alert>
        </div>
      </div>

      <!-- Persistent modal footer across all sections: status on the left,
           Discard (when dirty) and Save on the right. -->
      <div
        class="flex flex-wrap items-center justify-between gap-3 border-t border-border bg-sidebar px-5 py-3"
      >
        <p class="min-w-0 truncate text-xs text-muted-foreground">
          <template v-if="saving || agentsConfigSaving">
            {{ t("settings.subagents.saving") }}
          </template>
          <template v-else-if="hasUnsavedDrafts">
            <span class="text-warning">{{ t("settings.subagents.dirty") }}</span>
          </template>
          <template v-else-if="savedNotice">
            <span class="text-primary">{{ t("settings.subagents.saved") }}</span>
          </template>
          <span v-else-if="configPath" class="font-mono text-[10.5px]">
            {{ configPath }}
          </span>
        </p>

        <div class="flex shrink-0 items-center gap-2">
          <Button
            v-if="hasUnsavedDrafts"
            type="button"
            variant="ghost"
            size="sm"
            class="text-xs text-muted-foreground hover:text-foreground"
            :disabled="saving || agentsConfigSaving"
            @click="handleDiscardAll"
          >
            {{ t("settings.subagents.discard") }}
          </Button>
          <Button
            type="button"
            size="sm"
            class="rounded-lg bg-primary px-4 text-xs font-semibold text-primary-foreground hover:bg-primary/90"
            :disabled="loading || saving || agentsConfigSaving"
            @click="handleSaveAll"
          >
            {{
              saving || agentsConfigSaving
                ? t("settings.subagents.saving")
                : t("settings.subagents.save")
            }}
          </Button>
        </div>
      </div>

      <DialogDescription class="sr-only">
        {{ t("settings.description") }}
      </DialogDescription>
    </DialogContent>
  </Dialog>
</template>
