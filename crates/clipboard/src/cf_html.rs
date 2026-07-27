const START_FRAGMENT_MARKER: &[u8] = b"<!--StartFragment-->";
const END_FRAGMENT_MARKER: &[u8] = b"<!--EndFragment-->";

pub fn decode(bytes: &[u8]) -> Option<String> {
    let range = offset_range(bytes, b"StartFragment:", b"EndFragment:")
        .or_else(|| marker_range(bytes))
        .or_else(|| offset_range(bytes, b"StartHTML:", b"EndHTML:"))?;
    Some(String::from_utf8_lossy(&bytes[range]).into_owned())
}

pub fn encode(fragment: &str) -> Vec<u8> {
    const HEADER_TEMPLATE: &str = concat!(
        "Version:1.0\r\n",
        "StartHTML:0000000000\r\n",
        "EndHTML:0000000000\r\n",
        "StartFragment:0000000000\r\n",
        "EndFragment:0000000000\r\n",
    );
    const HTML_PREFIX: &str = "<html><body>\r\n<!--StartFragment-->";
    const HTML_SUFFIX: &str = "<!--EndFragment-->\r\n</body></html>";

    let start_html = HEADER_TEMPLATE.len();
    let start_fragment = start_html + HTML_PREFIX.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + HTML_SUFFIX.len();
    let header = format!(
        "Version:1.0\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
    );

    let mut encoded = Vec::with_capacity(end_html);
    encoded.extend_from_slice(header.as_bytes());
    encoded.extend_from_slice(HTML_PREFIX.as_bytes());
    encoded.extend_from_slice(fragment.as_bytes());
    encoded.extend_from_slice(HTML_SUFFIX.as_bytes());
    encoded
}

fn offset_range(
    bytes: &[u8],
    start_label: &[u8],
    end_label: &[u8],
) -> Option<std::ops::Range<usize>> {
    let start = offset(bytes, start_label)?;
    let end = offset(bytes, end_label)?;
    (start <= end && end <= bytes.len()).then_some(start..end)
}

fn offset(bytes: &[u8], label: &[u8]) -> Option<usize> {
    let label_start = bytes
        .windows(label.len())
        .position(|value| value == label)?;
    let value_start = label_start + label.len();
    let value_end = bytes[value_start..]
        .iter()
        .position(|byte| matches!(byte, b'\r' | b'\n'))
        .map_or(bytes.len(), |end| value_start + end);
    let value = std::str::from_utf8(&bytes[value_start..value_end])
        .ok()?
        .trim();
    let value = value.parse::<i64>().ok()?;
    usize::try_from(value).ok()
}

fn marker_range(bytes: &[u8]) -> Option<std::ops::Range<usize>> {
    let start = bytes
        .windows(START_FRAGMENT_MARKER.len())
        .position(|value| value == START_FRAGMENT_MARKER)?
        + START_FRAGMENT_MARKER.len();
    let end = bytes[start..]
        .windows(END_FRAGMENT_MARKER.len())
        .position(|value| value == END_FRAGMENT_MARKER)?
        + start;
    Some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_chrome_fragment_offsets_instead_of_whole_document() {
        let fixture = b"Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000195\r\nStartFragment:0000000139\r\nEndFragment:0000000161\r\n<html><body>\r\n<!--StartFragment--><p>Chrome fragment</p><!--EndFragment-->\r\n</body></html>";

        assert_eq!(decode(fixture).as_deref(), Some("<p>Chrome fragment</p>"));
    }

    #[test]
    fn decodes_legacy_html_offsets_without_fragment_fields() {
        let fixture = b"Version:0.9\r\nStartHTML:0000000055\r\nEndHTML:0000000087\r\n<html><body>legacy</body></html>";

        assert_eq!(
            decode(fixture).as_deref(),
            Some("<html><body>legacy</body></html>")
        );
    }

    #[test]
    fn markers_recover_fragment_when_legacy_offsets_are_unusable() {
        let fixture = b"Version:1.0\r\nStartHTML:-1\r\nEndHTML:-1\r\nStartFragment:-1\r\nEndFragment:-1\r\n<html><!--StartFragment-->\xce\xbb<!--EndFragment--></html>";

        assert_eq!(decode(fixture).as_deref(), Some("λ"));
    }

    #[test]
    fn round_trip_preserves_utf8_and_emits_exact_byte_offsets() {
        let fragment = "<b>hello 📋</b>";
        let encoded = encode(fragment);

        assert_eq!(decode(&encoded).as_deref(), Some(fragment));
        let start_html = offset(&encoded, b"StartHTML:").unwrap();
        let end_html = offset(&encoded, b"EndHTML:").unwrap();
        let start_fragment = offset(&encoded, b"StartFragment:").unwrap();
        let end_fragment = offset(&encoded, b"EndFragment:").unwrap();
        assert_eq!(
            &encoded[start_html..start_fragment],
            b"<html><body>\r\n<!--StartFragment-->"
        );
        assert_eq!(&encoded[start_fragment..end_fragment], fragment.as_bytes());
        assert_eq!(
            &encoded[end_fragment..end_html],
            b"<!--EndFragment-->\r\n</body></html>"
        );
        assert_eq!(end_html, encoded.len());
    }
}
