# Install the extracted portable package for the current user; no administrator needed.
$ErrorActionPreference = 'Stop'
$destination = Join-Path $env:LOCALAPPDATA 'Vole'
$source = $PSScriptRoot
if ([IO.Path]::GetFullPath($source) -ne [IO.Path]::GetFullPath($destination)) {
    if (Get-Process vole -ErrorAction SilentlyContinue) {
        throw 'Close Vole before replacing an installed copy.'
    }
    New-Item -ItemType Directory -Force $destination | Out-Null
    Get-ChildItem -LiteralPath $source | Copy-Item -Destination $destination -Recurse -Force
}
$shortcutDirectory = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$shortcutPath = Join-Path $shortcutDirectory 'Vole.lnk'
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcutPath)
$shortcut.TargetPath = Join-Path $destination 'vole.exe'
$shortcut.WorkingDirectory = $destination
$shortcut.Description = 'Assemble and simulate teaching machines'
$shortcut.Save()
Write-Output "Installed Vole in $destination. Open Vole from the Start menu."
