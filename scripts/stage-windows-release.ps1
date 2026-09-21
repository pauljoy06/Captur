param(
    [Parameter(Mandatory = $true)][string]$SourcePath,
    [string]$DestinationPath = (Join-Path $env:LOCALAPPDATA 'Captur\captur.exe'),
    [switch]$SkipIfRunning
)

$ErrorActionPreference = 'Stop'

$source = [IO.Path]::GetFullPath($SourcePath)
$destination = [IO.Path]::GetFullPath($DestinationPath)
$directory = Split-Path -Parent $destination
$temp = Join-Path $directory ('.captur-' + [Guid]::NewGuid().ToString('N') + '.tmp')

if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    throw "Captur release executable was not found: $source"
}

$running = @(
    Get-Process captur -ErrorAction SilentlyContinue | Where-Object {
        try {
            -not [string]::IsNullOrWhiteSpace($_.Path) -and
                [IO.Path]::GetFullPath($_.Path).Equals(
                    $destination,
                    [StringComparison]::OrdinalIgnoreCase
                )
        } catch {
            $false
        }
    }
)
if ($running.Count -gt 0) {
    $processIds = ($running | ForEach-Object Id) -join ', '
    $message = "Captur is running from $destination (PID $processIds). Exit it from the tray, then rebuild."
    if ($SkipIfRunning) {
        Write-Warning $message
        return
    }
    throw $message
}

New-Item -ItemType Directory -Path $directory -Force | Out-Null
try {
    Copy-Item -LiteralPath $source -Destination $temp -Force

    $sourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash
    $tempHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $temp).Hash
    if ($sourceHash -ne $tempHash) {
        throw 'The staged Captur executable did not match the release build.'
    }

    Move-Item -LiteralPath $temp -Destination $destination -Force
    Write-Output $destination
} finally {
    if (Test-Path -LiteralPath $temp) {
        Remove-Item -LiteralPath $temp -Force
    }
}
