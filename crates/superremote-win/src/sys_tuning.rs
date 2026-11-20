//! 系统级 RDP 优化设置的检测与恢复。
//!
//! 检测 `AVC444ModePreferred` 与 `SelectTransport` 是否偏离默认。只恢复这两项，
//! 不把恢复操作描述为 UAC 或安全桌面卡顿的修复。
//!
//! 检测永远只读、普通权限即可；恢复默认需要管理员，通过
//! `ShellExecuteW("runas")` 重启自身并带 `--fix-rdp-tuning` 完成，绝不静默
//! 改用户系统。

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RdpTuning {
    pub avc444_preferred: Option<u32>,
    pub select_transport: Option<u32>,
    /// `PromptOnSecureDesktop`：UAC 是否切到 Winlogon 安全桌面。
    pub secure_desktop: Option<u32>,
}

impl RdpTuning {
    /// 是否命中两个非默认值。
    /// `AVC444ModePreferred` 默认不存在，`SelectTransport` 默认是 0（TCP+UDP）。
    pub fn needs_fix(&self) -> bool {
        self.avc444_preferred == Some(1) || self.select_transport == Some(2)
    }

    /// 安全桌面是否开启。读不到时按 Windows 默认（开启）算。
    pub fn secure_desktop(&self) -> bool {
        self.secure_desktop != Some(0)
    }
}

pub fn probe() -> RdpTuning {
    platform::probe()
}

/// 把两项写回默认。调用者必须已经提权，否则会返回权限错误。
pub fn apply_defaults() -> Result<(), String> {
    platform::apply_defaults()
}

/// 以管理员身份重启自身执行 [`apply_defaults`]，由用户在 UAC 上确认。
pub fn request_fix() -> Result<(), String> {
    platform::request_fix()
}

/// 开关 UAC 安全桌面。`false` = 提权提示直接在当前桌面弹出，不切桌面。
pub fn set_secure_desktop(enabled: bool) -> Result<(), String> {
    platform::set_secure_desktop(enabled)
}

/// 以管理员身份重启自身执行 [`set_secure_desktop`]。
pub fn request_secure_desktop(enabled: bool) -> Result<(), String> {
    platform::request_secure_desktop(enabled)
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, RegSetKeyValueW, HKEY_LOCAL_MACHINE, REG_DWORD, RRF_RT_REG_DWORD,
    };
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use super::RdpTuning;

    const TS_POLICY: &str = r"SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services";
    const RDP_TCP: &str = r"SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp";
    const UAC_POLICY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
    const AVC444: &str = "AVC444ModePreferred";
    const SELECT_TRANSPORT: &str = "SelectTransport";
    const SECURE_DESKTOP: &str = "PromptOnSecureDesktop";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn read_dword(subkey: &str, value: &str) -> Option<u32> {
        let subkey = wide(subkey);
        let value = wide(value);
        let mut data = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: 两个字符串都以 NUL 结尾且活到调用结束；`data`/`size` 描述同一块
        // 4 字节缓冲区，类型传 null 表示不关心值类型。
        let status: WIN32_ERROR = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                &mut data as *mut u32 as *mut _,
                &mut size,
            )
        };
        (status == ERROR_SUCCESS).then_some(data)
    }

    fn write_dword(subkey: &str, value: &str, data: u32) -> Result<(), String> {
        let subkey = wide(subkey);
        let value = wide(value);
        // SAFETY: 字符串 NUL 结尾；`data` 是 4 字节 DWORD，与 `cbdata` 一致。
        let status: WIN32_ERROR = unsafe {
            RegSetKeyValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                value.as_ptr(),
                REG_DWORD,
                &data as *const u32 as *const _,
                4,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("写入注册表失败（Win32 错误 {status}）"))
        }
    }

    pub fn probe() -> RdpTuning {
        RdpTuning {
            avc444_preferred: read_dword(TS_POLICY, AVC444),
            select_transport: read_dword(RDP_TCP, SELECT_TRANSPORT),
            secure_desktop: read_dword(UAC_POLICY, SECURE_DESKTOP),
        }
    }

    pub fn apply_defaults() -> Result<(), String> {
        // 0 = 不优先 AVC444；0 = TCP + UDP（默认）。设成 0 与「删除该值」
        // 的效果一致，但幂等、好回读。
        write_dword(TS_POLICY, AVC444, 0)?;
        write_dword(RDP_TCP, SELECT_TRANSPORT, 0)?;
        Ok(())
    }

    pub fn set_secure_desktop(enabled: bool) -> Result<(), String> {
        write_dword(UAC_POLICY, SECURE_DESKTOP, u32::from(!enabled))
    }

    pub fn request_fix() -> Result<(), String> {
        relaunch("--fix-rdp-tuning")
    }

    pub fn request_secure_desktop(enabled: bool) -> Result<(), String> {
        relaunch(if enabled {
            "--secure-desktop=on"
        } else {
            "--secure-desktop=off"
        })
    }

    /// 用 `runas` 重启自身，让提权实例去写 HKLM。命令行只带一个开关，
    /// 提权实例写完就退出，不建窗。
    fn relaunch(argument: &str) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|err| format!("无法定位程序路径：{err}"))?;
        let exe = wide(&exe.to_string_lossy());
        let operation = wide("runas");
        let parameters = wide(argument);
        // SAFETY: 三个字符串都以 NUL 结尾并活到调用结束；窗口句柄与工作目录传 null。
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                exe.as_ptr(),
                parameters.as_ptr(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecute 约定：返回值 > 32 才算成功。
        if result as isize > 32 {
            Ok(())
        } else {
            Err("未能请求管理员权限（UAC 被取消？）".to_owned())
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::RdpTuning;

    pub fn probe() -> RdpTuning {
        RdpTuning::default()
    }

    pub fn apply_defaults() -> Result<(), String> {
        Err("系统 RDP 设置只在 Windows 上可改".to_owned())
    }

    pub fn request_fix() -> Result<(), String> {
        Err("系统 RDP 设置只在 Windows 上可改".to_owned())
    }

    pub fn set_secure_desktop(_enabled: bool) -> Result<(), String> {
        Err("系统 RDP 设置只在 Windows 上可改".to_owned())
    }

    pub fn request_secure_desktop(_enabled: bool) -> Result<(), String> {
        Err("系统 RDP 设置只在 Windows 上可改".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_never_panics_and_classifies_defaults() {
        // 真实环境的值不可预期，这里只钉住「能读、能判定」这条契约。
        let tuning = probe();
        let _ = tuning.needs_fix();
        let _ = tuning.secure_desktop();
        assert!(!RdpTuning::default().needs_fix());
        assert!(RdpTuning {
            avc444_preferred: Some(1),
            ..RdpTuning::default()
        }
        .needs_fix());
        assert!(RdpTuning {
            select_transport: Some(2),
            ..RdpTuning::default()
        }
        .needs_fix());
        assert!(!RdpTuning {
            avc444_preferred: Some(0),
            select_transport: Some(0),
            ..RdpTuning::default()
        }
        .needs_fix());
        // 安全桌面：读不到（None）按开启算，只有显式 0 才算关闭。
        assert!(RdpTuning::default().secure_desktop());
        assert!(RdpTuning {
            secure_desktop: Some(1),
            ..RdpTuning::default()
        }
        .secure_desktop());
        assert!(!RdpTuning {
            secure_desktop: Some(0),
            ..RdpTuning::default()
        }
        .secure_desktop());
    }
}
