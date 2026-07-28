# Platform support

macOS, Windows, and Linux are maintained desktop targets. Each has a native
clipboard backend, actionable notifications, a tray host, autostart, and
runtime tests. Validation depth differs by platform and desktop environment.

| Platform | Desktop integration | Notes |
| --- | --- | --- |
| macOS 13+ | AppKit, UserNotifications, `SMAppService` | Primary development environment. |
| Windows 10/11 | Win32 tray and clipboard, toast activation | MSI and portable registration are implemented; planned artifacts are not code-signed yet. |
| Linux | X11/XWayland or data-control Wayland, portal notifications, StatusNotifierItem | Session capability varies by compositor; run `doctor`. |

Support is best effort across OS versions, distributions, desktop
environments, compositors, and packaging systems. Actionable reports may need
logs and a minimal reproducer when the maintainer cannot reproduce the
environment.

## Diagnose

```sh
clipboard-transformer doctor
clipboard-transformer paths
```

The runtime log is `<state_dir>/clipboard-transformer.log`.

See the [Linux session guide](linux.md) for supported protocols and failure
behavior.
