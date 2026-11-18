# Build VibeStorm even while it is running.
# Windows locks a running exe, but allows renaming it: every locked vibestorm*.exe
# under target\release is moved aside first, then cargo builds a fresh one.
# After it finishes, restart VibeStorm (desktop shortcut) to get the new build.

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'

Get-ChildItem target\release -Recurse -Filter 'vibestorm*.exe' -ErrorAction SilentlyContinue | ForEach-Object {
    $f = $_.FullName
    if ($_.Name -like '*.old-*') {
        # leftovers from earlier builds: delete the ones no longer running
        try { Remove-Item $f -ErrorAction Stop } catch {}
        return
    }
    try {
        $s = [System.IO.File]::Open($f, 'Open', 'ReadWrite', 'None'); $s.Close()
    } catch {
        $new = "$($_.BaseName).old-$stamp.exe"
        Rename-Item $f $new
        Write-Host "moved running $($_.Name) -> $new"
    }
}

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host ""
Write-Host "Built target\release\vibestorm.exe - restart VibeStorm to use it."
