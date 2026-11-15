#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
arch="$(uname -m)"
case "$arch" in
  arm64) core_hash=060b64b0f2c9a310831db0fe4adb2878de67bfa606c311813c1f13966aa504b6 ;;
  x86_64) arch=amd64; core_hash=551ae54ef8d77a5e21f7a8185a59b441fb13a0fb0514fcdef5ca2816f6f50015 ;;
  *) echo "Unsupported macOS architecture: $arch" >&2; exit 1 ;;
esac
work="$root/artifacts/macos"
downloads="$work/downloads"
mkdir -p "$downloads"
download() {
  local url="$1" dest="$2" hash="$3"
  if ! test -f "$dest" || ! echo "$hash  $dest" | shasum -a 256 -c - >/dev/null 2>&1; then
    curl -fL --retry 3 "$url" -o "$dest.partial"
    echo "$hash  $dest.partial" | shasum -a 256 -c -
    mv "$dest.partial" "$dest"
  fi
}
core="mihomo-darwin-$arch-v1.19.33.gz"
source="mihomo-v1.19.33-source.zip"
download "https://github.com/MetaCubeX/mihomo/releases/download/v1.19.33/$core" "$downloads/$core" "$core_hash"
download "https://codeload.github.com/MetaCubeX/mihomo/zip/refs/tags/v1.19.33" "$downloads/$source" bb705aa873fc0e9db352fdc69b992c93fad5b310756e0e5f81194a0e0c108e21
cargo build --release --locked -p multilink-app
version="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "multilink-app"))')"
name="EvertyMultiLink-$version-macos-$arch${MULTILINK_PACKAGE_SUFFIX:-}"
stage="$work/$name"
app="$stage/Everty MultiLink.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/Licenses"
cp target/release/multilink-app "$app/Contents/MacOS/"
gzip -dc "$downloads/$core" > "$app/Contents/MacOS/mihomo"
chmod 755 "$app/Contents/MacOS/"*
cp LICENSE "$app/Contents/Resources/Licenses/Everty-MultiLink.LICENSE.txt"
cp assets/meta-rules-dat.LICENSE "$app/Contents/Resources/Licenses/Meta-Rules-Dat.LICENSE.txt"
cp installer/Third-party.txt "$app/Contents/Resources/Licenses/"
cp "$downloads/$source" "$app/Contents/Resources/Licenses/"
unzip -p "$downloads/$source" mihomo-1.19.33/LICENSE > "$app/Contents/Resources/Licenses/Mihomo.LICENSE.txt"
cp docs/macos.md "$stage/README.md"
cp docs/macos.md "$app/Contents/Resources/README.md"
iconset="$work/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" logo.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" logo.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
python3 - "$app/Contents/Info.plist" "$version" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'wb') as f:
    plistlib.dump(dict(CFBundleName='Everty MultiLink', CFBundleDisplayName='Everty MultiLink',
        CFBundleIdentifier='ru.everty.multilink', CFBundleExecutable='multilink-app',
        CFBundlePackageType='APPL', CFBundleShortVersionString=sys.argv[2],
        CFBundleVersion=sys.argv[2], CFBundleIconFile='AppIcon', LSMinimumSystemVersion='11.0',
        CFBundleDevelopmentRegion='ru', CFBundleLocalizations=['ru', 'en', 'de', 'es', 'zh'],
        LSApplicationCategoryType='public.app-category.utilities',
        NSHighResolutionCapable=True, NSLocalNetworkUsageDescription='Обмен подписками, поиск и подключение к LAN-хосту Everty.'), f)
PY
# Local ad-hoc signature. Distribution signing and notarization require Developer ID.
codesign --force --sign - "$app/Contents/MacOS/mihomo"
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
ditto -c -k --sequesterRsrc --keepParent "$stage" "$work/$name.zip"
(cd "$work" && shasum -a 256 "$name.zip" > "$name.zip.sha256")
echo "$work/$name.zip"
