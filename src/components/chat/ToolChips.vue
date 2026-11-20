<script setup lang="ts">
// The chips of one step's tool calls — the compact badges the transcript has
// always rendered (`navigate · running`, `eval · done`, a failed one in the
// destructive variant).
//
// One component, three call sites: the step running right now shows them outside
// the turn's fold, a finished step shows them inside it, and `ToolSummary`
// counts them on the collapsed line of the turn. Extracting them is what keeps
// the disclosure honest — expanding a turn reveals exactly the same markup, not a
// look-alike.
//
// H4 of `odd/tasks/subagent-helpers.md` is the one exception: a `run_agent` call is
// a delegation, not a tool step of this chat, so it renders as the helper card in
// the same position and order the chip would have taken. Everything else stays a
// chip.
import type { Component } from "vue";
import { CheckIcon, CircleAlertIcon, LoaderIcon } from "@lucide/vue";
import SubagentCard from "@/components/chat/SubagentCard.vue";
import { Badge } from "@/components/ui/badge";
import { t } from "@/lib/i18n";
import type { ToolEntry } from "@/lib/session-store";

withDefaults(
  defineProps<{
    /** The step's tool calls, oldest first. */
    tools: ToolEntry[];
    /** The chat that made the calls; a helper card points at its child through it. */
    parentSessionId?: string | null;
    /** The workspace of that chat. */
    workspaceId?: number | null;
  }>(),
  { parentSessionId: null, workspaceId: null },
);

/** The one tool whose call is a delegation to a child session (H2, S2). */
const SUBAGENT_RUN_TOOL = "run_agent";

function isDelegation(tool: ToolEntry): boolean {
  return tool.toolName === SUBAGENT_RUN_TOOL;
}

function toolIcon(state: ToolEntry["state"]): Component {
  switch (state) {
    case "running":
      return LoaderIcon;
    case "done":
      return CheckIcon;
    case "error":
      return CircleAlertIcon;
  }
}

function toolBadgeVariant(
  state: ToolEntry["state"],
): "secondary" | "outline" | "destructive" {
  switch (state) {
    case "running":
      return "secondary";
    case "done":
      return "outline";
    case "error":
      return "destructive";
  }
}

function toolStateLabel(state: ToolEntry["state"]): string {
  switch (state) {
    case "running":
      return t("chat.tool.running");
    case "done":
      return t("chat.tool.done");
    case "error":
      return t("chat.tool.failed");
  }
}
</script>

<template>
  <div class="flex flex-wrap items-center gap-2">
    <template v-for="tool in tools" :key="tool.toolCallId">
      <SubagentCard
        v-if="isDelegation(tool)"
        :tool="tool"
        :parent-session-id="parentSessionId"
        :workspace-id="workspaceId"
      />
      <Badge v-else :variant="toolBadgeVariant(tool.state)">
        <component :is="toolIcon(tool.state)" />
        {{ tool.toolName }} · {{ toolStateLabel(tool.state) }}
      </Badge>
    </template>
  </div>
</template>
