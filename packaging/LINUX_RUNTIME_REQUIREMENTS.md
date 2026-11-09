# Linux host requirements

The AppImage includes Any NLE and its verified FFmpeg/ffprobe executables. It does
not redistribute Linux distribution shared libraries. This alpha therefore targets
Ubuntu 22.04 or a compatible desktop with glibc 2.35 or later, rather than promising
a fully self-contained application on every Linux distribution.

A working X11 or Wayland desktop, compatible OpenGL/EGL drivers, and the desktop's
D-Bus session and file-picker portal are required. On Ubuntu, the usual runtime
packages include `libx11-6`, `libxcursor1`, `libxrandr2`, `libxi6`, `libxinerama1`,
`libxkbcommon0`, `libwayland-client0`, `libegl1`, `libgl1`, `libdbus-1-3`,
`xdg-desktop-portal` and the portal backend for that desktop. The per-package
`linux-runtime.json` lists the exact SONAME dependencies observed for the built
executables; dynamically loaded desktop and driver requirements remain additional.

The pinned AppImage runtime statically includes FUSE 3 and needs access to the
host kernel FUSE interface for mounting; no system `libfuse2` is required.
Alternatively, use the runtime's `--appimage-extract-and-run` option to run without
mounting through FUSE. Desktop libraries and drivers still come from
the host. CLI use does not require launching the desktop wizard.

The release jobs check host dependencies on both Ubuntu 22.04 architectures and
verify package extraction and CLI/backend execution. Broader Linux desktop and
native GUI compatibility require their own checks.
