param(
    [Parameter(Mandatory = $true)][string]$ExePath,
    [Parameter(Mandatory = $true)][string]$ResultPath
)

$ErrorActionPreference = 'Stop'

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class CapturAcceptanceNative {
    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int X; public int Y; }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

    public delegate bool EnumWindowsProc(IntPtr window, IntPtr parameter);

    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT point);
    [DllImport("user32.dll")] public static extern bool GetPhysicalCursorPos(out POINT point);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out RECT rect);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr window, IntPtr insertAfter, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] public static extern uint GetClipboardSequenceNumber();
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr parameter);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr window, StringBuilder text, int maximum);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr window, StringBuilder text, int maximum);

    public static IntPtr FindWindowForProcess(uint processId) {
        IntPtr result = IntPtr.Zero;
        long largestArea = -1;
        EnumWindows(delegate(IntPtr window, IntPtr parameter) {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == processId) {
                RECT rect;
                GetWindowRect(window, out rect);
                long area = Math.Max(0, rect.Right - rect.Left) * (long)Math.Max(0, rect.Bottom - rect.Top);
                if (area > largestArea) {
                    largestArea = area;
                    result = window;
                }
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }

    public static string DescribeWindows(uint processId) {
        StringBuilder output = new StringBuilder();
        EnumWindows(delegate(IntPtr window, IntPtr parameter) {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == processId) {
                RECT rect;
                GetWindowRect(window, out rect);
                StringBuilder title = new StringBuilder(256);
                StringBuilder className = new StringBuilder(256);
                GetWindowText(window, title, title.Capacity);
                GetClassName(window, className, className.Capacity);
                output.AppendFormat("0x{0:x} visible={1} rect={2},{3} {4}x{5} class={6} title={7} | ",
                    window.ToInt64(), IsWindowVisible(window), rect.Left, rect.Top,
                    rect.Right - rect.Left, rect.Bottom - rect.Top, className, title);
            }
            return true;
        }, IntPtr.Zero);
        return output.ToString();
    }
}
'@

[void][CapturAcceptanceNative]::SetProcessDpiAwarenessContext([IntPtr](-4))
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

function Send-Chord([byte]$key) {
    [CapturAcceptanceNative]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
    [CapturAcceptanceNative]::keybd_event(0x10, 0, 0, [UIntPtr]::Zero)
    [CapturAcceptanceNative]::keybd_event($key, 0, 0, [UIntPtr]::Zero)
    [CapturAcceptanceNative]::keybd_event($key, 0, 2, [UIntPtr]::Zero)
    [CapturAcceptanceNative]::keybd_event(0x10, 0, 2, [UIntPtr]::Zero)
    [CapturAcceptanceNative]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
}

function Send-Key([byte]$key) {
    [CapturAcceptanceNative]::keybd_event($key, 0, 0, [UIntPtr]::Zero)
    [CapturAcceptanceNative]::keybd_event($key, 0, 2, [UIntPtr]::Zero)
}

function Get-CapturWindow {
    if ($null -eq $script:capturProcess) { return [IntPtr]::Zero }
    return [CapturAcceptanceNative]::FindWindowForProcess([uint32]$script:capturProcess.Id)
}

function Wait-WindowVisible([bool]$visible, [int]$timeoutMs) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.ElapsedMilliseconds -lt $timeoutMs) {
        $window = Get-CapturWindow
        $isVisible = $window -ne [IntPtr]::Zero -and [CapturAcceptanceNative]::IsWindowVisible($window)
        if ($isVisible -eq $visible) { return [int]$timer.ElapsedMilliseconds }
        Start-Sleep -Milliseconds 5
    }
    throw "Captur window visibility did not become $visible within $timeoutMs ms"
}

function Wait-ClipboardImage([uint32]$before, [int]$timeoutMs) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.ElapsedMilliseconds -lt $timeoutMs) {
        if ([CapturAcceptanceNative]::GetClipboardSequenceNumber() -ne $before) {
            try {
                if ([Windows.Forms.Clipboard]::ContainsImage()) {
                    $image = [Windows.Forms.Clipboard]::GetImage()
                    if ($null -ne $image) {
                        $result = [ordered]@{
                            elapsed_ms = [int]$timer.ElapsedMilliseconds
                            width = $image.Width
                            height = $image.Height
                        }
                        $image.Dispose()
                        return $result
                    }
                }
            } catch [System.Runtime.InteropServices.ExternalException] {}
        }
        Start-Sleep -Milliseconds 2
    }
    throw "Clipboard did not receive a bitmap within $timeoutMs ms"
}

function Copy-ClipboardBackup {
    $copy = New-Object Windows.Forms.DataObject
    try {
        $source = [Windows.Forms.Clipboard]::GetDataObject()
        if ($null -eq $source) { return $copy }
        foreach ($format in $source.GetFormats($false)) {
            try {
                $data = $source.GetData($format, $false)
                if ($data -is [Drawing.Image]) { $data = $data.Clone() }
                elseif ($data -is [byte[]]) { $data = $data.Clone() }
                $copy.SetData($format, $false, $data)
            } catch {}
        }
    } catch {}
    return $copy
}

$results = [ordered]@{}
$backup = Copy-ClipboardBackup
$backupFormats = @($backup.GetFormats($false))
[CapturAcceptanceNative+POINT]$originalCursor = New-Object CapturAcceptanceNative+POINT
[void][CapturAcceptanceNative]::GetCursorPos([ref]$originalCursor)
$script:capturProcess = $null
$targetForm = $null

try {
    $startup = [Diagnostics.Stopwatch]::StartNew()
    $script:capturProcess = Start-Process -FilePath $ExePath -ArgumentList '--background' -PassThru
    if ($null -eq $script:capturProcess) { throw 'Start-Process returned no process' }

    while ((Get-CapturWindow) -eq [IntPtr]::Zero -and $startup.ElapsedMilliseconds -lt 15000) {
        if ($script:capturProcess.HasExited) {
            throw "Captur exited during startup with code $($script:capturProcess.ExitCode)"
        }
        Start-Sleep -Milliseconds 10
    }
    if ((Get-CapturWindow) -eq [IntPtr]::Zero) { throw 'Captur did not create a top-level window' }
    $results.startup_to_resident_ms = [int]$startup.ElapsedMilliseconds
    [void](Wait-WindowVisible $false 2000)
    Start-Sleep -Milliseconds 1000

    $virtual = [Windows.Forms.SystemInformation]::VirtualScreen
    $primary = [Windows.Forms.Screen]::PrimaryScreen.Bounds
    $results.display = [ordered]@{
        monitor_count = [Windows.Forms.Screen]::AllScreens.Count
        virtual = "$($virtual.Left),$($virtual.Top) $($virtual.Width)x$($virtual.Height)"
        primary = "$($primary.Left),$($primary.Top) $($primary.Width)x$($primary.Height)"
    }

    $hotkeyTimer = [Diagnostics.Stopwatch]::StartNew()
    Send-Chord 0x34
    [void](Wait-WindowVisible $true 15000)
    $overlayWidth = 0
    $overlayHeight = 0
    [CapturAcceptanceNative+RECT]$overlayRect = New-Object CapturAcceptanceNative+RECT
    while ($hotkeyTimer.ElapsedMilliseconds -lt 15000) {
        $window = Get-CapturWindow
        [void][CapturAcceptanceNative]::GetWindowRect($window, [ref]$overlayRect)
        $overlayWidth = $overlayRect.Right - $overlayRect.Left
        $overlayHeight = $overlayRect.Bottom - $overlayRect.Top
        if ($overlayWidth -ge ($virtual.Width - 4) -and $overlayHeight -ge ($virtual.Height - 4)) {
            break
        }
        Start-Sleep -Milliseconds 2
    }
    $results.region_hotkey_to_visible_ms = [int]$hotkeyTimer.ElapsedMilliseconds
    $results.overlay_bounds = "$($overlayRect.Left),$($overlayRect.Top) ${overlayWidth}x${overlayHeight}"
    $results.process_windows = [CapturAcceptanceNative]::DescribeWindows([uint32]$script:capturProcess.Id)
    if ($overlayWidth -lt ($virtual.Width - 4) -or $overlayHeight -lt ($virtual.Height - 4)) {
        [void][CapturAcceptanceNative]::SetWindowPos(
            $window,
            [IntPtr](-1),
            $virtual.Left,
            $virtual.Top,
            $virtual.Width,
            $virtual.Height,
            0x0040
        )
        Start-Sleep -Milliseconds 100
        [void][CapturAcceptanceNative]::GetWindowRect($window, [ref]$overlayRect)
        $results.external_setwindowpos_after_100ms = "$($overlayRect.Left),$($overlayRect.Top) $(($overlayRect.Right - $overlayRect.Left))x$(($overlayRect.Bottom - $overlayRect.Top))"
        throw "Overlay did not cover virtual desktop: ${overlayWidth}x${overlayHeight} versus $($virtual.Width)x$($virtual.Height)"
    }

    $startX = $primary.Left + [Math]::Min(160, [int]($primary.Width / 5))
    $startY = $primary.Top + [Math]::Min(140, [int]($primary.Height / 5))
    $endX = [Math]::Min($primary.Right - 80, $startX + 320)
    $endY = [Math]::Min($primary.Bottom - 80, $startY + 180)
    [void][CapturAcceptanceNative]::SetCursorPos($startX, $startY)
    [CapturAcceptanceNative+POINT]$physicalStart = New-Object CapturAcceptanceNative+POINT
    [void][CapturAcceptanceNative]::GetPhysicalCursorPos([ref]$physicalStart)
    Start-Sleep -Milliseconds 40
    [CapturAcceptanceNative]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    # Give the overlay one input/repaint cycle to latch the physical drag origin before moving.
    Start-Sleep -Milliseconds 80
    for ($step = 1; $step -le 12; $step++) {
        $x = $startX + [int](($endX - $startX) * $step / 12)
        $y = $startY + [int](($endY - $startY) * $step / 12)
        [void][CapturAcceptanceNative]::SetCursorPos($x, $y)
        Start-Sleep -Milliseconds 8
    }
    [CapturAcceptanceNative+POINT]$physicalEnd = New-Object CapturAcceptanceNative+POINT
    [void][CapturAcceptanceNative]::GetPhysicalCursorPos([ref]$physicalEnd)
    Start-Sleep -Milliseconds 40
    $before = [CapturAcceptanceNative]::GetClipboardSequenceNumber()
    [CapturAcceptanceNative]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    $region = Wait-ClipboardImage $before 5000
    $expectedWidth = [Math]::Abs($physicalEnd.X - $physicalStart.X)
    $expectedHeight = [Math]::Abs($physicalEnd.Y - $physicalStart.Y)
    if ($region.width -lt 2 -or $region.height -lt 2) {
        throw "Region clipboard bitmap was empty: $($region.width)x$($region.height)"
    }
    if ([Math]::Abs($region.width - $expectedWidth) -gt 4 -or [Math]::Abs($region.height - $expectedHeight) -gt 4) {
        throw "Region clipboard bitmap did not match the physical drag: $($region.width)x$($region.height) versus ${expectedWidth}x${expectedHeight}"
    }
    $results.region_release_to_clipboard_ms = $region.elapsed_ms
    $results.region_automation_physical_delta = "${expectedWidth}x${expectedHeight}"
    $results.region_dimensions = "$($region.width)x$($region.height)"
    [void](Wait-WindowVisible $false 3000)

    $before = [CapturAcceptanceNative]::GetClipboardSequenceNumber()
    $sameTimer = [Diagnostics.Stopwatch]::StartNew()
    Send-Chord 0x35
    $same = Wait-ClipboardImage $before 15000
    if ($same.width -ne $region.width -or $same.height -ne $region.height) {
        throw "Same-region dimensions changed to $($same.width)x$($same.height)"
    }
    $results.same_region_hotkey_to_clipboard_ms = [int]$sameTimer.ElapsedMilliseconds
    $results.same_region_dimensions = "$($same.width)x$($same.height)"
    [void](Wait-WindowVisible $false 3000)

    [void][CapturAcceptanceNative]::SetCursorPos(
        $primary.Left + [int]($primary.Width / 2),
        $primary.Top + [int]($primary.Height / 2)
    )
    $before = [CapturAcceptanceNative]::GetClipboardSequenceNumber()
    $monitorTimer = [Diagnostics.Stopwatch]::StartNew()
    Send-Chord 0x37
    $monitor = Wait-ClipboardImage $before 15000
    if ($monitor.width -lt 100 -or $monitor.height -lt 100) {
        throw "Monitor capture bitmap was implausibly small: $($monitor.width)x$($monitor.height)"
    }
    $results.monitor_hotkey_to_clipboard_ms = [int]$monitorTimer.ElapsedMilliseconds
    $results.monitor_dimensions = "$($monitor.width)x$($monitor.height)"
    [void](Wait-WindowVisible $false 3000)

    $targetForm = New-Object Windows.Forms.Form
    $targetForm.Text = 'Captur Acceptance Target'
    $targetForm.StartPosition = 'Manual'
    $targetForm.Location = New-Object Drawing.Point(($primary.Left + 240), ($primary.Top + 180))
    $targetForm.ClientSize = New-Object Drawing.Size(640, 360)
    $targetForm.Show()
    $targetForm.Activate()
    [void][CapturAcceptanceNative]::SetForegroundWindow($targetForm.Handle)
    [Windows.Forms.Application]::DoEvents()
    Start-Sleep -Milliseconds 150
    $before = [CapturAcceptanceNative]::GetClipboardSequenceNumber()
    $activeTimer = [Diagnostics.Stopwatch]::StartNew()
    Send-Chord 0x38
    $active = Wait-ClipboardImage $before 15000
    if ($active.width -lt 100 -or $active.height -lt 100) {
        throw "Active-window dimensions were implausible: $($active.width)x$($active.height)"
    }
    $results.active_window_hotkey_to_clipboard_ms = [int]$activeTimer.ElapsedMilliseconds
    $results.active_window_dimensions = "$($active.width)x$($active.height)"
    $targetForm.Close()
    $targetForm.Dispose()
    $targetForm = $null
    [void](Wait-WindowVisible $false 3000)

    $beforeCancel = [CapturAcceptanceNative]::GetClipboardSequenceNumber()
    Send-Chord 0x34
    [void](Wait-WindowVisible $true 15000)
    Send-Key 0x1B
    $results.escape_cancel_to_hidden_ms = Wait-WindowVisible $false 3000
    if ([CapturAcceptanceNative]::GetClipboardSequenceNumber() -ne $beforeCancel) {
        throw 'Escape cancellation unexpectedly changed the clipboard'
    }

    $results.capture_acceptance = 'passed'
} finally {
    if ($null -ne $targetForm) {
        try { $targetForm.Close(); $targetForm.Dispose() } catch {}
    }
    [void][CapturAcceptanceNative]::SetCursorPos($originalCursor.X, $originalCursor.Y)
    if ($null -ne $script:capturProcess -and -not $script:capturProcess.HasExited) {
        Stop-Process -Id $script:capturProcess.Id -Force
        $script:capturProcess.WaitForExit()
    }
    $clipboardRestored = $false
    for ($attempt = 0; $attempt -lt 8 -and -not $clipboardRestored; $attempt++) {
        try {
            if ($backupFormats.Count -gt 0) { [Windows.Forms.Clipboard]::SetDataObject($backup, $true) }
            else { [Windows.Forms.Clipboard]::Clear() }
            $clipboardRestored = $true
        } catch {
            $results.clipboard_restore_error = $_.Exception.Message
            Start-Sleep -Milliseconds 20
        }
    }
    $results.clipboard_restored = $clipboardRestored
    $results | ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 $ResultPath
}
