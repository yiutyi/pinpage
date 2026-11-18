// 功能：合并工具条的本地状态；首帧依据窗口模式初始化，后续只覆盖事件确实给出的字段。
import type { ToolbarStatus, ToolbarTab } from '../../services/toolbar';

/// 工具条创建时的初始状态：模式由 Rust 写入查询参数，首帧就能决定是否显示唤出按钮。
export function initialStatus(mode: 'desktop' | 'floating' | null): ToolbarStatus {
  return { pinned: false, lifted: false, liftable: mode === 'desktop', tabs: [] };
}

/// 缺字段的事件不应把已有状态重置掉，否则按钮会在收到部分更新后消失。
export function mergeStatus(
  current: ToolbarStatus,
  next: Partial<ToolbarStatus> | null | undefined,
): ToolbarStatus {
  if (!next) {
    return current;
  }
  return {
    pinned: typeof next.pinned === 'boolean' ? next.pinned : current.pinned,
    lifted: typeof next.lifted === 'boolean' ? next.lifted : current.lifted,
    liftable: typeof next.liftable === 'boolean' ? next.liftable : current.liftable,
    tabs: Array.isArray(next.tabs) ? next.tabs : current.tabs,
  };
}

/// 只有一个标签时不显示标签条，工具条保持原来的紧凑宽度。
export function tabStripVisible(tabs: ToolbarTab[]): boolean {
  return tabs.length > 1;
}

/// 首页标签不可关闭：关闭最后一个额外标签就回到首页。
export function canCloseTab(tab: ToolbarTab): boolean {
  return !tab.home;
}
