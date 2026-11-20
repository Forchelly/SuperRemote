//! 原生目录选择器。设置页「更改目录」用。
//!
//! 用 `SHBrowseForFolderW` 而不是引入 `rfd`：只多开三个 windows-sys 特性，
//! 不增加包体与额外依赖；对话框由系统绘制，不需要自己维护一套浏览器式 UI。

use std::path::PathBuf;

/// 弹出模态目录选择框。取消或平台不支持时返回 `None`。
#[cfg(windows)]
pub fn pick_folder(owner: Option<isize>, title: &str) -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    };

    // Shell 文件夹对话框要求调用线程先初始化 COM；winit 不保证替我们做。
    // 已经以别的 apartment 初始化时（hr < 0）照常弹框，但不能再 Uninitialize。
    //
    // SAFETY: no pointers escape this call; the apartment constant is valid.
    let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    let initialized = hr >= 0;

    let picked = pick_folder_inner(owner, title);

    if initialized {
        // SAFETY: balanced with a successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }

    picked
}

#[cfg(windows)]
fn pick_folder_inner(owner: Option<isize>, title: &str) -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        SHBrowseForFolderW, SHGetPathFromIDListW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS,
        BROWSEINFOW,
    };

    let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    let mut display = vec![0u16; 260];
    let hwnd = owner
        .map(|handle| handle as windows_sys::Win32::Foundation::HWND)
        .unwrap_or(std::ptr::null_mut());

    let info = BROWSEINFOW {
        hwndOwner: hwnd,
        pidlRoot: std::ptr::null_mut(),
        pszDisplayName: display.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        lpfn: None,
        lParam: 0,
        iImage: 0,
    };

    // SAFETY: `info` and its buffers are valid for the duration of the modal call;
    // the returned PIDL is owned by the shell allocator or null on cancel.
    let pidl = unsafe { SHBrowseForFolderW(&info) };
    if pidl.is_null() {
        return None;
    }

    let mut path = vec![0u16; 32768];
    // SAFETY: `pidl` is non-null and owned; `path` is a writable buffer large enough
    // for the Win32 path returned by the shell.
    let ok = unsafe { SHGetPathFromIDListW(pidl, path.as_mut_ptr()) };
    // SAFETY: `pidl` came from the shell allocator and is freed exactly once here.
    unsafe { CoTaskMemFree(pidl.cast()) };

    if ok == 0 {
        return None;
    }

    let length = path.iter().position(|unit| *unit == 0).unwrap_or(0);
    if length == 0 {
        return None;
    }

    Some(PathBuf::from(String::from_utf16_lossy(&path[..length])))
}

#[cfg(not(windows))]
pub fn pick_folder(_owner: Option<isize>, _title: &str) -> Option<PathBuf> {
    None
}
