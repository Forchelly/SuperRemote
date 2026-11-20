//! 显示模式：分辨率下拉的数据来源与主显示器尺寸。
//!
//! 旧版用 `EnumDisplaySettings` 枚举显卡上报的全部显示模式，本模块是对等的 Rust
//! 实现。非 Windows 目标没有这套 API，退回空列表，由调用方用预置档补齐。
//!
//! 枚举结果缓存在进程级 `OnceLock` 里：分辨率下拉每帧都要重建，而枚举要循环
//! 上百次系统调用，放在 `view` 里跑等于每帧一次全量枚举。显示器热插拔属于
//! 极低频事件，重启后生效可以接受。

use std::sync::OnceLock;

use superremote_core::session::{
    DESKTOP_PRESETS, MAX_DESKTOP_HEIGHT, MAX_DESKTOP_WIDTH, MIN_DESKTOP_HEIGHT, MIN_DESKTOP_WIDTH,
};

static MODES: OnceLock<Vec<(u32, u32)>> = OnceLock::new();

/// 枚举次数上限。显卡上报的模式通常在几十到两百之间，给一个硬上限是为了
/// 不让驱动异常（一直返回成功）把界面拖死。
const MAX_MODES: u32 = 512;

/// 分辨率下拉的候选：系统上报的模式并入预置档，去重后按宽度降序。
///
/// 顺序固定且与当前选中值无关——Iced 按子元素位置 diff，选项列表每帧重排
/// 会让下拉的选中态错位。
pub fn resolutions() -> &'static [(u32, u32)] {
    MODES
        .get_or_init(|| merge_modes(&platform::modes()))
        .as_slice()
}

/// 主显示器的当前分辨率。RDP 选「全屏」时用它写 `desktopwidth/height`。
pub fn primary_size() -> Option<(u32, u32)> {
    platform::primary_size()
}

/// 系统枚举结果与预置档合并：夹到合法区间、去重、宽高降序。
fn merge_modes(system: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut all: Vec<(u32, u32)> = DESKTOP_PRESETS.to_vec();
    all.extend(system.iter().copied().filter_map(clamp));

    // 降序排：大分辨率在上，与旧版 `OrderByDescending` 的顺序一致。
    all.sort_unstable_by(|a, b| b.cmp(a));
    all.dedup();
    all
}

fn clamp((width, height): (u32, u32)) -> Option<(u32, u32)> {
    if !(MIN_DESKTOP_WIDTH..=MAX_DESKTOP_WIDTH).contains(&width)
        || !(MIN_DESKTOP_HEIGHT..=MAX_DESKTOP_HEIGHT).contains(&height)
    {
        return None;
    }
    Some((width, height))
}

#[cfg(windows)]
mod platform {
    use std::ptr;
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS,
    };

    use super::MAX_MODES;

    /// 一个清零的 `DEVMODEW`，`dmSize` 填成结构体实际大小。
    ///
    /// `EnumDisplaySettingsW` 靠 `dmSize` 判断调用方给它的是哪一版结构，
    /// 不填或填错会让调用直接失败。
    fn devmode() -> DEVMODEW {
        DEVMODEW {
            dmSize: u16::try_from(std::mem::size_of::<DEVMODEW>()).unwrap_or(0),
            ..DEVMODEW::default()
        }
    }

    /// 枚举主显示器的全部显示模式。设备名传 `null` 表示「当前默认设备」，
    /// 与旧版 `EnumDisplaySettings(null, i)` 的语义一致。
    pub(super) fn modes() -> Vec<(u32, u32)> {
        let mut modes = Vec::new();

        for index in 0..MAX_MODES {
            let mut mode = devmode();
            // SAFETY: `mode` 是已初始化且可写的 `DEVMODEW`，`dmSize` 已填为结构体
            // 实际大小；`EnumDisplaySettingsW` 只写这一块内存。设备名传 `null` 是
            // 文档允许的取值（取默认显示设备）。返回值只用来判断是否还有下一项。
            let ok = unsafe { EnumDisplaySettingsW(ptr::null(), index, &mut mode) };
            if ok == 0 {
                break;
            }

            modes.push((mode.dmPelsWidth, mode.dmPelsHeight));
        }

        modes
    }

    /// 主显示器**当前**的分辨率（`ENUM_CURRENT_SETTINGS`），不是某个枚举出来的模式。
    pub(super) fn primary_size() -> Option<(u32, u32)> {
        let mut mode = devmode();

        // SAFETY: 同上，`mode` 是已初始化的可写 `DEVMODEW`，`dmSize` 正确。
        let ok = unsafe { EnumDisplaySettingsW(ptr::null(), ENUM_CURRENT_SETTINGS, &mut mode) };

        (ok != 0).then_some((mode.dmPelsWidth, mode.dmPelsHeight))
    }
}

#[cfg(not(windows))]
mod platform {
    pub(super) fn modes() -> Vec<(u32, u32)> {
        Vec::new()
    }

    pub(super) fn primary_size() -> Option<(u32, u32)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_survive_an_empty_system_list() {
        let merged = merge_modes(&[]);

        assert!(merged.contains(&(1280, 720)));
        assert!(merged.contains(&(3840, 2160)));
        // 降序：第一项必须是宽度最大的那一档。
        assert_eq!(merged.first(), Some(&(3840, 2160)));
    }

    #[test]
    fn system_modes_are_merged_deduped_and_sorted() {
        let merged = merge_modes(&[(1600, 900), (1920, 1080), (1600, 900)]);

        assert_eq!(
            merged.iter().filter(|mode| **mode == (1600, 900)).count(),
            1
        );
        let widths: Vec<u32> = merged.iter().map(|(width, _)| *width).collect();
        let mut sorted = widths.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(widths, sorted);
    }

    #[test]
    fn out_of_range_modes_are_dropped() {
        assert!(clamp((10, 10)).is_none());
        assert!(clamp((99999, 1080)).is_none());
        assert_eq!(clamp((1366, 768)), Some((1366, 768)));
    }

    /// 本机枚举一次，只断言「不崩且结果合法」，不断言具体分辨率——
    /// 不同机器/不同显卡报的模式不一样，写死数字会让测试在别人机器上挂掉。
    #[test]
    fn enumerating_on_this_machine_stays_within_bounds() {
        for (width, height) in resolutions() {
            assert!((MIN_DESKTOP_WIDTH..=MAX_DESKTOP_WIDTH).contains(width));
            assert!((MIN_DESKTOP_HEIGHT..=MAX_DESKTOP_HEIGHT).contains(height));
        }
    }
}
