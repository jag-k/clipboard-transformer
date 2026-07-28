//! Embedded manifest extraction from WASM modules.
//!
//! Discovery must read static plugin metadata without executing plugin code,
//! so the manifest lives in a custom section named
//! [`super::protocol::MANIFEST_SECTION_NAME`]. Core WASM sections are simple
//! enough to walk by hand: this parser only understands the outer section
//! framing and ignores every non-custom section.

use anyhow::{bail, Context, Result};

use ct_plugin_api::{PluginManifest, MANIFEST_SECTION_NAME};

/// Maximum module size discovery is willing to read.
pub const MAX_MODULE_BYTES: u64 = 64 * 1024 * 1024;
/// Maximum embedded manifest payload size.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

const WASM_MAGIC: [u8; 4] = *b"\0asm";
const CORE_MODULE_VERSION: [u8; 4] = [1, 0, 0, 0];

/// Extracts and validates the embedded manifest from raw module bytes.
pub fn extract_manifest(module: &[u8]) -> Result<PluginManifest> {
    let payload = find_custom_section(module, MANIFEST_SECTION_NAME)?
        .with_context(|| format!("missing custom section {MANIFEST_SECTION_NAME:?}"))?;
    if payload.len() > MAX_MANIFEST_BYTES {
        bail!(
            "embedded manifest is {} bytes; the limit is {MAX_MANIFEST_BYTES}",
            payload.len()
        );
    }
    let manifest: PluginManifest =
        serde_json::from_slice(payload).context("parse embedded manifest JSON")?;
    manifest.validate()?;
    Ok(manifest)
}

/// Finds a custom section by name in a core WASM module.
fn find_custom_section<'a>(module: &'a [u8], name: &str) -> Result<Option<&'a [u8]>> {
    if module.len() < 8 {
        bail!("file is too small to be a WASM module");
    }
    if module[0..4] != WASM_MAGIC {
        bail!("file does not start with the WASM magic bytes");
    }
    if module[4..8] != CORE_MODULE_VERSION {
        bail!(
            "unsupported WASM binary version {:?}; only core modules are supported",
            &module[4..8]
        );
    }

    let mut cursor = &module[8..];
    while !cursor.is_empty() {
        let section_id = cursor[0];
        cursor = &cursor[1..];
        let (size, rest) = read_leb128_u32(cursor).context("read section size")?;
        let size = size as usize;
        if rest.len() < size {
            bail!("section extends past the end of the module");
        }
        let (section, remaining) = rest.split_at(size);
        cursor = remaining;
        if section_id != 0 {
            continue;
        }
        let (name_len, name_rest) = read_leb128_u32(section).context("read section name size")?;
        let name_len = name_len as usize;
        if name_rest.len() < name_len {
            bail!("custom section name extends past the section");
        }
        let (section_name, payload) = name_rest.split_at(name_len);
        if section_name == name.as_bytes() {
            return Ok(Some(payload));
        }
    }
    Ok(None)
}

fn read_leb128_u32(bytes: &[u8]) -> Result<(u32, &[u8])> {
    let mut value: u32 = 0;
    let mut shift = 0u32;
    for (index, byte) in bytes.iter().enumerate() {
        if shift >= 32 {
            bail!("LEB128 value does not fit in 32 bits");
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, &bytes[index + 1..]));
        }
        shift += 7;
    }
    bail!("unterminated LEB128 value");
}

#[cfg(test)]
pub(crate) fn build_module_with_sections(sections: &[(&str, &[u8])]) -> Vec<u8> {
    let mut module = Vec::new();
    module.extend_from_slice(&WASM_MAGIC);
    module.extend_from_slice(&CORE_MODULE_VERSION);
    for (name, payload) in sections {
        let mut body = Vec::new();
        write_leb128_u32(&mut body, name.len() as u32);
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(payload);
        module.push(0);
        write_leb128_u32(&mut module, body.len() as u32);
        module.extend_from_slice(&body);
    }
    module
}

#[cfg(test)]
fn write_leb128_u32(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json() -> Vec<u8> {
        serde_json::json!({
            "id": "dev.example.demo",
            "name": "Demo",
            "version": "0.1.0",
            "api_version": 1,
            "rules": [{"type": "demo"}],
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn extracts_manifest_from_custom_section() {
        let module = build_module_with_sections(&[
            ("other-section", b"ignored"),
            (MANIFEST_SECTION_NAME, &manifest_json()),
        ]);
        let manifest = extract_manifest(&module).unwrap();
        assert_eq!(manifest.id, "dev.example.demo");
    }

    #[test]
    fn missing_section_is_an_error() {
        let module = build_module_with_sections(&[("other-section", b"ignored")]);
        let error = extract_manifest(&module).unwrap_err().to_string();
        assert!(error.contains(MANIFEST_SECTION_NAME), "{error}");
    }

    #[test]
    fn skips_non_custom_sections() {
        let mut module = build_module_with_sections(&[]);
        // Type section (id 1) with an empty vector of types.
        module.extend_from_slice(&[1, 1, 0]);
        let tail = build_module_with_sections(&[(MANIFEST_SECTION_NAME, &manifest_json())]);
        module.extend_from_slice(&tail[8..]);
        let manifest = extract_manifest(&module).unwrap();
        assert_eq!(manifest.id, "dev.example.demo");
    }

    #[test]
    fn rejects_non_wasm_files() {
        assert!(extract_manifest(b"not wasm at all").is_err());
        assert!(extract_manifest(b"\0asm").is_err());
    }

    #[test]
    fn rejects_truncated_sections() {
        let mut module = build_module_with_sections(&[]);
        module.extend_from_slice(&[0, 200]);
        assert!(extract_manifest(&module).is_err());
    }

    #[test]
    fn rejects_component_model_binaries() {
        let mut module = build_module_with_sections(&[]);
        module[4..8].copy_from_slice(&[13, 0, 1, 0]);
        let error = extract_manifest(&module).unwrap_err().to_string();
        assert!(error.contains("only core modules"), "{error}");
    }

    #[test]
    fn rejects_oversized_manifest_payloads() {
        let payload = vec![b' '; MAX_MANIFEST_BYTES + 1];
        let module = build_module_with_sections(&[(MANIFEST_SECTION_NAME, &payload)]);
        let error = extract_manifest(&module).unwrap_err().to_string();
        assert!(error.contains("limit"), "{error}");
    }
}
