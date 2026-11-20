# 本地补丁清单（iced_tiny_skia 0.14.1）

这个目录是上游 [iced-rs/iced](https://github.com/iced-rs/iced) `iced_tiny_skia 0.14.1`
的本地副本，经根 `Cargo.toml` 的 `[patch.crates-io]` 生效。上游 0.14.1 的软渲染后端
按**脏区**呈现窗口，下面每一条都是围绕它的修正。**升级 iced 版本时，按这份清单把
补丁逐条重放到新副本上**——`[patch.crates-io]` 指向的路径没变不等于补丁还在生效，
重放后必须跑过「验证」一节列出的回归测试。

改动集中在两个文件：

- `src/layer.rs` — `Layer::damage` 的脏区计算（补丁 1、2）
- `src/window/compositor.rs` — `present` 的判据与脏区合并（补丁 3、4、5）

---

## 补丁 1：quad 脏区按阴影扩散范围扩张（`layer.rs::damage`）

上游对 quad 的脏区只取 `quad.bounds`。但 `engine::draw_quad` 的阴影画在
`bounds + offset ± blur_radius`（`shadow_bounds`），脏区不盖住这一圈的话，
卡片阴影永远不算进脏区，表现为**卡片周围残留灰块**。

本地改动：脏区按 `1 + blur_radius + max(|offset.x|, |offset.y|)` 扩张，
再与当前层 `bounds` 求交。

## 补丁 2：文本脏区扩 2px（`layer.rs::damage`）

字形墨迹会略微超出行盒（亚像素定位 + hinting）。脏区只取 `visible_bounds`
的话，边缘留一像素宽的旧字（切语言时最明显）。本地改动：文本脏区统一
`expand(2.0)`。

## 补丁 3：present 先整层比相等，脏区算空时退回整窗重绘（`compositor.rs::present`）

上游直接信 `Layer::damage` 的结果：damage 为空 ⇒ 整帧跳过。当 damage 的
计算漏掉某个图元（补丁 1、2 修的正是两类漏算，但不能保证穷尽），漏掉的
图元会被**永久留在屏幕上**。

本地改动：在走 damage 之前先把整层栈与当前帧做一次相等比较——内容一模一样
才允许跳过；有任何差异就至少走脏区路径，**脏区算空则退回整窗重绘**。这是
「宁可多画一帧，也不让漏算变成永久残影」的兜底。

## 补丁 4：把大量零碎脏区收敛成一个包围盒（`compositor.rs::coalesce`）

`Renderer::draw` 对**每个**脏区都会重新遍历全部图层与图元。整页逐条文本
变更（典型：切语言）会产生近百个小区块（实测 96 块），单帧从 ~17ms 涨到
450ms；这些块本身已覆盖大半个窗口，合成一个包围盒只多画极少量像素，却把
图元遍历从 96 次降到 1 次。

本地规则（常量在 `compositor.rs` 顶部）：

- `MAX_DAMAGE_RECTS = 4`：不多于 4 块不合并（小区块各自局部重画更省）。
- `MAX_DAMAGE_OVERDRAW = 3.0`：包围盒面积 / 脏区总面积超过 3 倍视为稀疏
  （例如终端里两处零散光标变化），不合并，避免为几行字重画整窗。

## 补丁 5：临时诊断模块（`compositor.rs::mod diag` + `Surface` 诊断字段）

「拖边时亮一下暗一下」暗帧取证用：present 判据、缓冲四点采样、帧耗时写
`SR2_FRAME_LOG`。**默认关闭**（逐帧写盘曾把 `%TEMP%\sr2-frame.log` 积到
268MB），取证时设 `SR2_FRAME_LOG=<路径>`（值为 `1` 时落回默认路径）。
配套对照开关 `SR2_FULL_REPAINT=1` 强制整窗重绘。定位完成后整块删除，
连同 `Surface` 上的三个诊断字段（`max_age`/`layer_stack` 快照等）。

---

## 验证

补丁 1、4 的行为由仓库内的回归测试守住（`cargo test -p iced_tiny_skia`）：

| 测试 | 守住的不变量 |
|------|--------------|
| `quad_damage_covers_the_shadow_spread` | 阴影扩散圈必须被脏区覆盖（补丁 1） |
| `many_adjacent_damage_rects_are_coalesced` | 密集碎块合成包围盒（补丁 4） |
| `few_damage_rects_are_left_alone` | ≤4 块不合并（补丁 4） |
| `sparse_damage_rects_are_not_coalesced` | 稀疏碎块不合并（补丁 4） |

补丁 2、3 没有单独的单元测试（需要真实渲染管线比对像素），升级重放后按
下列场景手工过一遍：

1. 空闲窗口无重绘残留（补丁 3 的相等判据）。
2. 切换界面语言，无旧字形残影、无明显卡顿（补丁 2 + 4）。
3. 详情页卡片阴影周围无灰块残留（补丁 1）。

## 升级重放步骤

1. 拷贝新版 `iced_tiny_skia` 源码到本目录，替换 `src/`。
2. 按本文档逐条重放 `layer.rs` 与 `compositor.rs` 的改动（改动处都带
   「上游」「本地补丁」「兜底判据」等中文注释，grep 这些词可以定位）。
3. 跑 `cargo test -p iced_tiny_skia`，四条回归必须全绿。
4. 手工过上面三个场景。
5. 确认根 `Cargo.toml` 的 `[patch.crates-io]` 指向仍为本目录，且
   `cargo tree -p superremote-app | findstr tiny_skia` 解析到本地路径
   而不是 crates.io 版本。
