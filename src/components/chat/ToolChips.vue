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
import type { Component } from "vue";
import { CheckIcon, CircleAlertIcon, LoaderIcon } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { t } from "@/lib/i18n";
import type { ToolEntry } from "@/lib/session-store";

defineProps<{
  /** The step's tool calls, oldest first. */
  tools: ToolEntry[];
}>();

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
  <div class="flex flex-wrap gap-2">
    <Badge
      v-for="tool in tools"
      :key="tool.toolCallId"
      :variant="toolBadgeVariant(tool.state)"
    >
      <component :is="toolIcon(tool.state)" />
      {{ tool.toolName }} · {{ toolStateLabel(tool.state) }}
    </Badge>
  </div>
</template>
