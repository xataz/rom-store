//! Decode XML without losing the encodings accepted by the previous server.
//! Parsing remains delegated to roxmltree, with DTD/entity declarations disabled.
use anyhow::{bail, Context, Result};
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};

pub fn decode(bytes: &[u8]) -> Result<String> {
    let (encoding, skip) = if let Some(bom) = Encoding::for_bom(bytes) {
        bom
    } else if bytes.starts_with(b"<\0?\0") {
        (UTF_16LE, 0)
    } else if bytes.starts_with(b"\0<\0?") {
        (UTF_16BE, 0)
    } else {
        let label = if bytes.starts_with(b"<?xml") {
            let end = bytes
                .windows(2)
                .position(|pair| pair == b"?>")
                .context("Déclaration XML incomplète")?;
            let declaration =
                std::str::from_utf8(&bytes[..end]).context("Déclaration XML invalide")?;
            declaration.split_once("encoding").and_then(|(_, tail)| {
                let tail = tail.trim_start().strip_prefix('=')?.trim_start();
                let quote = tail.chars().next()?;
                if !matches!(quote, '\'' | '"') {
                    return None;
                }
                tail[1..].split_once(quote).map(|(label, _)| label)
            })
        } else {
            None
        };
        if label.is_some_and(|label| {
            matches!(
                label.to_ascii_lowercase().as_str(),
                "iso-8859-1" | "iso_8859-1" | "latin1"
            )
        }) {
            return Ok(bytes.iter().map(|byte| char::from(*byte)).collect());
        }
        let encoding = match label {
            Some(label) => {
                Encoding::for_label(label.as_bytes()).context("Encodage XML non pris en charge")?
            }
            None => UTF_8,
        };
        (encoding, 0)
    };
    let (text, malformed) = encoding.decode_without_bom_handling(&bytes[skip..]);
    if malformed {
        bail!("Octets invalides pour l'encodage XML");
    }
    Ok(text.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_utf16_and_latin1_preserve_metadata_without_entities() {
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><gameList><name>Éclair 🎮</name></gameList>";
        for big_endian in [false, true] {
            let mut bytes = if big_endian {
                vec![0xfe, 0xff]
            } else {
                vec![0xff, 0xfe]
            };
            for unit in xml.encode_utf16() {
                bytes.extend_from_slice(&if big_endian {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
            let decoded = decode(&bytes).unwrap();
            let document = roxmltree::Document::parse(&decoded).unwrap();
            assert_eq!(
                document
                    .descendants()
                    .find(|node| node.has_tag_name("name"))
                    .unwrap()
                    .text(),
                Some("Éclair 🎮")
            );
        }
        let decoded = decode(b"<?xml version='1.0' encoding='ISO-8859-1'?><gameList><name>\xc9clair</name></gameList>").unwrap();
        assert!(decoded.contains("Éclair"));
        let decoded = decode(b"\xef\xbb\xbf<gameList/>").unwrap();
        assert_eq!(decoded, "<gameList/>");
        assert!(decode(b"<gameList>\xff</gameList>").is_err());
        assert!(decode(b"<?xml version='1.0' encoding='unknown'?><gameList/>").is_err());
    }
}
