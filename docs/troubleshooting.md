# Troubleshooting

Start with the built-in diagnostics:

```sh
clipboard-transformer doctor
clipboard-transformer paths
clipboard-transformer config check
```

## The app does not transform

1. Confirm transformations are not paused in the tray.
2. Validate the active config path printed by `paths`.
3. Preview the current clipboard with `clipboard-transformer transform --preview`,
   or pipe an exact value through `clipboard-transformer transform -`.
4. Check `apps`/`app_mode`, `formats`, disabled rules, and ruleset ordering.
5. Read `<state_dir>/clipboard-transformer.log`.

The first clipboard item present at startup is deliberately not transformed.
Copy it again after the app is running.

## A config reload failed

The desktop app keeps the last valid rules when a reload fails. Run
`clipboard-transformer config check`, fix the reported config/import/plugin issue,
then choose **Reload config** in the tray.

Remote imports may use a cached copy when refresh fails. Their cache lives
under `<state_dir>/url-imports/`.

## A plugin rule is skipped

```sh
clipboard-transformer plugin list
clipboard-transformer plugin doctor
clipboard-transformer plugin inspect <plugin-id>
```

Check the namespaced rule type, plugin settings, and the intersection of
requested and granted permissions. Do not grant capabilities the plugin does
not need.

## Linux exits at startup

Run `clipboard-transformer doctor` in the same graphical session. Unsupported
Wayland sessions fail explicitly instead of falling back to a nonfunctional
clipboard backend. See [Linux desktop support](linux.md).

## Find the logs

`clipboard-transformer paths` prints `state_dir`. Logs rotate at 5 MiB, with
three previous generations retained.

When reporting a bug, include the app version or commit, OS/session details,
the relevant diagnostic output, a minimal config, and redacted log lines.
