// 功能：验证唤出配置的几何换算与层级切换，不依赖真实窗口。
use super::lifted;
use crate::models::{PageSource, PinConfig, WindowMode};

/// 构造一个桌面模式的页面配置，几何值用于覆盖换算结果。
fn pin() -> PinConfig {
    PinConfig {
        id: "p-1".into(),
        name: "计时器".into(),
        source: PageSource::Url {
            url: "http://localhost:8080/".into(),
        },
        width: 400.,
        height: 300.,
        x: Some(120),
        y: Some(80),
        scale: 1.,
        always_on_top: false,
        decorations: false,
        enabled: true,
        mode: WindowMode::Desktop,
        rounded: true,
        position_locked: false,
        size_locked: false,
        transparent_background: false,
        page_context_menu: false,
        preset: None,
        presets: Vec::new(),
        folder_root: None,
    }
}

#[test]
/// 唤出只改变窗口层级，桌面模式仍保留在配置里，收回时按原模式挂回。
fn lift_keeps_desktop_mode_in_config() {
    let config = lifted(&pin(), (800, 600), 1.5, (200, 100));
    assert_eq!(config.mode, WindowMode::Floating);
    assert!(config.always_on_top);
    assert!(!config.decorations);
    assert!(config.rounded, "圆角属于外观设置，不因唤出改变");
    assert_eq!(pin().mode, WindowMode::Desktop);
}

#[test]
/// 物理像素按 DPI 还原成逻辑尺寸，避免唤出时窗口被放大一到两倍。
fn lift_converts_physical_size_with_scale() {
    let config = lifted(&pin(), (1200, 900), 2., (200, 100));
    assert_eq!(config.width, 600.);
    assert_eq!(config.height, 450.);
    assert_eq!((config.x, config.y), (Some(200), Some(100)));
}

#[test]
/// 几何查询失败时保留配置里的尺寸，不能把窗口缩成 0。
fn lift_keeps_size_when_window_reports_zero() {
    let config = lifted(&pin(), (0, 0), 1., (10, 20));
    assert_eq!((config.width, config.height), (400., 300.));
}

