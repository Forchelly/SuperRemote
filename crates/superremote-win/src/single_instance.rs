//! 单实例护栏：第二个实例不与第一个并存。
//!
//! 防的是数据丢失，不是防多用户：两个实例各自持有内存里的会话树，自动保存
//! 互相盲写覆盖，后保存的会把先保存的改动冲掉。互斥体放在 `Local\` 命名空间，
//! 按登录会话隔离，不同用户各开一个是允许的。
//!
//! 设了 `SUPERREMOTE_DATA_DIR` 时直接放行——那是开发/测试的数据隔离工作流，
//! 双开是特性不是事故。
//!
//! 第二个实例只弹窗告知并退出，不尝试激活第一个实例的窗口：主窗口标题随
//! 会话名变，按标题找窗口不可靠。
//! ponytail: 想做「激活已有窗口」时，用命名文件映射共享主窗口 HWND，别按标题找。

/// 本进程是否可以继续启动。
///
/// 返回 `false` 表示已有实例在运行（并已向用户说明）；调用方应立即退出。
/// 创建互斥体失败时宁可放行：护栏失效顶多回到「双开互相覆盖」的旧行为，
/// 不该变成「谁也启动不了」。
pub fn guard() -> bool {
    if std::env::var_os("SUPERREMOTE_DATA_DIR").is_some() {
        return true;
    }

    if probe(&mutex_name()) {
        platform::announce();
        return false;
    }

    true
}

/// 互斥体名。带产品名防撞名；`Local\` 前缀表示按登录会话隔离。
fn mutex_name() -> String {
    "Local\\SuperRemote-SingleInstance".to_owned()
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONINFORMATION, MB_SETFOREGROUND,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 名为 `name` 的互斥体是否已存在（= 已有实例持有）。
    pub fn probe(name: &str) -> bool {
        let name = wide(name);

        // SAFETY: 名称字符串以 NUL 结尾并活到调用返回；返回的句柄归系统所有，
        // 进程退出时自动释放，不需要也不应该在这里 CloseHandle。
        unsafe {
            let handle = CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr());
            if handle.is_null() {
                return false;
            }
            GetLastError() == ERROR_ALREADY_EXISTS
        }
    }

    /// 告知用户为什么这个实例起不来。进程没有控制台（windows 子系统），
    /// 弹窗是唯一出口；标题与文案刻意中英并列，此时语言偏好还读不到。
    pub fn announce() {
        let caption = wide("SuperRemote");
        let text = wide("SuperRemote 已在运行 / SuperRemote is already running");

        // SAFETY: 两个字符串都以 NUL 结尾并活到调用返回；弹窗关闭后控制权交还。
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                caption.as_ptr(),
                MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn probe(_name: &str) -> bool {
        false
    }

    pub fn announce() {}
}

// 平台层暴露给测试：同名互斥体的二次探测必须报告「已存在」。
pub use platform::probe;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_probe_of_the_same_name_reports_an_existing_instance() {
        // 测试进程里没有别的实例干扰；名字带上进程号避免与真实运行撞车。
        let name = format!("Local\\SuperRemote-test-{}", std::process::id());
        assert!(!probe(&name), "本进程首次创建同名互斥体，应视为初次启动");
        assert!(probe(&name), "同名互斥体已存在，应判定已有实例");
    }
}
