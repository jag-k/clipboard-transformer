# Linux support

Clipboard Transformer needs global clipboard observation, an actionable
notification portal, and a StatusNotifierItem tray host. It runs only when the
current session provides the complete contract.

## Supported clipboard sessions

- X11 with XFixes.
- Wayland with a working XWayland clipboard bridge exposed through `DISPLAY`.
- Native Wayland with `ext-data-control-v1`.
- Native Wayland with `wlr-data-control-unstable-v1`.

Native data-control is preferred when both native Wayland and XWayland are
available. Ordinary `wl_data_device` access is not a global observation API and
is not used as a fallback.

Native packages require glibc and XDG Desktop Portal. The X11 backend is
implemented without libX11, so an X11-only installation does not need the
Wayland client libraries. The native Wayland backend loads `libwayland-client`
dynamically; install the platform package (`wayland` on Arch Linux or
`libwayland-client0` on Debian/Ubuntu) only for native Wayland clipboard
support. `xdg-utils` is also optional and provides the fallback used to open
support links when the portal OpenURI interface is unavailable.

GNOME Wayland without XWayland and without a data-control protocol is
unsupported. The desktop app sends a best-effort high-priority notification,
writes the blockers to its log, and exits non-zero. CLI `watch` writes the same
kind of error to stderr, keeps stdout empty, and exits non-zero; it does not
wait indefinitely or read stdin.

## Diagnose the current session

Run:

```sh
clipboard-transformer doctor
```

The important fields are:

```text
clipboard_backend=x11-xwayland
clipboard_backend=wayland-ext-data-control
clipboard_backend=wayland-wlr-data-control
desktop_runtime_ready=true
```

When `desktop_runtime_ready=false`, every `desktop_blocker=` line names a
missing requirement. Common fixes are:

- enable/install XWayland when the compositor has no native data-control;
- install and start an XDG Desktop Portal backend;
- enable a StatusNotifierItem/AppIndicator host in desktops that do not ship
  one by default.

`clipboard-transformer paths` prints the state directory containing
`clipboard-transformer.log`.

## Install and package formats

The [installation guide](install.md#linux) covers the published AUR packages,
AppImage, DEB, Pacman, RPM, Homebrew bundle, and portable CLI archive.

Native packages and both AUR alternatives install the desktop entry, icon,
public CLI, desktop host, and D-Bus activation metadata used by the support
action in fatal startup notifications. The AppImage exposes the desktop
application but does not install system integration or a system CLI; pair it
with the separate CLI archive when needed.

Published packages have passed automated build and content checks. Installed
package validation is still incomplete across Xubuntu/X11, GNOME with
XWayland, pure data-control Wayland, SteamOS, and intentional failure sessions
without clipboard or tray capabilities. Release availability is not a claim
that every compositor and package combination has been exercised.
