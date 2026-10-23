//! Filename component rules shared by metadata resolution and filesystem consumers.
//!
//! The policies preserve existing compatibility differences. Tightening a policy
//! changes which packages can be processed and should be reviewed separately.
use std::path::{Component, Path};

/// Compatibility rules for one filename component, without any parent path.
#[derive(Clone, Copy, Debug)]
pub enum ComponentPolicy {
    /// One normal host component, excluding separators, NUL and alternate streams.
    Extraction,
    /// Managed update names, excluding trailing dots/spaces and ASCII DOS devices.
    Installation,
    /// Installation rules plus console devices and superscript COM/LPT aliases.
    Metadata,
}

impl ComponentPolicy {
    /// Checks the component syntax; this does not inspect the host filesystem.
    pub fn accepts(self, name: &str) -> bool {
        let mut components = Path::new(name).components();
        if name.is_empty()
            || name.contains(['/', '\\', ':', '\0'])
            || !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
        {
            return false;
        }
        if matches!(self, Self::Extraction) {
            return true;
        }
        if name.ends_with(['.', ' '])
            || name
                .chars()
                .any(|c| c.is_control() || "<>\"|?*".contains(c))
        {
            return false;
        }
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
            return false;
        }
        let extended = matches!(self, Self::Metadata);
        if extended && matches!(stem.as_str(), "CONIN$" | "CONOUT$") {
            return false;
        }
        let port = stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"));
        !port.is_some_and(|number| {
            matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                || (extended && matches!(number, "¹" | "²" | "³"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ComponentPolicy::*;

    #[test]
    fn components_preserve_policy_specific_compatibility() {
        for (name, expected) in [
            ("param.sfo", [true, true, true]),
            ("日本語.bin", [true, true, true]),
            ("COM10.bin", [true, true, true]),
            ("", [false, false, false]),
            (".", [false, false, false]),
            ("..", [false, false, false]),
            ("a/b", [false, false, false]),
            ("a\\b", [false, false, false]),
            ("C:stream", [false, false, false]),
            ("a\0b", [false, false, false]),
            ("a\nb", [true, false, false]),
            ("file.", [true, false, false]),
            ("file ", [true, false, false]),
            ("a?b", [true, false, false]),
            ("nul.txt", [true, false, false]),
            ("CoM1.bin", [true, false, false]),
            ("CONIN$", [true, true, false]),
            ("CONOUT$.txt", [true, true, false]),
            ("COM¹.bin", [true, true, false]),
            ("lpt²", [true, true, false]),
        ] {
            let actual = [Extraction, Installation, Metadata].map(|policy| policy.accepts(name));
            assert_eq!(actual, expected, "{name:?}");
        }
    }
}
