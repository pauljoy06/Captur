param(
    [Parameter(Mandatory = $true)][string]$ExePath,
    [Parameter(Mandatory = $true)][string]$ScreenshotPath,
    [switch]$ScrollToBottom
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ProofSnipWorkspaceNative {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
    public delegate bool EnumWindowsProc(IntPtr window, IntPtr parameter);
    [DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr parameter);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out RECT rect);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
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
}
'@

function Send-Chord([byte]$key) {
    [ProofSnipWorkspaceNative]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
    [ProofSnipWorkspaceNative]::keybd_event(0x10, 0, 0, [UIntPtr]::Zero)
    [ProofSnipWorkspaceNative]::keybd_event($key, 0, 0, [UIntPtr]::Zero)
    [ProofSnipWorkspaceNative]::keybd_event($key, 0, 2, [UIntPtr]::Zero)
    [ProofSnipWorkspaceNative]::keybd_event(0x10, 0, 2, [UIntPtr]::Zero)
    [ProofSnipWorkspaceNative]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
}

$backup = [Windows.Forms.Clipboard]::GetDataObject()
$process = $null
try {
    $process = Start-Process -FilePath $ExePath -PassThru
    Start-Sleep -Seconds 3
    Send-Chord 0x37
    Start-Sleep -Seconds 2
    Send-Chord 0x36
    Start-Sleep -Seconds 2
    $window = [ProofSnipWorkspaceNative]::FindLargestVisibleWindow([uint32]$process.Id)
    if ($window -eq [IntPtr]::Zero) { throw 'ProofSnip workspace did not become visible' }
    [ProofSnipWorkspaceNative+RECT]$rect = New-Object ProofSnipWorkspaceNative+RECT
    [void][ProofSnipWorkspaceNative]::GetWindowRect($window, [ref]$rect)
    if ($ScrollToBottom) {
        [void][ProofSnipWorkspaceNative]::SetCursorPos(($rect.Left + $rect.Right) / 2, ($rect.Top + $rect.Bottom) / 2)
        [ProofSnipWorkspaceNative]::mouse_event(0x0800, 0, 0, 4294955296, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 500
    }
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    $bitmap = New-Object Drawing.Bitmap($width, $height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
    $graphics.Dispose()
    $bitmap.Save($ScreenshotPath, [Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
    Write-Output "workspace=$($rect.Left),$($rect.Top) ${width}x${height} screenshot=$ScreenshotPath"
} finally {
    if ($null -ne $process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
        $process.WaitForExit()
    }
    if ($null -ne $backup) {
        for ($attempt = 0; $attempt -lt 20; $attempt++) {
            try { [Windows.Forms.Clipboard]::SetDataObject($backup, $true); break }
            catch { Start-Sleep -Milliseconds 20 }
        }
    }
}
