<script setup lang="ts">
// One line of dialogue (spec S2 of `session-lifecycle.md`): user turns are
// right-aligned bubbles with a timestamp and a copy control; assistant turns are
// full-width prose, so streamed deltas read like a document rather than a chat
// log.
//
// An assistant turn's text goes through `AssistantText`, which turns fenced code
// into copyable blocks without a markdown dependency. Its reasoning and its tool
// activity are no longer part of this component: they belong to the turn's
// disclosure (`ToolSummary` inside `TurnGroup`), where they are folded per step
// instead of per message.
import { computed, ref } from "vue";
import { CheckIcon, CopyIcon, Loader2Icon, SquareIcon, Volume2Icon } from "@lucide/vue";
import AssistantText from "@/components/chat/AssistantText.vue";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import { speakText, speakingError, speakingKey, speakingLoading } from "@/lib/voice-player";

const props = defineProps<{
  role: "user" | "assistant";
  text: string;
  /** `HH:MM`, user turns only. */
  at?: string;
}>();

const copied = ref(false);
let resetHandle: number | null = null;
const bubbleSpeechKey = `msg-${Math.random().toString(36).slice(2, 10)}`;
const lastTriedThis = ref(false);
const isSpeakingThis = computed(() => speakingKey.value === bubbleSpeechKey);
const isLoadingThis = computed(
  () => isSpeakingThis.value && speakingLoading.value,
);

function toggleSpeak() {
  lastTriedThis.value = true;
  void speakText(bubbleSpeechKey, props.text);
}

async function copyText() {
  try {
    await navigator.clipboard.writeText(props.text);
    copied.value = true;
    if (resetHandle !== null) window.clearTimeout(resetHandle);
    resetHandle = window.setTimeout(() => {
      copied.value = false;
      resetHandle = null;
    }, 1500);
  } catch {
    // A denied clipboard write leaves the control in its idle state; there is
    // no toast layer yet to report it.
    copied.value = false;
  }
}
</script>

<template>
  <div v-if="role === 'user'" class="nabla-enter flex flex-col items-end gap-1">
    <div
      class="max-w-[80%] rounded-2xl rounded-br-sm border border-border bg-secondary px-4 py-2.5 text-[13px] leading-relaxed whitespace-pre-wrap text-secondary-foreground"
    >
      {{ text }}
    </div>
    <div class="flex items-center gap-1 font-mono text-[10px] text-muted-foreground">
      <span v-if="at">{{ at }}</span>
      <Button
        variant="ghost"
        size="icon-xs"
        class="text-muted-foreground hover:text-foreground"
        :aria-label="t('message.copy')"
        @click="copyText"
      >
        <CheckIcon v-if="copied" class="text-primary" />
        <CopyIcon v-else />
      </Button>
    </div>
  </div>

  <div v-else class="group/assistant-msg nabla-enter flex flex-col gap-1.5 text-[13px] leading-relaxed text-foreground">
    <AssistantText v-if="text !== ''" :text="text" />
    <div
      v-if="text !== ''"
      class="flex items-center gap-1 transition-opacity"
      :class="
        isSpeakingThis
          ? 'opacity-100'
          : 'opacity-0 group-hover/assistant-msg:opacity-100 focus-within:opacity-100'
      "
    >
      <Button
        variant="ghost"
        size="icon-xs"
        class="text-muted-foreground hover:text-foreground"
        :class="isSpeakingThis ? 'text-primary' : ''"
        :aria-label="isSpeakingThis ? t('message.stopListening') : t('message.listen')"
        :title="
          speakingError && isSpeakingThis
            ? speakingError
            : isSpeakingThis
              ? t('message.stopListening')
              : t('message.listen')
        "
        @click="toggleSpeak"
      >
        <Loader2Icon v-if="isLoadingThis" class="size-3.5 animate-spin" />
        <SquareIcon v-else-if="isSpeakingThis" class="size-3 fill-current" />
        <Volume2Icon v-else class="size-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        class="text-muted-foreground hover:text-foreground"
        :aria-label="t('message.copy')"
        :title="t('message.copy')"
        @click="copyText"
      >
        <CheckIcon v-if="copied" class="size-3.5 text-primary" />
        <CopyIcon v-else class="size-3.5" />
      </Button>
      <span
        v-if="speakingError && (isSpeakingThis || lastTriedThis)"
        class="ml-1 truncate text-[11px] text-destructive"
      >
        {{ speakingError }}
      </span>
    </div>
  </div>
</template>
