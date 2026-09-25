$ErrorActionPreference = "Stop"

Set-Location $PSScriptRoot

Write-Host "Installazione dipendenze..." -ForegroundColor Cyan
npm install

Write-Host "Verifica frontend..." -ForegroundColor Cyan
npm run build

Write-Host "Verifica backend Rust..." -ForegroundColor Cyan
cargo check --manifest-path .\src-tauri\Cargo.toml

Write-Host "Creazione installer NSIS..." -ForegroundColor Cyan
npm run tauri build -- --bundles nsis

$installer = Get-ChildItem `
    ".\src-tauri\target\release\bundle\nsis\*.exe" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if (-not $installer) {
    throw "Installer NSIS non trovato."
}

Write-Host ""
Write-Host "INSTALLER CREATO:" -ForegroundColor Green
Write-Host $installer.Name -ForegroundColor Green
Write-Host ""
Write-Host "Cartella:" $installer.DirectoryName

Start-Process explorer.exe $installer.DirectoryName