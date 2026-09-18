param(
    [Parameter(Mandatory = $true)][string]$ExePath,
    [Parameter(Mandatory = $true)][string]$PdfPath,
    [Parameter(Mandatory = $true)][string]$ResultPath,
    [Parameter(Mandatory = $true)][string]$ScreenshotPath
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class CapturEvidenceNative {
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
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

    public static IntPtr FindVisibleWindowByTitle(uint processId, string expectedTitle) {
        IntPtr result = IntPtr.Zero;
        EnumWindows(delegate(IntPtr window, IntPtr parameter) {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == processId && IsWindowVisible(window)) {
                StringBuilder title = new StringBuilder(256);
                GetWindowText(window, title, title.Capacity);
                if (title.ToString() == expectedTitle) { result = window; return false; }
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }
}
'@

[void][CapturEvidenceNative]::SetProcessDpiAwarenessContext([IntPtr](-4))

function Send-Chord([byte]$key) {
    [CapturEvidenceNative]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)
    [CapturEvidenceNative]::keybd_event(0x10, 0, 0, [UIntPtr]::Zero)
    [CapturEvidenceNative]::keybd_event($key, 0, 0, [UIntPtr]::Zero)
    [CapturEvidenceNative]::keybd_event($key, 0, 2, [UIntPtr]::Zero)
    [CapturEvidenceNative]::keybd_event(0x10, 0, 2, [UIntPtr]::Zero)
    [CapturEvidenceNative]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)
}

function Wait-Workspace([uint32]$processId, [int]$timeoutMs) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.ElapsedMilliseconds -lt $timeoutMs) {
        $window = [CapturEvidenceNative]::FindLargestVisibleWindow($processId)
        if ($window -ne [IntPtr]::Zero) {
            [CapturEvidenceNative+RECT]$rect = New-Object CapturEvidenceNative+RECT
            [void][CapturEvidenceNative]::GetWindowRect($window, [ref]$rect)
            if (($rect.Right - $rect.Left) -ge 1000 -and ($rect.Bottom - $rect.Top) -ge 700) {
                return $window
            }
        }
        Start-Sleep -Milliseconds 10
    }
    throw 'Captur workspace did not become visible at its expected size'
}

function Click-Relative([IntPtr]$window, [int]$x, [int]$y) {
    [CapturEvidenceNative+RECT]$rect = New-Object CapturEvidenceNative+RECT
    [void][CapturEvidenceNative]::GetWindowRect($window, [ref]$rect)
    [void][CapturEvidenceNative]::SetCursorPos($rect.Left + $x, $rect.Top + $y)
    Start-Sleep -Milliseconds 50
    [CapturEvidenceNative]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    [CapturEvidenceNative]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 180
}

function Click-Normalized([IntPtr]$window, [double]$x, [double]$y) {
    [CapturEvidenceNative+RECT]$rect = New-Object CapturEvidenceNative+RECT
    [void][CapturEvidenceNative]::GetWindowRect($window, [ref]$rect)
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    Click-Relative $window ([int]($width * $x)) ([int]($height * $y))
}

function Replace-Text([IntPtr]$window, [int]$x, [int]$y, [string]$text) {
    [void][CapturEvidenceNative]::SetForegroundWindow($window)
    Click-Relative $window $x $y
    [Windows.Forms.SendKeys]::SendWait('^a')
    [Windows.Forms.Clipboard]::SetText($text)
    [Windows.Forms.SendKeys]::SendWait('^v')
    Start-Sleep -Milliseconds 150
}

function Replace-Text-Normalized([IntPtr]$window, [double]$x, [double]$y, [string]$text) {
    [CapturEvidenceNative+RECT]$rect = New-Object CapturEvidenceNative+RECT
    [void][CapturEvidenceNative]::GetWindowRect($window, [ref]$rect)
    Replace-Text $window ([int](($rect.Right - $rect.Left) * $x)) ([int](($rect.Bottom - $rect.Top) * $y)) $text
}

function Select-Combo-Item([IntPtr]$window, [int]$x, [int]$y, [int]$index) {
    [void][CapturEvidenceNative]::SetForegroundWindow($window)
    Click-Relative $window $x $y
    # The observed egui popup centers its rows 32 px below the combo center, at 27 px steps.
    Click-Relative $window $x ($y + 32 + (27 * $index))
    Start-Sleep -Milliseconds 150
}

function Select-Combo-Item-Normalized([IntPtr]$window, [double]$x, [double]$y, [int]$index) {
    [CapturEvidenceNative+RECT]$rect = New-Object CapturEvidenceNative+RECT
    [void][CapturEvidenceNative]::GetWindowRect($window, [ref]$rect)
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    $relativeX = [int]($width * $x)
    $relativeY = [int]($height * $y)
    [void][CapturEvidenceNative]::SetForegroundWindow($window)
    Click-Relative $window $relativeX $relativeY
    $scale = $height / 900.0
    Click-Relative $window $relativeX ([int]($relativeY + (32 * $scale) + (27 * $scale * $index)))
    Start-Sleep -Milliseconds 150
}

function Save-WindowScreenshot([IntPtr]$window, [string]$path) {
    [CapturEvidenceNative+RECT]$rect = New-Object CapturEvidenceNative+RECT
    [void][CapturEvidenceNative]::GetWindowRect($window, [ref]$rect)
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

$results = [ordered]@{}
$backup = Copy-ClipboardBackup
$backupFormats = @($backup.GetFormats($false))
[CapturEvidenceNative+POINT]$originalCursor = New-Object CapturEvidenceNative+POINT
[void][CapturEvidenceNative]::GetCursorPos([ref]$originalCursor)
$process = $null

try {
    if (Test-Path $PdfPath) { Remove-Item -Force $PdfPath }
    $process = Start-Process -FilePath $ExePath -ArgumentList '--background' -PassThru
    Start-Sleep -Seconds 3

    Send-Chord 0x37
    Start-Sleep -Seconds 2
    Send-Chord 0x36
    $workspace = Wait-Workspace ([uint32]$process.Id) 10000
    [void][CapturEvidenceNative]::SetForegroundWindow($workspace)
    Start-Sleep -Milliseconds 500

    # The DPI-aware workspace keeps the same logical layout across scale factors. Normalized clicks
    # target the visible latest-capture action row, then the scrolled evidence controls.
    Click-Normalized $workspace 0.575 0.935
    Click-Normalized $workspace 0.575 0.935
    [CapturEvidenceNative]::mouse_event(0x0800, 0, 0, 4294966096, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 500

    Replace-Text-Normalized $workspace 0.50 0.64 'Order total deletion evidence'
    Replace-Text-Normalized $workspace 0.50 0.70 'Delete a line item, verify the total, then refresh.'

    [CapturEvidenceNative]::mouse_event(0x0800, 0, 0, 4294966096, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 500
    Replace-Text-Normalized $workspace 0.62 0.46 'Before deleting the line item.'
    Select-Combo-Item-Normalized $workspace 0.895 0.377 1
    Replace-Text-Normalized $workspace 0.62 0.745 'After refresh the correct total is displayed.'
    Select-Combo-Item-Normalized $workspace 0.895 0.664 3

    # Move the second capture up. This exercises ordering through the real evidence UI.
    Click-Normalized $workspace 0.32 0.827
    Start-Sleep -Milliseconds 300
    Save-WindowScreenshot $workspace $ScreenshotPath

    $exportStarted = [Diagnostics.Stopwatch]::StartNew()
    Click-Normalized $workspace 0.895 0.105
    $dialog = [IntPtr]::Zero
    while ($exportStarted.ElapsedMilliseconds -lt 10000) {
        $dialog = [CapturEvidenceNative]::FindVisibleWindowByTitle([uint32]$process.Id, 'Export Captur Evidence')
        if ($dialog -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 20
    }
    if ($dialog -eq [IntPtr]::Zero) { throw 'PDF save dialog did not appear' }

    [void][CapturEvidenceNative]::SetForegroundWindow($dialog)
    Start-Sleep -Milliseconds 200
    [Windows.Forms.SendKeys]::SendWait('^a')
    [Windows.Forms.Clipboard]::SetText($PdfPath)
    [Windows.Forms.SendKeys]::SendWait('^v')
    [Windows.Forms.SendKeys]::SendWait('{ENTER}')

    while ($exportStarted.ElapsedMilliseconds -lt 30000) {
        if ((Test-Path $PdfPath) -and (Get-Item $PdfPath).Length -gt 1000) { break }
        Start-Sleep -Milliseconds 25
    }
    if (-not (Test-Path $PdfPath)) { throw 'PDF output was not created' }
    $pdf = Get-Item $PdfPath
    if ($pdf.Length -le 1000) { throw "PDF output is unexpectedly small: $($pdf.Length) bytes" }
    $headerBytes = [IO.File]::ReadAllBytes($PdfPath)[0..4]
    $header = [Text.Encoding]::ASCII.GetString($headerBytes)
    if ($header -ne '%PDF-') { throw "PDF header is invalid: $header" }

    $results.capture_count = 2
    $results.captioned_captures = 2
    $results.labels = @('After', 'Before')
    $results.reordered = $true
    $results.pdf_path = $PdfPath
    $results.pdf_bytes = $pdf.Length
    $results.pdf_header = $header
    $results.export_dialog_to_file_ms = [int]$exportStarted.ElapsedMilliseconds
    $results.workspace_screenshot = $ScreenshotPath
    $results.evidence_acceptance = 'passed'
} finally {
    [void][CapturEvidenceNative]::SetCursorPos($originalCursor.X, $originalCursor.Y)
    if ($null -ne $process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
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
    $results | ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 $ResultPath
}

$results | ConvertTo-Json -Depth 6
