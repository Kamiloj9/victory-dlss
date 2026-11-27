# Launch the measured RTX 4080 SUPER Series S profile.
[CmdletBinding(SupportsShouldProcess)]
param(
    [Parameter(Mandatory)][string]$GameExe,
    [string]$VictoryExe,
    [string]$StateDir = (Join-Path $env:LOCALAPPDATA 'VIctory/rtx-4080-super')
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (!$VictoryExe) { $VictoryExe = Join-Path $root 'target/diag/victory.exe' }
$game = (Resolve-Path -LiteralPath $GameExe).ProviderPath
$victory = (Resolve-Path -LiteralPath $VictoryExe).ProviderPath
if (!(Test-Path -LiteralPath $game -PathType Leaf) -or !(Test-Path -LiteralPath $victory -PathType Leaf)) {
    throw 'GameExe and VictoryExe must point to files.'
}
$state = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($StateDir)
$settings = @{
    VICTORY_AMBIENT = '0.8'
    VICTORY_CARD_BUFFERS_MB = '1536'
    VICTORY_CARD_BUFFERS_MIN_KB = '64'
    VICTORY_CARD_REHASH_RUNS = '1'
    VICTORY_CARD_RESERVE_BUDGETS = '1'
    VICTORY_CARD_STEPS = '2'
    VICTORY_CARD_WINDOWS_MB = '1024'
    VICTORY_D3D12_DRED = 'light'
    VICTORY_DYNAMIC_SLOTS = '524288'
    VICTORY_INDIRECT_BATCH = '0'
    VICTORY_INDIRECT_WINDOW = '1'
    VICTORY_JOIN_COPIES = '1'
    VICTORY_JOIN_PARTS = '8'
    VICTORY_LAND_THREADS = '4'
    VICTORY_MAP_THREADS = '1'
    VICTORY_OWN_UPSCALER = '1'
    VICTORY_PIPELINE_WORKERS = '3'
    VICTORY_PLACED_ORDER = '1'
    VICTORY_RELAY = '2'
    VICTORY_RELAY_COPY = '1'
    VICTORY_SHOW_AFTER = '1'
    VICTORY_SKIP_PIPELINES = '2da4beb386509b77'
    VICTORY_TEXTURE_NO_WAIT = '1'
    VICTORY_THIN = '1'
    VICTORY_THIN_THREADS = '8'
    VICTORY_TITLE_PRIORITIES = '0'
    VICTORY_UNDER_THREADS = '6'
    VICTORY_UPLOAD_AHEAD = '1'
    VICTORY_UPSCALER = '6'
    VICTORY_VSYNC = '1'
    VICTORY_WIDE_BACK = '1'
    VICTORY_WORKING_SET_MB = '8192'
    VICTORY_GAME_DIR = Split-Path -Parent $game
    VICTORY_CACHE_DIR = Join-Path $state 'cache'
    VICTORY_DATA_DIR = Join-Path $state 'data'
    VICTORY_PHYSICAL_BACKING_DIR = Join-Path $state 'physical'
    VICTORY_LOG = 'info'
}
if (!$PSCmdlet.ShouldProcess($game, "Launch the RTX 4080 SUPER Series S profile using $victory")) { return }
foreach ($key in 'VICTORY_CACHE_DIR', 'VICTORY_DATA_DIR', 'VICTORY_PHYSICAL_BACKING_DIR') {
    [void][IO.Directory]::CreateDirectory($settings[$key])
}
$previous = @{}
Get-ChildItem Env: | Where-Object { $_.Name -like 'VICTORY_*' -or $_.Name -eq 'RUST_LOG' } | ForEach-Object {
    $previous[$_.Name] = $_.Value
}
try {
    foreach ($key in $previous.Keys) { Remove-Item -LiteralPath "Env:$key" }
    foreach ($key in $settings.Keys) { Set-Item -LiteralPath "Env:$key" -Value $settings[$key] }
    & $victory run $game --console series-s --execute
    if ($LASTEXITCODE -ne 0) { throw "VIctory exited with code $LASTEXITCODE." }
} finally {
    foreach ($key in $settings.Keys) { Remove-Item -LiteralPath "Env:$key" -ErrorAction SilentlyContinue }
    foreach ($key in $previous.Keys) { Set-Item -LiteralPath "Env:$key" -Value $previous[$key] }
}
