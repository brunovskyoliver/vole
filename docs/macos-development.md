# Build and launch on macOS

Clone the repository using SSH (or its HTTPS URL):

```sh
git clone git@github.com:brunovskyoliver/vole.git
cd vole
```

Install full Xcode and [Homebrew](https://brew.sh) first. Open Xcode once to
finish its license/component setup. Command Line Tools alone are insufficient
for this GPUI build: the pinned Apple backend compiles Metal shaders.

```sh
make mac-setup
make mac-run
```

`mac-setup` installs Homebrew Python, CMake, Ninja, pkgconf, LLVM and LLD, and
the repository's pinned Rust toolchain with rustfmt and Clippy. It installs
Homebrew rustup if no existing rustup is available. It does not change your
global Rust default. If Metal tools are missing, it clears the `xcrun` discovery
cache, downloads Xcode's Metal Toolchain component and checks both `metal` and
`metallib` again. A failed download or unresolved tool remains a setup failure.
LLVM and LLD are separate Homebrew formulae; their paths
are discovered without requiring changes to your shell profile. Homebrew LLVM
also provides the Clang used for C documents and its freestanding headers.
Xcode's Apple clang cannot build the guest ELF images, so `mac-check` rejects
it with instructions if it is the only clang found.
[LLVM formula](https://formulae.brew.sh/formula/llvm),
[LLD formula](https://formulae.brew.sh/formula/lld),
[rustup formula](https://formulae.brew.sh/formula/rustup).

`mac-run` checks prerequisites, builds the app and CLI for your native Rust
host, creates `target/macos/debug/Vole.app`, signs it locally with an ad hoc
signature, and launches it through macOS LaunchServices. Apple Silicon and
Intel hosts are supported by the same commands. Each launch opens a new
instance so the latest build is used; close earlier instances before rebuilding.
No process is stopped automatically.

The app bundle includes LLVM and Clang launchers under `Contents/Resources/toolchain`, so
Finder and Dock launches do not depend on terminal environment variables.
These launchers reference the installed tools on this Mac, preserving their
Homebrew library paths. Keep that local toolchain installed. This bundle is
for local development. Use the existing static LLVM toolchain
builder and packager for redistribution.

| Command | Result |
| --- | --- |
| `make mac-check` | Check native tools, Xcode, Metal and Rust |
| `make mac-build` | Build the local `.app` without opening it |
| `make mac-run PROFILE=release` | Build and launch an optimized `.app` |
| `make mac-verify` | Run formatting, Rust tests, Clippy, bundle checks, all five guest round trips and C on the four real targets |
| `make test-macos-script` | Test build orchestration on any host |

`CARGO_TARGET_DIR` is respected; the app is created inside its `macos/<profile>`
directory. `PYTHON=/absolute/path/to/python3` overrides Python discovery.
Python 3.11+ is required. `VOLE_LLVM_MC`, `VOLE_LLD`, `VOLE_CLANG` and
`VOLE_TOOLCHAIN_DIR` can select another toolchain; the latter can name its
prefix or `bin` folder. The explicit executable overrides take precedence.

If Xcode is installed but its developer directory is not selected:

```sh
sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
```

If you cloned before Metal component installation was added to setup:

```sh
git pull --ff-only
make mac-setup
make mac-run
```

To install the additional component manually and retry:

```sh
xcodebuild -downloadComponent MetalToolchain
make mac-check
make mac-run
```

See [Apple's Xcode component instructions](https://developer.apple.com/documentation/xcode/downloading-and-installing-additional-xcode-components).

`mac-verify` records guest results beside the bundle in
`engine-verification.json`. It proves guest assembly and execution, not window
layout or AeroSpace tiling. Native macOS launch, traffic lights, Retina,
fullscreen, Dock reopen and tiling still require the
[manual host acceptance checks](native-verification.md).
