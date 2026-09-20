param(
    [Parameter(Mandatory = $true)][string]$ExePath,
    [Parameter(Mandatory = $true)][string]$ResultPath,
    [string]$ScreenshotPath
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Security
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class CapturUxNative {
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct NOTIFYICONIDENTIFIER {
        public uint cbSize;
        public IntPtr hWnd;
        public uint uID;
        public Guid guidItem;
    }
    public delegate bool EnumWindowsProc(IntPtr window, IntPtr parameter);

    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
    [DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT point);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr parameter);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out RECT rect);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr window, StringBuilder text, int maximum);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr window, StringBuilder text, int maximum);
    [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtr(IntPtr window, int index);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, UIntPtr wparam, IntPtr lparam);
    [DllImport("user32.dll")] public static extern uint GetClipboardSequenceNumber();
    [DllImport("shell32.dll")] public static extern int Shell_NotifyIconGetRect(ref NOTIFYICONIDENTIFIER identifier, out RECT iconLocation);

    public static IntPtr FindWindow(uint processId, string title, string className, bool visibleOnly) {
        IntPtr result = IntPtr.Zero;
        EnumWindows(delegate(IntPtr window, IntPtr parameter) {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != processId || (visibleOnly && !IsWindowVisible(window))) return true;
            StringBuilder actualTitle = new StringBuilder(256);
            StringBuilder actualClass = new StringBuilder(256);
            GetWindowText(window, actualTitle, actualTitle.Capacity);
            GetClassName(window, actualClass, actualClass.Capacity);
            if ((title == null || actualTitle.ToString() == title) &&
                (className == null || actualClass.ToString() == className)) {
                result = window;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }

    public static IntPtr FindLargestVisibleWindow(uint processId) {
        IntPtr result = IntPtr.Zero;
        long largest = -1;
        EnumWindows(delegate(IntPtr window, IntPtr parameter) {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == processId && IsWindowVisible(window)) {
                RECT rect;
                GetWindowRect(window, out rect);
                long area = Math.Max(0, rect.Right - rect.Left) * (long)Math.Max(0, rect.Bottom - rect.Top);
                if (area > largest) { largest = area; result = window; }
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }

    public static IntPtr FindLargestSecondaryVisibleWindow(uint processId, IntPtr excluded) {
        IntPtr result = IntPtr.Zero;
        long largest = -1;
        EnumWindows(delegate(IntPtr window, IntPtr parameter) {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == processId && window != excluded && IsWindowVisible(window)) {
                RECT rect;
                GetWindowRect(window, out rect);
                long area = Math.Max(0, rect.Right - rect.Left) * (long)Math.Max(0, rect.Bottom - rect.Top);
                if (area > largest) { largest = area; result = window; }
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }
}
'@

[void][CapturUxNative]::SetProcessDpiAwarenessContext([IntPtr](-4))
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$valueName = 'Captur'

function Send-Chord([byte]$key) {
    [CapturUxNative]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
    [CapturUxNative]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
    [CapturUxNative]::keybd_event($key, 0, 0, [UIntPtr]::Zero)
    [CapturUxNative]::keybd_event($key, 0, 2, [UIntPtr]::Zero)
    [CapturUxNative]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
    [CapturUxNative]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
}

function Send-Key([byte]$key) {
    [CapturUxNative]::keybd_event($key, 0, 0, [UIntPtr]::Zero)
    [CapturUxNative]::keybd_event($key, 0, 2, [UIntPtr]::Zero)
}

function Wait-Workspace([uint32]$processId, [int]$timeoutMs) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.ElapsedMilliseconds -lt $timeoutMs) {
        $window = [CapturUxNative]::FindLargestVisibleWindow($processId)
        if ($window -ne [IntPtr]::Zero) {
            [CapturUxNative+RECT]$rect = New-Object CapturUxNative+RECT
            [void][CapturUxNative]::GetWindowRect($window, [ref]$rect)
            if (($rect.Right - $rect.Left) -ge 1000 -and ($rect.Bottom - $rect.Top) -ge 700) {
                return $window
            }
        }
        Start-Sleep -Milliseconds 10
    }
    throw 'Captur workspace did not become visible at its expected size'
}

function Wait-WindowVisibility([IntPtr]$window, [bool]$visible, [int]$timeoutMs) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.ElapsedMilliseconds -lt $timeoutMs) {
        if ([CapturUxNative]::IsWindowVisible($window) -eq $visible) { return }
        Start-Sleep -Milliseconds 10
    }
    throw "Window visibility did not become $visible"
}

function Click-Relative([IntPtr]$window, [int]$x, [int]$y) {
    [CapturUxNative+RECT]$rect = New-Object CapturUxNative+RECT
    [void][CapturUxNative]::GetWindowRect($window, [ref]$rect)
    [void][CapturUxNative]::SetCursorPos($rect.Left + $x, $rect.Top + $y)
    Start-Sleep -Milliseconds 50
    [CapturUxNative]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    [CapturUxNative]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 180
}

function Click-Normalized([IntPtr]$window, [double]$x, [double]$y) {
    [CapturUxNative+RECT]$rect = New-Object CapturUxNative+RECT
    [void][CapturUxNative]::GetWindowRect($window, [ref]$rect)
    Click-Relative $window ([int](($rect.Right - $rect.Left) * $x)) ([int](($rect.Bottom - $rect.Top) * $y))
}

function Scroll-Workspace-To-Bottom([IntPtr]$window) {
    [CapturUxNative+RECT]$rect = New-Object CapturUxNative+RECT
    [void][CapturUxNative]::GetWindowRect($window, [ref]$rect)
    [void][CapturUxNative]::SetForegroundWindow($window)
    [void][CapturUxNative]::SetCursorPos(
        [int](($rect.Left + $rect.Right) / 2),
        [int](($rect.Top + $rect.Bottom) / 2)
    )
    for ($step = 0; $step -lt 3; $step++) {
        [CapturUxNative]::mouse_event(0x0800, 0, 0, 4294955296, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 120
    }
    Start-Sleep -Milliseconds 500
}

function Save-WindowScreenshot([IntPtr]$window, [string]$path) {
    if ([string]::IsNullOrWhiteSpace($path)) { return }
    [CapturUxNative+RECT]$rect = New-Object CapturUxNative+RECT
    [void][CapturUxNative]::GetWindowRect($window, [ref]$rect)
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    $bitmap = New-Object Drawing.Bitmap($width, $height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
    $graphics.Dispose()
    $bitmap.Save($path, [Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
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

function Get-ClipboardImageInfo {
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        try {
            if ([Windows.Forms.Clipboard]::ContainsImage()) {
                $image = [Windows.Forms.Clipboard]::GetImage()
                if ($null -ne $image) {
                    $stream = New-Object IO.MemoryStream
                    $image.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
                    $sha = [Security.Cryptography.SHA256]::Create()
                    $hash = [BitConverter]::ToString($sha.ComputeHash($stream.ToArray())).Replace('-', '').ToLowerInvariant()
                    $result = [ordered]@{ width = $image.Width; height = $image.Height; sha256 = $hash }
                    $sha.Dispose(); $stream.Dispose(); $image.Dispose()
                    return $result
                }
            }
        } catch [System.Runtime.InteropServices.ExternalException] {}
        Start-Sleep -Milliseconds 20
    }
    throw 'Clipboard does not contain an image'
}

function Wait-ClipboardImageChange([uint32]$before, [int]$timeoutMs) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.ElapsedMilliseconds -lt $timeoutMs) {
        if ([CapturUxNative]::GetClipboardSequenceNumber() -ne $before) {
            return Get-ClipboardImageInfo
        }
        Start-Sleep -Milliseconds 5
    }
    throw 'Annotated image did not reach the clipboard'
}

$results = [ordered]@{}
$backup = Copy-ClipboardBackup
$backupFormats = @($backup.GetFormats($false))
[CapturUxNative+POINT]$originalCursor = New-Object CapturUxNative+POINT
[void][CapturUxNative]::GetCursorPos([ref]$originalCursor)
$originalStartupExists = $false
$originalStartupValue = $null
try {
    $property = Get-ItemProperty -Path $runKey -Name $valueName -ErrorAction Stop
    $originalStartupExists = $true
    $originalStartupValue = [string]$property.$valueName
} catch [System.Management.Automation.ItemNotFoundException] {
} catch [System.Management.Automation.PSArgumentException] {
}
$process = $null

try {
    $process = Start-Process -FilePath $ExePath -ArgumentList '--background' -PassThru
    Start-Sleep -Seconds 3
    $tray = [CapturUxNative]::FindWindow([uint32]$process.Id, 'Captur Tray', 'CapturTrayWindow', $false)
    if ($tray -eq [IntPtr]::Zero) { throw 'Captur tray window was not created' }
    $results.tray_window_created = $true

    $associatedIcon = [Drawing.Icon]::ExtractAssociatedIcon((Resolve-Path $ExePath).Path)
    if ($null -eq $associatedIcon) { throw 'captur.exe did not expose an associated icon' }
    $iconBitmap = $associatedIcon.ToBitmap()
    $iconStream = New-Object IO.MemoryStream
    $iconBitmap.Save($iconStream, [Drawing.Imaging.ImageFormat]::Png)
    $iconSha = [Security.Cryptography.SHA256]::Create()
    $iconHash = [BitConverter]::ToString($iconSha.ComputeHash($iconStream.ToArray())).Replace('-', '').ToLowerInvariant()
    $results.executable_icon = [ordered]@{
        width = $iconBitmap.Width
        height = $iconBitmap.Height
        sha256 = $iconHash
    }
    $iconSha.Dispose()
    $iconStream.Dispose()
    $iconBitmap.Dispose()
    $associatedIcon.Dispose()

    [CapturUxNative+NOTIFYICONIDENTIFIER]$trayIdentifier = New-Object CapturUxNative+NOTIFYICONIDENTIFIER
    $trayIdentifier.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($trayIdentifier)
    $trayIdentifier.hWnd = $tray
    $trayIdentifier.uID = 1
    [CapturUxNative+RECT]$trayRect = New-Object CapturUxNative+RECT
    $trayResult = [CapturUxNative]::Shell_NotifyIconGetRect([ref]$trayIdentifier, [ref]$trayRect)
    if ($trayResult -ne 0) { throw "Captur tray icon was not registered (HRESULT $trayResult)" }
    $results.tray_icon = [ordered]@{
        registered = $true
        bounds = "$($trayRect.Left),$($trayRect.Top) $(($trayRect.Right-$trayRect.Left))x$(($trayRect.Bottom-$trayRect.Top))"
    }

    Send-Chord 0x4D
    Start-Sleep -Seconds 2
    $source = Get-ClipboardImageInfo
    [void][CapturUxNative]::PostMessage($tray, 0x0111, [UIntPtr]([uint64]1), [IntPtr]::Zero)
    $workspace = Wait-Workspace ([uint32]$process.Id) 10000
    [void][CapturUxNative]::SetForegroundWindow($workspace)
    Start-Sleep -Milliseconds 300

    # Enter the real annotation workspace from the latest-capture card.
    Click-Normalized $workspace 0.685 0.935
    Start-Sleep -Milliseconds 500
    Save-WindowScreenshot $workspace $ScreenshotPath
    [void][CapturUxNative]::SetForegroundWindow($workspace)
    Send-Key 0x31
    Click-Normalized $workspace 0.50 0.55
    $clipboardBefore = [CapturUxNative]::GetClipboardSequenceNumber()
    Send-Key 0x0D
    $annotated = Wait-ClipboardImageChange $clipboardBefore 5000
    if ($annotated.width -ne $source.width -or $annotated.height -ne $source.height) {
        throw 'Annotation changed the capture dimensions'
    }
    if ($annotated.sha256 -eq $source.sha256) { throw 'Annotation did not change the screenshot pixels' }
    $results.annotation = [ordered]@{
        source = "$($source.width)x$($source.height)"
        output = "$($annotated.width)x$($annotated.height)"
        pixels_changed = $true
        clipboard_updated = $true
    }

    # Pin the annotated capture and verify that the real secondary viewport is topmost.
    $workspace = Wait-Workspace ([uint32]$process.Id) 5000
    [CapturUxNative]::mouse_event(0x0800, 0, 0, 4294966096, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 400
    Click-Normalized $workspace 0.582 0.432
    $pin = [IntPtr]::Zero
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.ElapsedMilliseconds -lt 5000) {
        $pin = [CapturUxNative]::FindLargestSecondaryVisibleWindow([uint32]$process.Id, $workspace)
        if ($pin -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 20
    }
    if ($pin -eq [IntPtr]::Zero) { throw 'Pinned screenshot viewport did not appear' }
    $extendedStyle = [CapturUxNative]::GetWindowLongPtr($pin, -20).ToInt64()
    if (($extendedStyle -band 0x8) -eq 0) { throw 'Pinned screenshot viewport is not topmost' }
    [CapturUxNative+RECT]$pinRect = New-Object CapturUxNative+RECT
    [void][CapturUxNative]::GetWindowRect($pin, [ref]$pinRect)
    $results.pin = [ordered]@{
        visible = $true
        topmost = $true
        bounds = "$($pinRect.Left),$($pinRect.Top) $(($pinRect.Right-$pinRect.Left))x$(($pinRect.Bottom-$pinRect.Top))"
    }
    [void][CapturUxNative]::PostMessage($pin, 0x0010, [UIntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 300

    # Toggle the visible startup control and observe the actual current-user Run value.
    Scroll-Workspace-To-Bottom $workspace
    $expectedAfterFirstToggle = -not $originalStartupExists
    # Click the label rather than the small checkbox glyph so the public hit target is exercised.
    Click-Normalized $workspace 0.17 0.844
    $startupAfterToggle = $null
    $startupTimer = [Diagnostics.Stopwatch]::StartNew()
    while ($startupTimer.ElapsedMilliseconds -lt 3000) {
        $startupAfterToggle = $null
        try { $startupAfterToggle = [string](Get-ItemPropertyValue -Path $runKey -Name $valueName -ErrorAction Stop) } catch {}
        if (($expectedAfterFirstToggle -and $null -ne $startupAfterToggle -and $startupAfterToggle -match '--background') -or
            (-not $expectedAfterFirstToggle -and $null -eq $startupAfterToggle)) { break }
        Start-Sleep -Milliseconds 50
    }
    if ($expectedAfterFirstToggle) {
        if ($null -eq $startupAfterToggle -or $startupAfterToggle -notmatch '--background') {
            throw 'Startup checkbox did not create the expected Run value'
        }
    } elseif ($null -ne $startupAfterToggle) {
        throw 'Startup checkbox did not remove the existing Run value'
    }
    Click-Normalized $workspace 0.17 0.844
    $restoredStartupValue = $null
    $startupTimer.Restart()
    while ($startupTimer.ElapsedMilliseconds -lt 3000) {
        $restoredStartupValue = $null
        try { $restoredStartupValue = [string](Get-ItemPropertyValue -Path $runKey -Name $valueName -ErrorAction Stop) } catch {}
        if (($originalStartupExists -and $restoredStartupValue -eq $originalStartupValue) -or
            (-not $originalStartupExists -and $null -eq $restoredStartupValue)) { break }
        Start-Sleep -Milliseconds 50
    }
    if (($originalStartupExists -and $restoredStartupValue -ne $originalStartupValue) -or
        (-not $originalStartupExists -and $null -ne $restoredStartupValue)) {
        throw 'Startup checkbox did not restore the original Run value'
    }
    $results.startup_toggle = [ordered]@{
        original_enabled = $originalStartupExists
        toggled_enabled = $expectedAfterFirstToggle
        registry_observed = $true
        restored_through_ui = $true
    }

    # Close the workspace, prove the process remains resident, show it via the tray command,
    # close it again, then use the real tray command to exit cleanly.
    [void][CapturUxNative]::PostMessage($workspace, 0x0010, [UIntPtr]::Zero, [IntPtr]::Zero)
    Wait-WindowVisibility $workspace $false 5000
    if ($process.HasExited) { throw 'Captur exited instead of closing to the tray' }
    [void][CapturUxNative]::PostMessage($tray, 0x0111, [UIntPtr]([uint64]1), [IntPtr]::Zero)
    $workspace = Wait-Workspace ([uint32]$process.Id) 5000
    $results.close_to_tray = $true
    $results.tray_show_workspace = $true

    [void][CapturUxNative]::PostMessage($workspace, 0x0010, [UIntPtr]::Zero, [IntPtr]::Zero)
    Wait-WindowVisibility $workspace $false 5000
    [void][CapturUxNative]::PostMessage($tray, 0x0111, [UIntPtr]([uint64]2), [IntPtr]::Zero)
    if (-not $process.WaitForExit(5000)) { throw 'Tray Exit did not stop Captur' }
    $results.tray_exit = $true
    $results.ux_acceptance = 'passed'
} finally {
    [void][CapturUxNative]::SetCursorPos($originalCursor.X, $originalCursor.Y)
    if ($null -ne $process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }
    if ($originalStartupExists) {
        New-ItemProperty -Path $runKey -Name $valueName -Value $originalStartupValue -PropertyType String -Force | Out-Null
    } else {
        Remove-ItemProperty -Path $runKey -Name $valueName -ErrorAction SilentlyContinue
    }
    if ($null -ne $backup) {
        for ($attempt = 0; $attempt -lt 20; $attempt++) {
            try { [Windows.Forms.Clipboard]::SetDataObject($backup, $true); break }
            catch { Start-Sleep -Milliseconds 20 }
        }
    }
    $restored = $false
    try {
        $current = [Windows.Forms.Clipboard]::GetDataObject()
        $restored = $null -ne $current -and @($current.GetFormats($false)).Count -eq $backupFormats.Count
    } catch {}
    $results.clipboard_restored = $restored
    $results.startup_registry_restored = $true
    $results | ConvertTo-Json -Depth 7 | Set-Content -Encoding UTF8 $ResultPath
}

$results | ConvertTo-Json -Depth 7
