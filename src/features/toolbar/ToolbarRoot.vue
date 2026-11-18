<!-- 功能：居中工具条的 SVG 图标按钮、固定/唤出/收起、危险动作确认与撤销倒计时。 -->
<script setup lang="ts">
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { text, toUiMessage } from '../../i18n/messages';
import {
  toolbarAction,
  type ToolbarAction,
  type ToolbarStatus,
  type ToolbarTab,
} from '../../services/toolbar';
import type { UiMessage } from '../../types';
import { canCloseTab, initialStatus, mergeStatus, tabStripVisible } from './status';
import ToolbarIcon from './ToolbarIcon.vue';

/// `pin` 是磁贴 ID：工具条属于整个标签组，切标签不会重建这个界面。
const props = defineProps<{
  pin: string;
  mode?: 'desktop' | 'floating';
  slot?: 'top' | 'left' | 'right';
}>();
const { t } = useI18n();
const confirming = ref<'restart' | 'shutdown' | null>(null);
const pending = ref<'restart' | 'shutdown' | null>(null);
const seconds = ref(0);
const error = ref<UiMessage | null>(null);
/// 首帧按窗口模式决定是否显示唤出按钮，不依赖首个异步查询是否完成。
const status = ref<ToolbarStatus>(initialStatus(props.mode ?? null));
const pinned = computed(() => status.value.pinned);
/// 页面是否被唤出到前台；桌面模式的页面才能唤出，浮动页隐藏该按钮。
const lifted = computed(() => status.value.lifted);
const liftable = computed(() => status.value.liftable);
/// 组内标签；只有一个标签时不显示标签条。
const tabs = computed(() => status.value.tabs);
const strip = computed(() => tabStripVisible(status.value.tabs));
let timer: ReturnType<typeof setInterval> | null = null;
const unlisten: UnlistenFn[] = [];

const confirmText = computed(() =>
  confirming.value === 'shutdown'
    ? t('ui.toolbar.shutdownConfirm')
    : t('ui.toolbar.restartConfirm'),
);

function stopTimer() {
  if (timer !== null) {
    clearInterval(timer);
    timer = null;
  }
}

/// 应用 Rust 回传的状态；只覆盖实际给出的字段，按钮图标与高亮都以原生状态为准。
function apply(next: Partial<ToolbarStatus> | null | undefined) {
  status.value = mergeStatus(status.value, next);
}

/// 读取固定与唤出状态；工具条可能被自动隐藏，界面仍需与 Rust 侧一致。
async function refreshStatus() {
  try {
    apply(await toolbarAction(props.pin, 'state'));
  } catch {
    // 查询失败时保持默认值，不阻断按钮操作。
  }
}

/// 确认与撤销期间强制常驻，避免倒计时入口被自动隐藏。
async function setHold(value: boolean) {
  try {
    await toolbarAction(props.pin, value ? 'hold' : 'release');
  } catch {
    // 显隐控制失败不影响动作本身。
  }
}

/// 执行动作；标签操作用 target 指定目标窗口，失败时把消息覆盖在工具条上。
async function run(action: ToolbarAction, confirm = false, target?: string) {
  error.value = null;
  try {
    apply(await toolbarAction(props.pin, action, confirm, target));
  } catch (reason) {
    error.value = toUiMessage(reason);
  }
}

function ask(action: 'restart' | 'shutdown') {
  confirming.value = action;
  void setHold(true);
}

function cancelConfirm() {
  confirming.value = null;
  void setHold(false);
}

/// 确认后才提交带确认标记的关机或重启，并开启 30 秒撤销窗口。
function confirmAction() {
  const action = confirming.value;
  if (!action) return;
  confirming.value = null;
  pending.value = action;
  const deadline = Date.now() + 30_000;
  seconds.value = 30;
  stopTimer();
  timer = setInterval(() => {
    seconds.value = Math.max(0, Math.ceil((deadline - Date.now()) / 1000));
    if (seconds.value === 0) {
      stopTimer();
      void setHold(false);
    }
  }, 500);
  void run(action, true);
}

/// 撤销系统关机或重启，同时退出倒计时态。
function undo() {
  stopTimer();
  pending.value = null;
  void setHold(false);
  void run('cancelShutdown');
}

function dismiss() {
  stopTimer();
  pending.value = null;
  void setHold(false);
}

/// 先订阅快捷键触发的变化，再读取一次当前值；订阅失败也不能挡住状态读取，
/// 否则按钮会停留在初始值（此前唤出按钮首次不显示就是这个原因）。
onMounted(async () => {
  try {
    unlisten.push(
      await listen<Partial<ToolbarStatus>>('lift-changed', (event) => {
        apply(event.payload);
      }),
    );
  } catch {
    // 订阅被拒绝时按钮仍可点击，动作返回的状态会继续更新界面。
  }
  try {
    // 标题变化、开标签与关标签都以完整列表广播，界面不自行推断顺序。
    unlisten.push(
      await listen<ToolbarTab[]>('tabs-changed', (event) => {
        apply({ tabs: event.payload });
      }),
    );
  } catch {
    // 同上：标签条至少保留首帧或最近一次动作返回的内容。
  }
  await refreshStatus();
});
onUnmounted(() => {
  for (const off of unlisten) off();
  stopTimer();
});
</script>

<template>
  <div
    class="toolbar-shell"
    :class="`toolbar-${props.slot ?? 'top'}`"
    role="toolbar"
    :aria-label="t('ui.toolbar.aria')"
  >
    <template v-if="confirming">
      <span class="toolbar-note">{{ confirmText }}</span>
      <button type="button" class="toolbar-text toolbar-danger" @click="confirmAction">
        {{ t('ui.toolbar.confirm') }}
      </button>
      <button type="button" class="toolbar-text" @click="cancelConfirm">
        {{ t('ui.toolbar.cancel') }}
      </button>
    </template>
    <template v-else-if="pending">
      <span class="toolbar-note">{{ t('ui.toolbar.pending', { seconds }) }}</span>
      <button type="button" class="toolbar-text toolbar-danger" @click="undo">
        {{ t('ui.toolbar.undo') }}
      </button>
      <button type="button" class="toolbar-text" @click="dismiss">
        {{ t('ui.toolbar.backToList') }}
      </button>
    </template>
    <template v-else>
      <div v-if="strip" class="toolbar-tabs" role="tablist" :aria-label="t('ui.toolbar.tabs')">
        <div
          v-for="tab in tabs"
          :key="tab.window"
          class="toolbar-tab"
          :class="{ active: tab.active }"
        >
          <button
            type="button"
            class="toolbar-tab-main"
            role="tab"
            :aria-selected="tab.active"
            :title="tab.title"
            @click="run('tab-switch', false, tab.window)"
          >
            {{ tab.title || t('ui.toolbar.tabs') }}
          </button>
          <button
            v-if="canCloseTab(tab)"
            type="button"
            class="toolbar-tab-close"
            :title="t('ui.toolbar.tabClose')"
            :aria-label="t('ui.toolbar.tabClose')"
            @click.stop="run('tab-close', false, tab.window)"
          >
            <ToolbarIcon name="close" />
          </button>
        </div>
      </div>
      <div class="toolbar-group">
        <button
          type="button"
          class="toolbar-btn"
          :title="t('ui.toolbar.back')"
          :aria-label="t('ui.toolbar.back')"
          @click="run('back')"
        >
          <ToolbarIcon name="back" />
        </button>
        <button
          type="button"
          class="toolbar-btn"
          :title="t('ui.toolbar.forward')"
          :aria-label="t('ui.toolbar.forward')"
          @click="run('forward')"
        >
          <ToolbarIcon name="forward" />
        </button>
        <button
          type="button"
          class="toolbar-btn"
          :title="t('ui.toolbar.home')"
          :aria-label="t('ui.toolbar.home')"
          @click="run('home')"
        >
          <ToolbarIcon name="home" />
        </button>
        <span class="toolbar-sep" aria-hidden="true"></span>
        <button
          type="button"
          class="toolbar-btn"
          :title="t('ui.toolbar.recycle')"
          :aria-label="t('ui.toolbar.recycle')"
          @click="run('recycle')"
        >
          <ToolbarIcon name="recycle" />
        </button>
        <button
          type="button"
          class="toolbar-btn"
          :title="t('ui.toolbar.restart')"
          :aria-label="t('ui.toolbar.restart')"
          @click="ask('restart')"
        >
          <ToolbarIcon name="restart" />
        </button>
        <button
          type="button"
          class="toolbar-btn"
          :title="t('ui.toolbar.shutdown')"
          :aria-label="t('ui.toolbar.shutdown')"
          @click="ask('shutdown')"
        >
          <ToolbarIcon name="shutdown" />
        </button>
      </div>
      <span class="toolbar-sep" aria-hidden="true"></span>
      <button
        v-if="liftable"
        type="button"
        class="toolbar-btn"
        :class="{ active: lifted }"
        :title="t(lifted ? 'ui.toolbar.recall' : 'ui.toolbar.summon')"
        :aria-label="t(lifted ? 'ui.toolbar.recall' : 'ui.toolbar.summon')"
        :aria-pressed="lifted"
        @click="run(lifted ? 'recall' : 'summon')"
      >
        <ToolbarIcon :name="lifted ? 'drop' : 'lift'" />
      </button>
      <button
        type="button"
        class="toolbar-btn"
        :class="{ active: pinned }"
        :title="pinned ? t('ui.toolbar.unpin') : t('ui.toolbar.pin')"
        :aria-label="pinned ? t('ui.toolbar.unpin') : t('ui.toolbar.pin')"
        :aria-pressed="pinned"
        @click="run('pin')"
      >
        <ToolbarIcon name="pin" />
      </button>
      <!-- 收起只影响工具条：不取消图钉，也不把唤出的页面送回桌面。 -->
      <button
        type="button"
        class="toolbar-btn toolbar-subtle"
        :title="t('ui.toolbar.hide')"
        :aria-label="t('ui.toolbar.hide')"
        @click="run('hide')"
      >
        <ToolbarIcon name="hide" />
      </button>
    </template>
    <div v-if="error" class="toolbar-error" role="alert">
      <span>{{ text(error) }}</span>
      <button type="button" class="toolbar-text" @click="error = null">
        {{ t('ui.toolbar.cancel') }}
      </button>
    </div>
  </div>
</template>
