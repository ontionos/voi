# Prove that Voi's notification-area icon is there, from the user's side.
#
#   powershell -ExecutionPolicy Bypass -File scripts\windows-tray-check.ps1 [-Start] [-RestartExplorer]
#
# Run it as the user who is logged on at the PC, from their desktop or over
# SSH: when this shell is not in that user's desktop session (SSH is not), the
# checks run there through a one-shot scheduled task, windowless, and their
# report is printed here. No VBS, no wscript.
#
# What it asserts, in order:
#   1. Voi is running (with -Start: the running Voi is stopped and $Voi is
#      started in the desktop session first, the way a login would start it).
#   2. The shell holds an icon owned by that Voi process - read from the
#      notification area's own toolbars (the visible part and the overflow
#      behind ^), so it is the shell's record, not Voi's word for it.
#   3. It is on screen as a user would find it: in the visible tray, or after
#      opening the ^ chevron, as a button whose name (its tooltip) begins with
#      "Voi", read through the accessibility layer screen readers use (MSAA;
#      UI Automation finds the chevron). A screenshot of
#      what was seen goes into the report folder.
#   4. Right-clicking that button opens Voi's menu, with Settings and Quit Voi
#      in it; Esc closes it again and the pointer is put back where it was.
#   5. Taken out of the notification area from outside (NIM_DELETE with its
#      window and id, which is how the shell dropping it looks to Voi), the
#      icon is put back by Voi itself within 20 seconds.
#   6. With -RestartExplorer: explorer.exe is ended once (Windows starts it
#      again by itself, or the script does), and the icon must come back on its
#      own - the TaskbarCreated case: Explorer crashing, restarting, or the
#      taskbar being rebuilt. Open File Explorer windows close when it does.
#
# Steps 3 and 4 move the pointer and open the overflow and the menu for a
# second or so, so they wait until nobody has touched the PC for -IdleSeconds.
# Exit code 0 and a last line "RESULT: PASS" when everything held.
param(
    [string]$Voi = (Join-Path $env:LOCALAPPDATA 'Voi\Voi.exe'),
    [switch]$Start,
    [switch]$RestartExplorer,
    [int]$IdleSeconds = 20,
    [string]$Out = (Join-Path ([IO.Path]::GetTempPath()) 'voi-tray-check'),
    [switch]$InSession
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
New-Item -ItemType Directory -Force $Out | Out-Null
$Report = Join-Path $Out 'report.txt'

# --- Outside the desktop session: hand the work to a scheduled task there. ---

# The session this user's desktop (their explorer.exe) runs in.
function Desktop-Session {
    foreach ($p in Get-CimInstance Win32_Process -Filter "Name = 'explorer.exe'") {
        $owner = Invoke-CimMethod -InputObject $p -MethodName GetOwner
        if ($owner.User -eq $env:USERNAME) { return $p.SessionId }
    }
    $null
}

function Run-InDesktop([string]$Name, [string]$Exe, [string]$Arguments, [int]$WaitSeconds) {
    $action = New-ScheduledTaskAction -Execute $Exe -Argument $Arguments -WorkingDirectory $Out
    $principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 10)
    Register-ScheduledTask -TaskName $Name -Action $action -Principal $principal -Settings $settings -Force | Out-Null
    try {
        Start-ScheduledTask -TaskName $Name
        Start-Sleep -Milliseconds 800
        $t0 = Get-Date
        while ($WaitSeconds -gt 0 -and (Get-ScheduledTask -TaskName $Name).State -eq 'Running' -and ((Get-Date) - $t0).TotalSeconds -lt $WaitSeconds) {
            Start-Sleep -Milliseconds 500
        }
    } finally {
        Unregister-ScheduledTask -TaskName $Name -Confirm:$false
    }
}

if (-not $InSession) {
    $session = Desktop-Session
    if ($null -eq $session) { Write-Host "RESULT: FAIL - $env:USERNAME has no desktop session on this PC"; exit 1 }
    $Voi = (Resolve-Path $Voi).Path
    if ($Start) {
        # One Voi at a time (it holds a mutex): the installed one or a fresh build.
        Get-Process Voi, voi-app -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Seconds 1
        # The task only starts Voi and ends; Voi keeps running in the desktop.
        $ps = "powershell.exe -NoProfile -NonInteractive -Command Start-Process -FilePath '$Voi'"
        Run-InDesktop 'VoiTrayCheckStart' 'conhost.exe' "--headless $ps" 30
        Write-Host "started $Voi in session $session"
    }
    $here = (Get-Process -Id $PID).SessionId
    Remove-Item $Report -ErrorAction SilentlyContinue
    $flags = "-InSession -Voi `"$Voi`" -Out `"$Out`" -IdleSeconds $IdleSeconds"
    if ($RestartExplorer) { $flags += ' -RestartExplorer' }
    if ($here -eq $session) {
        & $PSCommandPath -InSession -Voi $Voi -Out $Out -IdleSeconds $IdleSeconds -RestartExplorer:$RestartExplorer
    } else {
        # conhost --headless: PowerShell with no console window at all.
        $ps = "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$PSCommandPath`" $flags"
        Run-InDesktop 'VoiTrayCheck' 'conhost.exe' "--headless $ps" 600
    }
    if (-not (Test-Path $Report)) { Write-Host "RESULT: FAIL - the check left no report in $Out"; exit 1 }
    $lines = Get-Content $Report
    $lines | Write-Host
    if ($lines[-1] -eq 'RESULT: PASS') { exit 0 } else { exit 1 }
}

# --- Inside the desktop session. ---

$failures = New-Object System.Collections.Generic.List[string]
function Log([string]$line) { Add-Content -Path $Report -Value $line -Encoding UTF8 }
function Fail([string]$line) { $failures.Add($line); Log "FAIL: $line" }

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing, Accessibility
Add-Type -ReferencedAssemblies Accessibility @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class Shell {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindowW(string c, string n);
    [DllImport("user32.dll")] static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr p, EnumProc f, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint a, bool i, uint pid);
    [DllImport("kernel32.dll")] static extern IntPtr VirtualAllocEx(IntPtr p, IntPtr a, UIntPtr s, uint t, uint pr);
    [DllImport("kernel32.dll")] static extern bool VirtualFreeEx(IntPtr p, IntPtr a, UIntPtr s, uint t);
    [DllImport("kernel32.dll")] static extern bool ReadProcessMemory(IntPtr p, IntPtr a, byte[] b, IntPtr n, out IntPtr r);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);

    [StructLayout(LayoutKind.Sequential)] struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
    [DllImport("user32.dll")] static extern bool GetLastInputInfo(ref LASTINPUTINFO l);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] static extern void mouse_event(uint f, int x, int y, uint d, UIntPtr e);
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint f, UIntPtr e);
    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();

    public static void DpiAware() { SetProcessDPIAware(); }

    public static string ClassOf(IntPtr h) { var s = new StringBuilder(256); GetClassNameW(h, s, 256); return s.ToString(); }
    public static uint PidOf(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }

    public static uint IdleMs() {
        var l = new LASTINPUTINFO(); l.cbSize = 8; GetLastInputInfo(ref l);
        return (uint)Environment.TickCount - l.dwTime;
    }

    // The notification area's toolbars: "taskbar" for the always-visible part,
    // "overflow" for the window behind the ^ chevron. Absent on Windows builds
    // that draw the whole tray in XAML; the caller then relies on UI Automation.
    public static List<KeyValuePair<string, IntPtr>> Toolbars() {
        var found = new List<KeyValuePair<string, IntPtr>>();
        foreach (var top in new[] { new[] { "Shell_TrayWnd", "taskbar" }, new[] { "NotifyIconOverflowWindow", "overflow" } }) {
            IntPtr w = FindWindowW(top[0], null);
            if (w == IntPtr.Zero) continue;
            string where = top[1];
            EnumChildWindows(w, (h, x) => { if (ClassOf(h) == "ToolbarWindow32") found.Add(new KeyValuePair<string, IntPtr>(where, h)); return true; }, IntPtr.Zero);
        }
        return found;
    }

    // Every icon the shell holds in one toolbar, as "owner pid, owner window,
    // icon id": TB_GETBUTTON into Explorer's memory, then the TRAYDATA the
    // button's dwData points at (owner HWND first, then uID).
    public static List<uint[]> Icons(IntPtr toolbar) {
        var icons = new List<uint[]>();
        uint explorer = PidOf(toolbar);
        IntPtr proc = OpenProcess(0x0008 | 0x0010 | 0x0020 | 0x0400, false, explorer);
        if (proc == IntPtr.Zero) return icons;
        IntPtr mem = VirtualAllocEx(proc, IntPtr.Zero, (UIntPtr)64, 0x3000, 0x04);
        try {
            int n = (int)SendMessageW(toolbar, 0x0418, IntPtr.Zero, IntPtr.Zero); // TB_BUTTONCOUNT
            for (int i = 0; i < n; i++) {
                SendMessageW(toolbar, 0x0417, (IntPtr)i, mem); // TB_GETBUTTON
                byte[] button = new byte[32]; IntPtr read;
                if (!ReadProcessMemory(proc, mem, button, (IntPtr)32, out read)) continue;
                long data = BitConverter.ToInt64(button, 16);
                byte[] tray = new byte[16];
                if (!ReadProcessMemory(proc, (IntPtr)data, tray, (IntPtr)16, out read)) continue;
                IntPtr owner = (IntPtr)BitConverter.ToInt64(tray, 0);
                icons.Add(new uint[] { PidOf(owner), (uint)owner.ToInt64(), BitConverter.ToUInt32(tray, 8) });
            }
        } finally {
            VirtualFreeEx(proc, mem, UIntPtr.Zero, 0x8000);
            CloseHandle(proc);
        }
        return icons;
    }

    // What the accessibility layer (MSAA, which Narrator reads) says each
    // button of each tray toolbar is: area, name (the icon's tooltip), and
    // where it is on screen. UI Automation's toolbar proxy returns no
    // children for these toolbars on Windows 11 21H2; MSAA does.
    [DllImport("oleacc.dll")] static extern int AccessibleObjectFromWindow(IntPtr h, uint id, ref Guid iid, [MarshalAs(UnmanagedType.IUnknown)] out object o);
    public static List<object[]> Buttons() {
        var found = new List<object[]>();
        foreach (var tb in Toolbars()) {
            Guid iid = new Guid("618736E0-3C3D-11CF-810C-00AA00389B71");
            object acc;
            if (AccessibleObjectFromWindow(tb.Value, 0xFFFFFFFC, ref iid, out acc) != 0) continue; // OBJID_CLIENT
            var a = (Accessibility.IAccessible)acc;
            for (int i = 1; i <= a.accChildCount; i++) {
                int x = 0, y = 0, w = 0, h = 0;
                try { a.accLocation(out x, out y, out w, out h, i); } catch (Exception) { }
                found.Add(new object[] { tb.Key, (a.get_accName(i) ?? "").Split('\n')[0], x, y, w, h });
            }
        }
        return found;
    }

    // The entries of a popup menu on screen, through MSAA as well.
    public static List<string> MenuItems(IntPtr menu) {
        var items = new List<string>();
        Guid iid = new Guid("618736E0-3C3D-11CF-810C-00AA00389B71");
        object acc;
        if (AccessibleObjectFromWindow(menu, 0xFFFFFFFC, ref iid, out acc) != 0) return items;
        var a = (Accessibility.IAccessible)acc;
        for (int i = 1; i <= a.accChildCount; i++) {
            string name = a.get_accName(i);
            if (!String.IsNullOrEmpty(name)) items.Add(name.Split('\t')[0]);
        }
        return items;
    }

    // Take an icon out of the notification area the way the shell losing it
    // would look to its owner: NIM_DELETE with the owner's window and id.
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct NOTIFYICONDATAW {
        public uint cbSize; public IntPtr hWnd; public uint uID; public uint uFlags; public uint uCallbackMessage; public IntPtr hIcon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string szTip;
        public uint dwState; public uint dwStateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string szInfo;
        public uint uVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string szInfoTitle;
        public uint dwInfoFlags; public Guid guidItem; public IntPtr hBalloonIcon;
    }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)] static extern bool Shell_NotifyIconW(uint m, ref NOTIFYICONDATAW d);
    public static bool Remove(long owner, uint id) {
        var d = new NOTIFYICONDATAW();
        d.cbSize = (uint)Marshal.SizeOf(typeof(NOTIFYICONDATAW));
        d.hWnd = (IntPtr)owner; d.uID = id;
        return Shell_NotifyIconW(2, ref d); // NIM_DELETE
    }

    // The popup menu (class #32768) a process has on screen, if any.
    public static IntPtr MenuOf(uint pid) {
        IntPtr menu = IntPtr.Zero;
        EnumWindows((h, x) => { if (ClassOf(h) == "#32768" && IsWindowVisible(h) && PidOf(h) == pid) { menu = h; return false; } return true; }, IntPtr.Zero);
        return menu;
    }

    public static void RightClick(int x, int y) {
        SetCursorPos(x, y);
        System.Threading.Thread.Sleep(80);
        mouse_event(0x0008, 0, 0, 0, UIntPtr.Zero); // right down
        mouse_event(0x0010, 0, 0, 0, UIntPtr.Zero); // right up
    }
    public static void LeftClick(int x, int y) {
        SetCursorPos(x, y);
        System.Threading.Thread.Sleep(80);
        mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero); // left down
        mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero); // left up
    }
    public static void MoveTo(int x, int y) { SetCursorPos(x, y); }
    public static void Escape() {
        keybd_event(0x1B, 0, 0, UIntPtr.Zero);
        keybd_event(0x1B, 0, 2, UIntPtr.Zero);
    }
}
"@
[Shell]::DpiAware()
$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]
$All = [System.Windows.Automation.Condition]::TrueCondition
function By($property, $value) { New-Object System.Windows.Automation.PropertyCondition($property, $value) }

function Voi-Process {
    Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Voi } | Select-Object -First 1
}

# Where the shell has Voi's icon: "taskbar", "overflow", or $null.
function Shell-Record([int]$voiPid) {
    foreach ($tb in [Shell]::Toolbars()) {
        foreach ($icon in [Shell]::Icons($tb.Value)) {
            if ($icon[0] -eq $voiPid) {
                return [pscustomobject]@{ Where = $tb.Key; Owner = ('0x{0:X}' -f $icon[1]); Id = $icon[2] }
            }
        }
    }
    $null
}

function Wait-ShellRecord([int]$voiPid, [int]$seconds) {
    $t0 = Get-Date
    do {
        $record = Shell-Record $voiPid
        if ($record) { return $record }
        Start-Sleep -Milliseconds 500
    } while (((Get-Date) - $t0).TotalSeconds -lt $seconds)
    $null
}

function Shot($rect, [string]$name) {
    if ($rect.IsEmpty -or $rect.Width -lt 1) { return }
    $pad = 8
    $bmp = New-Object System.Drawing.Bitmap ([int]$rect.Width + 2 * $pad), ([int]$rect.Height + 2 * $pad)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen([int]$rect.X - $pad, [int]$rect.Y - $pad, 0, 0, $bmp.Size)
    $g.Dispose()
    $path = Join-Path $Out $name
    $bmp.Save($path)
    $bmp.Dispose()
    Log "  screenshot: $path"
}

function Wait-Idle {
    $t0 = Get-Date
    while ([Shell]::IdleMs() -lt $IdleSeconds * 1000) {
        if (((Get-Date) - $t0).TotalMinutes -gt 5) { return $false }
        Start-Sleep -Seconds 2
    }
    $true
}

function Taskbar { $AE::RootElement.FindFirst($TS::Children, (By $AE::ClassNameProperty 'Shell_TrayWnd')) }
function Overflow { $AE::RootElement.FindFirst($TS::Children, (By $AE::ClassNameProperty 'NotifyIconOverflowWindow')) }

# Voi's button as the accessibility layer has it, in the visible tray or the
# overflow: @{ Area; Name; X; Y; W; H }, or $null.
function Voi-Button {
    foreach ($b in [Shell]::Buttons()) {
        if ($b[1] -match '^Voi\b') { return [pscustomobject]@{ Area = $b[0]; Name = $b[1]; X = $b[2]; Y = $b[3]; W = $b[4]; H = $b[5] } }
    }
    $null
}

function Chevron {
    $bar = Taskbar
    if (-not $bar) { return $null }
    $notify = $bar.FindFirst($TS::Descendants, (By $AE::ClassNameProperty 'TrayNotifyWnd'))
    if (-not $notify) { return $null }
    $notify.FindFirst($TS::Children, (By $AE::ClassNameProperty 'Button'))
}

# Click ^ as a user would (on this Windows it offers no Invoke pattern), and
# put the pointer back where it was.
function Toggle-Overflow {
    $chevron = Chevron
    if (-not $chevron) { return $false }
    $rect = $chevron.Current.BoundingRectangle
    $p = New-Object Shell+POINT
    [Shell]::GetCursorPos([ref]$p) | Out-Null
    [Shell]::LeftClick([int]($rect.X + $rect.Width / 2), [int]($rect.Y + $rect.Height / 2))
    [Shell]::MoveTo($p.X, $p.Y)
    $true
}

# As a user would look: the visible tray first, then behind ^. Leaves the
# overflow open when that is where it was found; returns the button.
function Find-AsUser {
    $button = Voi-Button
    if (-not $button) { Log "  no tray button is named Voi"; return $null }
    if ($button.Area -eq 'taskbar') {
        Log "  '$($button.Name)' on the taskbar, visible without opening anything"
        $tray = (Taskbar).FindFirst($TS::Descendants, (By $AE::ClassNameProperty 'TrayNotifyWnd'))
        Shot $tray.Current.BoundingRectangle 'taskbar.png'
        return $button
    }
    if (-not (Toggle-Overflow)) { Log "  no ^ chevron found"; return $null }
    $t0 = Get-Date
    do {
        Start-Sleep -Milliseconds 300
        $flyout = Overflow
    } while (-not $flyout -and ((Get-Date) - $t0).TotalSeconds -lt 3)
    if (-not $flyout) { Log "  the overflow did not open"; return $null }
    Start-Sleep -Milliseconds 300
    $names = @([Shell]::Buttons() | Where-Object { $_[0] -eq 'overflow' } | ForEach-Object { $_[1] })
    Log ("  overflow, opened: {0} icons: {1}" -f $names.Count, ($names -join ' | '))
    Shot $flyout.Current.BoundingRectangle 'overflow.png'
    $button = Voi-Button
    if ($button -and $button.W -gt 0) {
        Log "  '$($button.Name)' behind ^, at $($button.X),$($button.Y) ($($button.W)x$($button.H))"
        return $button
    }
    Log "  Voi's button has no place on screen in the open overflow"
    $null
}

function Close-Overflow {
    if (Overflow) { Toggle-Overflow | Out-Null; Start-Sleep -Milliseconds 300 }
    if (Overflow) { [Shell]::Escape() }
}

function Check-Menu($button, [int]$voiPid) {
    $x = [int]($button.X + $button.W / 2); $y = [int]($button.Y + $button.H / 2)
    $p = New-Object Shell+POINT
    [Shell]::GetCursorPos([ref]$p) | Out-Null
    try {
        [Shell]::RightClick($x, $y)
        $menu = [IntPtr]::Zero
        $t0 = Get-Date
        while ($menu -eq [IntPtr]::Zero -and ((Get-Date) - $t0).TotalSeconds -lt 3) {
            Start-Sleep -Milliseconds 150
            $menu = [Shell]::MenuOf($voiPid)
        }
        if ($menu -eq [IntPtr]::Zero) { Fail "right-click on the icon opened no Voi menu"; return }
        Start-Sleep -Milliseconds 200
        $items = @([Shell]::MenuItems($menu))
        Log ("  right-click menu: {0}" -f ($items -join ' | '))
        Shot $AE::FromHandle($menu).Current.BoundingRectangle 'menu.png'
        foreach ($want in 'Settings', 'Quit Voi') {
            if (-not ($items | Where-Object { $_ -like "$want*" })) { Fail "the menu has no '$want'" }
        }
        [Shell]::Escape()
        Start-Sleep -Milliseconds 300
        if ([Shell]::MenuOf($voiPid) -ne [IntPtr]::Zero) { [Shell]::Escape() }
    } finally {
        [Shell]::MoveTo($p.X, $p.Y)
    }
}

Log ("Voi tray check, {0:yyyy-MM-dd HH:mm:ss}, {1} on Windows {2}, session {3}" -f (Get-Date), $env:USERNAME, [Environment]::OSVersion.Version, (Get-Process -Id $PID).SessionId)
Log "Voi: $Voi"

try {
    # 1. Running.
    $t0 = Get-Date
    while (-not ($proc = Voi-Process) -and ((Get-Date) - $t0).TotalSeconds -lt 20) { Start-Sleep -Milliseconds 500 }
    if (-not $proc) {
        Fail "Voi is not running from $Voi"
    } else {
        Log "1. Voi is running: pid $($proc.Id), started $($proc.StartTime)"

        # 2. The shell's own record.
        $record = Wait-ShellRecord $proc.Id 15
        if ($record) {
            Log "2. the shell holds Voi's icon: $($record.Where), owner window $($record.Owner), id $($record.Id)"
        } elseif (@([Shell]::Toolbars()).Count -eq 0) {
            Log "2. this Windows draws the tray without toolbars; left to UI Automation"
        } else {
            Fail "the shell holds no icon owned by Voi (pid $($proc.Id))"
        }

        # 3 and 4. As a user sees it, and its menu.
        if (-not (Wait-Idle)) {
            Fail "the PC was in use for five minutes; the visible checks were not run"
        } else {
            $seen = Find-AsUser
            try {
                if (-not $seen) {
                    Fail "no Voi icon on screen, in the tray or behind ^"
                } else {
                    Log "3. a user sees the icon: yes"
                    Check-Menu $seen $proc.Id
                    if ($failures.Count -eq 0) { Log "4. right-click opens Voi's menu: yes" }
                }
            } finally {
                Close-Overflow
            }
        }

        # 5. Dropped from outside, put back by Voi.
        $record = Shell-Record $proc.Id
        if ($record) {
            $removed = [Shell]::Remove([Convert]::ToInt64($record.Owner.Substring(2), 16), $record.Id)
            Start-Sleep -Milliseconds 500
            if (-not $removed -or (Shell-Record $proc.Id)) {
                Fail "could not take the icon out to see it come back"
            } else {
                $t0 = Get-Date
                $back = Wait-ShellRecord $proc.Id 20
                if ($back) {
                    Log ("5. taken out of the notification area, back after {0:N1} s: {1}, owner window {2}, id {3}" -f ((Get-Date) - $t0).TotalSeconds, $back.Where, $back.Owner, $back.Id)
                } else {
                    Fail "the icon was taken out and Voi did not put it back within 20 s"
                }
            }
        }

        # 6. Explorer restarted under it.
        if ($RestartExplorer -and -not (Wait-Idle)) {
            Fail "the PC was in use for five minutes; Explorer was not restarted"
        } elseif ($RestartExplorer) {
            $before = (Get-Process explorer -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $proc.SessionId }).Id
            Log "6. ending explorer.exe (pid $($before -join ', '))"
            Get-Process explorer -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $proc.SessionId } | Stop-Process -Force
            $t0 = Get-Date
            while (-not [Shell]::Toolbars() -and ((Get-Date) - $t0).TotalSeconds -lt 15) { Start-Sleep -Milliseconds 500 }
            if (-not (Get-Process explorer -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $proc.SessionId })) {
                Log "  Windows did not restart it; starting explorer.exe"
                Start-Process explorer.exe
            }
            $t0 = Get-Date
            while (-not (Get-Process explorer -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $proc.SessionId }) -and ((Get-Date) - $t0).TotalSeconds -lt 15) { Start-Sleep -Milliseconds 500 }
            Start-Sleep -Seconds 5
            $alive = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
            $record = Wait-ShellRecord $proc.Id 20
            if (-not $alive) {
                Fail "Voi did not survive Explorer restarting"
            } elseif ($record) {
                Log "  icon back after the restart: $($record.Where), owner window $($record.Owner), id $($record.Id), same Voi pid $($proc.Id)"
            } else {
                Fail "the icon did not come back after Explorer restarted"
            }
        }
    }
} catch {
    Fail ("the check itself broke at line {0}: {1}" -f $_.InvocationInfo.ScriptLineNumber, $_)
}

if ($failures.Count -eq 0) { Log 'RESULT: PASS' } else { Log ("RESULT: FAIL ({0})" -f $failures.Count) }
