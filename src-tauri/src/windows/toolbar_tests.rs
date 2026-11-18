// 功能：验证工具条显隐的优先级、收起门闩、状态隔离与自适应轮询间隔。
use super::{
    expanded, keep_above, pending_visibility, poll_interval, release_dismissed, VERTICAL_HEIGHT,
    VERTICAL_WIDTH,
};
use crate::state::{ToolbarRuntime, ToolbarState};
use std::time::{Duration, Instant};

fn state(pinned: bool, dismissed: bool, hold: bool) -> ToolbarState {
    ToolbarState {
        pinned,
        dismissed,
        hold,
    }
}

#[test]
/// 图钉、倒计时与前台唤出是三个独立来源，任一成立即常驻。
fn permanent_reasons_keep_toolbar_visible() {
    assert!(expanded(state(true, false, false), true, false, false));
    assert!(expanded(state(false, false, true), true, false, false));
    assert!(expanded(state(false, false, false), true, false, true));
    assert!(expanded(state(true, false, true), true, false, false));
}

#[test]
/// 未固定、未唤出时，只在光标宽限期内展开。
fn auto_hide_after_idle() {
    assert!(expanded(state(false, false, false), true, true, false));
    assert!(!expanded(state(false, false, false), true, false, false));
}

#[test]
/// 主动收起是临时门闩：覆盖图钉、唤出、倒计时与宽限期，但不改写它们。
fn dismissed_gates_every_reason_without_clearing_it() {
    let dismissed = state(true, true, true);
    assert!(!expanded(dismissed, true, true, true));
    assert!(dismissed.pinned);
    assert!(dismissed.hold);
}

#[test]
/// 页面隐藏时任何状态都不显示按钮。
fn hidden_page_collapses() {
    assert!(!expanded(state(false, false, false), false, true, false));
    assert!(!expanded(state(true, false, false), false, true, true));
    assert!(!expanded(state(false, false, true), false, true, true));
}

#[test]
/// 收起后鼠标仍在按钮条上不会弹回，只有重新进入顶部热区才恢复。
fn dismissed_is_released_only_by_hot_zone() {
    assert!(release_dismissed(true, false), "悬停与静止都保持收起");
    assert!(!release_dismissed(true, true), "重新进入热区才解除门闩");
    assert!(!release_dismissed(false, false));
}

#[test]
/// 改图钉或倒计时不碰宽限期与上一帧显隐，轮询状态不会被动作覆盖。
fn intent_change_keeps_polling_state() {
    let deadline = Instant::now() + Duration::from_secs(1);
    let runtime = ToolbarRuntime {
        state: state(false, false, false),
        grace_until: Some(deadline),
        visible: true,
    };
    let next = runtime.with_state(state(true, true, true));
    assert!(next.state.pinned);
    assert_eq!(next.grace_until, Some(deadline));
    assert!(next.visible);
}

#[test]
/// 页面关闭只清临时状态：图钉偏好留到下次开启，删除磁贴才彻底清理。
fn reset_transient_keeps_pin() {
    let runtime = ToolbarRuntime {
        state: state(true, true, true),
        grace_until: Some(Instant::now()),
        visible: true,
    };
    let next = runtime.reset_transient();
    assert!(next.state.pinned);
    assert!(!next.state.dismissed);
    assert!(!next.state.hold);
    assert!(next.grace_until.is_none());
    assert!(!next.visible);
}

#[test]
/// 空闲降频不影响交互：鼠标移动、宽限期和 Alt 拖动都立即回到 25ms。
fn polling_slows_down_only_when_idle() {
    assert_eq!(
        poll_interval(0, false, false, false),
        Duration::from_millis(150)
    );
    assert_eq!(
        poll_interval(2, false, false, false),
        Duration::from_millis(50)
    );
    assert_eq!(
        poll_interval(2, true, false, false),
        Duration::from_millis(25)
    );
    assert_eq!(
        poll_interval(2, false, true, false),
        Duration::from_millis(25)
    );
    assert_eq!(
        poll_interval(0, false, false, true),
        Duration::from_millis(25)
    );
}

#[test]
/// 工具条显示期间一律压回页面上方；收起后不再调整 z 序。
fn visible_toolbar_keeps_above_page() {
    assert!(keep_above(true));
    assert!(!keep_above(false));
}

#[test]
/// 两秒宽限期结束后才允许降频。
fn pending_visibility_expires_after_deadline() {
    let future = Instant::now() + Duration::from_secs(1);
    let past = Instant::now() - Duration::from_secs(3);
    assert!(pending_visibility(&[past, future]));
    assert!(!pending_visibility(&[past]));
    assert!(!pending_visibility(&[]));
}

#[test]
/// 竖排尺寸是两个固定常量：标签数不再改变窗口大小，避免尺寸抖动。
fn vertical_size_is_constant() {
    assert!(VERTICAL_WIDTH >= 40. && VERTICAL_WIDTH <= 64.);
    assert!(VERTICAL_HEIGHT >= VERTICAL_WIDTH * 4.);
}
