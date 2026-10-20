use std::error::Error as StdError;
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn diagnostic(message: &str) {
    let _ = writeln!(io::stderr().lock(), "{message}");
}

pub(crate) fn escaped(path: &Path) -> String {
    path.to_string_lossy().escape_debug().to_string()
}

pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(crate) fn has_io_kind(mut error: &(dyn StdError + 'static), kind: io::ErrorKind) -> bool {
    loop {
        if error
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == kind)
        {
            return true;
        }
        match error.source() {
            Some(source) => error = source,
            None => return false,
        }
    }
}

pub(crate) fn error_message(error: &(dyn StdError + 'static)) -> String {
    let mut messages = vec![error.to_string()];
    let mut source = error.source();
    while let Some(error) = source {
        let message = error.to_string();
        if !messages.last().unwrap().contains(&message) {
            messages.push(message);
        }
        source = error.source();
    }
    messages.join(": ").escape_debug().to_string()
}
