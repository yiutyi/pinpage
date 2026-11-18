// 功能：封装工具条动作调用；动作白名单与目标窗口由 Rust 命令再次校验。
import { invoke } from '@tauri-apps/api/core';

export type ToolbarAction =
  | 'back'
  | 'forward'
  | 'home'
  | 'recycle'
  | 'restart'
  | 'shutdown'
  | 'cancelShutdown'
  | 'state'
  | 'pin'
  | 'hide'
  | 'summon'
  | 'recall'
  | 'hold'
  | 'release'
  | 'tab-switch'
  | 'tab-close';

/// 工具条标签条里的一项：窗口标签用于切换与关闭，标题用于显示。
export interface ToolbarTab {
  window: string;
  title: string;
  active: boolean;
  home: boolean;
}

/// 工具条需要同步的原生状态：是否常驻、是否已唤出、是否可唤出、组内标签。
export interface ToolbarStatus {
  pinned: boolean;
  lifted: boolean;
  /// 只有桌面模式的页面可以唤出，浮动页不显示唤出入口。
  liftable: boolean;
  tabs: ToolbarTab[];
}

/// 返回动作执行后的工具条状态；标签操作通过 target 指定目标窗口。
export function toolbarAction(
  id: string,
  action: ToolbarAction,
  confirm = false,
  target?: string,
): Promise<ToolbarStatus> {
  return invoke('toolbar_action', { id, action, confirm, target: target ?? null });
}
