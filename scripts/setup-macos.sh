#!/usr/bin/env bash
# Explicit local development setup; full Xcode and Homebrew must exist first.
set -euo pipefail

if [[ $(uname -s) != Darwin ]]; then
    echo 'mac-setup requires macOS. On Debian/Ubuntu use scripts/setup-linux.sh.' >&2
    exit 1
fi

# Locate Homebrew even in a fresh shell whose profile has not been reloaded.
if ! command -v brew >/dev/null; then
    for prefix in /opt/homebrew /usr/local; do
        if [[ -x "$prefix/bin/brew" ]]; then
            export PATH="$prefix/bin:$PATH"
            break
        fi
    done
fi
if ! command -v brew >/dev/null; then
    echo 'Install Homebrew from https://brew.sh, then run make mac-setup again.' >&2
    exit 1
fi
if ! xcodebuild -version >/dev/null 2>&1; then
    echo 'Install full Xcode, open it to finish setup, and select its developer directory.' >&2
    echo 'For the standard location: sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer' >&2
    exit 1
fi

brew install python cmake ninja pkgconf llvm lld
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$(brew --prefix)/bin:$PATH"
if ! command -v rustup >/dev/null; then
    brew install rustup
    export PATH="$(brew --prefix rustup)/bin:$PATH"
fi

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_root"
toolchain=$(sed -n 's/^channel = "\([^"]*\)"/\1/p' rust-toolchain.toml)
[[ -n "$toolchain" ]] || { echo 'Cannot read the pinned Rust toolchain.' >&2; exit 1; }
rustup toolchain install "$toolchain" --profile minimal --component rustfmt,clippy
python3 scripts/macos.py setup
