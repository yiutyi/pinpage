// 功能：把桌面映射按钮兑换为固定的系统动作，目标与参数只由 Rust 决定。
//
// 不执行桌面上任意 .lnk 文件：快捷方式可被替换成其他命令，工具条只需要
// 「打开回收站 / 关机 / 重启」三种语义。关机与重启统一延时 30 秒，
// 期间可通过「撤销」动作取消，避免误触后无法挽回。
use crate::errors::AppMessage;
use std::ptr::{null, null_mut};
use windows_sys::core::{w, PCWSTR};
use windows_sys::Win32::UI::{
    Shell::ShellExecuteW,
    WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL},
};

/// 把 UTF-8 参数转成以 NUL 结尾的 UTF-16 缓冲，供 ShellExecute 使用。
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 启动固定目标；ShellExecute 返回不大于 32 的值表示失败。
fn execute(file: PCWSTR, parameters: &str, show: i32) -> Result<(), AppMessage> {
    let parameters = wide(parameters);
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            w!("open"),
            file,
            parameters.as_ptr(),
            null(),
            show,
        )
    };
    if (result as isize) <= 32 {
        return Err(
            AppMessage::new("errors.desktopActionFailed").detail(std::io::Error::last_os_error())
        );
    }
    Ok(())
}

/// 分发工具条的桌面映射动作；未知动作由调用方拦截，此处再兜底一次。
pub fn desktop_action(action: &str) -> Result<(), AppMessage> {
    match action {
        "recycle" => execute(w!("explorer.exe"), "shell:RecycleBinFolder", SW_SHOWNORMAL),
        "shutdown" => execute(
            w!("shutdown.exe"),
            "/s /t 30 /c \"PinPage shutdown\"",
            SW_HIDE,
        ),
        "restart" => execute(
            w!("shutdown.exe"),
            "/r /t 30 /c \"PinPage restart\"",
            SW_HIDE,
        ),
        "cancelShutdown" => execute(w!("shutdown.exe"), "/a", SW_HIDE),
        _ => Err(AppMessage::new("errors.unknownAction")),
    }
}
