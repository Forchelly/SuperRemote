//! 终端字体候选：枚举系统里真正装了哪些等宽字体族。
//!
//! 为什么只收等宽：终端网格用一个测出来的字宽（`terminal::Metrics::cell_width`）
//! 排所有列，非等宽字体的字形会互相压叠。旧版 `CommonServices.GetEqualWidthFonts()`
//! 也是同一个判据（`lfPitchAndFamily & 0x3 == 1`），这里是对等的 Rust 实现。
//!
//! 枚举结果缓存在进程级 `OnceLock` 里：字体下拉每帧都要重建，而枚举要回调几百次
//! GDI。新装字体属于极低频事件，重启后生效可以接受——与 `display::resolutions()`
//! 同一口径。

use std::sync::OnceLock;

/// 系统等宽字体族名（按字母序，大小写不敏感）。
///
/// 一个都没有时用兜底列表：非 Windows 目标没有 GDI，精简系统或未装字体的容器
/// 也会返回空——列表空掉会让下拉只剩「继承」一档，比给出几个常见名字更难懂。
pub fn monospace_families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();

    FAMILIES.get_or_init(|| {
        let mut names = platform::monospace_families();
        if names.is_empty() {
            names = FALLBACK_FAMILIES
                .iter()
                .map(|name| (*name).to_owned())
                .collect();
        }
        names.sort_by_key(|name| name.to_lowercase());
        names
    })
}

/// 系统一个等宽字体都没上报时的候选。前三者是 Windows/Linux 的默认等宽字体，
/// Menlo 是 macOS 的。
const FALLBACK_FAMILIES: [&str; 5] = [
    "Consolas",
    "Cascadia Mono",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "Menlo",
];

#[cfg(windows)]
mod platform {
    use std::ptr;

    use windows_sys::Win32::Foundation::LPARAM;
    use windows_sys::Win32::Graphics::Gdi::{
        EnumFontFamiliesExW, GetDC, ReleaseDC, DEFAULT_CHARSET, LOGFONTW, TEXTMETRICW,
    };

    /// `lfPitchAndFamily` 低两位是字距类别，`01` 表示固定字距（等宽）。
    /// 高位那截是字体族索引（roman/swiss/…），与等宽无关，所以只取低两位。
    const PITCH_MASK: u8 = 0x03;
    const PITCH_FIXED: u8 = 0x01;

    pub(super) fn monospace_families() -> Vec<String> {
        let mut names: Vec<String> = Vec::new();

        // `EnumFontFamiliesExW` 要求 `lfFaceName` 为空、`lfCharSet` 有值：
        // 前者表示「所有族」，后者决定上报哪一套字形命名（DEFAULT_CHARSET = 本机首选）。
        let probe = LOGFONTW {
            lfCharSet: DEFAULT_CHARSET,
            ..LOGFONTW::default()
        };

        // SAFETY: `GetDC(null)` 取屏幕设备上下文，是文档允许的用法；返回空句柄
        // 说明拿不到 DC，直接放弃枚举（调用方会退回兜底列表）。
        let hdc = unsafe { GetDC(ptr::null_mut()) };
        if hdc.is_null() {
            return names;
        }

        // SAFETY: `probe` 是已初始化的 `LOGFONTW`，GDI 只读它；`names` 在整段调用
        // 期间被本函数可变持有，指针经 `LPARAM` 传给回调，回调期间没有别的借用者。
        let enumerated = unsafe {
            EnumFontFamiliesExW(
                hdc,
                &probe,
                Some(collect),
                &mut names as *mut Vec<String> as LPARAM,
                0,
            )
        };

        // SAFETY: `hdc` 来自上面的 `GetDC(null)`，成对释放；窗口句柄传 null 与之匹配。
        unsafe { ReleaseDC(ptr::null_mut(), hdc) };

        // 返回 0 有两种意思：真的没有匹配的字体，或参数有误。两者都由调用方
        // 的兜底列表接住，这里不必区分——枚举失败不该让界面报错。
        let _ = enumerated;
        names
    }

    /// GDI 的枚举回调。返回 1 表示「继续枚举」。
    ///
    /// 回调签名里第一个参数被 `windows-sys` 声明成 `*const LOGFONTW`，实际传进来的是
    /// `ENUMLOGFONTEXW`。取 `LOGFONTW` 是安全的：`ENUMLOGFONTEXW` 是 `#[repr(C)]`，
    /// 而它的第一个字段就是 `elfLogFont: LOGFONTW`，两者首地址相同；本回调只读这个前缀。
    unsafe extern "system" fn collect(
        log_font: *const LOGFONTW,
        _metrics: *const TEXTMETRICW,
        _font_type: u32,
        state: LPARAM,
    ) -> i32 {
        if log_font.is_null() {
            return 1;
        }

        // SAFETY: 上面已判空，且 GDI 保证回调期间这块内存可读。
        let font = unsafe { &*log_font };

        if font.lfPitchAndFamily & PITCH_MASK != PITCH_FIXED {
            return 1;
        }

        let Some(name) = face_name(&font.lfFaceName) else {
            return 1;
        };
        // `@` 开头的是竖排/旋转字形（如 `@SimSun`），与同名的横排字体是一个东西，
        // 列出来只会让下拉里出现两份。
        if name.is_empty() || name.starts_with('@') {
            return 1;
        }

        // SAFETY: `state` 是本函数里 `&mut names` 转成的 `LPARAM`，生命周期覆盖整段枚举。
        let names = unsafe { &mut *(state as *mut Vec<String>) };
        if !names.iter().any(|existing| existing == &name) {
            names.push(name);
        }
        1
    }

    /// `lfFaceName` 是固定 32 个 `u16` 的 UTF-16 串，以 NUL 截断。
    fn face_name(raw: &[u16; 32]) -> Option<String> {
        let end = raw.iter().position(|unit| *unit == 0).unwrap_or(raw.len());
        String::from_utf16(&raw[..end]).ok()
    }
}

#[cfg(not(windows))]
mod platform {
    pub(super) fn monospace_families() -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机枚举一次：只断言「非空、没有空串、没有 `@` 变体、已排序、不重」，
    /// 不断言具体字体名——每台机器装的字体不一样，写死名字会在别人机器上挂掉。
    #[test]
    fn enumerated_families_are_clean_sorted_and_unique() {
        let families = monospace_families();
        assert!(!families.is_empty(), "空列表会让字体下拉只剩「继承」一档");

        let lower: Vec<String> = families
            .iter()
            .map(|family| family.to_lowercase())
            .collect();

        let mut sorted = lower.clone();
        sorted.sort();
        assert_eq!(lower, sorted, "顺序必须稳定：Iced 按下标 diff 下拉项");

        let mut unique = lower.clone();
        unique.dedup();
        assert_eq!(
            unique.len(),
            lower.len(),
            "同一个族不能出现两次（大小写不同也算）"
        );

        for family in families {
            assert!(!family.trim().is_empty(), "族名不能是空白");
            assert!(!family.starts_with('@'), "竖排变体不该出现在列表里");
        }
    }

    /// 枚举没生效时会静默退回 5 条兜底名，界面看起来正常、下拉却少了本机真装的字体。
    /// 兜底名单以外的名字只能来自 GDI，所以用它证明枚举真的跑过。
    #[cfg(windows)]
    #[test]
    fn windows_enumeration_reaches_beyond_the_fallback_list() {
        assert!(
            monospace_families()
                .iter()
                .any(|family| !FALLBACK_FAMILIES.contains(&family.as_str())),
            "Windows 上没拿到兜底列表以外的字体，说明 EnumFontFamiliesExW 没生效"
        );
    }
}
