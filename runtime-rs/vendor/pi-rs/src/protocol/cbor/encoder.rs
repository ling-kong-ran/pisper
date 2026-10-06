//! CBOR encoder. Port of `packages/protocol/src/cbor/encoder.ts` — the
//! protocol's strict, definite-length RFC 8949 subset.
//!
//! Divergences D1-D3 (parent module docs): the lone-surrogate round-trip
//! check, `undefined`-value rules, cycle detection, and non-JSON JS value
//! rejection are structurally unobservable on the owned [`CborValue`] tree.
//! All reachable checks (depth, lengths, safe integers, finite floats) keep
//! the exact upstream error text.

use super::options::{
    resolve_options, CborError, CborFailure, CborOptions, ResolvedCborOptions, MAX_SAFE_INTEGER,
    MAX_UINT32,
};
use super::CborValue;

/// Encodes the protocol's strict, definite-length RFC 8949 subset.
pub fn encode_cbor(value: &CborValue, options: CborOptions) -> Result<Vec<u8>, CborFailure> {
    let resolved = resolve_options(options)?;
    let mut writer = CborWriter::new(resolved.max_byte_length);
    encode_value(&mut writer, value, &resolved, 0)?;
    Ok(writer.finish())
}

struct CborWriter {
    buffer: Vec<u8>,
    max_byte_length: u64,
}

impl CborWriter {
    fn new(max_byte_length: u64) -> CborWriter {
        CborWriter {
            buffer: Vec::with_capacity(256.min(max_byte_length as usize)),
            max_byte_length,
        }
    }

    fn write_byte(&mut self, value: u8) -> Result<(), CborError> {
        self.ensure_capacity(1)?;
        self.buffer.push(value);
        Ok(())
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), CborError> {
        self.ensure_capacity(bytes.len())?;
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn write_uint16(&mut self, value: u16) -> Result<(), CborError> {
        self.ensure_capacity(2)?;
        self.buffer.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn write_uint32(&mut self, value: u32) -> Result<(), CborError> {
        self.ensure_capacity(4)?;
        self.buffer.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn write_uint64(&mut self, value: u64) -> Result<(), CborError> {
        self.ensure_capacity(8)?;
        self.buffer.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn write_float64(&mut self, value: f64) -> Result<(), CborError> {
        self.ensure_capacity(9)?;
        self.buffer.push(0xfb);
        self.buffer
            .extend_from_slice(&value.to_bits().to_be_bytes());
        Ok(())
    }

    fn finish(self) -> Vec<u8> {
        self.buffer
    }

    fn ensure_capacity(&mut self, additional_bytes: usize) -> Result<(), CborError> {
        let required = self.buffer.len() as u64 + additional_bytes as u64;
        if required > self.max_byte_length {
            return Err(CborError::new(format!(
                "CBOR byte length exceeds configured limit of {}",
                self.max_byte_length
            )));
        }
        Ok(())
    }
}

fn write_argument(writer: &mut CborWriter, major_type: u8, value: u64) -> Result<(), CborError> {
    let prefix = major_type << 5;
    if value < 24 {
        writer.write_byte(prefix | value as u8)
    } else if value <= 0xff {
        writer.write_byte(prefix | 24)?;
        writer.write_byte(value as u8)
    } else if value <= 0xffff {
        writer.write_byte(prefix | 25)?;
        writer.write_uint16(value as u16)
    } else if value <= u64::from(MAX_UINT32) {
        writer.write_byte(prefix | 26)?;
        writer.write_uint32(value as u32)
    } else {
        writer.write_byte(prefix | 27)?;
        writer.write_uint64(value)
    }
}

fn unsafe_integer_error() -> CborError {
    CborError::new("CBOR integers must be safe JavaScript integers")
}

fn encode_text(
    writer: &mut CborWriter,
    value: &str,
    options: &ResolvedCborOptions,
) -> Result<(), CborError> {
    let bytes = value.as_bytes();
    if bytes.len() as u64 > options.max_byte_length {
        return Err(CborError::new(format!(
            "CBOR text string length exceeds configured limit of {}",
            options.max_byte_length
        )));
    }
    // D1: Rust `&str` values are valid Unicode scalar values by construction,
    // so upstream's decode-round-trip check cannot fail here.
    write_argument(writer, 3, bytes.len() as u64)?;
    writer.write_bytes(bytes)
}

fn encode_value(
    writer: &mut CborWriter,
    value: &CborValue,
    options: &ResolvedCborOptions,
    depth: u64,
) -> Result<(), CborError> {
    if depth > options.max_depth {
        return Err(CborError::new(format!(
            "CBOR nesting depth exceeds configured limit of {}",
            options.max_depth
        )));
    }

    match value {
        CborValue::Null => writer.write_byte(0xf6),
        CborValue::Bool(value) => writer.write_byte(if *value { 0xf5 } else { 0xf4 }),
        // D3: cycles are impossible in the owned tree, so the ancestors set
        // from upstream has no Rust counterpart.
        CborValue::Uint(value) => {
            if i128::from(*value) > MAX_SAFE_INTEGER {
                return Err(unsafe_integer_error());
            }
            write_argument(writer, 0, *value)
        }
        CborValue::Int(value) => {
            if i128::from(*value) < -MAX_SAFE_INTEGER {
                return Err(unsafe_integer_error());
            }
            write_argument(writer, 1, (-1 - i128::from(*value)) as u64)
        }
        CborValue::Float(value) => {
            if !value.is_finite() {
                return Err(CborError::new("CBOR numbers must be finite"));
            }
            // Upstream `Number.isInteger(value) && !Object.is(value, -0)`:
            // integral floats take the integer path (so 8.0 encodes as 0x08),
            // while -0.0 stays on the float path.
            if value.fract() == 0.0 && !(*value == 0.0 && value.is_sign_negative()) {
                if value.abs() > MAX_SAFE_INTEGER as f64 {
                    return Err(unsafe_integer_error());
                }
                if *value >= 0.0 {
                    write_argument(writer, 0, *value as u64)
                } else {
                    write_argument(writer, 1, (-1 - *value as i128) as u64)
                }
            } else {
                writer.write_float64(*value)
            }
        }
        CborValue::Text(value) => encode_text(writer, value, options),
        CborValue::Bytes(value) => {
            if value.len() as u64 > options.max_byte_length {
                return Err(CborError::new(format!(
                    "CBOR byte string length exceeds configured limit of {}",
                    options.max_byte_length
                )));
            }
            write_argument(writer, 2, value.len() as u64)?;
            writer.write_bytes(value)
        }
        // D2: arrays cannot contain holes or `undefined` on the owned tree.
        CborValue::Array(items) => {
            if items.len() as u64 > options.max_container_length {
                return Err(CborError::new(format!(
                    "CBOR array length exceeds configured limit of {}",
                    options.max_container_length
                )));
            }
            write_argument(writer, 4, items.len() as u64)?;
            for item in items {
                encode_value(writer, item, options, depth + 1)?;
            }
            Ok(())
        }
        CborValue::Map(entries) => {
            if entries.len() as u64 > options.max_container_length {
                return Err(CborError::new(format!(
                    "CBOR map length exceeds configured limit of {}",
                    options.max_container_length
                )));
            }
            write_argument(writer, 5, entries.len() as u64)?;
            for (key, value) in entries {
                encode_text(writer, key, options)?;
                encode_value(writer, value, options, depth + 1)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn encode(value: &CborValue) -> String {
        hex(&encode_cbor(value, CborOptions::new()).unwrap())
    }

    fn err(value: &CborValue) -> String {
        encode_cbor(value, CborOptions::new())
            .unwrap_err()
            .message()
            .to_string()
    }

    #[test]
    fn encodes_rfc8949_known_vectors() {
        use CborValue::*;

        // Vectors and expected wire hex from the upstream cbor.test.ts table,
        // confirmed against the node oracle (tests/fixtures/protocol_oracle).
        let vectors: Vec<(CborValue, &str)> = vec![
            (CborValue::Null, "f6"),
            (CborValue::Bool(false), "f4"),
            (CborValue::Bool(true), "f5"),
            (CborValue::Uint(0), "00"),
            (CborValue::Uint(1), "01"),
            (CborValue::Uint(10), "0a"),
            (CborValue::Uint(23), "17"),
            (CborValue::Uint(24), "1818"),
            (CborValue::Uint(25), "1819"),
            (CborValue::Uint(100), "1864"),
            (CborValue::Uint(1000), "1903e8"),
            (CborValue::Uint(1_000_000), "1a000f4240"),
            (CborValue::Uint(1_000_000_000_000), "1b000000e8d4a51000"),
            (CborValue::Uint(9007199254740991), "1b001fffffffffffff"),
            (CborValue::Int(-1), "20"),
            (CborValue::Int(-10), "29"),
            (CborValue::Int(-24), "37"),
            (CborValue::Int(-25), "3818"),
            (CborValue::Int(-100), "3863"),
            (CborValue::Int(-1000), "3903e7"),
            (CborValue::Int(-1_000_000), "3a000f423f"),
            (CborValue::Int(-9007199254740991), "3b001ffffffffffffe"),
            (CborValue::Float(1.1), "fb3ff199999999999a"),
            (CborValue::Float(-0.0), "fb8000000000000000"),
            (CborValue::Bytes(vec![1, 2, 3, 4]), "4401020304"),
            (CborValue::Text(String::new()), "60"),
            (CborValue::Text("IETF".into()), "6449455446"),
            (CborValue::Text("ü".into()), "62c3bc"),
            (CborValue::Text("水".into()), "63e6b0b4"),
            (CborValue::Text("𐅑".into()), "64f0908591"),
            (CborValue::Array(vec![]), "80"),
            (
                CborValue::Array(vec![Uint(1), Uint(2), Uint(3)]),
                "83010203",
            ),
            (
                CborValue::Array(vec![
                    Uint(1),
                    Array(vec![Uint(2), Uint(3)]),
                    Array(vec![Uint(4), Uint(5)]),
                ]),
                "8301820203820405",
            ),
            (
                CborValue::Map(vec![
                    ("a".into(), Uint(1)),
                    ("b".into(), Array(vec![Uint(2), Uint(3)])),
                ]),
                "a26161016162820203",
            ),
        ];
        for (value, wire) in &vectors {
            assert_eq!(&encode(value), wire, "vector {wire}");
        }
    }

    #[test]
    fn integral_floats_take_the_integer_path() {
        // Node oracle: encode:float-integral-8.0 etc.
        assert_eq!(encode(&CborValue::Float(8.0)), "08");
        assert_eq!(encode(&CborValue::Float(-8.0)), "27");
        assert_eq!(encode(&CborValue::Float(1e10)), "1b00000002540be400");
        assert_eq!(encode(&CborValue::Float(1.5)), "fb3ff8000000000000");
        assert_eq!(encode(&CborValue::Float(-0.0)), "fb8000000000000000");
    }

    #[test]
    fn rejects_unsafe_integers_nonfinite_floats() {
        assert_eq!(
            err(&CborValue::Uint(9007199254740992)),
            "CBOR integers must be safe JavaScript integers"
        );
        assert_eq!(
            err(&CborValue::Int(-9007199254740992)),
            "CBOR integers must be safe JavaScript integers"
        );
        assert_eq!(
            err(&CborValue::Float(9007199254740992.0)),
            "CBOR integers must be safe JavaScript integers"
        );
        assert_eq!(
            err(&CborValue::Float(2f64.powf(60.0))),
            "CBOR integers must be safe JavaScript integers"
        );
        assert_eq!(
            err(&CborValue::Float(f64::NAN)),
            "CBOR numbers must be finite"
        );
        assert_eq!(
            err(&CborValue::Float(f64::INFINITY)),
            "CBOR numbers must be finite"
        );
        assert_eq!(
            err(&CborValue::Float(f64::NEG_INFINITY)),
            "CBOR numbers must be finite"
        );
    }

    #[test]
    fn omits_nothing_and_keeps_falsey_values() {
        // Upstream: `{omitted: undefined, zero: 0, empty: "", no: false,
        // nil: null}` encodes to exactly the four falsey entries (D2: the
        // undefined property cannot exist on the Rust tree).
        let value = CborValue::Map(vec![
            ("zero".into(), CborValue::Uint(0)),
            ("empty".into(), CborValue::Text(String::new())),
            ("no".into(), CborValue::Bool(false)),
            ("nil".into(), CborValue::Null),
        ]);
        assert_eq!(
            encode(&value),
            "a4647a65726f0065656d70747960626e6ff4636e696cf6"
        );
    }

    #[test]
    fn treats_proto_keys_as_data() {
        let value = CborValue::Map(vec![("__proto__".into(), CborValue::Text("safe".into()))]);
        let decoded = super::super::decoder::decode_cbor(
            &encode_cbor(&value, CborOptions::new()).unwrap(),
            CborOptions::new(),
        )
        .unwrap();
        assert_eq!(
            decoded.get("__proto__"),
            Some(&CborValue::Text("safe".into()))
        );
    }

    #[test]
    fn all_undefined_map_reduces_to_empty() {
        // Oracle: {a: undefined, b: undefined} -> a0.
        let value = CborValue::Map(vec![]);
        assert_eq!(encode(&value), "a0");
    }

    #[test]
    fn rejects_excessive_depth() {
        // 65 nested arrays around null, upstream default depth 64.
        let mut value = CborValue::Null;
        for _ in 0..65 {
            value = CborValue::Array(vec![value]);
        }
        assert_eq!(
            err(&value),
            "CBOR nesting depth exceeds configured limit of 64"
        );
        // One level shallower encodes fine.
        let mut ok = CborValue::Null;
        for _ in 0..64 {
            ok = CborValue::Array(vec![ok]);
        }
        assert!(encode_cbor(&ok, CborOptions::new()).is_ok());
    }

    #[test]
    fn enforces_strict_caller_limits() {
        use CborValue::*;

        let value = CborValue::Array(vec![Uint(1), Uint(2), Uint(3)]);
        let error =
            encode_cbor(&value, CborOptions::new().with_max_container_length(2)).unwrap_err();
        assert_eq!(
            error.message(),
            "CBOR array length exceeds configured limit of 2"
        );
        let error = encode_cbor(
            &CborValue::Text("ab".into()),
            CborOptions::new().with_max_byte_length(2),
        )
        .unwrap_err();
        // The writer capacity limit fires before the text-length check
        // (oracle: encode:strict-enc-bytes).
        assert_eq!(
            error.message(),
            "CBOR byte length exceeds configured limit of 2"
        );
        // Oracle: 17 x's against maxByteLength 16.
        let error = encode_cbor(
            &CborValue::Text("x".repeat(17)),
            CborOptions::new().with_max_byte_length(16),
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "CBOR text string length exceeds configured limit of 16"
        );
        let error = encode_cbor(
            &CborValue::Bytes(vec![1, 2, 3]),
            CborOptions::new().with_max_byte_length(2),
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "CBOR byte string length exceeds configured limit of 2"
        );
        // The writer capacity limit also fires against total output size.
        let error = encode_cbor(
            &CborValue::Array(vec![Uint(1), Uint(2)]),
            CborOptions::new().with_max_byte_length(2),
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "CBOR byte length exceeds configured limit of 2"
        );
    }

    #[test]
    fn rejects_options_overflows() {
        let error = encode_cbor(
            &CborValue::Null,
            CborOptions::new().with_max_byte_length(4_294_967_296),
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "maxByteLength must be an integer between 0 and 4294967295"
        );
    }
}
