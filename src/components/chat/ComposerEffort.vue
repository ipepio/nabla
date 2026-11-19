<script setup lang="ts">
// Effort selector of the composer (S2 of `odd/tasks/composer-inputs.md`): the
// thinking level the session's agent is reasoning at, next to the permission
// chip — the row where the human decides what the next turn costs.
//
// Three sources, in this order, and the order is the whole design:
//
//   1. what pi CONFIRMED for this session (`thinking_level_changed`, read from
//      `session-store.ts`) — pi clamps a request to what the model accepts, so
//      its confirmation is the only honest value to render;
//   2. the levels the session's model accepts, asked of that live session
//      (`session_thinking_levels` → pi's `get_available_thinking_levels`); a
//      session with no process yet answers nothing, so the documented list is the
//      fallback (pi clamps whatever it is given, so nothing here can break);
//   3. `settings.json`'s persisted level, which is what a fresh session starts
//      with: it is also the value shown before pi has said anything.
//
// A pick applies to the live session (no respawn) AND is persisted, so the next
// session starts there too; with no session open the pick only changes that
// default, which is all there is to change.
//
// The one case this cannot see: pi emits `thinking_level_changed` only when the
// effective level actually changes, so a request it clamps back to the level
// already in effect leaves the selector showing the requested word until pi
// reports another level. Reading the level back is not in the protocol (only
// `cycle_thinking_level` answers it, and it cycles), so the alternative would be
// inventing a value — this stays with pi's event and shows the stored default
// until it arrives.
//
// No `watch`: the levels are re-read when the menu OPENS (one interaction, one
// read — cheap and always current, including after switching sessions), and the
// current level is a getter over reactive state.
import { computed, ref } from "vue";
import { CheckIcon, ChevronDownIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  sessionThinkingLevels,
  setSessionThinkingLevel,
} from "@/contracts/commands";
import type { ThinkingLevel } from "@/contracts/types";
import { t } from "@/lib/i18n";
import {
  setThinkingLevel,
  THINKING_LEVELS,
  thinkingLevel,
} from "@/lib/preferences";
import { activeSession, sessionThinkingLevel } from "@/lib/session-store";

/** The levels the menu offers: pi's answer while a session can give one. */
const levels = ref<ThinkingLevel[]>([...THINKING_LEVELS]);

/** A pick in flight: the trigger is inert so a double click cannot race a write. */
const saving = ref(false);

/**
 * The level on screen: pi's confirmation for this session, else the persisted
 * default. `null` until pi has said anything is why the default is here at all.
 */
const current = computed<ThinkingLevel>(
  () => sessionThinkingLevel(activeSession.value?.sessionId ?? null) ?? thinkingLevel.value,
);

/** Hover/aria text: the level, and what the control does. */
const title = computed(() => t("chat.composer.effortHint"));

/**
 * Reads the levels of the running model. Called when the menu opens — never on
 * every render — and tolerant by design: an unreachable answer (the session is
 * still spawning, or already gone) leaves the documented list in place instead of
 * emptying the menu or reporting an error nobody can act on.
 */
async function refreshLevels(open: boolean): Promise<void> {
  if (!open) return;
  const session = activeSession.value;
  if (session === null) {
    levels.value = [...THINKING_LEVELS];
    return;
  }
  try {
    const answer = await sessionThinkingLevels(
      session.workspaceId,
      Number(session.sessionId),
    );
    levels.value = answer.length > 0 ? answer : [...THINKING_LEVELS];
  } catch {
    levels.value = [...THINKING_LEVELS];
  }
}

/**
 * Applies one level. With a session the command does both halves — live child and
 * persisted default — and answers what the file now holds, so the shared default
 * is updated from that answer rather than from the wish. Without a session there
 * is nothing live to change, and the default is the change.
 */
async function choose(level: ThinkingLevel): Promise<void> {
  if (saving.value) return;
  const session = activeSession.value;
  if (session !== null && level === current.value) return;
  if (session === null && level === thinkingLevel.value) return;
  saving.value = true;
  try {
    if (session === null) {
      await setThinkingLevel(level);
    } else {
      thinkingLevel.value = await setSessionThinkingLevel(
        session.workspaceId,
        Number(session.sessionId),
        level,
      );
    }
  } catch {
    // A failed apply changed nothing, and the level on screen comes from pi's own
    // event: painting the wish would claim the agent reasons at a level it never
    // adopted.
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <DropdownMenu @update:open="refreshLevels">
    <DropdownMenuTrigger as-child>
      <Button
        variant="ghost"
        size="sm"
        class="h-8 gap-1 border-0 bg-transparent px-2 text-xs font-normal text-muted-foreground shadow-none hover:bg-muted aria-expanded:bg-muted dark:bg-transparent dark:hover:bg-muted"
        :disabled="saving"
        :aria-label="t('chat.composer.effortLevel', { level: current })"
        :title="title"
        :aria-busy="saving"
        data-slot="composer-effort"
      >
        <span class="shrink-0">{{ t("chat.composer.effort") }}</span>
        <span class="text-foreground">{{ current }}</span>
        <ChevronDownIcon class="size-3 shrink-0 opacity-60" />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent
      align="end"
      side="top"
      class="w-44"
      :aria-label="t('chat.composer.effort')"
    >
      <DropdownMenuItem
        v-for="level in levels"
        :key="level"
        class="text-xs"
        @select="choose(level)"
      >
        <CheckIcon v-if="level === current" class="size-3.5 shrink-0" />
        <span v-else class="size-3.5 shrink-0" aria-hidden="true" />
        <span class="min-w-0 truncate">{{ level }}</span>
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
</template>
