<script setup lang="ts">
// H4 of `odd/tasks/subagent-helpers.md`: a `run_agent` call of the delegating turn
// renders as this card instead of a bare chip, so the delegation is visible where
// it happened (S2: "la tarjeta del subagente vive en el turno que lo lanzó").
//
// The status it shows is the call's own, and that is honest by construction:
// `run_agent` blocks until the child answers, so "the tool call is running" and
// "the child is working" are the same fact. `done`/`failed` come from the call's
// own end, with the extension's word (`details.status`) when it differs from the
// generic one.
//
// v1 ships no per-child Stop. The children of an extension run in-process inside
// the parent's pi, so stopping the parent chat is what stops them, and the model
// has `agent_cancel` for a single one: a Stop here would need a channel that does
// not exist yet. The card shows the status and "Abrir" only.
import { computed, onMounted } from "vue";
import { BotIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import type { ToolEntry } from "@/lib/session-store";
import {
  loadSubagentChildren,
  openSubagentView,
  resolveSubagentChild,
  subagentStatusDot,
  subagentStatusLabel,
  subagentStatusWord,
} from "@/lib/subagent-store";
import type { SubagentStatus } from "@/contracts/types";

const props = defineProps<{
  /** The `run_agent` chip this card stands for. */
  tool: ToolEntry;
  /** The chat that made the call; the child's row is nested under it. */
  parentSessionId: string | null;
  /** Workspace of both, which every child query takes. */
  workspaceId: number | null;
}>();

/** What the card calls the delegation: the agent definition it ran. */
const agentName = computed<string>(
  () => props.tool.subagent?.agent ?? props.tool.subagent?.name ?? props.tool.toolName,
);

/**
 * The status of the call. Only three states can be told from here: the call is on
 * its way, it ended in an error, or it ended — the extension's own word refines the
 * last one (`failed` for a run that answered with a failure and no error flag).
 */
const status = computed<SubagentStatus>(() => {
  if (props.tool.state === "running") return "running";
  if (props.tool.state === "error") return "failed";
  const word = props.tool.subagent?.outcome;
  return word === null || word === undefined ? "done" : subagentStatusWord(word);
});

const statusLabel = computed(() => subagentStatusLabel(status.value));
const dotClass = computed(() => subagentStatusDot(status.value));

/**
 * The child this call started, matched against the parent's known children.
 *
 * `sessionPath` is the reliable half — the extension answers the call with the
 * child's own pi session file and the index row carries that same file — so the
 * match is the child itself. The agent name only answers when the file is not known
 * yet, and then the newest child of that agent wins. `null` means the card cannot
 * point at a child (yet): the status still shows and "Abrir" is simply not offered,
 * which is the honest state instead of a button that opens something else.
 */
const child = computed(() =>
  resolveSubagentChild(props.parentSessionId, {
    sessionPath: props.tool.subagent?.sessionPath ?? null,
    agent: props.tool.subagent?.agent ?? null,
  }),
);

function handleOpen(): void {
  const target = child.value;
  const parent = props.parentSessionId;
  const workspace = props.workspaceId;
  if (target === null || parent === null || workspace === null) return;
  openSubagentView(target, parent, workspace);
}

// The card is mounted by the turn that made the call, so it is the earliest point
// at which the chat panel can ask for this parent's children — the sidebar may
// never have been expanded in this workspace. `force` is off: a cached answer is
// enough, and the poll takes over from there while anything runs.
onMounted(() => {
  const parent = props.parentSessionId;
  const workspace = props.workspaceId;
  if (parent === null || workspace === null) return;
  if (props.tool.subagent === undefined) return;
  void loadSubagentChildren(workspace, [parent]);
});
</script>

<template>
  <div
    class="flex min-w-0 items-center gap-2 rounded-md border border-border/70 bg-muted/20 px-2 py-1.5"
  >
    <BotIcon class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
    <div class="flex min-w-0 flex-col">
      <span class="truncate text-xs font-medium">{{ agentName }}</span>
      <span
        class="flex items-center gap-1 text-[10px] text-muted-foreground"
        :title="statusLabel"
      >
        <span
          class="size-1.5 shrink-0 rounded-full"
          :class="dotClass"
          aria-hidden="true"
        />
        <span class="truncate">{{ statusLabel }}</span>
      </span>
    </div>
    <Button
      v-if="child !== null"
      variant="outline"
      size="xs"
      class="ml-auto shrink-0"
      @click="handleOpen"
    >
      {{ t("chat.subagent.open") }}
    </Button>
  </div>
</template>
