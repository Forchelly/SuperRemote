//! 系统外观偏好：Windows 的「应用模式」是深色还是浅色。
//!
//! 只在启动、以及用户点「跟随系统」时读一次。系统主题切换后需要重启程序或
//! 重选一次——为此挂一个常驻轮询会让空闲进程每几秒醒一次，对一个强调
//! 「空载不唤醒」的工具不值得。

pub fn prefers_dark() -> bool {
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::Registry::{
            RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD,
        };

        let wide = |value: &str| -> Vec<u16> {
            OsStr::new(value)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        };

        let subkey = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        let name = wide("AppsUseLightTheme");
        let mut data: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;

        // SAFETY: 两个字符串都以 NUL 结尾且在本作用域内存活；`data` 的大小
        // 由 `size` 如实告知，注册表 API 只会写入不超过该长度的 DWORD。
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                &mut data as *mut u32 as *mut core::ffi::c_void,
                &mut size,
            )
        };

        // 读不到（老系统、策略锁定）时按亮色处理：白底总比在黑屋子里突然
        // 弹出一块 OLED 黑要温和。
        status == 0 && data == 0
    }

    #[cfg(not(windows))]
    {
        false
    }
}
