# Builds synq and installs it as the `synq` command (into ~\.cargo\bin).
# Run from the project folder:  .\install.ps1
# Add -NoWhisper to build without speech recognition (needs no CMake/libclang).
param([switch]$NoWhisper)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

if (Get-Process synq -ErrorAction SilentlyContinue) {
    Write-Host 'synq is running: close it first (Windows cannot replace a running program).' -ForegroundColor Yellow
    exit 1
}

$env:Path += ";$env:USERPROFILE\.cargo\bin"

if (-not $NoWhisper) {
    # CMake and Ninja come with Visual Studio; libclang with LLVM.
    $vs = Get-ChildItem "C:\Program Files\Microsoft Visual Studio\*\*\Common7\IDE\CommonExtensions\Microsoft\CMake" -Directory -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($vs) {
        $env:Path += ";$($vs.FullName)\CMake\bin;$($vs.FullName)\Ninja"
    }
    if (-not $env:LIBCLANG_PATH) {
        foreach ($dir in "$env:USERPROFILE\.tools\libclang", 'C:\Program Files\LLVM\bin') {
            if (Test-Path "$dir\libclang.dll") { $env:LIBCLANG_PATH = $dir; break }
        }
    }
    if (-not $env:LIBCLANG_PATH) {
        Write-Host 'libclang.dll not found. Install LLVM (winget install LLVM.LLVM) or run: .\install.ps1 -NoWhisper' -ForegroundColor Yellow
        exit 1
    }
}

$args = @('install', '--path', '.', '--locked', '--force')
if ($NoWhisper) { $args += '--no-default-features' }
& cargo @args
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host 'Done. Start it with: synq' -ForegroundColor Green
