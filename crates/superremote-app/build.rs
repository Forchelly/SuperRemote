// 在 Windows 上把 assets/icon.ico 链接进 exe，作为应用图标（资源管理器/任务栏显示）。
// 同时编译 RDP ActiveX 小宿主：它由 `host/Program.cs` 生成，运行时释放到临时目录，
// 由 Rust 侧嵌入标签页。宿主只是一个直连 RDP 的原生窗口，不写 `.rdp` 文件。
// 非 Windows 构建为空操作；资源编译失败时只告警，不阻断构建。
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    // 版本资源里 ProductName / FileDescription 是任务管理器与「属性 → 详细信息」
    // 直接读的字段。winresource 默认拿 **Cargo 包名**填这两项（`superremote-app`），
    // 不改的话任务管理器会显示内部 crate 名而不是产品名。
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    res.set("ProductName", "SuperRemote")
        .set("FileDescription", "SuperRemote - 运维连接管理")
        .set("ProductVersion", &version)
        .set("FileVersion", &version)
        .set("InternalName", "SuperRemote")
        .set("OriginalFilename", "SuperRemote.exe");
    if let Err(e) = res.compile() {
        println!("cargo:warning=嵌入 Windows 图标资源失败: {e}");
    }

    compile_rdp_host();
}

/// 编译 C# RDP 宿主。csc.exe 随 .NET Framework 一起装在 Windows 上；
/// 这里直接调用它，不引入 MSBuild/NuGet。
fn compile_rdp_host() {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let source = manifest.join("host").join("Program.cs");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    // 宿主进程与主程序同名族（`SuperRemote.Host.exe`）：任务管理器里能一眼看出
    // 它是本产品的子进程，而不是陌生的 `superremote-rdp-host`。文件名被 `rdp.rs` 的
    // `include_bytes!` 引用，改这里必须同步改那边。
    let target = out_dir.join("SuperRemote.Host.exe");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed=build.rs");
    // 图标与版本资源都进 exe：不声明依赖的话，换图标后 cargo 会认为 nothing
    // changed，旧 exe 里仍是旧图标。
    println!("cargo:rerun-if-changed=assets/icon.ico");

    let windows = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    let frameworks = [
        PathBuf::from(&windows).join("Microsoft.NET\\Framework64\\v4.0.30319"),
        PathBuf::from(&windows).join("Microsoft.NET\\Framework\\v4.0.30319"),
    ];
    let Some(framework) = frameworks.iter().find(|dir| dir.join("csc.exe").is_file()) else {
        panic!("找不到 .NET Framework 的 csc.exe；RDP ActiveX 宿主无法编译");
    };
    let csc = framework.join("csc.exe");

    let output = Command::new(&csc)
        .current_dir(framework)
        .args([
            "-nologo",
            "-target:winexe",
            "-platform:x64",
            "-optimize+",
            &format!("-out:{}", target.display()),
            "-r:System.dll",
            "-r:System.Drawing.dll",
            "-r:System.Windows.Forms.dll",
            "-r:Microsoft.CSharp.dll",
        ])
        .arg(&source)
        .output()
        .unwrap_or_else(|err| panic!("无法启动 C# 编译器 {}：{err}", csc.display()));

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "编译 RDP ActiveX 宿主失败（{}）：\n{stdout}\n{stderr}",
            source.display()
        );
    }
    assert!(
        Path::new(&target).is_file(),
        "C# 编译器没有产出 {}",
        target.display()
    );
}
