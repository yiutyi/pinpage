// 功能：验证标签宽度计算与关闭后的邻居选择，不依赖真实窗口。
use super::{neighbor, tabs_width, MAX_TABS, TAB_WIDTH};
use crate::state::TabEntry;

/// 构造一组标签条目，窗口标签按顺序编号。
fn tabs(count: usize) -> Vec<TabEntry> {
    (0..count)
        .map(|index| TabEntry {
            window: format!("p-1-t{index}"),
            title: format!("标签 {index}"),
        })
        .collect()
}

#[test]
/// 只有一个标签时不显示标签条，工具条保持原宽度。
fn single_tab_keeps_base_width() {
    assert_eq!(tabs_width(0), 0.);
    assert_eq!(tabs_width(1), 0.);
}

#[test]
/// 每个标签占固定宽度，工具条按数量线性加宽，上限由 MAX_TABS 约束。
fn multiple_tabs_grow_linearly() {
    assert_eq!(tabs_width(2), TAB_WIDTH * 2.);
    assert_eq!(tabs_width(MAX_TABS), TAB_WIDTH * MAX_TABS as f64);
    assert!(tabs_width(MAX_TABS) < 600.);
}

#[test]
/// 关闭标签后优先切到前一个，第一个额外标签则切到后一个。
fn neighbor_prefers_previous() {
    let items = tabs(4);
    assert_eq!(neighbor(&items, "p-1-t2").as_deref(), Some("p-1-t1"));
    assert_eq!(neighbor(&items, "p-1-t1").as_deref(), Some("p-1-t0"));
}

#[test]
/// 关闭最后一个标签时回退到前一个；不在组内的标签没有邻居。
fn neighbor_falls_back_and_rejects_unknown() {
    let items = tabs(1);
    assert_eq!(neighbor(&items, "p-1-t0"), None);
    assert_eq!(neighbor(&tabs(2), "p-missing"), None);
}
