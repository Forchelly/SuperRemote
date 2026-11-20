//! 终端选择区的系统剪贴板读写。
//!
//! 只做纯文本（`CF_UNICODETEXT`），不碰富文本/图片；Windows 剪贴板 API
//! 是进程级的同步调用，失败就返回错误，不静默吞掉。

/// `CF_UNICODETEXT`。windows-sys 把它放在 Ole 模块里，这里直接保留数值，
/// 省掉为一个常量额外开一个 feature。
const CF_UNICODETEXT: u32 = 13;

pub fn read_text() -> Option<String> {
    platform::read_text()
}

pub fn write_text(text: &str) -> Result<(), String> {
    platform::write_text(text)
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
        OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
    };

    use super::CF_UNICODETEXT;

    pub fn read_text() -> Option<String> {
        // SAFETY: 这一段是「打开 → 取句柄 → 锁定 → 读出 → 解锁 → 关闭」的完整序列，
        // 每条早退分支都在返回前关掉了剪贴板，所以不会把系统剪贴板留在锁定状态。
        // - `OpenClipboard(null)` 传空 owner 是文档允许的（把剪贴板关联到当前任务）。
        // - `GetClipboardData` 返回的句柄只在剪贴板打开期间有效，下面所有读取都在
        //   关闭之前完成；句柄本身归系统所有，我们不解引用也不释放它。
        // - `GlobalLock` 返回的指针在 `GlobalUnlock` 之前保持有效，长度用 `GlobalSize`
        //   现算（单位是字节，除以 2 得 u16 个数），读到 NUL 就提前停止，不会越界。
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0
                || OpenClipboard(std::ptr::null_mut()) == 0
            {
                return None;
            }

            let handle = GetClipboardData(CF_UNICODETEXT);
            if handle.is_null() {
                CloseClipboard();
                return None;
            }

            let pointer = GlobalLock(handle as HGLOBAL) as *const u16;
            if pointer.is_null() {
                CloseClipboard();
                return None;
            }

            let units = GlobalSize(handle as HGLOBAL) / std::mem::size_of::<u16>();
            let mut buffer = Vec::with_capacity(units);
            for index in 0..units {
                let unit = *pointer.add(index);
                if unit == 0 {
                    break;
                }
                buffer.push(unit);
            }

            GlobalUnlock(handle as HGLOBAL);
            CloseClipboard();

            Some(String::from_utf16_lossy(&buffer))
        }
    }

    pub fn write_text(text: &str) -> Result<(), String> {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = wide.len() * std::mem::size_of::<u16>();

        // SAFETY: 这块内存由 `GlobalAlloc` 分配、由我们独占，`wide` 是另一块独立的
        // `Vec` 缓冲，`copy_nonoverlapping` 的源/目标不重叠且目标容量足够（字节数由
        // `wide.len()` 算出）。所有权转移按文档要求处理：
        // - `SetClipboardData` 成功 → 系统接管句柄，此处**不能**再 `GlobalFree`；
        // - 失败 → 系统没有接管，由我们在返回前释放；
        // - 中途任何一步失败（锁定/打开剪贴板）都在返回前释放，不留泄漏。
        // `GlobalUnlock` 必须在交出所有权之前调用，这正是枚举/复制完立刻解锁的原因。
        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if handle.is_null() {
                return Err("无法为剪贴板分配内存".to_owned());
            }

            let pointer = GlobalLock(handle) as *mut u16;
            if pointer.is_null() {
                GlobalFree(handle);
                return Err("无法锁定剪贴板内存".to_owned());
            }

            std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer, wide.len());
            GlobalUnlock(handle);

            if OpenClipboard(std::ptr::null_mut()) == 0 {
                GlobalFree(handle);
                return Err("无法打开系统剪贴板".to_owned());
            }

            EmptyClipboard();
            let stored: HANDLE = SetClipboardData(CF_UNICODETEXT, handle);
            CloseClipboard();

            if stored.is_null() {
                // 失败时系统没有接管这块内存，必须自己释放。
                GlobalFree(handle);
                Err("无法写入系统剪贴板".to_owned())
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn read_text() -> Option<String> {
        None
    }

    pub fn write_text(_text: &str) -> Result<(), String> {
        Err("系统剪贴板只在 Windows 上可用".to_owned())
    }
}
