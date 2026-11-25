# The default run uses only synthetic inputs and keeps reports outside the checkout.
[CmdletBinding()]
param(
    [string]$ReportDirectory = (Join-Path ([System.IO.Path]::GetTempPath()) "victory-shaders-$PID"),
    [string[]]$CorpusPath,
    [string]$PixCapture,
    [string]$PixTool,
    [switch]$PixOnly
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'The shader workflow requires Windows.' }
if ($PixOnly -and (-not $PixCapture -or $CorpusPath)) {
    throw '-PixOnly requires -PixCapture and does not take -CorpusPath.'
}
if (Test-Path -LiteralPath $ReportDirectory) { throw "Report directory already exists: $ReportDirectory" }
$report = (New-Item -ItemType Directory -Path $ReportDirectory).FullName
$corpus = @(
    foreach ($path in $CorpusPath) { (Resolve-Path -LiteralPath $path).ProviderPath }
)
if ($PixCapture) { $PixCapture = (Resolve-Path -LiteralPath $PixCapture).ProviderPath }

function Invoke-Tool {
    param([string]$Tool, [string[]]$Arguments, [string]$Log)
    & $Tool @Arguments 2>&1 | Tee-Object -FilePath (Join-Path $report $Log)
    if ($LASTEXITCODE -ne 0) { throw "$Tool failed with exit code $LASTEXITCODE. See $Log." }
}

if ($PixCapture) {
    if (-not $PixTool) {
        $PixTool = (Get-Command pixtool.exe -ErrorAction SilentlyContinue).Source
    }
    if (-not $PixTool) {
        $pixHome = Join-Path $env:ProgramFiles 'Microsoft PIX'
        if (Test-Path -LiteralPath $pixHome) {
            $PixTool = Get-ChildItem -LiteralPath $pixHome -Directory |
                Where-Object Name -Match '^\d+\.\d+.*' |
                Sort-Object Name -Descending |
                ForEach-Object { Join-Path $_.FullName 'pixtool.exe' } |
                Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
                Select-Object -First 1
        }
    }
    if (-not $PixTool) { throw 'Install PIX on Windows or pass the path to pixtool.exe with -PixTool.' }
    Invoke-Tool $PixTool @('open-capture', $PixCapture, 'save-event-list', (Join-Path $report 'pix-events.csv'), '--counter-groups=D3D*') 'pix-replay.log'
    if ($PixOnly) {
        Write-Host "Shader report: $report"
        return
    }
}

$cargo = (Get-Command cargo.exe -CommandType Application -ErrorAction SilentlyContinue).Source
if (-not $cargo) {
    $cargo = Join-Path ([Environment]::GetFolderPath('UserProfile')) '.cargo/bin/cargo.exe'
    if (-not (Test-Path -LiteralPath $cargo -PathType Leaf)) { throw 'Install Rust with rustup.' }
}
$sdk = Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Windows Kits/10/bin'
$dxc = $env:VICTORY_PC_DXC
if (-not $dxc -and (Test-Path -LiteralPath $sdk)) {
    $dxc = Get-ChildItem -LiteralPath $sdk -Directory |
        Where-Object Name -Match '^10\.\d+\.\d+\.\d+$' |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64/dxc.exe' } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
        Select-Object -First 1
}
if (-not $dxc) { throw 'Install the Windows SDK or set VICTORY_PC_DXC to dxc.exe.' }
$dxc = (Resolve-Path -LiteralPath $dxc).ProviderPath
$dxil = $env:VICTORY_DXIL
if (-not $dxil) { $dxil = Join-Path (Split-Path -Parent $dxc) 'dxil.dll' }
$dxil = (Resolve-Path -LiteralPath $dxil).ProviderPath
if ($env:VICTORY_CONSOLE_DXC -and -not (Test-Path -LiteralPath $env:VICTORY_CONSOLE_DXC -PathType Leaf)) {
    throw 'VICTORY_CONSOLE_DXC must point to an installed Xbox compiler.'
}

$reference = Join-Path $report 'bindless-cube.hlsl'
$container = Join-Path $report 'bindless-cube.dxil'
@'
cbuffer Indices : register(b0) { uint cubeIndex; uint arrayIndex; uint samplerIndex; }
RWByteAddressBuffer output : register(u0);

[RootSignature("RootFlags(CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED | SAMPLER_HEAP_DIRECTLY_INDEXED), RootConstants(num32BitConstants=3, b0), UAV(u0)")]
[numthreads(4, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    TextureCube<float4> cube = ResourceDescriptorHeap[NonUniformResourceIndex(cubeIndex + id.x)];
    TextureCubeArray<float4> cubes = ResourceDescriptorHeap[NonUniformResourceIndex(arrayIndex + id.x)];
    SamplerState samplerState = SamplerDescriptorHeap[samplerIndex];
    float3 direction = float3(1, 0, 0);
    float4 color = cube.SampleLevel(samplerState, direction, 0)
                 + cubes.SampleLevel(samplerState, float4(direction, id.x), 0);
    output.Store4(16 * id.x, asuint(color));
}
'@ | Set-Content -LiteralPath $reference -Encoding ascii

Invoke-Tool $dxc @('--version') 'dxc-version.log'
Invoke-Tool $dxc @('-T', 'cs_6_6', '-E', 'main', '-WX', '-Fo', $container, $reference) 'dxc-compile.log'
Invoke-Tool $dxc @('-dumpbin', $container) 'bindless-cube.dxil.txt'

$stage = (New-Item -ItemType Directory -Path (Join-Path $report 'staging')).FullName
$options = @{
    VICTORY_PC_DXC = $dxc
    VICTORY_DXIL = $dxil
    # Shader tests select either WARP or hardware themselves. Forcing WARP
    # here would route hardware-only wave-size tests to the software adapter.
    VICTORY_WARP = '0'
    VICTORY_D3D12_WARP = '1'
    VICTORY_D3D12_DEBUG = '1'
    VICTORY_D3D12_GBV = '1'
    VICTORY_CARD_BUFFERS = '1'
    VICTORY_CARD_BUFFERS_MIN_MB = '1'
    VICTORY_CARD_STAGING_DIR = $stage
    VICTORY_CARD_WINDOWS_MB = '16'
    VICTORY_WINDOW_KB = '4096'
    VICTORY_RELAY = '0'
    VICTORY_LATE_WAIT = '0'
    VICTORY_QUEUE2 = '0'
    VICTORY_INPLACE = '0'
}
$previous = @{}
Push-Location (Join-Path $PSScriptRoot '..')
try {
    foreach ($name in $options.Keys) {
        $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        [Environment]::SetEnvironmentVariable($name, $options[$name], 'Process')
    }

    Invoke-Tool $cargo @('test', '--locked', '-p', 'victory-shader', '--lib', '--', '--test-threads=1') 'shader-tests.log'
    $gpu = @('test', '--locked', '-p', 'victory-gpu', '--features', 'diagnostics', '--lib')
    Invoke-Tool $cargo ($gpu + @('descriptor::', '--', '--test-threads=1')) 'descriptor-tests.log'
    Invoke-Tool $cargo ($gpu + @('warp_file_backed_loose_window_roundtrips_gpu_writes', '--', '--ignored', '--nocapture')) 'warp-gpu-validation.log'
    if ($corpus.Count) {
        $env:VICTORY_WARP = '1'
        Invoke-Tool $cargo (@('run', '--locked', '-p', 'victory', '--', 'translate', '--validate', '--pipelines', '--list', '--') + $corpus) 'local-corpus.log'
    }
} finally {
    foreach ($name in $previous.Keys) {
        [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process')
    }
    Pop-Location
}

Write-Host "Shader report: $report"
