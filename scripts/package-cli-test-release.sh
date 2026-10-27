#!/usr/bin/env bash
# Build on Apple Silicon from the tagged tree; package only tracked source and verified CLI binaries.
set -euo pipefail
bin="${1:-target/aarch64-apple-darwin/release}"
archive=kritic-cli-macos-arm64-test.tar.gz
stage=package-mac/kritic
if [[ -e package-mac || -e "$archive" ]]; then
  echo 'Release staging already exists; use a fresh checkout.' >&2
  exit 1
fi
[[ "$(uname -m)" == arm64 ]] || { echo 'Apple Silicon packaging requires arm64.' >&2; exit 1; }
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
revision=$(git rev-parse HEAD)
if [[ "${GITHUB_REF_TYPE:-}" == tag && "${GITHUB_REF_NAME:-}" != "v$version" ]]; then
  echo 'Release tag does not match package version.' >&2
  exit 1
fi
for name in kev kev-filter; do
  [[ "$("$bin/$name" --version)" == "kev-filter $version" ]] || { echo "CLI version mismatch: $name" >&2; exit 1; }
done
mkdir -p "$stage/bin"
git archive HEAD --format=tar --output=package-mac/source.tar
tar -xf package-mac/source.tar -C "$stage"
rm -- package-mac/source.tar
cp -- "$bin/kev" "$bin/kev-filter" "$stage/bin/"
printf '%s\n' "$revision" > "$stage/SOURCE-REVISION.txt"
python3 - "$stage" "$version" "$revision" <<'PY'
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
manifest = dict(version=sys.argv[2], source_revision=sys.argv[3], platform='macos-arm64',
                distribution='CLI binaries and source; Kritic MCP server requires a source build',
                binaries={f'bin/{name}': hashlib.sha256((root/'bin'/name).read_bytes()).hexdigest()
                          for name in ('kev', 'kev-filter')})
(root/'RELEASE-MANIFEST.json').write_text(json.dumps(manifest, indent=2)+'\n')
PY
tar -czf "$archive" -C package-mac kritic
