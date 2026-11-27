<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { GaugeIcon, RefreshCwIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { commandErrorMessage, getUsageSnapshot } from "@/contracts/commands";
import type {
  LocalUsageRow,
  ProviderQuotaSnapshot,
  QuotaWindow,
  UsageSnapshot,
} from "@/contracts/types";

const props = defineProps<{
  /** Active provider id from the composer/session (e.g. `anthropic`, `openai-codex`, `acp:antigravity`). */
  activeProvider?: string | null;
}>();

const open = ref(false);
const loading = ref(false);
const error = ref<string | null>(null);
const snapshot = ref<UsageSnapshot | null>(null);
const selectedTab = ref<string>("all");

/** Maps a session/composer provider id to its corresponding quota providerId. */
function mapProviderToQuotaId(provider: string | null | undefined): string | null {
  if (!provider) return null;
  const p = provider.toLowerCase();
  if (p.includes("openai") || p.includes("codex") || p.includes("chatgpt")) return "openai";
  if (p.includes("anthropic") || p.includes("claude")) return "claude";
  if (p.includes("antigravity")) return "antigravity";
  return p;
}

const activeQuotaId = computed<string | null>(() => mapProviderToQuotaId(props.activeProvider));

function syncTabToActiveProvider(): void {
  const s = snapshot.value;
  const target = activeQuotaId.value;
  if (s && target && s.quotas.some((q) => q.providerId === target)) {
    selectedTab.value = target;
  } else {
    selectedTab.value = "all";
  }
}

async function refresh(): Promise<void> {
  if (loading.value) return;
  loading.value = true;
  error.value = null;
  try {
    snapshot.value = await getUsageSnapshot();
    if (selectedTab.value === "all") {
      syncTabToActiveProvider();
    }
  } catch (err) {
    error.value = commandErrorMessage(err);
  } finally {
    loading.value = false;
  }
}

function handleOpenChange(next: boolean): void {
  open.value = next;
  if (next) {
    syncTabToActiveProvider();
    void refresh();
  }
}

watch(activeQuotaId, () => {
  if (!open.value) {
    syncTabToActiveProvider();
  }
});

const tabs = computed(() => {
  const s = snapshot.value;
  const items: { id: string; label: string }[] = [{ id: "all", label: "All" }];
  if (!s) return items;
  for (const q of s.quotas) {
    items.push({ id: q.providerId, label: q.name });
  }
  return items;
});

const visibleQuotas = computed<ProviderQuotaSnapshot[]>(() => {
  const s = snapshot.value;
  if (!s) return [];
  if (selectedTab.value === "all") return s.quotas;
  return s.quotas.filter((q) => q.providerId === selectedTab.value);
});

const visibleLocal = computed<LocalUsageRow[]>(() => {
  const s = snapshot.value;
  if (!s) return [];
  if (selectedTab.value === "all") return s.local;
  const needle = selectedTab.value.toLowerCase();
  return s.local.filter((r) => {
    const p = r.provider.toLowerCase();
    if (needle === "openai") return p.includes("openai") || p.includes("codex");
    if (needle === "claude") return p.includes("anthropic") || p.includes("claude");
    if (needle === "antigravity") return p.includes("antigravity") || p.includes("gemini");
    return p.includes(needle);
  });
});

/**
 * Header pill quota: only shows remaining % for the currently selected provider
 * (never a misleading percentage from an unrelated provider).
 */
const activeProviderBadge = computed<{ label: string; usedPercent: number } | null>(() => {
  const s = snapshot.value;
  const target = activeQuotaId.value;
  if (!s || !target) return null;
  const quota = s.quotas.find((q) => q.providerId === target);
  if (!quota || quota.windows.length === 0) return null;
  let max: number | null = null;
  for (const w of quota.windows) {
    if (max === null || w.usedPercent > max) {
      max = w.usedPercent;
    }
  }
  if (max === null) return null;
  return { label: quota.name, usedPercent: Math.round(max) };
});

function remainingPercent(used: number): number {
  return Math.max(0, Math.round(100 - used));
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

function formatCost(usd: number): string {
  if (usd <= 0) return "$0.00";
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(2)}`;
}

function formatReset(w: QuotaWindow): string {
  let secs = w.resetInSecs;
  if (secs === null && w.resetAt) {
    const target = Date.parse(w.resetAt);
    if (!Number.isNaN(target)) {
      secs = Math.max(0, Math.floor((target - Date.now()) / 1000));
    }
  }
  if (secs === null || secs < 0) return "";
  const days = Math.floor(secs / 86400);
  const hours = Math.floor((secs % 86400) / 3600);
  const mins = Math.floor((secs % 3600) / 60);
  if (days > 0) return `resets in ${days}d ${hours}h`;
  if (hours > 0) return `resets in ${hours}h ${mins}m`;
  return `resets in ${mins}m`;
}

function barColorClass(pct: number): string {
  if (pct >= 90) return "bg-destructive";
  if (pct >= 75) return "bg-warning";
  return "bg-primary";
}
</script>

<template>
  <DropdownMenu :open="open" @update:open="handleOpenChange">
    <DropdownMenuTrigger as-child>
      <Button
        variant="ghost"
        size="sm"
        class="h-7 gap-1.5 px-2 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground"
        title="Usage & Quotas"
        aria-label="Usage & Quotas"
      >
        <GaugeIcon class="size-3.5 shrink-0" />
        <span
          v-if="activeProviderBadge !== null"
          class="font-mono text-[10px] tabular-nums"
          :title="`${activeProviderBadge.label}: ${remainingPercent(activeProviderBadge.usedPercent)}% left`"
        >
          {{ remainingPercent(activeProviderBadge.usedPercent) }}% left
        </span>
      </Button>
    </DropdownMenuTrigger>

    <DropdownMenuContent align="end" side="bottom" class="w-84 p-3">
      <div class="flex items-center justify-between gap-2 pb-2 border-b border-border">
        <span class="text-xs font-semibold text-foreground">Usage &amp; Quotas</span>
        <Button
          variant="ghost"
          size="icon-xs"
          :disabled="loading"
          title="Refresh usage"
          @click.stop="refresh"
        >
          <RefreshCwIcon class="size-3.5" :class="loading ? 'animate-spin' : ''" />
        </Button>
      </div>

      <!-- Provider switcher pills (CodexBar style) -->
      <div v-if="tabs.length > 1" class="mt-2 flex flex-wrap items-center gap-1">
        <button
          v-for="tab in tabs"
          :key="tab.id"
          type="button"
          class="rounded-md px-2 py-0.5 text-[11px] font-medium transition-colors"
          :class="
            selectedTab === tab.id
              ? 'bg-primary text-primary-foreground'
              : 'bg-secondary/60 text-muted-foreground hover:bg-secondary hover:text-foreground'
          "
          @click.stop="selectedTab = tab.id"
        >
          {{ tab.label }}
        </button>
      </div>

      <div
        v-if="error"
        class="mt-2 rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-[11px] text-destructive"
      >
        {{ error }}
      </div>

      <div v-else-if="loading && !snapshot" class="py-6 text-center text-xs text-muted-foreground">
        Loading usage…
      </div>

      <div v-else-if="snapshot" class="mt-2.5 space-y-3">
        <!-- Live subscription quota bars -->
        <div v-if="visibleQuotas.length > 0" class="space-y-2.5">
          <div
            v-for="quota in visibleQuotas"
            :key="quota.providerId"
            class="rounded-lg border border-border bg-secondary/40 p-2.5 space-y-2"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="text-xs font-medium text-foreground">{{ quota.name }}</span>
              <span
                v-if="quota.plan"
                class="rounded-full bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium uppercase text-primary"
              >
                {{ quota.plan }}
              </span>
            </div>

            <div v-for="win in quota.windows" :key="win.label" class="space-y-1">
              <div class="flex items-center justify-between text-[11px]">
                <span class="font-medium text-muted-foreground">{{ win.label }}</span>
                <span class="font-mono tabular-nums text-foreground">
                  {{ remainingPercent(win.usedPercent) }}% left
                  <span class="text-[10px] text-muted-foreground">
                    (<template v-if="win.detail">{{ win.detail }}</template><template v-else>{{ win.usedPercent.toFixed(0) }}% used</template><template v-if="formatReset(win)"> · {{ formatReset(win) }}</template>)
                  </span>
                </span>
              </div>
              <div class="h-1.5 w-full overflow-hidden rounded-full bg-muted">
                <div
                  class="h-full rounded-full transition-all"
                  :class="barColorClass(win.usedPercent)"
                  :style="{ width: `${Math.min(100, Math.max(0, win.usedPercent))}%` }"
                />
              </div>
            </div>

            <p
              v-if="quota.windows.length === 0 && quota.error"
              class="text-[11px] text-muted-foreground"
            >
              {{ quota.error }}
            </p>
          </div>
        </div>

        <!-- Local Nabla token & cost summary -->
        <div class="grid grid-cols-2 gap-2">
          <div class="rounded-lg border border-border bg-secondary/40 px-2.5 py-2">
            <div class="text-[10px] text-muted-foreground">Today</div>
            <div class="mt-0.5 font-mono text-xs font-semibold tabular-nums text-foreground">
              {{ formatTokens(snapshot.totalTodayTokens) }} tok
            </div>
            <div class="font-mono text-[10px] tabular-nums text-muted-foreground">
              {{ formatCost(snapshot.totalTodayCost) }}
            </div>
          </div>
          <div class="rounded-lg border border-border bg-secondary/40 px-2.5 py-2">
            <div class="text-[10px] text-muted-foreground">Last 7 days</div>
            <div class="mt-0.5 font-mono text-xs font-semibold tabular-nums text-foreground">
              {{ formatTokens(snapshot.totalWeekTokens) }} tok
            </div>
            <div class="font-mono text-[10px] tabular-nums text-muted-foreground">
              {{ formatCost(snapshot.totalWeekCost) }}
            </div>
          </div>
        </div>

        <!-- Top models breakdown -->
        <div v-if="visibleLocal.length > 0" class="space-y-1">
          <div class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            By model (7d)
          </div>
          <div class="max-h-36 space-y-1 overflow-y-auto pr-0.5">
            <div
              v-for="row in visibleLocal.slice(0, 8)"
              :key="`${row.provider}:${row.model}`"
              class="flex items-center justify-between gap-2 rounded-md px-1.5 py-1 text-[11px] hover:bg-secondary/50"
            >
              <span class="min-w-0 truncate text-foreground" :title="`${row.provider} / ${row.model}`">
                <span class="text-muted-foreground">{{ row.provider }} / </span>{{ row.model }}
              </span>
              <span class="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground">
                {{ formatTokens(row.weekTokens) }} · {{ formatCost(row.weekCost) }}
              </span>
            </div>
          </div>
        </div>
        <p
          v-else-if="visibleQuotas.length === 0"
          class="py-2 text-center text-[11px] text-muted-foreground"
        >
          No recent usage recorded yet.
        </p>
      </div>
    </DropdownMenuContent>
  </DropdownMenu>
</template>
