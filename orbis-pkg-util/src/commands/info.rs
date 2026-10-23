use super::{Cli, Package, Result, diagnostic, error_message, human_size, open, write_json};
use orbis_pkg::sfo::{Sfo, Value};
use serde::{Serialize, Serializer, ser::SerializeMap};
use std::{borrow::Cow, collections::BTreeMap, io::Write, path::Path};

// Keep fields in the established JSON order; both renderers consume this report.
#[derive(Serialize)]
struct InfoReport<'a> {
    application: Option<ApplicationReport>,
    metadata: HeaderReport<'a>,
    package: Cow<'a, str>,
    schema_version: u32,
    sfo: Option<BTreeMap<String, SfoField>>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct HeaderReport<'a> {
    content_flags: u32,
    content_flags_name: String,
    content_id: &'a str,
    content_type: u32,
    content_type_name: &'static str,
    content_version: &'a str,
    drm_type: u32,
    drm_type_name: &'static str,
    entry_count: usize,
    file_count: u32,
    label: &'a str,
    pfs_offset: usize,
    pfs_size: usize,
    pkg_size: u64,
    pkg_type: u32,
    publisher_code: &'a str,
    service_id: &'a str,
    table_offset: usize,
    title_id: &'a str,
}

impl<'a> From<&'a orbis_pkg::header::PkgHeader> for HeaderReport<'a> {
    fn from(header: &'a orbis_pkg::header::PkgHeader) -> Self {
        let id = header.content_id();
        Self {
            content_flags: header.content_flags().bits(),
            content_flags_name: header.content_flags().to_string(),
            content_id: id.as_str(),
            content_type: header.content_type(),
            content_type_name: header.content_type_name(),
            content_version: id.version(),
            drm_type: header.drm_type(),
            drm_type_name: header.drm_type_name(),
            entry_count: header.entry_count(),
            file_count: header.file_count(),
            label: id.label(),
            pfs_offset: header.pfs_offset(),
            pfs_size: header.pfs_size(),
            pkg_size: header.pkg_size(),
            pkg_type: header.pkg_type(),
            publisher_code: id.publisher_code(),
            service_id: id.service_id(),
            table_offset: header.table_offset(),
            title_id: id.title_id(),
        }
    }
}

#[derive(Serialize)]
struct ApplicationReport {
    app_version: Option<String>,
    build: BTreeMap<String, String>,
    category: Option<String>,
    content_id: Option<String>,
    package_version: Option<String>,
    region_from_content_id: Option<&'static str>,
    required_system_version: Option<String>,
    title: Option<String>,
    title_id: Option<String>,
}

enum SfoField {
    String(String),
    Integer(u32),
    Binary(String),
}

impl Serialize for SfoField {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut field = serializer.serialize_map(Some(2))?;
        match self {
            Self::String(value) => {
                field.serialize_entry("type", "string")?;
                field.serialize_entry("value", value)?;
            }
            Self::Integer(value) => {
                field.serialize_entry("type", "integer")?;
                field.serialize_entry("value", value)?;
            }
            Self::Binary(hex) => {
                field.serialize_entry("hex", hex)?;
                field.serialize_entry("type", "binary")?;
            }
        }
        field.end()
    }
}

struct SfoReport {
    application: ApplicationReport,
    fields: BTreeMap<String, SfoField>,
}

fn read_sfo(pkg: &Package) -> Result<Option<SfoReport>> {
    let Some(data) = pkg.param_sfo_data()? else {
        return Ok(None);
    };
    let sfo = Sfo::parse(&data)?;
    let fields = sfo
        .entries()
        .map(|(key, value)| {
            let value = match value {
                Value::String(s) => SfoField::String((*s).to_owned()),
                Value::Integer(n) => SfoField::Integer(*n),
                Value::Binary(b) => {
                    SfoField::Binary(b.iter().map(|v| format!("{v:02x}")).collect())
                }
            };
            (key.to_owned(), value)
        })
        .collect();
    let firmware = match sfo.get("SYSTEM_VER") {
        Some(Value::Integer(n)) => Some(format!("{:x}.{:02x}", n >> 24, (n >> 16) & 0xff)),
        _ => None,
    };
    let build = sfo
        .string("PUBTOOLINFO")
        .unwrap_or("")
        .split(',')
        .filter_map(|field| field.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect();
    let region = match pkg.header().content_id().service_id() {
        "UP" => Some("North America"),
        "EP" => Some("Europe"),
        "JP" => Some("Japan"),
        "HP" => Some("Asia"),
        _ => None,
    };
    let string = |key| sfo.string(key).ok().map(str::to_owned);
    Ok(Some(SfoReport {
        application: ApplicationReport {
            app_version: string("APP_VER"),
            build,
            category: string("CATEGORY"),
            content_id: string("CONTENT_ID"),
            package_version: string("VERSION"),
            region_from_content_id: region,
            required_system_version: firmware,
            title: string("TITLE"),
            title_id: string("TITLE_ID"),
        },
        fields,
    }))
}

pub(super) fn run(
    path: &Path,
    as_json: bool,
    header_only: bool,
    all_sfo: bool,
    cli: &Cli,
    out: &mut impl Write,
) -> Result<u8> {
    let mut pkg = open(
        path,
        if header_only {
            None
        } else {
            cli.passcode_file.as_deref()
        },
    )?;
    if !header_only {
        let _ = pkg.load_entry_keys();
    }
    let mut report = InfoReport {
        application: None,
        metadata: HeaderReport::from(pkg.header()),
        package: path.to_string_lossy(),
        schema_version: 1,
        sfo: None,
        warnings: Vec::new(),
    };
    if !header_only {
        match read_sfo(&pkg) {
            Ok(Some(sfo)) => {
                report.application = Some(sfo.application);
                report.sfo = Some(sfo.fields);
            }
            Ok(None) => {}
            Err(error) => report.warnings.push(format!(
                "PARAM.SFO unavailable: {}; showing header metadata",
                error_message(error.as_ref())
            )),
        }
    }
    if as_json {
        write_json(out, &report)?;
    } else {
        report.write_text(out, all_sfo)?;
    }
    for warning in &report.warnings {
        diagnostic(&format!("warning: {warning}"));
    }
    Ok(if report.warnings.is_empty() { 0 } else { 3 })
}

impl InfoReport<'_> {
    fn write_text(&self, out: &mut impl Write, all_sfo: bool) -> Result<()> {
        let h = &self.metadata;
        writeln!(out, "PKG: {}\n", self.package.escape_debug())?;
        writeln!(out, "Content ID:       {}", h.content_id.escape_debug())?;
        writeln!(out, "  Service ID:     {}", h.service_id.escape_debug())?;
        writeln!(out, "  Publisher:      {}", h.publisher_code.escape_debug())?;
        writeln!(out, "  Title ID:       {}", h.title_id.escape_debug())?;
        writeln!(
            out,
            "  Content version: {}",
            h.content_version.escape_debug()
        )?;
        writeln!(out, "  Label:          {}", h.label.escape_debug())?;
        writeln!(
            out,
            "Content Type:     0x{:02X} ({})",
            h.content_type, h.content_type_name
        )?;
        writeln!(out, "Content Flags:    {}", h.content_flags_name)?;
        writeln!(
            out,
            "DRM Type:         0x{:02X} ({})",
            h.drm_type, h.drm_type_name
        )?;
        writeln!(out, "PKG Type:         0x{:08X}", h.pkg_type)?;
        writeln!(
            out,
            "PKG Size:         {} ({} bytes)",
            human_size(h.pkg_size),
            h.pkg_size
        )?;
        writeln!(out, "File Count:       {}", h.file_count)?;
        writeln!(out, "Entry Count:      {}", h.entry_count)?;
        writeln!(out, "Table Offset:     0x{:X}", h.table_offset)?;
        writeln!(out, "PFS Offset:       0x{:X}", h.pfs_offset)?;
        writeln!(
            out,
            "PFS Size:         {} ({} bytes)",
            human_size(h.pfs_size as u64),
            h.pfs_size
        )?;
        if let Some(application) = &self.application {
            application.write_text(out)?;
        }
        if let Some(sfo) = self.sfo.as_ref().filter(|_| all_sfo) {
            writeln!(out, "\nPARAM.SFO:")?;
            for (key, value) in sfo {
                writeln!(
                    out,
                    "  {}: {}",
                    key.escape_debug(),
                    serde_json::to_string(value)?
                )?;
            }
        }
        Ok(())
    }
}

impl ApplicationReport {
    fn write_text(&self, out: &mut impl Write) -> Result<()> {
        fn field(out: &mut impl Write, key: &str, value: Option<&str>) -> Result<()> {
            if let Some(value) = value {
                writeln!(out, "  {key}: {}", value.escape_debug())?;
            }
            Ok(())
        }
        writeln!(out, "\nApplication:")?;
        field(out, "app_version", self.app_version.as_deref())?;
        writeln!(out, "  build: {}", serde_json::to_string(&self.build)?)?;
        for (key, value) in [
            ("category", self.category.as_deref()),
            ("content_id", self.content_id.as_deref()),
            ("package_version", self.package_version.as_deref()),
            ("region_from_content_id", self.region_from_content_id),
            (
                "required_system_version",
                self.required_system_version.as_deref(),
            ),
            ("title", self.title.as_deref()),
            ("title_id", self.title_id.as_deref()),
        ] {
            field(out, key, value)?;
        }
        Ok(())
    }
}
