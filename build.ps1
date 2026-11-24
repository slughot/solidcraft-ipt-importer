# Builds the SolidCraft plugin package: plugin/ipt-importer.wasm and dist/ipt-importer-<version>.zip.
# Needs: Rust with the wasm32-unknown-unknown target, wasm-tools (cargo install wasm-tools --locked).
# -Check also runs `solidcraft-cli plugin check`, and `plugin test` when test .ipt files are present.
param([switch]$Check)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

# Rust embeds source paths in panic messages. Remap the builder's Cargo home, rustup home and this
# folder to neutral names so the published .wasm names no local folders. Paths are found at run
# time, so none is stored in the repository. CARGO_ENCODED_RUSTFLAGS replaces .cargo/config.toml's
# rustflags, so it repeats the getrandom backend cfg.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $HOME '.rustup' }
$flags = @('--cfg', 'getrandom_backend="custom"')
if (Test-Path $cargoHome) { $flags += "--remap-path-prefix=$((Resolve-Path $cargoHome).Path)=/cargo" }
if (Test-Path $rustupHome) { $flags += "--remap-path-prefix=$((Resolve-Path $rustupHome).Path)=/rustup" }
$flags += "--remap-path-prefix=$PSScriptRoot=/build"
$env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f
try {
    cargo build --release --target wasm32-unknown-unknown
    if ($LASTEXITCODE) { throw 'cargo build failed' }
} finally {
    Remove-Item Env:CARGO_ENCODED_RUSTFLAGS
}

# Refuse to package a binary that still names a local folder.
$bytes = [IO.File]::ReadAllBytes("$PSScriptRoot/target/wasm32-unknown-unknown/release/ipt_importer_src.wasm")
$text = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
foreach ($p in @($HOME, $cargoHome, $PSScriptRoot, $env:USERNAME) | Where-Object { $_ }) {
    if ($text.IndexOf($p, [StringComparison]::OrdinalIgnoreCase) -ge 0) { throw "the built .wasm still contains a local path or name; not packaging it" }
}
wasm-tools component new target/wasm32-unknown-unknown/release/ipt_importer_src.wasm -o plugin/ipt-importer.wasm
if ($LASTEXITCODE) { throw 'wasm-tools component new failed' }

$version = (Get-Content plugin/plugin.json -Raw | ConvertFrom-Json).version
New-Item -ItemType Directory -Force dist | Out-Null
$zip = "dist/ipt-importer-$version.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path plugin/plugin.json, plugin/ipt-importer.wasm -DestinationPath $zip
Write-Host "built $zip"

if ($Check) {
    $cli = Join-Path $env:ProgramFiles 'SolidCraft\solidcraft-cli.exe'
    & $cli plugin check plugin
    if (Get-ChildItem plugin/tests -Filter *.ipt -ErrorAction SilentlyContinue) {
        & $cli plugin test plugin
    } else {
        Write-Host 'plugin test skipped: no .ipt files in plugin/tests (see README, Tests)'
    }
}
