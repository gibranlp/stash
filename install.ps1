$ErrorActionPreference = "Stop"

$REPO    = "gibranlp/stash"
$ASSET   = "stash-windows-x86_64.exe"
$INSTALL = "$env:LOCALAPPDATA\Programs\stash"

Write-Host "Fetching latest stash release..."
$release = Invoke-RestMethod "https://api.github.com/repos/$REPO/releases/latest"
$tag     = $release.tag_name
$url     = "https://github.com/$REPO/releases/download/$tag/$ASSET"

Write-Host "Installing stash $tag..."
New-Item -ItemType Directory -Force -Path $INSTALL | Out-Null
Invoke-WebRequest -Uri $url -OutFile "$INSTALL\stash.exe"

Write-Host ""
Write-Host "Installed to $INSTALL\stash.exe"

# Add to user PATH if not already there
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$INSTALL*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$INSTALL", "User")
    Write-Host "Added $INSTALL to your user PATH."
    Write-Host "Restart your terminal for the change to take effect."
} else {
    Write-Host "$INSTALL is already in your PATH."
}
