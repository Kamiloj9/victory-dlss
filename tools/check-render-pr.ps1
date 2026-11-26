# Validate comparison evidence without executing text from the PR description.
$ErrorActionPreference = 'Stop'

function Test-ChangedPath([string]$Base, [string[]]$Paths) {
    # Git reports a difference via exit code 1. Check both sides of renames.
    $PSNativeCommandUseErrorActionPreference = $false
    & git diff --no-ext-diff --quiet --no-renames $Base HEAD -- @Paths
    switch ($LASTEXITCODE) {
        0 { return $false }
        1 { return $true }
        default { throw "Could not compare the tested merge against its base (git exit $LASTEXITCODE)." }
    }
}

function Get-EvidenceField([string]$Body, [string]$Label) {
    $match = [regex]::Match($Body, '(?m)^' + [regex]::Escape($Label) + ':[ \t]*([^\r\n]*)')
    $value = $match.Groups[1].Value.Trim()
    if (!$value) { throw "Fill in '$Label' in the PR description." }
    return $value
}

function Confirm-Evidence($Pr) {
    $body = [regex]::Replace([string]$Pr.body, '(?s)<!--.*?-->', '')
    foreach ($ref in @('base', 'head')) {
        $label = if ($ref -eq 'base') { 'Baseline commit' } else { 'Candidate commit' }
        $sha = (Get-EvidenceField $body $label).Trim([char]96)
        if ($sha -notmatch '^[0-9a-f]{7,40}$' -or
            !$Pr.$ref.sha.StartsWith($sha, [StringComparison]::OrdinalIgnoreCase)) {
            throw "'$label' must identify the current PR $ref commit."
        }
    }
    $null = Get-EvidenceField $body 'Scene and settings'
    if ((Get-EvidenceField $body 'Visual comparison') -notmatch 'https://[^\s<>)]+') {
        throw 'Link the baseline and candidate images or comparison report using HTTPS.'
    }
    $number = '([0-9]+(?:\.[0-9]+)?)'
    $times = [regex]::Match((Get-EvidenceField $body 'Frame times (median/p95 ms)'),
                            "^$number\s*/\s*$number\s*->\s*$number\s*/\s*$number$")
    if (!$times.Success) { throw 'Enter frame times as baseline median/p95 -> candidate median/p95 in ms.' }
    $before, $beforeP95, $after, $afterP95 = foreach ($i in 1..4) {
        $value = [decimal]0
        if (![decimal]::TryParse($times.Groups[$i].Value, [Globalization.NumberStyles]::AllowDecimalPoint,
                                  [Globalization.CultureInfo]::InvariantCulture, [ref]$value)) {
            throw 'Frame times must be finite decimal numbers within range.'
        }
        $value
    }
    if ($before -le 0 -or $beforeP95 -lt $before -or $after -le 0 -or $afterP95 -lt $after) {
        throw 'Frame times must be positive and p95 must be at least the median.'
    }
    if ($after -gt $before * 1.10d -or $afterP95 -gt $beforeP95 * 1.10d) {
        throw 'Median or p95 frame time regressed by more than 10% versus baseline.'
    }
    if ($body -notmatch '(?m)^- \[[xX]\] I verified that this change does not regress 3D rendering or frame pacing\.[ \t]*\r?$') {
        throw 'Resolve regressions and check the rendering verification box.'
    }
}

$run = $true
if ($env:GITHUB_EVENT_NAME -eq 'pull_request') {
    $pr = (Get-Content -Raw -LiteralPath $env:GITHUB_EVENT_PATH | ConvertFrom-Json).pull_request
    if ($pr.base.sha -notmatch '^[0-9a-f]{40}$') { throw 'Missing or invalid PR base SHA.' }
    $gate = @(
        '.github/workflows/shader-toolchain.yml', '.github/pull_request_template.md',
        'tools/check-render-pr.ps1', 'tools/check-render-pr.py', 'tools/shader-workflow.ps1'
    )
    $runtime = @('.', ':(exclude,icase,glob)**/*.md', ':(exclude,icase,glob)**/*.rst',
                 ':(exclude)LICENSE', ':(exclude).gitignore', ':(exclude).github/ISSUE_TEMPLATE')
    foreach ($path in $gate) { $runtime += ":(exclude,literal)$path" }
    if (Test-ChangedPath $pr.base.sha $runtime) {
        Confirm-Evidence $pr
    } else {
        $run = Test-ChangedPath $pr.base.sha $gate
        if ($run) { Write-Host 'Only the rendering gate changed. Running the GPU checks.' }
        else { Write-Host 'Only documentation or repository metadata changed.' }
    }
}
Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value ('render=' + $run.ToString().ToLowerInvariant()) -Encoding utf8
exit 0
