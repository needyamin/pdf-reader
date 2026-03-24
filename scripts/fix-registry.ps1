# Find and fix "Open with" registry entries showing "Electron"
$appPath = "${env:LOCALAPPDATA}\Programs\advanced-pdf-reader"
$exeName = "Advanced PDF Reader.exe"

# List all app entries
$apps = Get-ChildItem "HKCU:\Software\Classes\Applications\" -ErrorAction SilentlyContinue
foreach ($app in $apps) {
    $name = Split-Path $app.Name -Leaf
    if ($name -match "electron" -or $name -match "Advanced PDF") {
        Write-Host "Found: $name"
    }
}

# Check MuiCache for old Electron entries and update them
$muiCache = "HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache"
if (Test-Path $muiCache) {
    $props = Get-ItemProperty $muiCache
    foreach ($p in $props.PSObject.Properties) {
        if ($p.Value -eq "Electron" -or $p.Value -eq "electron") {
            Write-Host "Fixing MuiCache: $($p.Name) = $($p.Value)"
            Set-ItemProperty -Path $muiCache -Name $p.Name -Value "Advanced PDF Reader"
        }
    }
}

# Refresh icon cache
ie4uinit.exe -show
Write-Host "Done. Restart Explorer or reboot for changes to take effect."
