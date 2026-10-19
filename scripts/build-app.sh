#!/bin/bash
# Builds build/app.noindex/LiteMail.app (Rust core + SwiftUI app), ad-hoc signed.
#   scripts/build-app.sh            build
#   scripts/build-app.sh --install  build and copy to ~/Applications
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/build/app.noindex/LiteMail.app"
VERSION="1.0.0"

"$ROOT/scripts/build-core.sh"

cd "$ROOT/app"
swift build -c release --product LiteMail
BIN="$(swift build -c release --show-bin-path)/LiteMail"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/Fonts"
cp "$BIN" "$APP/Contents/MacOS/LiteMail"
# English (development language) and Simplified Chinese strings
cp -R "$ROOT/app/Resources/en.lproj" "$ROOT/app/Resources/zh-Hans.lproj" "$APP/Contents/Resources/"
# Source Serif 4 (SIL Open Font License) for headings and reading
cp "$ROOT/app/Fonts/"*.ttf "$ROOT/app/Fonts/OFL.txt" "$APP/Contents/Resources/Fonts/"
strip -x "$APP/Contents/MacOS/LiteMail"

# Asset catalog: AccentColor (terracotta) — macOS uses it for sidebar selection and controls
# when the user's accent colour is "Multicolor" (the default).
xcrun actool --compile "$APP/Contents/Resources" --platform macosx --minimum-deployment-target 26.0 \
  --output-partial-info-plist "$ROOT/build/assets-info.plist" "$ROOT/app/Resources/Assets.xcassets" >/dev/null

# Icon
ICONSET="$ROOT/build/AppIcon.iconset"
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
swift "$ROOT/Tools/make-icon.swift" "$ROOT/build/icon-1024.png" >/dev/null
for s in 16 32 128 256 512; do
  sips -z $s $s "$ROOT/build/icon-1024.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d "$ROOT/build/icon-1024.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>uk.icoco.LiteMail</string>
  <key>CFBundleName</key><string>LiteMail</string>
  <key>CFBundleDisplayName</key><string>LiteMail</string>
  <key>CFBundleExecutable</key><string>LiteMail</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>NSAccentColorName</key><string>AccentColor</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$(date +%Y%m%d%H%M)</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleLocalizations</key><array><string>en</string><string>zh-Hans</string></array>
  <key>CFBundleAllowMixedLocalizations</key><true/>
  <key>LSMinimumSystemVersion</key><string>26.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>LiteMail — a light, native mail app</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>Email Address URL</string>
      <key>CFBundleURLSchemes</key><array><string>mailto</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST

# Sign with a stable identity. An ad-hoc signature changes with every build, and the keychain
# then refuses the new build and asks for the account password again. A Developer ID (or Apple
# Development) signature keeps the same designated requirement (bundle id + team) across builds.
# Override with LITEMAIL_SIGN_IDENTITY (a certificate name or SHA-1, or "-" for ad-hoc).
pick_identity() { security find-identity -v -p codesigning 2>/dev/null | grep "$1" | head -1 | awk '{print $2}'; }
IDENTITY="${LITEMAIL_SIGN_IDENTITY:-$(pick_identity 'Developer ID Application')}"
[ -n "$IDENTITY" ] || IDENTITY="$(pick_identity 'Apple Development')"
[ -n "$IDENTITY" ] || IDENTITY="-"
codesign --force --sign "$IDENTITY" --timestamp=none "$APP"
codesign --verify --strict "$APP"
# Keep the build copy out of Launchpad / Spotlight: only ~/Applications should be listed.
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -u "$APP" 2>/dev/null || true
echo "built $APP ($(du -sh "$APP" | cut -f1))"

if [ "${1:-}" = "--install" ]; then
  mkdir -p "$HOME/Applications"
  pkill -x LiteMail 2>/dev/null || true
  rm -rf "$HOME/Applications/LiteMail.app"
  cp -R "$APP" "$HOME/Applications/"
  echo "installed to ~/Applications/LiteMail.app"
fi
