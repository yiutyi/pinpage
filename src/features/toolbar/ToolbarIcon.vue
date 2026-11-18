<!-- 功能：工具条内联 SVG 图标，不依赖字体图标或外部资源。 -->
<script setup lang="ts">
defineProps<{ name: string }>();
</script>

<template>
  <svg
    class="toolbar-icon"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.8"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    focusable="false"
  >
    <template v-if="name === 'back'"><path d="M14.5 5 8 12l6.5 7" /></template>
    <template v-else-if="name === 'forward'"><path d="M9.5 5 16 12l-6.5 7" /></template>
    <template v-else-if="name === 'home'">
      <path d="M3.5 10.7 12 3.6l8.5 7.1V20a1 1 0 0 1-1 1h-5v-6H9.5v6h-5a1 1 0 0 1-1-1z" />
    </template>
    <template v-else-if="name === 'recycle'">
      <path d="M4 7h16M9.5 7V4.5h5V7M6.5 7l1 13h9l1-13M10 10.5v6M14 10.5v6" />
    </template>
    <template v-else-if="name === 'restart'">
      <path d="M20.5 12a8.5 8.5 0 1 1-2.5-6M20.5 4v5h-5" />
    </template>
    <template v-else-if="name === 'shutdown'">
      <path d="M12 3v9M6.6 6.6a8 8 0 1 0 10.8 0" />
    </template>
    <template v-else-if="name === 'pin'">
      <path d="M9 3h6l-.8 6 3.8 3v2H6v-2l3.8-3L9 3zM12 14v7" />
    </template>
    <template v-else-if="name === 'hide'"><path d="M6 15l6-6 6 6" /></template>
    <template v-else-if="name === 'close'"><path d="M7 7l10 10M17 7 7 17" /></template>
    <!-- 唤出：箭头离开桌面横线，表示把页面提到普通窗口层最前。 -->
    <template v-else-if="name === 'lift'">
      <path d="M12 4v11M7.5 8.5 12 4l4.5 4.5M4 20h16" />
    </template>
    <!-- 收回：箭头落回桌面横线，表示页面和工具条一起回到桌面。 -->
    <template v-else-if="name === 'drop'">
      <path d="M12 15V4M7.5 10.5 12 15l4.5-4.5M4 20h16" />
    </template>
  </svg>
</template>
