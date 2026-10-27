# Called after all Rust CI checks; this artifact is an unreleased test build.
$ErrorActionPreference = 'Stop'
$artifactDir = Join-Path $env:RUNNER_TEMP 'kritic-windows-test'
New-Item -ItemType Directory -Path $artifactDir -Force | Out-Null
Copy-Item -LiteralPath target/debug/kritic.exe -Destination "$artifactDir/kritic.exe"
$binaries = [ordered]@{}
foreach ($name in @('kritic.exe', 'kev.exe', 'kev-filter.exe')) {
    Copy-Item -LiteralPath "target/debug/$name" -Destination "$artifactDir/$name"
    $binaries[$name] = (Get-FileHash -LiteralPath "$artifactDir/$name" -Algorithm SHA256).Hash.ToLowerInvariant()
}
[ordered]@{
    head_sha = $env:PR_HEAD_SHA
    checkout_sha = (git rev-parse HEAD)
    ci_run_id = $env:GITHUB_RUN_ID
    filename = 'kritic.exe'
    platform = 'windows-x86_64'
    sha256 = (Get-FileHash -LiteralPath "$artifactDir/kritic.exe" -Algorithm SHA256).Hash.ToLowerInvariant()
    binaries = $binaries
} | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath "$artifactDir/manifest.json" -Encoding utf8
