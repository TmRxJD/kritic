# Run from the repository root after cargo build --release --locked.
param([switch]$Check)
# PowerShell parses the entire script before executing this no-op syntax check.
if ($Check) { return }
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Path package/kritic/bin/harness -Force | Out-Null
git archive HEAD --format=tar --output=package/source.tar
if ($LASTEXITCODE -ne 0) { throw 'git archive failed' }
tar -xf package/source.tar -C package/kritic
if ($LASTEXITCODE -ne 0) { throw 'source extraction failed' }
Remove-Item -LiteralPath package/source.tar
Copy-Item target/release/kritic.exe package/kritic/bin/
# Tracked docs, examples and skills already came from git archive.
Copy-Item harness/py package/kritic/bin/harness/ -Recurse
Compress-Archive -Path package/kritic -DestinationPath kritic-windows-x86_64-test.zip
