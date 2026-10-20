mod cli;
mod commands;
mod output;

use clap::Parser;
use cli::{Cli, Command};
use orbis_pkg_util::ExtractError;
use output::{diagnostic, error_message, has_io_kind};
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    if let Err(error) = ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed)) {
        diagnostic(&format!("error: cannot install interrupt handler: {error}"));
        return ExitCode::FAILURE;
    }
    let mut stdout = io::stdout().lock();
    match commands::run(&cli, &mut stdout, cancelled).and_then(|code| {
        stdout.flush()?;
        Ok(code)
    }) {
        Ok(code) => ExitCode::from(code),
        Err(error) if has_io_kind(error.as_ref(), io::ErrorKind::BrokenPipe) => ExitCode::SUCCESS,
        Err(error) => {
            diagnostic(&format!("error: {}", error_message(error.as_ref())));
            if cli.verbose {
                let mut source = error.source();
                while let Some(error) = source {
                    diagnostic(&format!("  caused by: {error}"));
                    source = error.source();
                }
            }
            if has_io_kind(error.as_ref(), io::ErrorKind::AlreadyExists) {
                diagnostic(
                    "hint: choose another --output directory, or use --force to replace existing files.",
                );
            }
            if matches!(cli.command, Command::Extract { .. }) {
                diagnostic(
                    "Files completed before the failure remain; unfinished replacements were discarded.",
                );
            }
            if matches!(
                error.downcast_ref::<ExtractError>(),
                Some(ExtractError::Cancelled)
            ) {
                ExitCode::from(130)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
