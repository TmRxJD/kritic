# Kept as PowerShell source so shell-specific YAML scanners do not parse it as Bash.
param([switch]$Check, [switch]$UseCiCandidate, [string]$Kritic = '', [string]$WorkDirectory = '')
if ($Check) { return }
$ErrorActionPreference = 'Stop'
if ($UseCiCandidate) {
    if (-not $env:RUNNER_TEMP) { throw 'UseCiCandidate requires RUNNER_TEMP.' }
    $Kritic = Join-Path $env:RUNNER_TEMP 'kritic-ci-candidate/kritic.exe'
}
if (-not $Kritic -or -not (Test-Path -LiteralPath $Kritic -PathType Leaf)) {
    throw 'Pass -Kritic with the built Windows executable path.'
}
$Kritic = (Resolve-Path -LiteralPath $Kritic).Path
if (-not $WorkDirectory) {
    if (-not $env:RUNNER_TEMP) { throw 'Pass -WorkDirectory with a fresh permanent test destination.' }
    $WorkDirectory = Join-Path $env:RUNNER_TEMP 'kritic-prompt-live'
}
if (-not [IO.Path]::IsPathRooted($WorkDirectory)) { throw 'WorkDirectory must be absolute.' }
$parent = Split-Path -Parent $WorkDirectory
New-Item -ItemType Directory -Path $parent -Force | Out-Null
$archive = "$WorkDirectory.ollama-v0.40.16.zip"
$runtime = "$WorkDirectory.ollama-v0.40.16"
if (Test-Path -LiteralPath $runtime) { throw 'Runtime staging already exists; preserve it and choose a fresh work directory.' }
if (-not (Test-Path -LiteralPath $archive)) {
    Invoke-WebRequest -Uri 'https://github.com/ollama/ollama/releases/download/v0.40.16/ollama-windows-amd64.zip' -OutFile $archive -TimeoutSec 300
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne '50fcf5ad54277f2fa5d173ebfc93bcb017adbc0c9ef2314daabc96ca9e6aa64e') {
    throw 'Ollama archive SHA256 mismatch; existing bytes were preserved.'
}
Expand-Archive -LiteralPath $archive -DestinationPath $runtime
$ollama = Join-Path $runtime 'ollama.exe'
python scripts/smoke-prompt-model.py --kritic $Kritic --ollama $ollama --work-dir $WorkDirectory --model 'qwen2.5:1.5b'
if ($LASTEXITCODE -ne 0) { throw 'CPU prompt inference smoke failed; inspect evidence.json and retained logs.' }
if ($UseCiCandidate) {
    Copy-Item -LiteralPath (Join-Path $env:RUNNER_TEMP 'kritic-ci-candidate/manifest.json') -Destination (Join-Path $WorkDirectory 'ci-source.json')
}
