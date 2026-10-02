#!/usr/bin/env bash
# Native GPUI build dependencies plus the guest assembler, linker and C compiler for Debian/Ubuntu.
set -euo pipefail
if ! command -v apt-get >/dev/null; then
  echo 'This script supports Debian/Ubuntu. Install equivalent X11, Wayland, font and Vulkan development packages on your distribution.' >&2
  exit 1
fi
privilege=()
if [[ $(id -u) != 0 ]]; then privilege=(sudo); fi
"${privilege[@]}" apt-get update
"${privilege[@]}" apt-get install -y --no-install-recommends \
  build-essential cmake ninja-build clang pkg-config git curl python3 \
  libssl-dev libfontconfig1-dev libfreetype6-dev libx11-dev libxcb1-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libvulkan-dev mesa-vulkan-drivers libegl1-mesa-dev \
  llvm-14 lld-14 clang-14 weston openbox xcompmgr xvfb xauth xdotool x11-utils imagemagick tesseract-ocr \
  dbus-x11 xdg-desktop-portal xdg-desktop-portal-gtk
