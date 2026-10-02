# Dependency notices and packaging

Vole's simulation engines are project-owned Rust interpreters. Unicorn and
Keystone are not included. Real-ISA assembly uses LLVM MC and LLD; Capstone
provides decoding. The project has not selected a license for its own code.
These development artifacts do not settle that distribution decision.

The native UI uses the pinned GPUI snapshot and GPUI Kit. Both manifests
declare Apache-2.0. Capstone includes BSD-style notices and LLVM-derived
code notices. LLVM 18.1.8 uses Apache-2.0 with LLVM exceptions and retains
notices for third-party source. Embedded IBM Plex fonts use the SIL Open
Font License. Read the exact texts in each package's `notices` directory.

Packaging reads the locked Cargo graph and copies license, copying and notice
files, including vendored native-code subdirectories. The generated
`rust-dependencies.json` records each resolved crate's name, version, license
expression, authors, source and copied notice files. It covers dependencies for other
platforms too. When a published archive excludes separate notice files, packaging preserves
its original `.crate` source archive and checks its SHA-256 against Cargo.lock.
It also includes the declared SPDX license texts. The original archive retains
authorship, package metadata and source copyright headers. This inventory is evidence for a release audit,
not a claim that an automated scan establishes every legal obligation.
Patched third-party path dependencies are included too, with their complete
source archive and retained notices; only Vole workspace members are skipped.

The toolchain inventory records exact versions and dynamic dependencies.
CI builds LLVM 18.1.8 from upstream commit
`3b5b5c1ec4a3095ab096dd780e84d7ab81f3d7ff` with ARM, AArch64 and X86 enabled.
It disables optional compression, terminal and XML libraries and links LLVM
statically into the three shipped tools. The build copies LLVM/LLD license
texts and records the source commit.
[LLVM release](https://github.com/llvm/llvm-project/releases/tag/llvmorg-18.1.8),
[LLVM license](https://github.com/llvm/llvm-project/blob/llvmorg-18.1.8/llvm/LICENSE.TXT),
[LLVM CMake options](https://llvm.org/docs/CMake.html).

A local Linux package may instead bundle the installed LLVM 14 tools.
For that path, the packaging script collects their transitive non-glibc
shared libraries and Debian/Ubuntu copyright files. `system-packages.json`
records binary and source package versions; corresponding source packages
are available from the distribution's source archive. This can include
LGPL libraries and GCC libraries with runtime exceptions. Preserve those
notices and obtain the matching source packages before public distribution.
Vole does not replace the host's glibc, display stack or graphics drivers.

The package also includes Cargo.lock and embedded font license texts.
SHA256SUMS records the exact packaged files. Public signing, notarization,
release publication and a final distribution-license audit are separate
release-owner actions. The workflows upload reviewable build artifacts and
request no publishing or signing credentials.

Primary package declarations:
[GPUI snapshot](https://crates.io/crates/gpui-pre/0.3.7),
[GPUI Kit](https://crates.io/crates/gpui-kit/0.7.0),
[Capstone source license](https://github.com/capstone-engine/capstone/blob/5.0.6/LICENSE.TXT),
[IBM Plex license](https://github.com/IBM/plex/blob/master/LICENSE.txt).
