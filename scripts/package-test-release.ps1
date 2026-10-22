# Run from the repository root after cargo build --release --locked.
param([switch]$Check)
# PowerShell parses the entire script before executing this no-op syntax check.
if ($Check) { return }
$ErrorActionPreference = 'Stop'
if ((Test-Path package) -or (Test-Path kritic-windows-x86_64-test.zip)) {
    throw 'Release staging already exists. Use a fresh checkout to avoid packaging stale files.'
}
$revision = (git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'source revision lookup failed' }
$versionLine = Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
$version = $versionLine.Matches[0].Groups[1].Value
$binaryVersion = (& ./target/release/kritic.exe --version | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $binaryVersion -ne "kritic $version") {
    throw "Built binary version mismatch: expected kritic $version; got $binaryVersion"
}
if ($env:GITHUB_REF_TYPE -eq 'tag' -and $env:GITHUB_REF_NAME -ne "v$version") {
    throw 'Release tag does not match the Cargo package version.'
}
New-Item -ItemType Directory -Path package/kritic/bin/harness -Force | Out-Null
git archive HEAD --format=tar --output=package/source.tar
if ($LASTEXITCODE -ne 0) { throw 'git archive failed' }
tar -xf package/source.tar -C package/kritic
if ($LASTEXITCODE -ne 0) { throw 'source extraction failed' }
Remove-Item -LiteralPath package/source.tar
Copy-Item target/release/kritic.exe package/kritic/bin/
# Copy only canonical tracked harness files, never working-tree caches.
Copy-Item package/kritic/harness/py package/kritic/bin/harness/ -Recurse
$binaryHash = (Get-FileHash package/kritic/bin/kritic.exe -Algorithm SHA256).Hash.ToLowerInvariant()
@{ version = $version; source_revision = $revision; platform = 'windows-x86_64'; binary_sha256 = $binaryHash } |
    ConvertTo-Json | Set-Content package/kritic/RELEASE-MANIFEST.json -Encoding utf8
Set-Content package/kritic/SOURCE-REVISION.txt -Value $revision -Encoding ascii
Compress-Archive -Path package/kritic -DestinationPath kritic-windows-x86_64-test.zip