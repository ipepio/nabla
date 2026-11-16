<script setup lang="ts">
import type { ScrollAreaScrollbarProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { cn } from 'cn'
import { ScrollAreaScrollbar, ScrollAreaThumb } from 'reka-ui'

const props = withDefaults(defineProps<ScrollAreaScrollbarProps & { class?: HTMLAttributes['class'] }>(), {
  orientation: 'vertical',
})

const delegatedProps = reactiveOmit(props, 'class')
</script>

<template>
  <ScrollAreaScrollbar
    data-slot="scroll-area-scrollbar"
    :data-orientation="orientation"
    v-bind="delegatedProps"
    :class="cn('data-horizontal:h-1.5 data-horizontal:flex-col data-vertical:h-full data-vertical:w-1.5 flex touch-none transition-colors select-none', props.class)"
  >
    <!-- Hairline thumb inside a slightly wider hit area: the track paints
         nothing, so the eye reads a 2px line while the pointer still gets a
         comfortable strip to grab. -->
    <ScrollAreaThumb
      data-slot="scroll-area-thumb"
      :class="
        cn(
          'relative flex-1 rounded-full bg-border/70',
          orientation === 'vertical' ? 'mx-auto w-0.5' : 'my-auto h-0.5',
        )
      "
    />
  </ScrollAreaScrollbar>
</template>
