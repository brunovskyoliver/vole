# Vole's X11 main-window visual patch

This directory contains the published `gpui-pre-linux` 0.3.7 crate. The crate's
Apache-2.0 license is retained in `LICENSE-APACHE`. Original archive checksum:
`0aac7022e347409454777701082201742710052813964a1e25160f7e22c965ad`.
Check the registry metadata when updating the dependency. All GPUI snapshot
crates must stay compatible with GPUI Kit's exact pinned versions.

The only source change is in `src/linux/x11/window.rs`, where the X11 window
chooses its visual. Upstream always prefers a transparent ARGB visual. Vole
prefers an opaque RGB visual for a normal window with a native titlebar whose
`appears_transparent` flag is false. Popup, floating, dialog, and custom
transparent-titlebar paths keep upstream's ARGB preference. If no opaque
visual exists, the original transparent visual remains a fallback.

## Reproduced failure

On 2026-10-02, a Linux Xvfb/Openbox desktop with Mesa llvmpipe Vulkan presented
an entirely black GPUI window. Native menus/title changes and frame callbacks
worked. Capturing the client window with ImageMagick produced exactly one
color. The same workbench rendered correctly through a nested Weston Wayland
session; a separate Vulkan cube rendered correctly on X11.

The failure also reproduced with a minimal GPUI view containing only a solid
blue rectangle over the workspace color. This eliminated the editor,
workbench layout, fonts, runtime state, and missing frame callbacks. Adding
an X11 compositor did not repair it. Switching the diagnostic process to the
OpenGL adapter rendered the two expected colors.

## Controlled fix and result

The original X11 visual was ID 64 at depth 32. With the narrow patch, the
same Vulkan adapter selected visual ID 33 at depth 24. The same minimal view
then produced the expected two colors. The complete native workbench produced
15,102 colors, with the actual program paused at PC 04 after two loads.
All five instructions and all 256 VOLE memory bytes were visible at the
default desktop size. No environment override is required by the shipped app.

A practical regression check is to launch the built app on a real X11 desktop
or Xvfb, find its current window ID, and capture the client:

```sh
DISPLAY=:83 target/debug/vole --visual-demo
DISPLAY=:83 xdotool search --name 'Untitled.vole'
import -display :83 -window "$VOLE_WINDOW_ID" "$TMPDIR/vole-native.png"
identify -format '%k colors\n' "$TMPDIR/vole-native.png"
```

The full workbench must have many colors and visibly contain the editor,
instruction list, registers, and memory. A live UI capture is required;
compilation and synthetic input tests do not establish presentation.

## Boundary

GPUI's `WindowParams` does not expose the requested window background at
visual selection time. The native-titlebar predicate therefore targets Vole's
opaque main window. It is not a general guarantee for applications that use
a native opaque-looking titlebar but later request a transparent main-window
background. Such a design needs the requested background propagated to this
creation API, or this predicate revised. Vole uses an opaque main window.

macOS, Windows, and Wayland code are unchanged. This local patch has no claim
about their native window acceptance. Re-test X11, Wayland, and native popup
behavior when updating GPUI, and remove the patch when an upstream release
handles this case.

Source: [published crate](https://crates.io/crates/gpui-pre-linux/0.3.7).
