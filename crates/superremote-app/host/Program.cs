using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Drawing;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows.Forms;
// 只取这三个类型：整条 using System.Threading 会让 HostForm 里的 WinForms
// Timer 与 System.Threading.Timer 撞成 CS0104。
using Thread = System.Threading.Thread;
using ApartmentState = System.Threading.ApartmentState;
using ManualResetEventSlim = System.Threading.ManualResetEventSlim;

// SuperRemote RDP ActiveX host.
//
// The Rust app embeds this window with SetParent, exactly like it used to embed
// mstsc.exe.  Unlike mstsc + .rdp, this control connects directly and never
// opens an .rdp file, so the April 2026 "unsigned RDP file" dialog is not
// involved.  RedirectionWarningDialogVersion=1 and AuthenticationLevel=0 are
// still set explicitly; see the Microsoft docs referenced in AGENTS.md.
//
// Keeping this helper C#-only (no AxInterop/MSTSCLib DLLs) relies on the
// generated AxHost and late-bound COM.
//
// One process can host several sessions.  Every session is its own top-level
// `HostForm` (Rust embeds each one into its own tab with SetParent) and carries
// its id in the window title, which stays the response channel:
// "SUPERREMOTE|<session>|connecting|", "SUPERREMOTE|<session>|connected|",
// "SUPERREMOTE|<session>|failed|<msg>".
// Commands from Rust go through a hidden control window with WM_COPYDATA
// (`add` / `close`); responses never use that channel.
/// 排查用时序日志。默认开启、2MB 封顶、只记变化与 2 秒心跳，写失败一律吞掉，
/// 日志本身绝不能影响连接或画面。与 Rust 侧的 rdp-app.log 放同一目录，复现
/// 一次即可按时间戳对齐「画面变蓝的几秒里谁停了」。
static class Trace
{
    const long MaxBytes = 2 * 1024 * 1024;
    static readonly object Gate = new object();
    static readonly string FilePath = System.IO.Path.Combine(
        System.IO.Path.GetTempPath(), "SimpleRemote2",
        "rdp-host-" + System.Diagnostics.Process.GetCurrentProcess().Id + ".log");
    static long _written = ExistingBytes();

    static long ExistingBytes()
    {
        try
        {
            var info = new System.IO.FileInfo(FilePath);
            return info.Exists ? info.Length : 0;
        }
        catch { return 0; }
    }

    public static string LogPath { get { return FilePath; } }

    public static void Write(string format, params object[] args)
    {
        try
        {
            string text = args.Length == 0 ? format : string.Format(format, args);
            string line = DateTime.Now.ToString("HH:mm:ss.fff")
                + " t" + Thread.CurrentThread.ManagedThreadId + " " + text + "\r\n";
            byte[] bytes = Encoding.UTF8.GetBytes(line);
            lock (Gate)
            {
                if (_written + bytes.Length > MaxBytes) return;
                System.IO.Directory.CreateDirectory(System.IO.Path.GetDirectoryName(FilePath));
                System.IO.File.AppendAllText(FilePath, line, Encoding.UTF8);
                _written += bytes.Length;
            }
        }
        catch { }
    }
}

static class Program
{
    const string TitlePrefix = "SUPERREMOTE|";
    internal const string ControlTitle = "SUPERREMOTE|ctl";
    internal const string CommandAdd = "add";
    internal const string CommandClose = "close";

    [STAThread]
    static void Main(string[] args)
    {
        // The parent Iced window is Per-Monitor V2.  A SystemAware helper would
        // be DPI-virtualized under it and the mouse cursor could jump between
        // scales; keep the child in the same awareness bucket.
        SetProcessDpiAwarenessContext(new IntPtr(-4));
        Application.EnableVisualStyles();
        Trace.Write("host-start pid={0} log={1}",
            System.Diagnostics.Process.GetCurrentProcess().Id, Trace.LogPath);
        Application.Run(new HostContext(Args.Read(args)));
    }

    [DllImport("user32.dll")]
    static extern bool SetProcessDpiAwarenessContext(IntPtr value);

    internal static string Title(string session, string state, string message)
    {
        return TitlePrefix + session + "|" + state + "|" + (message ?? string.Empty);
    }
}

sealed class Args
{
    readonly string[] _args;

    Args(string[] args) { _args = args; }

    public static Args Read(string[] args) { return new Args(args ?? new string[0]); }

    public string Get(string name, string fallback)
    {
        for (int i = 0; i + 1 < _args.Length; i++)
        {
            if (string.Equals(_args[i], "--" + name, StringComparison.OrdinalIgnoreCase))
                return _args[i + 1];
        }
        return fallback;
    }

    public int Int(string name, int fallback)
    {
        int value;
        return int.TryParse(Get(name, string.Empty), out value) ? value : fallback;
    }

    public bool Bool(string name, bool fallback)
    {
        string value = Get(name, string.Empty);
        if (string.IsNullOrEmpty(value)) return fallback;
        return value == "1" || string.Equals(value, "true", StringComparison.OrdinalIgnoreCase);
    }
}

// 进程仍然共用（省内存），但每路会话各自一条 STA 消息循环：一路的 COM 或窗口
// 调用阻塞时，不拖住同进程的其它会话。这里只隔离消息循环，不增加进程数。
sealed class HostContext : ApplicationContext
{
    sealed class SessionSlot
    {
        public Thread Thread;
        public HostForm Form;
    }

    readonly ControlWindow _control;
    readonly object _gate = new object();
    readonly Dictionary<string, SessionSlot> _sessions = new Dictionary<string, SessionSlot>();
    bool _closed;

    public HostContext(Args args)
    {
        _control = new ControlWindow(this);
        string session = args.Get("session", string.Empty);
        if (ValidSession(session))
            Add(session, args);
    }

    // Payload layout (UTF-16LE through WM_COPYDATA):
    //   add\n<session>\n--flag\n<value>\n...
    //   close\n<session>
    public void Dispatch(string text)
    {
        if (text == null) return;
        string[] lines = text.Replace("\r", string.Empty).Split('\n');
        if (lines.Length == 0) return;

        string command = lines[0];
        if (command == Program.CommandClose)
        {
            if (lines.Length > 1) Remove(lines[1]);
            return;
        }
        if (command != Program.CommandAdd || lines.Length < 3) return;

        string session = lines[1];
        string[] args = new string[lines.Length - 2];
        Array.Copy(lines, 2, args, 0, args.Length);
        Add(session, Args.Read(args));
    }

    void Add(string session, Args args)
    {
        // 会话 id 会进入窗口标题（`|` 是分隔符），并把窗口交给 Rust 按 id 认领，
        // 所以非法或重复的 id 直接拒绝，不创建第二份窗口。
        if (!ValidSession(session)) return;
        lock (_gate)
        {
            if (_sessions.ContainsKey(session)) return;
        }

        var slot = new SessionSlot();
        using (var ready = new ManualResetEventSlim(false))
        {
            slot.Thread = new Thread(() =>
            {
                HostForm form = null;
                try
                {
                    // 控件必须建在自己那条 GUI 线程上：WinForms Timer 与 ActiveX
                    // 的消息都跟着创建线程的消息循环走。
                    form = new HostForm(args, session);
                    slot.Form = form;
                }
                finally
                {
                    ready.Set();
                }
                if (form == null) return;
                form.FormClosed += delegate { Forget(session); };
                Application.Run(form);
            });
            slot.Thread.SetApartmentState(ApartmentState.STA);
            slot.Thread.IsBackground = true;
            slot.Thread.Start();
            ready.Wait();
        }

        if (slot.Form == null)
        {
            Forget(session);
            return;
        }
        lock (_gate) { _sessions.Add(session, slot); }
    }

    void Remove(string session)
    {
        SessionSlot slot;
        lock (_gate)
        {
            if (!_sessions.TryGetValue(session, out slot)) return;
        }
        HostForm form = slot.Form;
        if (form != null && form.IsHandleCreated)
        {
            try
            {
                form.BeginInvoke((Action)form.Close);
                return;
            }
            catch
            {
                // 窗口已经在拆，落到 Forget 收尾。
            }
        }
        Forget(session);
    }

    void Forget(string session)
    {
        bool last;
        lock (_gate)
        {
            _sessions.Remove(session);
            last = _sessions.Count == 0 && !_closed;
            if (last) _closed = true;
        }
        // ExitThread 必须在创建 ApplicationContext 的控制窗口线程上发生，
        // 这里只投递消息，不跨线程直接终止消息循环。
        if (last) _control.RequestExit();
    }

    static bool ValidSession(string session)
    {
        if (string.IsNullOrEmpty(session)) return false;
        for (int i = 0; i < session.Length; i++)
        {
            char c = session[i];
            if (c == '|' || char.IsControl(c)) return false;
        }
        return true;
    }

    protected override void ExitThreadCore()
    {
        _control.DestroyHandle();
        base.ExitThreadCore();
    }
}

// Command channel from Rust: a hidden top-level tool window (message-only
// windows are not enumerated by EnumWindows, so it must be a real one).
// The title is stable (`SUPERREMOTE|ctl`); Rust finds it by pid + title.
sealed class ControlWindow : NativeWindow
{
    const int WM_COPYDATA = 0x004A;
    const int WM_APP_EXIT = 0x8001;
    const int WS_EX_TOOLWINDOW = 0x00000080;
    const int ProtocolVersion = 1;

    [DllImport("user32.dll")]
    static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    struct COPYDATASTRUCT
    {
        public IntPtr dwData;
        public int cbData;
        public IntPtr lpData;
    }

    readonly HostContext _owner;

    public ControlWindow(HostContext owner)
    {
        _owner = owner;

        CreateParams cp = new CreateParams();
        cp.Caption = Program.ControlTitle;
        cp.Style = 0;
        cp.ExStyle = WS_EX_TOOLWINDOW;
        cp.X = -32000;
        cp.Y = -32000;
        cp.Width = 0;
        cp.Height = 0;
        CreateHandle(cp);
    }

    /// 最后一路会话退出后，把「结束主线程消息循环」投回控制窗口所属线程。
    public void RequestExit()
    {
        PostMessageW(Handle, WM_APP_EXIT, IntPtr.Zero, IntPtr.Zero);
    }

    protected override void WndProc(ref Message m)
    {
        if (m.Msg == WM_APP_EXIT)
        {
            _owner.ExitThread();
            m.Result = IntPtr.Zero;
            return;
        }
        if (m.Msg == WM_COPYDATA)
        {
            COPYDATASTRUCT data = (COPYDATASTRUCT)Marshal.PtrToStructure(
                m.LParam, typeof(COPYDATASTRUCT));
            if (data.dwData == (IntPtr)ProtocolVersion && data.cbData > 0)
            {
                byte[] bytes = new byte[data.cbData];
                Marshal.Copy(data.lpData, bytes, 0, bytes.Length);
                _owner.Dispatch(Encoding.Unicode.GetString(bytes));
            }
            m.Result = new IntPtr(1);
            return;
        }
        base.WndProc(ref m);
    }
}

sealed class HostForm : Form
{
    readonly AxRdpHost _host;
    readonly Timer _timer;
    readonly string _server;
    readonly string _user;
    readonly int _port;
    readonly int _width;
    readonly int _height;
    readonly bool _smartSizing;
    readonly int _color;
    readonly int _audio;
    readonly bool _audioCapture;
    readonly int _keyboard;
    readonly bool _redirectPrinters;
    readonly bool _redirectClipboard;
    readonly bool _redirectSmartcards;
    readonly bool _redirectPorts;
    readonly bool _redirectDrives;
    readonly int _performance;
    readonly bool _trimGraphics;
    readonly Color _backdrop;
    readonly string _session;

    string _state = "connecting";
    DateTime _started = DateTime.UtcNow;
    // 已连接后 `Connected` 连续为 0 的拍数（250ms 一拍）。远端 UAC 切安全桌面
    // 时这个 COM 属性会短暂归零或抛异常，一拍定死会把好会话误判断线。
    int _idleTicks;
    int _lastState = -1;
    int _tickCount;
    DateTime _lastHeartbeat = DateTime.MinValue;

    /// 用哪个版本的 RDP ActiveX 控件：`11`（默认，RDP 10 + AVC）或
    /// `10`（旧版 C# 同款，RDP 8.1）。该参数只选择协议与兼容性，不作为
    /// UAC 或安全桌面卡顿的开关。保留 `--rdp-control` / 环境变量做对照。
    readonly string _control = "11";

    public HostForm(Args args, string session)
    {
        _session = session;
        _server = args.Get("server", "127.0.0.1");
        _user = args.Get("user", string.Empty);
        _port = args.Int("port", 3389);
        _width = Math.Max(640, args.Int("width", 1280));
        _height = Math.Max(480, args.Int("height", 800));
        _smartSizing = args.Bool("smart-sizing", true);
        _color = args.Int("color", 32);
        _audio = args.Int("audio", 0);
        _audioCapture = args.Bool("audio-capture", false);
        _keyboard = args.Int("keyboard", 0);
        _redirectPrinters = args.Bool("redirect-printers", false);
        _redirectClipboard = args.Bool("redirect-clipboard", false);
        _redirectSmartcards = args.Bool("redirect-smartcards", false);
        _redirectPorts = args.Bool("redirect-ports", false);
        _redirectDrives = args.Bool("redirect-drives", false);
        _performance = args.Int("performance", 0);
        // 环境变量优先：命令行由 Rust 侧按连接设置固定传入，而排障时更想
        // 「不改配置、只换一次控件」做 A/B，所以把 env 放在最前面。
        _control = Environment.GetEnvironmentVariable("SUPERREMOTE_RDP_CONTROL")
            ?? args.Get("rdp-control", "11");
        // 只有目标确实是本机/内网时才裁剪「纯省内存」的体验项；桌面合成、
        // 主题与持久位图缓存属于会话管线，一律不裁。解析放到宿主进程里：
        // Rust 侧在 update 里做 DNS 会冻结界面。
        _trimGraphics = LocalNetwork.IsPrivateTarget(_server);
        _backdrop = ParseColor(args.Get("backdrop", string.Empty));

        FormBorderStyle = FormBorderStyle.None;
        AutoScaleMode = AutoScaleMode.None;
        ShowInTaskbar = false;
        StartPosition = FormStartPosition.Manual;
        // Stay off-screen until the Rust host reparents the window.  The
        // control still creates its HWND and connects normally.
        Location = new Point(-32000, -32000);
        Size = new Size(_width, _height);
        // Fills the area the control does not cover when the window is larger
        // than the remote desktop.  The colour is supplied by the Rust side
        // (`--backdrop`, derived from the active theme) so the native window and
        // the Iced backdrop behind a frozen drag frame are always identical.
        BackColor = _backdrop;
        Text = Program.Title(_session, "connecting", string.Empty);

        _host = new AxRdpHost(ControlClsid(_control));
        ((ISupportInitialize)_host).BeginInit();
        Controls.Add(_host);
        ((ISupportInitialize)_host).EndInit();
        // The control is sized explicitly instead of docked, so it can stay
        // clamped to the remote desktop size and keep the picture 1:1 from the
        // client origin: top-left anchored, never scaled and never centred.
        // See CoverClient().
        _host.Dock = DockStyle.None;
        CoverClient();
        ClientSizeChanged += OnClientSizeChanged;

        _timer = new Timer { Interval = 250 };
        _timer.Tick += OnTick;
        Shown += OnShown;
        FormClosed += delegate
        {
            Trace.Write("form-closed session={0} state={1}", _session, _state);
        };
        Trace.Write("form-created session={0} server={1} {2}x{3} control={4} log={5}",
            _session, _server, _width, _height, _control, Trace.LogPath);
    }

    /// 控件版本 → CLSID。注册名里的「version N」比 .NET 类名（MsRdpClientN）大 1：
    /// `MsRdpClient10NotSafeForScripting` 就是 "version 11"，v11 协商 RDP 10；
    /// 旧版 C# 的 `AxMsRdpClient9` 是 "version 10"，只协商到 RDP 8.1。
    static string ControlClsid(string version)
    {
        switch (version)
        {
            case "10": return "{8B918B82-7985-4C24-89DF-C33AD2BBFBCD}";
            case "8": return "{54D38BF7-B1EF-4479-9674-1BD6EA465258}";
            default: return DefaultClsid;
        }
    }

    const string DefaultClsid = "{A0C63C30-F08D-4AB4-907C-34905D770C7D}";

    /// 默认控件之外都算「对照模式」，扩展设置接口拿不到时要容忍而不是直接判失败。
    static bool IsDefaultControl(string version)
    {
        return ControlClsid(version) == DefaultClsid;
    }


    /// 记录窗口与显示状态变化。它们只能作为时间线参考，不能据此认定远端
    /// UAC 或安全桌面切换的开始与结束。
    protected override void WndProc(ref Message m)
    {
        switch (m.Msg)
        {
            case 0x001C: Trace.Write("msg WM_ACTIVATEAPP w={0}", m.WParam); break;
            case 0x0006: Trace.Write("msg WM_ACTIVATE w={0}", m.WParam); break;
            case 0x0018: Trace.Write("msg WM_SHOWWINDOW w={0}", m.WParam); break;
            case 0x007E: Trace.Write("msg WM_DISPLAYCHANGE w={0} l={1}", m.WParam, m.LParam); break;
            case 0x031E: Trace.Write("msg WM_DWMCOMPOSITIONCHANGED"); break;
            case 0x001A: Trace.Write("msg WM_SETTINGCHANGE w={0}", m.WParam); break;
        }
        base.WndProc(ref m);
    }

    // "#RRGGBB" -> Color.  Unparseable values fall back to the old neutral grey
    // so an older Rust build still gets a sane backdrop.
    static Color ParseColor(string value)
    {
        int rgb;
        if (value != null && value.Length == 7 && value[0] == '#' &&
            int.TryParse(value.Substring(1), System.Globalization.NumberStyles.HexNumber,
                System.Globalization.CultureInfo.InvariantCulture, out rgb))
            return Color.FromArgb((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
        return Color.FromArgb(0xAB, 0xAB, 0xAB);
    }

    void OnClientSizeChanged(object sender, EventArgs e)
    {
        CoverClient();
    }

    void CoverClient()
    {
        // Clamp the control between the client area and the remote desktop size,
        // anchored at the client origin:
        //  * client smaller than the desktop -> the control is smaller too, which
        //    is what makes the RDP ActiveX draw its scrollbars; inside it the
        //    picture stays 1:1 and top-left, so it is never scaled or centred.
        //  * client larger -> the control equals the desktop, so the ActiveX has
        //    no room to centre the picture in; BackColor fills the remainder.
        // A client-sized control instead centred the 1:1 picture and cropped both
        // sides (measured 35px bands above and below), which is why the clamp
        // replaces it.
        //
        // The two-pixel growth hides the 1px dark rim the ActiveX paints on its
        // own top/left edge; WinForms clips the overflow at the client area.  On
        // the clamped axis at most one pixel of the scrollbar's outer edge is
        // clipped, which keeps the thumb and arrows usable.
        Size client = ClientSize;
        int width = Math.Min(client.Width, _width) + 2;
        int height = Math.Min(client.Height, _height) + 2;
        _host.Bounds = new Rectangle(-1, -1, width, height);
    }

    // 位表以 IMsRdpClientAdvancedSettings::PerformanceFlags 为准：
    //   0x1/0x2/0x4/0x20 关壁纸、整窗拖动、菜单动画、光标阴影
    //   0x8/0x40 禁主题、禁光标设置；0x80/0x100 启用字体平滑、桌面合成
    // 合成、主题、壁纸与持久缓存保持旧版 C# 的默认体验，不按网络位置裁剪。
    // 这是视觉与兼容性约束，不是 UAC 或安全桌面卡顿的性能修复。
    const int SafeExperienceFlags = 0x2 | 0x4 | 0x20;
    const int DesktopCompositionFlag = 0x100;

    static void ApplyTrimmedGraphics(dynamic adv)
    {
        try
        {
            // 不设 DisableWallpaper：见 SafeExperienceFlags 的注释。
            adv.DisableFullWindowDrag = true;
            adv.DisableMenuAnims = true;
            adv.DisableCursorShadow = true;
            adv.EnableFontSmoothing = false;
            // 显式置 true，避免某些控件版本按属性回算掩码时把 0x100 抹掉。
            adv.EnableDesktopComposition = true;
        }
        catch
        {
            // 个别控件版本缺少其中某项；PerformanceFlags 仍能表达同一组开关。
        }
        try
        {
            adv.PerformanceFlags = SafeExperienceFlags | DesktopCompositionFlag;
        }
        catch
        {
            // 拿不到掩码就保持控件默认值：这只是省内存的优化，不能影响连接。
        }
    }

    void OnShown(object sender, EventArgs e)
    {
        Trace.Write("shown session={0}", _session);
        _timer.Start();
        Connect();
    }

    void Connect()
    {
        Trace.Write("connect-begin session={0}", _session);
        try
        {
            string password = Credential.Read("TERMSRV/" + _server);
            dynamic ocx = _host.Ocx;
            ocx.Server = _server;
            if (!string.IsNullOrEmpty(_user)) ocx.UserName = _user;
            ocx.DesktopWidth = _width;
            ocx.DesktopHeight = _height;
            ocx.ColorDepth = _color;

            dynamic adv = ocx.AdvancedSettings2;
            adv.RDPPort = _port;
            adv.ClearTextPassword = password ?? string.Empty;
            // SmartSizing stays off: with it on the ActiveX neither scaled the
            // picture (it kept its 1:1 size) nor drew scrollbars; it only
            // centred and cropped it.  Off, a control smaller than the desktop
            // gets scrollbars and the picture keeps the top-left origin.
            adv.SmartSizing = _smartSizing;
            // 持久位图缓存保持开启，对齐旧版 C#；不能按目标 IP 裁剪
            adv.BitmapPersistence = 1;
            adv.CachePersistenceActive = 1;
            if (_trimGraphics) ApplyTrimmedGraphics(adv);
            adv.GrabFocusOnConnect = false;
            adv.ContainerHandledFullScreen = 1;
            adv.RedirectPrinters = _redirectPrinters;
            adv.RedirectPorts = _redirectPorts;
            adv.RedirectSmartCards = _redirectSmartcards;
            adv.RedirectDrives = _redirectDrives;

            try { ocx.AdvancedSettings6.RedirectClipboard = _redirectClipboard; }
            catch { }
            try { ocx.AdvancedSettings5.AuthenticationLevel = 0; }
            catch { }
            try { ocx.AdvancedSettings7.EnableCredSspSupport = true; }
            catch { }
            try { ocx.AdvancedSettings6.AudioRedirectionMode = _audio; }
            catch { }
            try { ocx.AdvancedSettings8.AudioCaptureRedirectionMode = _audioCapture; }
            catch { }
            try { ocx.SecuredSettings2.KeyboardHookMode = _keyboard; }
            catch { }
            try { ocx.AdvancedSettings9.BandwidthDetection = _performance == 0; }
            catch { }
            try { if (_performance != 0) ocx.AdvancedSettings8.NetworkConnectionType = _performance; }
            catch { }

            // `IMsRdpExtendedSettings` 只有 v11 控件提供。降到 v10 时这一步必须容忍
            // 失败：v10 下拿不到它最多是多一个重定向提示框，不该把整条连接判死。
            try
            {
                IMsRdpExtendedSettings extended = (IMsRdpExtendedSettings)_host.Ocx;
                object version = (uint)1;
                extended.set_Property("RedirectionWarningDialogVersion", ref version);
            }
            catch (Exception ex)
            {
                if (IsDefaultControl(_control))
                {
                    Fail("无法关闭 Windows RDP 安全确认：" + ex.Message);
                    return;
                }
                Trace.Write("ext-settings-unavailable control={0} err={1}", _control, ex.Message);
            }

            ocx.Connect();
            Trace.Write("connect-invoked session={0}", _session);
        }
        catch (Exception ex)
        {
            Trace.Write("connect-threw session={0}: {1}", _session, ex.Message);
            Fail("启动远程桌面失败：" + ex.Message);
        }
    }

    void OnTick(object sender, EventArgs e)
    {
        if (_state == "failed") return;
        _tickCount++;
        Heartbeat();
        var comWatch = System.Diagnostics.Stopwatch.StartNew();
        try
        {
            dynamic ocx = _host.Ocx;
            // `Connected` 是 VARIANT_BOOL，但经 COM 类型库到这里可能变成 bool，
            // 也可能变成 Int16：实测同一台机器上冷启动（新建会话）停在 1 二十多秒，
            // 热连接不到 1 秒就报 2。只认 2 会把已经连上的会话判成 30 秒超时
            // （事件日志里明明已经有 1025/1401/1402），所以 1 与 2 都当已连接，
            // 0 才是未连接。
            object value = ocx.Connected;
            comWatch.Stop();
            int state = (value is bool) ? ((bool)value ? 1 : 0) : Convert.ToInt32(value);
            if (state != _lastState)
            {
                Trace.Write("connected-state {0} -> {1} com={2}ms", _lastState, state, comWatch.ElapsedMilliseconds);
                _lastState = state;
            }
            if (comWatch.ElapsedMilliseconds > 50)
                Trace.Write("WARN com-slow {0}ms", comWatch.ElapsedMilliseconds);
            if (state != 0)
            {
                _idleTicks = 0;
                if (_state != "connected")
                {
                    _state = "connected";
                    Text = Program.Title(_session, "connected", string.Empty);
                }
                return;
            }
            _idleTicks++;
            if (_state == "connected")
            {
                // 连续 8 拍（2 秒）为 0 才判断线。UAC / 锁屏 / Ctrl+Alt+End
                // 的桌面切换期间 ActiveX 会短暂不报连接态；一拍就 Fail 会让
                // Rust 侧关掉宿主，会话随之断开，画面长时间停在蓝底。
                if (_idleTicks >= 8) Fail("远程桌面连接已断开。");
                return;
            }
        }
        catch (Exception ex)
        {
            Trace.Write("WARN com-threw state={0} ex={1}", _state, ex.Message);
            // 桌面切换期间 COM 属性可能抛异常。已连接的会话绝不能被一次读取
            // 失败带进下面的「30 秒未建立」分支。
            if (_state == "connected") return;
        }

        // 启动超时只适用于还没连上的阶段。会话已建立后 _started 必然超过 30
        // 秒，漏掉这个判断会让任意一次 COM 异常直接把会话判成「未能建立」。
        if (_state != "connected" && (DateTime.UtcNow - _started).TotalSeconds > 30)
        {
            // 裸 AxHost 不生成事件包装，读不到 `OnDisconnected` 的断开码，
            // 所以这里说不出确切原因。但也不能说成「连接超时」：TLS/证书握手失败、
            // 会话被拒、口令错误都会走到这条分支，把它写成超时会把人引向网络问题
            // （2026-11-04 就是这样被一次证书握手失败带偏的）。确切错误码在
            // 系统事件日志 Microsoft-Windows-TerminalServices-RDPClient 里。
            Fail("远程桌面未能建立：30 秒内会话没有建立成功。确切错误码见系统事件日志 TerminalServices-RDPClient。");
        }
    }

    /// 2 秒一次心跳：只证明「本条消息循环还活着」。同一线程给自己发消息会
    /// 直接本地处理，测不出卡住；宿主 GUI 线程是否停摆由 Rust 侧 rdp-app.log
    /// 的跨进程探活来判断。
    void Heartbeat()
    {
        DateTime now = DateTime.UtcNow;
        if ((now - _lastHeartbeat).TotalSeconds < 2) return;
        _lastHeartbeat = now;
        Trace.Write("heartbeat ticks={0} state={1} connected={2} visible={3} idle={4}",
            _tickCount, _state, _lastState, _host.Visible, _idleTicks);
    }

    void Fail(string message)
    {
        _state = "failed";
        Text = Program.Title(_session, "failed", message);
        _timer.Stop();
        Trace.Write("FAIL session={0} msg={1}", _session, message);
    }
}

// 只有目标确实是本机/内网时才裁剪图形渲染。主机名按解析结果判断；解析不到
// 就按公网处理——宁可少省几 MB 内存，也不要把公网连接的画质顺手降掉。
static class LocalNetwork
{
    public static bool IsPrivateTarget(string server)
    {
        if (string.IsNullOrEmpty(server)) return false;

        IPAddress literal;
        if (IPAddress.TryParse(server, out literal))
            return IsPrivate(literal);

        try
        {
            IPAddress[] resolved = Dns.GetHostAddresses(server);
            if (resolved.Length == 0) return false;
            // 一个名字可能同时解析到公网与内网地址（双栈或 CDN）。保守起见要求
            // 全部是内网，否则对公网的那条链路也会被裁掉画质。
            for (int i = 0; i < resolved.Length; i++)
            {
                if (!IsPrivate(resolved[i])) return false;
            }
            return true;
        }
        catch
        {
            return false;
        }
    }

    static bool IsPrivate(IPAddress address)
    {
        if (address.AddressFamily == AddressFamily.InterNetwork)
        {
            byte[] b = address.GetAddressBytes();
            // 127/8 回环，以及 RFC1918 的 10/8、172.16/12、192.168/16。
            return b[0] == 127
                || b[0] == 10
                || (b[0] == 172 && b[1] >= 16 && b[1] <= 31)
                || (b[0] == 192 && b[1] == 168);
        }
        if (address.AddressFamily == AddressFamily.InterNetworkV6)
        {
            if (address.Equals(IPAddress.IPv6Loopback)) return true;
            byte[] b = address.GetAddressBytes();
            // fe80::/10 链路本地与 fc00::/7 唯一本地地址同样不出公网。
            return (b[0] == 0xFE && (b[1] & 0xC0) == 0x80)
                || (b[0] & 0xFE) == 0xFC;
        }
        return false;
    }
}

sealed class AxRdpHost : AxHost
{
    // CLSID 由调用方给：默认 "version 11"（MsRdpClient10、RDP 10 + AVC），
    // 对照模式可以降到 "version 10"（旧版 C# 的 AxMsRdpClient9、RDP 8.1）。
    public AxRdpHost(string clsid) : base(clsid) { }

    public object Ocx { get { return GetOcx(); } }
}

[ComImport, Guid("302D8188-0052-4807-806A-362B628F9AC5"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMsRdpExtendedSettings
{
    void set_Property([MarshalAs(UnmanagedType.BStr)] string name, [MarshalAs(UnmanagedType.Struct)] ref object value);
    [return: MarshalAs(UnmanagedType.Struct)]
    object get_Property([MarshalAs(UnmanagedType.BStr)] string name);
}

static class Credential
{
    [StructLayout(LayoutKind.Sequential)]
    struct CREDENTIAL
    {
        public uint Flags;
        public uint Type;
        public IntPtr TargetName;
        public IntPtr Comment;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastWritten;
        public uint CredentialBlobSize;
        public IntPtr CredentialBlob;
        public uint Persist;
        public uint AttributeCount;
        public IntPtr Attributes;
        public IntPtr TargetAlias;
        public IntPtr UserName;
    }

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredReadW(string target, int type, int flags, out IntPtr credential);
    [DllImport("advapi32.dll")]
    static extern void CredFree(IntPtr credential);

    public static string Read(string target)
    {
        IntPtr ptr;
        if (!CredReadW(target, 1, 0, out ptr)) return string.Empty;
        try
        {
            CREDENTIAL cred = (CREDENTIAL)Marshal.PtrToStructure(ptr, typeof(CREDENTIAL));
            if (cred.CredentialBlob == IntPtr.Zero || cred.CredentialBlobSize < 2) return string.Empty;
            return Marshal.PtrToStringUni(cred.CredentialBlob, (int)(cred.CredentialBlobSize / 2));
        }
        finally
        {
            CredFree(ptr);
        }
    }
}
