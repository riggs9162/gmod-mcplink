<#
.SYNOPSIS
Builds gmod-mcplink and installs it into a Garry's Mod directory on Windows.

.DESCRIPTION
Builds the native module for the 64-bit game, and for the 32-bit game when the i686 Rust target is installed, copies it into lua/bin for both realms, copies the Lua addon into addons/mcplink, and builds the MCP server.

.PARAMETER GameDir
The garrysmod directory of the game or dedicated server, the one that contains addons, lua and data.

.PARAMETER SkipServer
Leaves the MCP server alone; use this when installing into a second game directory.

.EXAMPLE
./tools/install.ps1 -GameDir "C:/Program Files (x86)/Steam/steamapps/common/GarrysMod/garrysmod"
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$GameDir,

    [switch]$SkipServer
)

$ErrorActionPreference = "Stop"

$repository = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $repository "module/Cargo.toml"
$binaries = Join-Path $GameDir "lua/bin"
$addon = Join-Path $GameDir "addons/mcplink"
$targets = @{
    "x86_64-pc-windows-msvc" = "win64"
    "i686-pc-windows-msvc"   = "win32"
}

if (-not (Test-Path (Join-Path $GameDir "addons"))) {
    throw "$GameDir does not look like a garrysmod directory: it has no addons folder."
}

$targetRoot = (cargo metadata --manifest-path $manifest --format-version 1 --no-deps | ConvertFrom-Json).target_directory
$installedTargets = rustup target list --installed

New-Item -ItemType Directory -Force $binaries | Out-Null

foreach ($target in $targets.Keys) {
    if ($installedTargets -notcontains $target) {
        Write-Host "Skipping ${target}: run 'rustup target add ${target}' to build it."
        continue
    }

    cargo build --release --manifest-path $manifest --target $target
    if ($LASTEXITCODE -ne 0) {
        throw "The module failed to build for $target."
    }

    $built = Join-Path $targetRoot "$target/release/mcplink.dll"
    foreach ($realm in "gmsv", "gmcl") {
        Copy-Item $built (Join-Path $binaries "${realm}_mcplink_$($targets[$target]).dll") -Force
    }
}

New-Item -ItemType Directory -Force $addon | Out-Null
Copy-Item (Join-Path $repository "addon/*") $addon -Recurse -Force

if (-not $SkipServer) {
    Push-Location (Join-Path $repository "server")
    try {
        npm install --no-audit --no-fund
        npm run build
        if ($LASTEXITCODE -ne 0) {
            throw "The MCP server failed to build."
        }
    }
    finally {
        Pop-Location
    }
}

$entry = (Join-Path $repository "server/dist/index.js") -replace "\\", "/"
Write-Host ""
Write-Host "Installed into $GameDir. Restart the game or change map, then register the server:"
Write-Host "  claude mcp add gmod --scope user --env GMOD_DIR=`"$($GameDir -replace '\\', '/')`" -- node `"$entry`""
