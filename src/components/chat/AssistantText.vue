<script setup lang="ts">
// Assistant prose, rendered without a markdown library: fenced code becomes a
// copyable block, inline `code` gets a monospace chip, and every other character
// is text. Interpolations keep that promise — Vue escapes them, so nothing the
// model wrote can turn into markup.
import { computed } from "vue";
import CodeBlock from "@/components/chat/CodeBlock.vue";
import { segmentsOf } from "@/components/chat/code-segments";

const props = defineProps<{ text: string }>();

const segments = computed(() => segmentsOf(props.text));
</script>

<template>
  <div class="flex flex-col gap-2">
    <template v-for="(segment, index) in segments" :key="index">
      <CodeBlock
        v-if="segment.type === 'code'"
        :language="segment.language"
        :source="segment.source"
      />
      <p v-else class="whitespace-pre-wrap break-words">
        <template v-for="(part, partIndex) in segment.parts" :key="partIndex">
          <code
            v-if="part.type === 'inlineCode'"
            class="rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]"
            >{{ part.code }}</code
          >
          <strong v-else-if="part.type === 'bold'">{{ part.text }}</strong>
          <em v-else-if="part.type === 'italic'">{{ part.text }}</em>
          <a
            v-else-if="part.type === 'link'"
            :href="part.href"
            target="_blank"
            rel="noopener noreferrer"
            class="underline decoration-muted-foreground/50 underline-offset-2 hover:decoration-foreground"
            >{{ part.text }}</a
          >
          <template v-else>{{ part.text }}</template>
        </template>
      </p>
    </template>
  </div>
</template>
