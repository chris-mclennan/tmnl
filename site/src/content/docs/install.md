---
title: Install
description: How to install tmnl on macOS, Linux, and Windows.
---

tmnl ships native binaries for all three major desktop OSes via [`cargo-dist`](https://opensource.dev/dist/). Pick the option that matches your system.

## macOS

Install with the shell installer (requires `curl`):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/chris-mclennan/tmnl-rs/releases/latest/download/tmnl-installer.sh | sh
```

Or download the `.pkg` installer from the [latest release](https://github.com/chris-mclennan/tmnl-rs/releases/latest).

## Linux

Same shell installer as macOS:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/chris-mclennan/tmnl-rs/releases/latest/download/tmnl-installer.sh | sh
```

You'll need a working OpenGL / Vulkan stack for `wgpu` to render. Most modern distros have this; check with `glxinfo` or `vulkaninfo` if tmnl fails to launch.

## Windows

PowerShell installer:

```powershell
irm https://github.com/chris-mclennan/tmnl-rs/releases/latest/download/tmnl-installer.ps1 | iex
```

Or grab the `.msi` from the [latest release](https://github.com/chris-mclennan/tmnl-rs/releases/latest) and double-click.

## Build from source

```sh
git clone https://github.com/chris-mclennan/tmnl-rs
cd tmnl-rs
cargo build --release
./target/release/tmnl
```

Build deps:

- **macOS**: Xcode command-line tools (`xcode-select --install`)
- **Linux**: X11 + Wayland + GTK headers — `sudo apt install libx11-dev libxcursor-dev libxrandr-dev libxi-dev libxkbcommon-dev libgl1-mesa-dev libwayland-dev libudev-dev libfontconfig1-dev libglib2.0-dev libgtk-3-dev libxdo-dev`
- **Windows**: MSVC build tools (included with Visual Studio 2022 or the standalone Build Tools installer)

## Verify

After install:

```sh
tmnl --version
```

If your shell can't find `tmnl`, you may need to restart it (the installer puts `tmnl` on `PATH` via your shell profile).
