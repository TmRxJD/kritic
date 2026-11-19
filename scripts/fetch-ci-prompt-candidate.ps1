param([switch]$Check)
if ($Check) { return }
$ErrorActionPreference = 'Stop'
if (-not $env:RUNNER_TEMP -or -not $env:GITHUB_REPOSITORY -or $env:CI_RUN_ID -notmatch '^[0-9]+$') {
    throw 'Set RUNNER_TEMP, GITHUB_REPOSITORY and numeric CI_RUN_ID for the intended CI run.'
}
$root = Join-Path $env:RUNNER_TEMP 'kritic-ci-candidate'
if (Test-Path -LiteralPath $root) { throw 'Candidate staging exists; preserve it and use a fresh runner.' }
gh run download $env:CI_RUN_ID --repo $env:GITHUB_REPOSITORY --name 'kritic-windows-test' --dir $root
if ($LASTEXITCODE -ne 0) { throw 'CI candidate download failed' }
$manifest = Get-Content -LiteralPath (Join-Path $root 'manifest.json') -Raw | ConvertFrom-Json
$sourceRun = gh api "repos/$env:GITHUB_REPOSITORY/actions/runs/$env:CI_RUN_ID" | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $sourceRun.conclusion -ne 'success') { throw 'Source CI must have passed' }
if ($manifest.ci_run_id -ne $env:CI_RUN_ID -or $manifest.head_sha -ne $sourceRun.head_sha) { throw 'CI source identity mismatch' }
$expected = $manifest.binaries.'kritic.exe'
if ($expected -notmatch '^[a-f0-9]{64}$') { throw 'CI manifest lacks a valid binary hash' }
if ((Get-FileHash -LiteralPath (Join-Path $root 'kritic.exe') -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
    throw 'CI binary SHA256 mismatch'
}
