# GPUI platform and delivery research

Research date: 2026-10-02. These are findings and implementation proposals. No native application has been built or run.

## Recommendation

Use Rust and GPUI for the desktop application, with the simulator in separate Rust crates that do not depend on GPUI. Start implementation with a small native window and build it on all three operating systems before developing the full workbench. GPUI has the necessary platform backends, but its published release, current source, and component ecosystem have different dependency sets.

This is a native GPU-rendered application. A browser preview can demonstrate the proposed layout, but cannot verify GPUI rendering or native macOS window behavior.

## Framework status and dependency choice

GPUI combines retained application state with declarative and imperative rendering. It has entities, views, custom elements, actions, and a platform-integrated executor. Its maintainers explicitly describe it as pre-1.0 with breaking changes. The current upstream README recommends `gpui_platform::application()` to choose the host backend, plus platform features for font rendering and Linux windowing. [Upstream README](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/README.md)

The homepage still shows `Application::new()`. Published `gpui` 0.2.2 documentation also shows that API and contains an older macOS/Linux-only introductory statement, despite its source archive including a Windows platform. Do not combine examples from these releases with current-main APIs. [GPUI homepage](https://gpui.rs/), [published 0.2.2 documentation](https://docs.rs/gpui/0.2.2/gpui/)

Verified package state at research time:

| Dependency source | Verified state | Proposed use |
| --- | --- | --- |
| Upstream `gpui` on crates.io | Latest published version 0.2.2 | Candidate for a minimal standalone app, subject to the platform spike |
| Upstream Zed Git | Snapshot `c83abe7d0e060de08db386fbf86f7e95bfe6cb09`; platform implementation split into separate crates | Fallback when required current APIs are absent from a published compatible set |
| `gpui_platform` package name | Not present on crates.io at research time | Do not copy upstream-main wildcard manifest instructions into a crates.io build |
| GPUI Kit, previously GPUI Component | Published 0.7.0; GPUI snapshots `gpui-pre` and `gpui-pre-platform` 0.3.7 | First candidate for reusable inputs, menus, editor primitives, and resizable panels |

Sources: [gpui registry metadata](https://crates.io/api/v1/crates/gpui), [gpui_platform registry endpoint](https://crates.io/api/v1/crates/gpui_platform), [gpui-kit registry metadata](https://crates.io/api/v1/crates/gpui-kit), [gpui-pre metadata](https://crates.io/api/v1/crates/gpui-pre), [gpui-pre-platform metadata](https://crates.io/api/v1/crates/gpui-pre-platform).

GPUI Kit's inspected manifest pins the renamed snapshot packages to exact matching versions. Its comments explain that allowing a newer snapshot previously broke component compilation. Preserve that compatibility set; do not add a second upstream GPUI version beside it. Use custom workbench components and theme tokens so the design remains specific to the simulator. [GPUI Kit manifest, snapshot `3467e647600290343885b500bd7464057e334d18`](https://github.com/longbridge/gpui-kit/blob/3467e647600290343885b500bd7464057e334d18/Cargo.toml), [kit documentation](https://gpui-kit.com/docs/)

At the first successful build, pin the Rust toolchain, dependency versions or Git revision, and commit the generated `Cargo.lock`. Upstream Zed's inspected snapshot uses Rust 1.98.1; that is evidence about this snapshot, not an established minimum for every published GPUI package. [Upstream toolchain](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/rust-toolchain.toml), [Cargo lockfile guidance](https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html)

## Platforms

| Host | Verified backend | Build and runtime considerations |
| --- | --- | --- |
| macOS | AppKit windows and Metal rendering | Build with Xcode, its macOS tools, and shader toolchain. Current upstream requires `font-kit` for rendered glyphs. Validate on Apple Silicon and Intel separately. |
| Windows | Win32 windows, DirectWrite text, DirectX rendering | Use the MSVC toolchain, Windows SDK, and native Windows CI. Keep Linux tooling and command assumptions out of app code. |
| Linux | X11 and Wayland, GPU rendering through the Linux renderer | Compile both backends and verify each in a real graphical session. Native file pickers require an available desktop portal implementation. |

Sources: [GPUI backend selection](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui_platform/src/gpui_platform.rs), [platform feature configuration](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui_platform/Cargo.toml), [macOS build instructions](https://zed.dev/docs/development/macos), [Windows build instructions](https://zed.dev/docs/development/windows), [Linux build instructions](https://zed.dev/docs/development/linux).

Zed is evidence that the framework ships on all three hosts. Its published requirements include macOS on Intel and Apple Silicon; Linux x86_64/aarch64 with Vulkan 1.3; and 64-bit Windows on x64/Arm64 with DirectX 11 graphics. These are Zed's requirements, not a tested support promise for this application. Establish Vole's minimum versions after the selected dependencies compile and run. [Zed installation requirements](https://zed.dev/docs/installation)

Zed's Linux binaries require glibc at least 2.31 on x86_64 and 2.35 on aarch64. That reflects how those binaries are built. Build Vole on the oldest distribution it intends to support, inspect its linked libraries, and publish its own tested runtime requirements. [Zed Linux requirements and troubleshooting](https://zed.dev/docs/linux)

Guest instruction sets and host CPU targets are separate. A 64-bit Apple Silicon application may simulate 8-bit VOLE or x86 machine code; it does not need an 8-bit or x86 host binary. Proposed initial host artifacts are macOS aarch64/x86_64, Windows x86_64, and Linux x86_64. Add Windows/Linux Arm64 artifacts when native runners and smoke-test machines are available. All supported guest ISAs should remain available on every validated host.

## Proper macOS windows

Current GPUI distinguishes a normal native window from floating, popup, and dialog panels. Its macOS backend applies titled, closable, resizable, and minimizable AppKit styles when a titlebar is present, according to the supplied options. Removing the titlebar follows a different style path, so a visually borderless design needs special care. Keep the system titlebar and traffic lights in the first build. [macOS window creation](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui_macos/src/window.rs#L992)

Proposed main-window policy:

- Open as `WindowKind::Normal`, with resizing and minimization enabled.
- Use the ordinary foreground application activation policy so the app participates in the Dock and menu bar.
- Package an `.app` with stable bundle identity, icon, executable, and version metadata.
- Provide File, Edit, Run, Window, and Help menus and conventional Command-key shortcuts.
- Restore the existing window on Dock reopen; create a main window if none remains. Preserve the simulator session on close until the app quits.
- Keep the minimum window size low enough for a tiled laptop workspace. Collapse secondary panels before forcing an oversized window.

The upstream macOS platform implements foreground activation and a reopen callback. Product behavior still has to be wired and tested. [macOS platform source](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui_macos/src/platform.rs)

Native acceptance must inspect the installed `.app`, not just a shell-launched executable. On a MacBook verify Dock and Command-Tab presence, traffic lights, resize, minimize/restore, fullscreen/Spaces, Retina text, native menus, file-picker behavior, and close/reopen without extra windows.

For AeroSpace, inspect the actual window's accessibility role/subrole and fullscreen control, then open it in a workspace with a neighboring tiled window. Confirm automatic tiling and changed neighbor geometry, close, and reopen through the same path. A window-list entry does not prove tiling. AeroSpace documents that missing fullscreen buttons can cause a window to be treated as a dialog and floated. [AeroSpace dialog heuristics](https://nikitabobko.github.io/AeroSpace/guide#dialog-heuristics)

This environment is Linux. Native macOS, Windows, and AeroSpace acceptance is unverified. Any compatibility rule must be narrow, based on the actual observed bundle/window, and documented as a local workaround. Do not change the user's AeroSpace configuration or make the app invoke AeroSpace to force its layout.

## Accessibility and responsiveness

Current upstream integrates AccessKit. Elements need stable IDs and roles to enter the accessibility tree; custom controls must respond to accessibility actions. The source also describes synthetic text children for custom editors. This does not make arbitrary painted memory grids or code editors automatically accessible. Validate the actual dependency version and provide keyboard navigation, focus indicators, labels, zoomable text, and meaningful table semantics. [GPUI accessibility guide](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/src/_accessibility.rs)

Proposed execution flow: a worker owns mutable machine state; the UI sends run/step/pause commands and receives bounded snapshots/deltas. A simulation must not monopolize the UI event loop. Publish view updates at a capped rate, virtualize memory/disassembly rows, and bound execution history. None of these proposals requires a web renderer or GPU compute for CPU emulation.

## Licensing boundaries

GPUI and its platform crates declare Apache-2.0. Zed's `ui` and `editor` crates declare GPL-3.0-or-later. GPUI Kit software and code examples use Apache-2.0, while its current documentation prose has separate attribution terms. Inspect dependency licenses and retain applicable notices during release preparation. Do not copy Zed editor/UI code merely because the surrounding GPUI framework is Apache-licensed. The project's own license remains a separate owner decision. [GPUI manifest](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/Cargo.toml), [Zed UI manifest](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/ui/Cargo.toml), [Zed editor manifest](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/editor/Cargo.toml), [kit license documentation](https://gpui-kit.com/docs/#license)

## Proposed CI and delivery gates

1. Run formatting, linting, and headless simulator tests on every change. Add native desktop builds on macOS, Windows, and Linux with the committed lockfile.
2. Keep platform compile checks separate from real GUI smoke tests. Headless GPUI tests cannot establish GPU rendering, native fullscreen, accessibility, or tiling behavior. Zed's own visual regression runner is currently macOS-only and requires Screen Recording permission. [Zed visual tests](https://zed.dev/docs/development/macos#visual-regression-tests)
3. Produce macOS `.app` bundles and a downloadable archive or DMG; Windows executable/installer artifacts; Linux archive plus `.desktop` entry and icon. Use a small application-specific packager, rather than importing Zed's entire release machinery. Zed's Linux docs show desktop integration and dependency inspection as part of packaging. [Linux packaging reference](https://zed.dev/docs/development/linux#notes-for-packaging-zed)
4. Before public macOS distribution, sign and notarize the application with the owner's distribution credentials. Signing credentials and release publishing come after the artifact is reviewable. [Apple notarization reference](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
5. Gate release claims on actual smoke tests for each advertised artifact. Keep Windows/Linux Arm64, minimum OS versions, installer signing, and native screen-reader support marked pending until tested.

The approval-stage repository should contain this research and the implementation plan only. The dependency spike, Cargo workspace, CI workflows, executable code, packaging scripts, and native verification belong to the approved implementation phase.
