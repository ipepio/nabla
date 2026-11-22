<script setup lang="ts">
// Assistant prose, rendered without a markdown library: fenced code becomes a
// copyable block, `#` headings become real headings, list rows become real
// lists, inline `code` gets a monospace chip, and every other character is text.
// Interpolations keep that promise — Vue escapes them, so nothing the model
// wrote can turn into markup.
//
// `segmentsOf` does all the recognising, so this file only decides what each
// block looks like. It runs through a cache (`createSegmentCache`) because a
// streamed answer only appends: every delta re-parsing the blocks that already
// closed is work that cannot change what they look like. A paragraph that appears
// mid-stream is therefore painted exactly as it will be once the turn is
// committed.
import { computed, Fragment, h, type FunctionalComponent, type VNodeChild } from "vue";
import CodeBlock from "@/components/chat/CodeBlock.vue";
import { createSegmentCache, type InlinePart } from "@/components/chat/code-segments";

const props = defineProps<{ text: string }>();

/** One cache per bubble: the text it is asked about only ever belongs to itself. */
const segmentsCache = createSegmentCache();
const segments = computed(() => segmentsCache.segmentsFor(props.text));

/**
 * One run of inline markup, painted inside whatever block holds it — a
 * paragraph, a heading or a list row. A render function instead of a template
 * so there is exactly one implementation of it: the same template would have to
 * be copied into each of those three blocks, and three copies of the escaping
 * rules is three chances to disagree.
 */
function renderPart(part: InlinePart): VNodeChild {
  switch (part.type) {
    case "inlineCode":
      return h(
        "code",
        { class: "rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]" },
        part.code,
      );
    case "bold":
      return h("strong", part.text);
    case "italic":
      return h("em", part.text);
    case "link":
      return h(
        "a",
        {
          href: part.href,
          target: "_blank",
          rel: "noopener noreferrer",
          class:
            "underline decoration-muted-foreground/50 underline-offset-2 hover:decoration-foreground",
        },
        part.text,
      );
    default:
      return part.text;
  }
}

/** The inline renderer, as a component the template can place anywhere. */
const InlineParts: FunctionalComponent<{ parts: InlinePart[] }> = (inline) =>
  h(Fragment, inline.parts.map(renderPart));

/**
 * A heading's weight and size, from the app's existing scale: the assistant body
 * is `text-sm`, so `h1`/`h2` step above it, `h3` keeps the body size and
 * `h4`–`h6` step below it. Only the size and the weight change, so every level
 * still reads as the same document.
 */
function headingClass(level: number): string {
  switch (level) {
    case 1:
      return "text-lg font-semibold";
    case 2:
      return "text-base font-semibold";
    case 4:
      return "text-sm font-medium";
    case 5:
      return "text-xs font-semibold";
    case 6:
      return "text-xs font-medium text-muted-foreground";
    default:
      // `h3`, the level an answer uses most: the body size, one weight up.
      return "text-sm font-semibold";
  }
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <template v-for="(segment, index) in segments" :key="index">
      <CodeBlock
        v-if="segment.type === 'code'"
        :language="segment.language"
        :source="segment.source"
      />
      <!-- A heading: the element carries the level, so the hierarchy survives
           copy/paste and the accessibility tree, not just the font size. -->
      <component
        :is="`h${segment.level}`"
        v-else-if="segment.type === 'heading'"
        class="break-words"
        :class="headingClass(segment.level)"
      >
        <InlineParts :parts="segment.parts" />
      </component>
      <ul
        v-else-if="segment.type === 'list' && !segment.ordered"
        class="list-disc break-words ps-5"
      >
        <li v-for="(item, itemIndex) in segment.items" :key="itemIndex">
          <InlineParts :parts="item" />
        </li>
      </ul>
      <ol
        v-else-if="segment.type === 'list'"
        class="list-decimal break-words ps-5"
      >
        <li v-for="(item, itemIndex) in segment.items" :key="itemIndex">
          <InlineParts :parts="item" />
        </li>
      </ol>
      <div
        v-else-if="segment.type === 'table'"
        class="my-1 overflow-x-auto rounded-lg border border-border bg-card/50"
      >
        <table class="w-full border-collapse text-xs">
          <thead>
            <tr class="border-b border-border bg-secondary/50">
              <th
                v-for="(header, hIdx) in segment.headers"
                :key="hIdx"
                class="border-r border-border px-3 py-2 font-semibold text-foreground last:border-r-0"
                :class="
                  segment.alignments[hIdx] === 'center'
                    ? 'text-center'
                    : segment.alignments[hIdx] === 'right'
                      ? 'text-right'
                      : 'text-left'
                "
              >
                <InlineParts :parts="header" />
              </th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="(row, rIdx) in segment.rows"
              :key="rIdx"
              class="border-b border-border/70 last:border-b-0 odd:bg-transparent even:bg-secondary/20"
            >
              <td
                v-for="(cell, cIdx) in row"
                :key="cIdx"
                class="border-r border-border/70 px-3 py-2 align-top text-foreground/90 last:border-r-0"
                :class="
                  segment.alignments[cIdx] === 'center'
                    ? 'text-center'
                    : segment.alignments[cIdx] === 'right'
                      ? 'text-right'
                      : 'text-left'
                "
              >
                <InlineParts :parts="cell" />
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <blockquote
        v-else-if="segment.type === 'blockquote'"
        class="border-l-2 border-primary/50 pl-3 text-muted-foreground italic whitespace-pre-wrap break-words"
      >
        <InlineParts :parts="segment.parts" />
      </blockquote>
      <hr v-else-if="segment.type === 'hr'" class="my-1 border-border" />
      <p
        v-else-if="segment.type === 'text'"
        class="whitespace-pre-wrap break-words"
      >
        <InlineParts :parts="segment.parts" />
      </p>
    </template>
  </div>
</template>
