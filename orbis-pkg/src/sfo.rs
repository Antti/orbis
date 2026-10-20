//! Bounds-checked PARAM.SFO metadata used to identify an application and version.
use std::{collections::BTreeMap, fmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfoError(pub String);

impl fmt::Display for SfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid PARAM.SFO: {}", self.0)
    }
}
impl std::error::Error for SfoError {}

/// Parsed values retain their SFO type; integer/blob fields cannot masquerade as strings.
#[derive(Debug, PartialEq, Eq)]
pub enum Value<'a> {
    String(&'a str),
    Integer(u32),
    Binary(&'a [u8]),
}

#[derive(Debug)]
pub struct Sfo<'a>(BTreeMap<&'a str, Value<'a>>);

impl<'a> Sfo<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, SfoError> {
        let bad = || SfoError("invalid header, entry, or offset".into());
        let u32_at = |at: usize| -> Result<u32, SfoError> {
            Ok(u32::from_le_bytes(
                data.get(at..at + 4).ok_or_else(bad)?.try_into().unwrap(),
            ))
        };
        if data.len() > 1024 * 1024 || data.get(..4) != Some(b"\0PSF") || u32_at(4)? != 0x101 {
            return Err(bad());
        }
        let keys = u32_at(8)? as usize;
        let values = u32_at(12)? as usize;
        let count = u32_at(16)? as usize;
        let table_end = count
            .checked_mul(16)
            .and_then(|n| n.checked_add(20))
            .ok_or_else(bad)?;
        if table_end > keys || keys > values || values > data.len() {
            return Err(bad());
        }
        let mut result = BTreeMap::new();
        for i in 0..count {
            let at = 20 + i * 16;
            let key_offset = u16::from_le_bytes(data[at..at + 2].try_into().unwrap()) as usize;
            let format = u16::from_le_bytes(data[at + 2..at + 4].try_into().unwrap());
            let len = u32_at(at + 4)? as usize;
            let capacity = u32_at(at + 8)? as usize;
            let offset = u32_at(at + 12)? as usize;
            let key_start = keys.checked_add(key_offset).ok_or_else(bad)?;
            let key_bytes = data.get(key_start..values).ok_or_else(bad)?;
            let end = key_bytes.iter().position(|b| *b == 0).ok_or_else(bad)?;
            let key = std::str::from_utf8(&key_bytes[..end]).map_err(|_| bad())?;
            let start = values.checked_add(offset).ok_or_else(bad)?;
            let end = start.checked_add(capacity).ok_or_else(bad)?;
            let payload = data
                .get(start..end)
                .and_then(|s| s.get(..len))
                .ok_or_else(bad)?;
            let value = match format {
                0x204 => {
                    let text = payload.strip_suffix(&[0]).ok_or_else(bad)?;
                    if text.contains(&0) {
                        return Err(bad());
                    }
                    Value::String(std::str::from_utf8(text).map_err(|_| bad())?)
                }
                0x404 if len == 4 => {
                    Value::Integer(u32::from_le_bytes(payload.try_into().unwrap()))
                }
                4 => Value::Binary(payload),
                _ => return Err(bad()),
            };
            if key.is_empty() || result.insert(key, value).is_some() {
                return Err(SfoError("empty or duplicate key".into()));
            }
        }
        Ok(Self(result))
    }

    pub fn string(&self, key: &str) -> Result<&'a str, SfoError> {
        match self.0.get(key) {
            Some(Value::String(value)) if !value.is_empty() => Ok(value),
            _ => Err(SfoError(format!("missing or invalid {key} string"))),
        }
    }
}

/// Numeric ordering for the PS4 NN.NN application version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AppVersion(u8, u8);

impl std::str::FromStr for AppVersion {
    type Err = SfoError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let b = value.as_bytes();
        if b.len() != 5 || b[2] != b'.' || ![b[0], b[1], b[3], b[4]].iter().all(u8::is_ascii_digit)
        {
            return Err(SfoError("APP_VER must use NN.NN".into()));
        }
        Ok(Self(
            (b[0] - b'0') * 10 + b[1] - b'0',
            (b[3] - b'0') * 10 + b[4] - b'0',
        ))
    }
}
impl fmt::Display for AppVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}.{:02}", self.0, self.1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppMetadata {
    pub title_id: String,
    pub content_id: String,
    pub category: String,
    pub version: AppVersion,
}

impl AppMetadata {
    pub fn parse(data: &[u8]) -> Result<Self, SfoError> {
        let sfo = Sfo::parse(data)?;
        let title_id = sfo.string("TITLE_ID")?;
        if title_id.len() != 9
            || !title_id.starts_with("CUSA")
            || !title_id.as_bytes()[4..].iter().all(u8::is_ascii_digit)
        {
            return Err(SfoError(
                "TITLE_ID must be CUSA followed by five digits".into(),
            ));
        }
        let content_id = sfo.string("CONTENT_ID")?;
        if content_id.len() != 36
            || content_id.get(7..16) != Some(title_id)
            || !content_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            || content_id.as_bytes()[6] != b'-'
            || content_id.get(16..17) != Some("_")
            || content_id.as_bytes()[19] != b'-'
        {
            return Err(SfoError("CONTENT_ID does not identify this title".into()));
        }
        Ok(Self {
            title_id: title_id.into(),
            content_id: content_id.into(),
            category: sfo.string("CATEGORY")?.into(),
            version: sfo.string("APP_VER")?.parse()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(fields: &[(&str, &str)]) -> Vec<u8> {
        let keys: Vec<_> = fields
            .iter()
            .flat_map(|(k, _)| k.bytes().chain([0]))
            .collect();
        let values: Vec<_> = fields
            .iter()
            .flat_map(|(_, v)| v.bytes().chain([0]))
            .collect();
        let key_start = 20 + fields.len() * 16;
        let mut data = vec![0; key_start];
        data[..4].copy_from_slice(b"\0PSF");
        for (at, n) in [
            (4, 0x101),
            (8, key_start),
            (12, key_start + keys.len()),
            (16, fields.len()),
        ] {
            data[at..at + 4].copy_from_slice(&(n as u32).to_le_bytes());
        }
        let (mut key, mut value) = (0u16, 0u32);
        for (i, (k, v)) in fields.iter().enumerate() {
            let at = 20 + i * 16;
            data[at..at + 2].copy_from_slice(&key.to_le_bytes());
            data[at + 2..at + 4].copy_from_slice(&0x204u16.to_le_bytes());
            for offset in [4, 8] {
                data[at + offset..at + offset + 4]
                    .copy_from_slice(&(v.len() as u32 + 1).to_le_bytes());
            }
            data[at + 12..at + 16].copy_from_slice(&value.to_le_bytes());
            key += k.len() as u16 + 1;
            value += v.len() as u32 + 1;
        }
        data.extend(keys);
        data.extend(values);
        data
    }

    #[test]
    fn reads_metadata_and_orders_versions_numerically() {
        let data = fixture(&[
            ("TITLE_ID", "CUSA00000"),
            ("CONTENT_ID", "UP0000-CUSA00000_00-0000000000000000"),
            ("CATEGORY", "gp"),
            ("APP_VER", "01.10"),
        ]);
        let meta = AppMetadata::parse(&data).unwrap();
        assert!(meta.version > "01.09".parse().unwrap());
        assert_eq!(meta.version.to_string(), "01.10");
        for version in ["1.10", "01.1", "nan", "01.00x", "01.-1"] {
            assert!(version.parse::<AppVersion>().is_err());
        }
        for end in 0..data.len() {
            assert!(AppMetadata::parse(&data[..end]).is_err());
        }
    }

    #[test]
    fn rejects_bad_ranges_types_duplicates_and_unterminated_strings() {
        let data = fixture(&[("CATEGORY", "gp")]);
        for at in [8, 12, 16, 24, 28, 32] {
            let mut bad = data.clone();
            bad[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(Sfo::parse(&bad).is_err(), "offset {at}");
        }
        let mut bad = data.clone();
        *bad.last_mut().unwrap() = b'x';
        assert!(Sfo::parse(&bad).is_err());
        assert!(Sfo::parse(&fixture(&[("CATEGORY", "gd"), ("CATEGORY", "gp")])).is_err());
        let mut bad = data;
        bad[22..24].copy_from_slice(&0x404u16.to_le_bytes());
        assert!(Sfo::parse(&bad).is_err());
    }
}
