//! CBOR decoder. Port of `packages/protocol/src/cbor/decoder.ts` — decodes
//! exactly one item from the protocol's strict RFC 8949 subset.
//!
//! Divergences D3 (parent module docs): the input `TypeError` check is
//! type-level. All reachable rejections keep the exact upstream error text.

use std::collections::HashSet;

use super::options::{
    resolve_options, CborError, CborFailure, CborOptions, ResolvedCborOptions, MAX_SAFE_INTEGER,
    UINT32_BASE,
};
use super::CborValue;

/// Decodes exactly one item from the protocol's strict RFC 8949 subset.
pub fn decode_cbor(bytes: &[u8], options: CborOptions) -> Result<CborValue, CborFailure> {
    let resolved = resolve_options(options)?;
    if bytes.len() as u64 > resolved.max_byte_length {
        return Err(CborError::new(format!(
            "CBOR byte length exceeds configured limit of {}",
            resolved.max_byte_length
        ))
        .into());
    }
    Ok(CborReader::new(bytes, resolved).decode()?)
}

struct CborReader<'a> {
    bytes: &'a [u8],
    offset: usize,
    options: ResolvedCborOptions,
}

impl<'a> CborReader<'a> {
    fn new(bytes: &'a [u8], options: ResolvedCborOptions) -> CborReader<'a> {
        CborReader {
            bytes,
            offset: 0,
            options,
        }
    }

    fn decode(&mut self) -> Result<CborValue, CborError> {
        let value = self.read_item(0)?;
        if self.offset != self.bytes.len() {
            return Err(CborError::new("CBOR payload contains trailing data"));
        }
        Ok(value)
    }

    fn read_item(&mut self, depth: u64) -> Result<CborValue, CborError> {
        if depth > self.options.max_depth {
            return Err(CborError::new(format!(
                "CBOR nesting depth exceeds configured limit of {}",
                self.options.max_depth
            )));
        }
        let initial = self.read_byte()?;
        let major_type = initial >> 5;
        let additional_information = initial & 0x1f;

        match major_type {
            0 => Ok(CborValue::Uint(self.read_argument(additional_information)?)),
            1 => {
                let value = -1 - i128::from(self.read_argument(additional_information)?);
                if value < -MAX_SAFE_INTEGER {
                    return Err(CborError::new(
                        "Decoded CBOR integer is outside the safe range",
                    ));
                }
                Ok(CborValue::Int(value as i64))
            }
            2 => {
                let length = self.read_length(
                    additional_information,
                    "byte string",
                    self.options.max_byte_length,
                )?;
                Ok(CborValue::Bytes(self.read_bytes(length)?.to_vec()))
            }
            3 => {
                let length = self.read_length(
                    additional_information,
                    "text string",
                    self.options.max_byte_length,
                )?;
                let bytes = self.read_bytes(length)?;
                // Rust UTF-8 validation is strict (rejects overlong forms and
                // surrogate encodings) and keeps leading BOMs, matching
                // upstream's fatal TextDecoder with ignoreBOM.
                match std::str::from_utf8(bytes) {
                    Ok(value) => Ok(CborValue::Text(value.to_string())),
                    Err(_) => Err(CborError::new("CBOR text string contains invalid UTF-8")),
                }
            }
            4 => {
                let length = self.read_length(
                    additional_information,
                    "array",
                    self.options.max_container_length,
                )?;
                let mut result = Vec::new();
                for _ in 0..length {
                    result.push(self.read_item(depth + 1)?);
                }
                Ok(CborValue::Array(result))
            }
            5 => {
                let length = self.read_length(
                    additional_information,
                    "map",
                    self.options.max_container_length,
                )?;
                let mut result = Vec::new();
                let mut keys = HashSet::new();
                for _ in 0..length {
                    let key = self.read_item(depth + 1)?;
                    let CborValue::Text(key) = key else {
                        return Err(CborError::new("CBOR map keys must be strings"));
                    };
                    if keys.contains(&key) {
                        return Err(CborError::new("CBOR map contains a duplicate key"));
                    }
                    keys.insert(key.clone());
                    let value = self.read_item(depth + 1)?;
                    result.push((key, value));
                }
                Ok(CborValue::Map(result))
            }
            6 => Err(CborError::new("CBOR tags are not supported")),
            7 => self.read_simple(additional_information),
            _ => Err(CborError::new("Malformed CBOR major type")),
        }
    }

    fn read_simple(&mut self, additional_information: u8) -> Result<CborValue, CborError> {
        match additional_information {
            20 => Ok(CborValue::Bool(false)),
            21 => Ok(CborValue::Bool(true)),
            22 => Ok(CborValue::Null),
            27 => {
                let bytes = self.read_bytes(8)?;
                let mut raw = [0u8; 8];
                raw.copy_from_slice(bytes);
                let value = f64::from_bits(u64::from_be_bytes(raw));
                if !value.is_finite() {
                    return Err(CborError::new("Decoded CBOR number must be finite"));
                }
                if value == value.trunc() && value.abs() > MAX_SAFE_INTEGER as f64 {
                    return Err(CborError::new(
                        "Decoded CBOR integer is outside the safe range",
                    ));
                }
                Ok(CborValue::Float(value))
            }
            31 => Err(CborError::new("CBOR break marker is not supported")),
            _ => Err(CborError::new(
                "Unsupported CBOR simple value or floating-point width",
            )),
        }
    }

    fn read_length(
        &mut self,
        additional_information: u8,
        kind: &str,
        limit: u64,
    ) -> Result<u64, CborError> {
        if additional_information == 31 {
            return Err(CborError::new(format!(
                "Indefinite-length CBOR {kind}s are not supported"
            )));
        }
        let length = self.read_argument(additional_information)?;
        if length > limit {
            return Err(CborError::new(format!(
                "CBOR {kind} length exceeds configured limit of {limit}"
            )));
        }
        Ok(length)
    }

    fn read_argument(&mut self, additional_information: u8) -> Result<u64, CborError> {
        if additional_information < 24 {
            return Ok(u64::from(additional_information));
        }
        match additional_information {
            24 => Ok(u64::from(self.read_byte()?)),
            25 => {
                let bytes = self.read_bytes(2)?;
                Ok(u64::from(bytes[0]) * 0x100 + u64::from(bytes[1]))
            }
            26 => {
                let bytes = self.read_bytes(4)?;
                Ok(u64::from(bytes[0]) * 0x1_000_000
                    + u64::from(bytes[1]) * 0x1_0000
                    + u64::from(bytes[2]) * 0x100
                    + u64::from(bytes[3]))
            }
            27 => {
                let high = self.read_argument(26)?;
                let low = self.read_argument(26)?;
                if high > 0x1f_ffff {
                    return Err(CborError::new(
                        "Decoded CBOR integer or length is outside the safe range",
                    ));
                }
                Ok(high * UINT32_BASE + low)
            }
            31 => Err(CborError::new(
                "Indefinite-length CBOR items are not supported",
            )),
            _ => Err(CborError::new("Malformed CBOR additional information")),
        }
    }

    fn read_byte(&mut self) -> Result<u8, CborError> {
        if self.offset >= self.bytes.len() {
            return Err(CborError::new("Truncated CBOR payload"));
        }
        let value = self.bytes[self.offset];
        self.offset += 1;
        Ok(value)
    }

    fn read_bytes(&mut self, length: u64) -> Result<&'a [u8], CborError> {
        let remaining = (self.bytes.len() - self.offset) as u64;
        if length > remaining {
            return Err(CborError::new("Truncated CBOR payload"));
        }
        let start = self.offset;
        self.offset += length as usize;
        Ok(&self.bytes[start..self.offset])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_hex(hex: &str) -> Vec<u8> {
        (0..hex.len() / 2)
            .map(|index| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap())
            .collect()
    }

    fn decode(hex: &str) -> CborValue {
        decode_cbor(&from_hex(hex), CborOptions::new()).unwrap()
    }

    fn decode_err(hex: &str) -> String {
        decode_cbor(&from_hex(hex), CborOptions::new())
            .unwrap_err()
            .message()
            .to_string()
    }

    #[test]
    fn decodes_rfc8949_known_vectors() {
        assert!(matches!(decode("f6"), CborValue::Null));
        assert_eq!(decode("f4"), CborValue::Bool(false));
        assert_eq!(decode("f5"), CborValue::Bool(true));
        assert_eq!(decode("00"), CborValue::Uint(0));
        assert_eq!(decode("17"), CborValue::Uint(23));
        assert_eq!(decode("1818"), CborValue::Uint(24));
        assert_eq!(decode("1903e8"), CborValue::Uint(1000));
        assert_eq!(decode("1a000f4240"), CborValue::Uint(1_000_000));
        assert_eq!(
            decode("1b000000e8d4a51000"),
            CborValue::Uint(1_000_000_000_000)
        );
        assert_eq!(
            decode("1b001fffffffffffff"),
            CborValue::Uint(9007199254740991)
        );
        assert_eq!(decode("20"), CborValue::Int(-1));
        assert_eq!(decode("3863"), CborValue::Int(-100));
        assert_eq!(decode("3903e7"), CborValue::Int(-1000));
        assert_eq!(
            decode("3b001ffffffffffffe"),
            CborValue::Int(-9007199254740991)
        );
        assert_eq!(decode("fb3ff199999999999a"), CborValue::Float(1.1));
        // -0.0 keeps its sign bit (upstream Object.is check).
        match decode("fb8000000000000000") {
            CborValue::Float(value) => assert!(value.is_sign_negative() && value == 0.0),
            other => panic!("expected -0.0 float, got {other:?}"),
        }
        assert_eq!(decode("4401020304"), CborValue::Bytes(vec![1, 2, 3, 4]));
        assert_eq!(decode("60"), CborValue::Text(String::new()));
        assert_eq!(decode("6449455446"), CborValue::Text("IETF".into()));
        assert_eq!(decode("62c3bc"), CborValue::Text("ü".into()));
        assert_eq!(decode("63e6b0b4"), CborValue::Text("水".into()));
        assert_eq!(decode("64f0908591"), CborValue::Text("𐅑".into()));
        assert_eq!(decode("80"), CborValue::Array(vec![]));
        assert_eq!(
            decode("83010203"),
            CborValue::Array(vec![
                CborValue::Uint(1),
                CborValue::Uint(2),
                CborValue::Uint(3)
            ])
        );
        assert_eq!(
            decode("8301820203820405"),
            CborValue::Array(vec![
                CborValue::Uint(1),
                CborValue::Array(vec![CborValue::Uint(2), CborValue::Uint(3)]),
                CborValue::Array(vec![CborValue::Uint(4), CborValue::Uint(5)]),
            ])
        );
        assert_eq!(
            decode("a26161016162820203"),
            CborValue::Map(vec![
                ("a".into(), CborValue::Uint(1)),
                (
                    "b".into(),
                    CborValue::Array(vec![CborValue::Uint(2), CborValue::Uint(3)])
                ),
            ])
        );
    }

    #[test]
    fn preserves_a_leading_unicode_bom() {
        assert_eq!(decode("63efbbbf"), CborValue::Text("\u{feff}".into()));
    }

    #[test]
    fn rejects_every_upstream_invalid_input_with_upstream_text() {
        let cases: &[(&str, &str, &str)] = &[
            ("empty input", "", "Truncated CBOR payload"),
            ("truncated integer", "18", "Truncated CBOR payload"),
            (
                "reserved additional information",
                "1c",
                "Malformed CBOR additional information",
            ),
            (
                "indefinite byte string",
                "5f",
                "Indefinite-length CBOR byte strings are not supported",
            ),
            (
                "indefinite text string",
                "7f",
                "Indefinite-length CBOR text strings are not supported",
            ),
            (
                "indefinite array",
                "9f",
                "Indefinite-length CBOR arrays are not supported",
            ),
            (
                "indefinite map",
                "bf",
                "Indefinite-length CBOR maps are not supported",
            ),
            ("tag", "c000", "CBOR tags are not supported"),
            (
                "undefined",
                "f7",
                "Unsupported CBOR simple value or floating-point width",
            ),
            (
                "unsupported simple value",
                "e0",
                "Unsupported CBOR simple value or floating-point width",
            ),
            (
                "break outside an indefinite item",
                "ff",
                "CBOR break marker is not supported",
            ),
            (
                "float16",
                "f93c00",
                "Unsupported CBOR simple value or floating-point width",
            ),
            (
                "float32",
                "fa3f800000",
                "Unsupported CBOR simple value or floating-point width",
            ),
            (
                "positive infinity",
                "fb7ff0000000000000",
                "Decoded CBOR number must be finite",
            ),
            (
                "NaN",
                "fb7ff8000000000000",
                "Decoded CBOR number must be finite",
            ),
            ("truncated float64", "fb3ff00000", "Truncated CBOR payload"),
            (
                "truncated byte string",
                "44010203",
                "Truncated CBOR payload",
            ),
            ("truncated text string", "636162", "Truncated CBOR payload"),
            ("truncated array", "8201", "Truncated CBOR payload"),
            ("truncated map", "a16161", "Truncated CBOR payload"),
            (
                "trailing data",
                "0000",
                "CBOR payload contains trailing data",
            ),
            (
                "non-string map key",
                "a10102",
                "CBOR map keys must be strings",
            ),
            (
                "duplicate map key",
                "a2616101616102",
                "CBOR map contains a duplicate key",
            ),
            (
                "invalid UTF-8 byte",
                "61ff",
                "CBOR text string contains invalid UTF-8",
            ),
            (
                "overlong UTF-8",
                "62c080",
                "CBOR text string contains invalid UTF-8",
            ),
            (
                "UTF-8 surrogate",
                "63eda080",
                "CBOR text string contains invalid UTF-8",
            ),
            (
                "unsafe positive integer",
                "1b0020000000000000",
                "Decoded CBOR integer or length is outside the safe range",
            ),
            (
                "unsafe negative integer",
                "3b001fffffffffffff",
                "Decoded CBOR integer is outside the safe range",
            ),
            (
                "unsafe integer encoded as float64",
                "fb4340000000000000",
                "Decoded CBOR integer is outside the safe range",
            ),
            (
                "unsafe negative float64",
                "fbc340000000000000",
                "Decoded CBOR integer is outside the safe range",
            ),
        ];
        for (label, wire, message) in cases {
            assert_eq!(&decode_err(wire), message, "case {label}");
        }
    }

    #[test]
    fn enforces_depth_limit_before_traversing_values() {
        let too_deep: Vec<u8> = std::iter::repeat_n(0x81, 65)
            .chain(std::iter::once(0xf6))
            .collect();
        let error = decode_cbor(&too_deep, CborOptions::new()).unwrap_err();
        assert_eq!(
            error.message(),
            "CBOR nesting depth exceeds configured limit of 64"
        );
        // One level shallower decodes fine.
        let ok: Vec<u8> = std::iter::repeat_n(0x81, 64)
            .chain(std::iter::once(0xf6))
            .collect();
        assert!(decode_cbor(&ok, CborOptions::new()).is_ok());
    }

    #[test]
    fn rejects_declared_lengths_over_limits_before_reading_payload() {
        let oversized = |prefix: &str, length: u64| format!("{prefix}{:08x}", length);
        let byte_length_limit = decode_cbor(
            &from_hex(&oversized("5a", 16 * 1024 * 1024 + 1)),
            CborOptions::new(),
        )
        .unwrap_err();
        assert_eq!(
            byte_length_limit.message(),
            "CBOR byte string length exceeds configured limit of 16777216"
        );
        let text_limit = decode_cbor(
            &from_hex(&oversized("7a", 16 * 1024 * 1024 + 1)),
            CborOptions::new(),
        )
        .unwrap_err();
        assert_eq!(
            text_limit.message(),
            "CBOR text string length exceeds configured limit of 16777216"
        );
        let array_limit =
            decode_cbor(&from_hex(&oversized("9a", 1_000_001)), CborOptions::new()).unwrap_err();
        assert_eq!(
            array_limit.message(),
            "CBOR array length exceeds configured limit of 1000000"
        );
        let map_limit =
            decode_cbor(&from_hex(&oversized("ba", 1_000_001)), CborOptions::new()).unwrap_err();
        assert_eq!(
            map_limit.message(),
            "CBOR map length exceeds configured limit of 1000000"
        );
    }

    #[test]
    fn supports_stricter_caller_limits() {
        let error = decode_cbor(
            &from_hex("83010203"),
            CborOptions::new().with_max_container_length(2),
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "CBOR array length exceeds configured limit of 2"
        );
        let error = decode_cbor(
            &from_hex("626162"),
            CborOptions::new().with_max_byte_length(2),
        )
        .unwrap_err();
        // The 3-byte input exceeds the overall byte limit first (oracle:
        // decode:strict-bytes).
        assert_eq!(
            error.message(),
            "CBOR byte length exceeds configured limit of 2"
        );
        // Input larger than the overall byte limit is rejected up front.
        let error = decode_cbor(
            &from_hex("83010203"),
            CborOptions::new().with_max_byte_length(2),
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "CBOR byte length exceeds configured limit of 2"
        );
    }

    #[test]
    fn float_decodes_stay_floats() {
        assert_eq!(decode("fb3ff8000000000000"), CborValue::Float(1.5));
        // Integral safe floats keep the float representation, like JS numbers.
        assert_eq!(decode("fb4020000000000000"), CborValue::Float(8.0));
    }
}
