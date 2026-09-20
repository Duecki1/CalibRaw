pub(crate) fn decode_uri_component(encoded: &str) -> Result<String, String> {
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err("truncated percent escape in Android library record".to_owned());
        }
        let high = hex_digit(bytes[index + 1])?;
        let low = hex_digit(bytes[index + 2])?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded)
        .map_err(|error| format!("Android library record is not valid UTF-8: {error}"))
}

fn hex_digit(value: u8) -> Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err("invalid percent escape in Android library record".to_owned()),
    }
}

pub(crate) fn decode_document_identity(identity: &str) -> Result<(String, String), String> {
    let (encoded_uri, encoded_display_name) = identity
        .split_once('\t')
        .ok_or_else(|| "Android document identity has no field separator".to_owned())?;
    if encoded_display_name.contains('\t') {
        return Err("Android document identity has multiple field separators".to_owned());
    }

    let uri = decode_uri_component(encoded_uri)?;
    let display_name = decode_uri_component(encoded_display_name)?;
    if uri.is_empty() || display_name.is_empty() {
        return Err("Android document identity has an empty field".to_owned());
    }

    Ok((uri, display_name))
}

#[cfg(test)]
mod tests {
    use super::{decode_document_identity, decode_uri_component};

    #[test]
    fn uri_component_decodes_percent_encoded_utf8() {
        assert_eq!(
            decode_uri_component("content%3A%2F%2Flibrary%2Fraw%252F1").unwrap(),
            "content://library/raw%2F1"
        );
        assert_eq!(decode_uri_component("caf%C3%A9").unwrap(), "café");
    }

    #[test]
    fn document_identity_decodes_newlines_tabs_unicode_and_percent() {
        let identity = concat!(
            "content%3A%2F%2Flibrary%2Fraw%252F%E2%98%83%0Apart%09two",
            "\t",
            "scan%0Apart%09%E2%98%83%25.dng"
        );

        assert_eq!(
            decode_document_identity(identity).unwrap(),
            (
                "content://library/raw%2F☃\npart\ttwo".to_owned(),
                "scan\npart\t☃%.dng".to_owned(),
            )
        );
    }

    #[test]
    fn document_identity_requires_exactly_one_literal_tab_separator() {
        assert!(decode_document_identity("content%3A%2F%2Fraw").is_err());
        assert!(decode_document_identity("uri\tname\textra").is_err());
    }

    #[test]
    fn malformed_percent_escapes_are_rejected() {
        for malformed in ["%", "%2", "%GG", "name%0Z.dng"] {
            assert!(decode_uri_component(malformed).is_err(), "accepted {malformed:?}");
        }
        assert!(decode_uri_component("%FF").is_err());
        assert!(decode_document_identity("content%3A%2F%2Fraw\tbad%2").is_err());
    }

    #[test]
    fn document_identity_rejects_empty_uri_or_display_name() {
        assert!(decode_document_identity("\tphoto.dng").is_err());
        assert!(decode_document_identity("content%3A%2F%2Fraw\t").is_err());
    }
}
