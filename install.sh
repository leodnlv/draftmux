#!/bin/sh
# Installs the latest Draftmux release for this user:
#
#   curl -fsSL https://raw.githubusercontent.com/raine/draftmux/main/install.sh | sh
#
# macOS (Apple silicon): Draftmux.app in /Applications, or ~/Applications when
# /Applications isn't writable, after checking its Developer ID signature.
# Linux (x86_64, arm64): the app tree in ~/.local/lib/draftmux. Both link
# the draftmux command into ~/.local/bin. Running it again updates Draftmux.
#
# DRAFTMUX_VERSION picks a release instead of the latest,
# DRAFTMUX_INSTALL_PREFIX replaces ~/.local, and DRAFTMUX_APP_DIR picks the
# folder for Draftmux.app on macOS.
set -eu

releases="${DRAFTMUX_RELEASES_URL:-https://github.com/raine/draftmux/releases}"
team=YG824X87Y2
bundle_id=fi.zendit.Draftmux

fail() {
  echo "draftmux install: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "$1 is required"
}

latest_version() {
  url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$releases/latest") ||
    fail "couldn't reach $releases"
  case $url in
    */tag/v*) echo "${url##*/tag/v}" ;;
    *) fail "no release found at $releases" ;;
  esac
}

# Downloads NAME and its checksum file CHECKSUM into the working directory
# and checks one against the other.
fetch() {
  curl -fsSL -o "$1" "$releases/download/v$version/$1" || fail "couldn't download $1"
  curl -fsSL -o "$2" "$releases/download/v$version/$2" || fail "couldn't download $2"
  expected=$(awk '{ print $1; exit }' "$2")
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$1" | awk '{ print $1 }')
  else
    actual=$(shasum -a 256 "$1" | awk '{ print $1 }')
  fi
  if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
    fail "checksum mismatch for $1"
  fi
}

install_macos() {
  [ "$(uname -m)" = arm64 ] || fail "Draftmux for macOS needs an Apple silicon Mac"
  need codesign
  dir="${DRAFTMUX_APP_DIR:-/Applications}"
  if [ -z "${DRAFTMUX_APP_DIR:-}" ] && [ ! -w "$dir" ]; then dir="$HOME/Applications"; fi
  target="$dir/Draftmux.app"
  bin="${DRAFTMUX_INSTALL_PREFIX:-$HOME/.local}/bin"
  link="$bin/draftmux"
  # Only ever replace a previous Draftmux install, checked before anything
  # is written.
  if [ -L "$target" ]; then
    fail "refusing to replace $target, which isn't Draftmux"
  elif [ -e "$target" ]; then
    existing=$(defaults read "$target/Contents/Info" CFBundleIdentifier 2>/dev/null || true)
    [ "$existing" = "$bundle_id" ] || fail "refusing to replace $target, which isn't Draftmux"
  fi
  if [ -e "$link" ] || [ -L "$link" ]; then
    case $(readlink "$link" || true) in
      */Draftmux.app/Contents/MacOS/draftmux) ;;
      *) fail "refusing to replace $link, which isn't Draftmux's command" ;;
    esac
  fi

  name="Draftmux-$version-arm64-app"
  fetch "$name.tar.gz" "$name.sha256"
  mkdir extracted
  tar -xzf "$name.tar.gz" -C extracted
  app=extracted/Draftmux.app
  codesign --verify --deep --strict "$app" 2>/dev/null || fail "Draftmux.app's signature is invalid"
  codesign --verify -R="anchor apple generic and certificate leaf[subject.OU] = \"$team\"" \
    "$app" 2>/dev/null || fail "Draftmux.app isn't signed by Draftmux's developer"

  mkdir -p "$dir" "$bin"
  # Staged next to the target so the swap is a rename, and swapped in so a
  # failed copy or move leaves the installed app in place.
  staging=$(mktemp -d "$dir/.Draftmux-install.XXXXXX")
  ditto "$app" "$staging/Draftmux.app"
  if [ -e "$target" ]; then mv "$target" "$staging/Draftmux.app.old"; fi
  if ! mv "$staging/Draftmux.app" "$target"; then
    if [ -e "$staging/Draftmux.app.old" ]; then mv "$staging/Draftmux.app.old" "$target"; fi
    fail "couldn't move Draftmux.app into $dir"
  fi
  rm -rf "$staging"
  staging=

  if [ -e "$link" ] || [ -L "$link" ]; then rm "$link"; fi
  ln -s "$target/Contents/MacOS/draftmux" "$link"
  echo "Installed Draftmux $version in $dir and linked $link"
  case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "Add $bin to your PATH to use the draftmux command." ;;
  esac
}

install_linux() {
  case $(uname -m) in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "no Linux build for $(uname -m)" ;;
  esac
  name="Draftmux-$version-linux-$arch"
  fetch "$name.tar.gz" "$name.sha256"
  tar -xzf "$name.tar.gz"
  draftmux-linux/install
  echo "Installed Draftmux $version"
}

main() {
  need curl
  need tar
  version="${DRAFTMUX_VERSION:-$(latest_version)}"
  version="${version#v}"
  staging=
  work=$(mktemp -d)
  trap 'rm -rf "$work" ${staging:+"$staging"}' EXIT
  trap 'exit 1' HUP INT TERM
  cd "$work"
  case $(uname -s) in
    Darwin) install_macos ;;
    Linux) install_linux ;;
    *) fail "Draftmux runs on macOS and Linux" ;;
  esac
}

# Runs only once the whole script has downloaded.
main
