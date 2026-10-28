mod audio;
mod config;
mod db;
mod library;
mod lyrics;
mod paths;
mod playlists;
#[cfg(test)]
mod testutil;
mod ui;

use anyhow::{Context, Result};
use tracing_appender::non_blocking::WorkerGuard;

use config::Config;
use paths::Paths;

fn main() -> Result<()> {
    let paths = Paths::detect().context("cannot determine base directory")?;
    paths
        .ensure_dirs()
        .with_context(|| format!("cannot create folders under {}", paths.base.display()))?;

    // Keep the guard alive so buffered log lines are flushed on exit.
    let _log_guard = init_logging(&paths)?;
    tracing::info!(base = %paths.base.display(), "starting synq");

    let (config, status) = match Config::load_or_create(&paths.config) {
        Ok(c) => (c, None),
        Err(e) => {
            tracing::error!("config: {e}");
            (
                Config::default(),
                Some(format!("config.toml: {e} (using defaults)")),
            )
        }
    };

    // Our hook must be installed before `ratatui::init`, which chains to it
    // after restoring the terminal.
    install_panic_logger();
    let mut terminal = ratatui::try_init().context("cannot initialize the terminal")?;
    let library_rx = library::spawn(paths.music.clone(), paths.db.clone());
    let playlists = playlists::spawn(&paths);
    let lyrics = lyrics::spawn(paths.db.clone(), paths.lyrics.clone(), config.lyrics.online);
    let result = ui::App::new(
        &config,
        status,
        library_rx,
        playlists,
        lyrics,
        paths.music.clone(),
    )
    .run(&mut terminal);
    ratatui::restore();

    if let Err(e) = &result {
        tracing::error!("fatal: {e:#}");
    }
    result
}

/// Log to `data/logs/` only: stdout belongs to the interface.
fn init_logging(paths: &Paths) -> Result<WorkerGuard> {
    let file = tracing_appender::rolling::daily(&paths.logs, "synq.log");
    let (writer, guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .map_err(|e| anyhow::anyhow!("cannot init logging: {e}"))?;
    Ok(guard)
}

fn install_panic_logger() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        previous(info);
    }));
}
