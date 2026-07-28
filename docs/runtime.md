# Runtime behavior

The desktop application owns clipboard watching and user interaction. The
standalone CLI shares the same config and rule engine but does not start the
tray runtime or write transformed values back to the clipboard.

## Clipboard lifecycle

The app seeds the current change counter at startup, so content copied before
launch is not transformed. For each later external change it:

1. reads native metadata and rejects app-owned, transient, concealed, and
   password-manager items;
2. applies the global source-app filter;
3. materializes only representations required by active rules;
4. applies rules serially;
5. writes the transformed item with an app-owned source marker;
6. records history and sends an actionable notification.

Native change counters guard the metadata/payload gap. If the clipboard changes
during a read, the incomplete observation is discarded and retried later.

## Tray

The tray menu provides:

- pause/resume transformations;
- recent transformations and explicit restore;
- rule edit and temporary disable actions;
- config/plugin reload;
- open/reveal config;
- autostart;
- clear history;
- quit.

Autostart is app-owned and controlled only through the tray. The CLI does not
offer a second registration path.

## Undo and history

Notification **Undo** is accepted only while the current clipboard still
matches that notification's transformed payload. This prevents an old
notification from overwriting newer copied content.

Choosing an entry from **Recent** is an explicit restore and may replace newer
clipboard content. History is bounded by `recent_items_count`,
`max_item_bytes`, and `max_history_bytes`; set `recent_items_count: 0` to
disable it.

History and state files are private user data. Corrupt files are quarantined
with a timestamp suffix and do not block startup.

## Pause

While paused, transformations and clipboard writes stop. Resuming reseeds
observation state, so content copied during the pause is not transformed
retroactively.

## Hot reload

The desktop app watches:

- the main config;
- its adjacent `.env`;
- local imports;
- cached remote imports;
- the plugin directory.

A failed reload keeps the last valid engine active and reports the error.
Choosing **Reload config** also resamples the Unix GUI login-shell environment.

## Single instance

macOS uses LaunchServices plus a state-directory PID lock and replaces launches
that bypass LaunchServices. Windows uses a per-user mutex and exits a second
desktop launch. Linux follows its native host/session ownership.

## Storage and logs

Run `clipboard-transformer paths` for the effective directories. Operational
state and clipboard history are stored under `state_dir`; runtime logs are in
`clipboard-transformer.log` and rotate automatically.

See [configuration](configuration.md) for the file map and
[troubleshooting](troubleshooting.md) for diagnostics.
