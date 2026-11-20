# Build VibeZen even while it is running.
# Windows locks a running exe, but allows renaming it: every locked vibezen*.exe
# under target\release is moved aside first, then cargo builds a fresh one.
# After it finishes, restart VibeZen (desktop shortcut) to get the new build.

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$exe = Join-Path $PSScriptRoot 'target\release\vibezen.exe'

Get-ChildItem target\release -Recurse -Filter '*.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -like 'vibezen*.exe' -or $_.Name -like '.old-*.exe' } | ForEach-Object {
    $f = $_.FullName
    $name = $_.Name
    if ($name -like '*.old-*') {
        # leftovers from earlier builds: delete the ones no longer running
        try { Remove-Item $f -ErrorAction Stop } catch {}
        return
    }
    try {
        $s = [System.IO.File]::Open($f, 'Open', 'ReadWrite', 'None'); $s.Close()
    } catch {
        $new = "$([System.IO.Path]::GetFileNameWithoutExtension($name)).old-$stamp.exe"
        Rename-Item $f $new
        Write-Host "moved running $name -> $new"
    }
}

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Windows link tracking makes shortcuts follow the renamed exe above, so point every
# VibeZen shortcut back at the fresh build (icon comes from the exe itself).
$shell = New-Object -ComObject WScript.Shell
$dirs = @(
    [Environment]::GetFolderPath('Desktop'),
    [Environment]::GetFolderPath('Programs'),
    "$env:APPDATA\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar"
)
Get-ChildItem $dirs -Filter '*.lnk' -Recurse -ErrorAction SilentlyContinue | ForEach-Object {
    $lnk = $shell.CreateShortcut($_.FullName)
    $t = $lnk.TargetPath
    if ($t -like "$PSScriptRoot\target\release\*.exe" -and ($t -ne $exe -or $lnk.IconLocation -ne "$exe,0")) {
        $lnk.TargetPath = $exe
        $lnk.IconLocation = "$exe,0"
        $lnk.Save()
        Write-Host "shortcut $($_.Name) -> target\release\vibezen.exe"
    }
}

Write-Host ""
Write-Host "Built target\release\vibezen.exe - restart VibeZen to use it."
