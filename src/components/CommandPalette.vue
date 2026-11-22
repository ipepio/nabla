<script setup lang="ts">
// Global command palette (⌘K / Ctrl+K): instant search across workspaces,
// sessions and shell actions. Lazy-loaded on first open so it costs zero bytes
// during app startup.
import { computed, nextTick, ref, watch } from "vue";
import {
  CirclePlusIcon,
  FileSearchIcon,
  FolderIcon,
  MessageSquareIcon,
  PanelRightIcon,
  SearchIcon,
  SettingsIcon,
} from "@lucide/vue";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { listSessions, listWorkspaces, searchSessions } from "@/contracts/commands";
import type { SessionSearchHit, SessionSummary, WorkspaceInfo } from "@/contracts/types";
import { t } from "@/lib/i18n";

const open = defineModel<boolean>("open", { default: false });

const emit = defineEmits<{
  (e: "selectWorkspace", workspace: WorkspaceInfo): void;
  (e: "selectSession", payload: { workspace: WorkspaceInfo; session: SessionSummary }): void;
  (e: "newSession"): void;
  (e: "toggleTools"): void;
  (e: "openSettings"): void;
}>();

interface PaletteItem {
  id: string;
  group: "workspaces" | "sessions" | "messages" | "actions";
  label: string;
  subtitle?: string;
  shortcut?: string;
  run: () => void;
}

const query = ref("");
const loading = ref(false);
const workspaces = ref<WorkspaceInfo[]>([]);
const sessionsByWorkspace = ref<Record<number, SessionSummary[]>>({});
const searchHits = ref<SessionSearchHit[]>([]);
let searchTimer: number | null = null;
const activeIndex = ref(0);
const inputRef = ref<HTMLInputElement | null>(null);

const isMac =
  typeof navigator !== "undefined" &&
  /mac/i.test(navigator.platform || navigator.userAgent);
const modKey = isMac ? "⌘" : "Ctrl+";

async function loadEntries(): Promise<void> {
  loading.value = true;
  try {
    const wsList = await listWorkspaces();
    workspaces.value = wsList;
    const entries = await Promise.all(
      wsList.map(async (ws) => {
        try {
          const sessions = await listSessions(ws.id);
          return [ws.id, sessions] as const;
        } catch {
          return [ws.id, [] as SessionSummary[]] as const;
        }
      }),
    );
    sessionsByWorkspace.value = Object.fromEntries(entries);
  } catch {
    workspaces.value = [];
    sessionsByWorkspace.value = {};
  } finally {
    loading.value = false;
  }
}

const items = computed<PaletteItem[]>(() => {
  const q = query.value.trim().toLowerCase();
  const result: PaletteItem[] = [];

  for (const ws of workspaces.value) {
    if (
      q === "" ||
      ws.name.toLowerCase().includes(q) ||
      ws.path.toLowerCase().includes(q)
    ) {
      result.push({
        id: `ws-${ws.id}`,
        group: "workspaces",
        label: ws.name,
        subtitle: ws.path,
        run: () => {
          emit("selectWorkspace", ws);
          open.value = false;
        },
      });
    }
  }

  const shownSessionIds = new Set<number>();
  for (const ws of workspaces.value) {
    const sessions = sessionsByWorkspace.value[ws.id] ?? [];
    for (const session of sessions) {
      const title = session.title?.trim() || t("session.untitled");
      if (
        q === "" ||
        title.toLowerCase().includes(q) ||
        ws.name.toLowerCase().includes(q)
      ) {
        shownSessionIds.add(session.id);
        result.push({
          id: `session-${session.id}`,
          group: "sessions",
          label: title,
          subtitle: ws.name,
          run: () => {
            emit("selectSession", { workspace: ws, session });
            open.value = false;
          },
        });
      }
    }
  }

  // Full-text FTS5 hits inside past conversation transcripts.
  for (const hit of searchHits.value) {
    if (shownSessionIds.has(hit.sessionId)) continue;
    const ws = workspaces.value.find((w) => w.id === hit.workspaceId);
    const session = (sessionsByWorkspace.value[hit.workspaceId] ?? []).find(
      (s) => s.id === hit.sessionId,
    );
    if (!ws || !session) continue;
    result.push({
      id: `fts-${hit.sessionId}`,
      group: "messages",
      label: hit.sessionTitle?.trim() || t("session.untitled"),
      subtitle: `${hit.workspaceName} · ${hit.snippet}`,
      run: () => {
        emit("selectSession", { workspace: ws, session });
        open.value = false;
      },
    });
  }

  const actions: PaletteItem[] = [
    {
      id: "action-new-session",
      group: "actions",
      label: t("palette.action.newSession"),
      shortcut: `${modKey}N`,
      run: () => {
        emit("newSession");
        open.value = false;
      },
    },
    {
      id: "action-toggle-tools",
      group: "actions",
      label: t("palette.action.toggleTools"),
      shortcut: `${modKey}.`,
      run: () => {
        emit("toggleTools");
        open.value = false;
      },
    },
    {
      id: "action-settings",
      group: "actions",
      label: t("palette.action.settings"),
      shortcut: `${modKey},`,
      run: () => {
        emit("openSettings");
        open.value = false;
      },
    },
  ];

  for (const action of actions) {
    if (q === "" || action.label.toLowerCase().includes(q)) {
      result.push(action);
    }
  }

  return result;
});

watch(query, (next) => {
  if (searchTimer !== null) window.clearTimeout(searchTimer);
  const trimmed = next.trim();
  if (trimmed.length < 2) {
    searchHits.value = [];
    return;
  }
  searchTimer = window.setTimeout(async () => {
    try {
      const hits = await searchSessions(trimmed, 10);
      if (query.value.trim() === trimmed) searchHits.value = hits;
    } catch {
      searchHits.value = [];
    }
  }, 160);
});

watch(items, (next) => {
  if (activeIndex.value >= next.length) activeIndex.value = 0;
});

watch(
  open,
  async (isOpen) => {
    if (!isOpen) return;
    query.value = "";
    searchHits.value = [];
    activeIndex.value = 0;
    void loadEntries();
    await nextTick();
    inputRef.value?.focus();
  },
  { immediate: true },
);

function onKeydown(event: KeyboardEvent): void {
  const list = items.value;
  if (list.length === 0) return;
  if (event.key === "ArrowDown") {
    event.preventDefault();
    activeIndex.value = (activeIndex.value + 1) % list.length;
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    activeIndex.value = (activeIndex.value - 1 + list.length) % list.length;
  } else if (event.key === "Enter") {
    event.preventDefault();
    list[activeIndex.value]?.run();
  }
}

function itemIcon(item: PaletteItem) {
  if (item.group === "workspaces") return FolderIcon;
  if (item.group === "sessions") return MessageSquareIcon;
  if (item.group === "messages") return FileSearchIcon;
  if (item.id === "action-new-session") return CirclePlusIcon;
  if (item.id === "action-toggle-tools") return PanelRightIcon;
  return SettingsIcon;
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent
      :show-close-button="false"
      class="top-[22%] flex w-full max-w-xl translate-y-0 flex-col gap-0 overflow-hidden rounded-2xl border border-border bg-card p-0 shadow-2xl sm:max-w-xl"
    >
      <DialogTitle class="sr-only">{{ t("palette.title") }}</DialogTitle>
      <DialogDescription class="sr-only">
        {{ t("palette.placeholder") }}
      </DialogDescription>

      <div class="flex items-center gap-2.5 border-b border-border px-4 py-3.5">
        <SearchIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <input
          ref="inputRef"
          v-model="query"
          type="text"
          autocomplete="off"
          spellcheck="false"
          :placeholder="t('palette.placeholder')"
          :aria-label="t('palette.title')"
          class="min-w-0 flex-1 bg-transparent text-sm text-foreground placeholder:text-muted-foreground outline-none"
          @keydown="onKeydown"
        />
        <kbd
          class="shrink-0 rounded border border-border bg-secondary px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground"
        >
          ESC
        </kbd>
      </div>

      <div class="max-h-80 overflow-y-auto p-2">
        <p
          v-if="loading && items.length === 0"
          class="px-3 py-6 text-center text-xs text-muted-foreground"
        >
          {{ t("common.loading") }}
        </p>
        <p
          v-else-if="items.length === 0"
          class="px-3 py-6 text-center text-xs text-muted-foreground"
        >
          {{ t("palette.empty") }}
        </p>
        <ul v-else role="listbox" class="flex flex-col gap-0.5">
          <li v-for="(item, index) in items" :key="item.id">
            <button
              type="button"
              role="option"
              :aria-selected="index === activeIndex"
              class="flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-xs transition-colors outline-hidden"
              :class="
                index === activeIndex
                  ? 'bg-sidebar-accent text-foreground'
                  : 'text-muted-foreground hover:bg-secondary hover:text-foreground'
              "
              @mouseenter="activeIndex = index"
              @click="item.run()"
            >
              <component
                :is="itemIcon(item)"
                class="size-3.5 shrink-0"
                :class="index === activeIndex ? 'text-primary' : 'text-muted-foreground'"
                aria-hidden="true"
              />
              <span class="min-w-0 flex-1 truncate font-medium text-foreground">
                {{ item.label }}
              </span>
              <span
                v-if="item.subtitle"
                class="max-w-44 shrink-0 truncate font-mono text-[10px] text-muted-foreground"
              >
                {{ item.subtitle }}
              </span>
              <kbd
                v-if="item.shortcut"
                class="shrink-0 rounded border border-border bg-secondary px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground"
              >
                {{ item.shortcut }}
              </kbd>
            </button>
          </li>
        </ul>
      </div>
    </DialogContent>
  </Dialog>
</template>
