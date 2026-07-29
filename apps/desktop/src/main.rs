#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod desktop;

fn main() {
    // Errors raised before desktop::run() installs the log file would otherwise
    // vanish: mirror them to stderr for console launches. GUI launches have no
    // stderr handle and the write is silently ignored.
    ct_runtime::logging::enable_stderr_mirror();

    // Hidden argv commands the host re-invokes itself with (environment probe,
    // D-Bus activation) must not start a second desktop instance.
    if ct_runtime::platform::handle_early_host_command() {
        return;
    }

    if let Err(error) = run_desktop() {
        ct_runtime::logging::event(format!("fatal error: {error:#}"));
        std::process::exit(1);
    }
}

fn run_desktop() -> anyhow::Result<()> {
    if let Ok(paths) = ct_runtime::config::ConfigPaths::resolve() {
        ct_runtime::logging::init(paths.state_dir.join("clipboard-transformer.log"));
    }
    let _activation = ct_runtime::platform::register_host_activation()?;
    desktop::run()
}
