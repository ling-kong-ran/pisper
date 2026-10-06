//! Number formatting for boundaries whose upstream values are JS Numbers.
//! This is not a general JSON.stringify implementation: callers still own
//! object enumeration, undefined omission and their input representation.

use serde::Serialize;
use serde_json::ser::Formatter;
use std::io;

/// ECMAScript Number::toString in radix 10 (including error-label values).
/// Reuse serde_json's shortest, correctly rounded binary64 significand; only
/// its decimal-point placement and exponent notation differ from JavaScript.
/// In particular, do not use fixed precision or trim a pre-rounded decimal.
pub(crate) fn js_number_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Infinity"
        } else {
            "Infinity"
        }
        .into();
    }

    let shortest = serde_json::Number::from_f64(value.abs())
        .expect("non-finite values handled above")
        .to_string();
    let (mantissa, exponent) = shortest
        .split_once('e')
        .map_or((shortest.as_str(), 0), |(m, e)| {
            (m, e.parse::<i32>().expect("serde_json decimal exponent"))
        });
    let point = mantissa.find('.').unwrap_or(mantissa.len()) as i32;
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let leading = digits.len() - digits.trim_start_matches('0').len();
    let digits = digits.trim_start_matches('0').trim_end_matches('0');
    let n = point + exponent - leading as i32;
    let k = digits.len();
    let mut out = String::with_capacity(32);
    if value.is_sign_negative() {
        out.push('-');
    }
    if (1..=21).contains(&n) {
        let n = n as usize;
        if k <= n {
            out.push_str(digits);
            out.extend(std::iter::repeat_n('0', n - k));
        } else {
            out.push_str(&digits[..n]);
            out.push('.');
            out.push_str(&digits[n..]);
        }
    } else if (-5..=0).contains(&n) {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n) as usize));
        out.push_str(digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        let exponent = n - 1;
        if exponent >= 0 {
            out.push('+');
        }
        out.push_str(&exponent.to_string());
    }
    out
}

struct JsNumberFormatter;

impl Formatter for JsNumberFormatter {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        if value.is_finite() {
            writer.write_all(js_number_string(value).as_bytes())
        } else {
            // Error labels use NaN/Infinity; JSON values must instead be null.
            self.write_null(writer)
        }
    }

    fn write_f32<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        self.write_f64(writer, f64::from(value))
    }

    // serde_json keeps integer input as i64/u64. At this JS wire boundary those
    // are Numbers too, not BigInts; convert to binary64 before printing. All
    // i8/i16/i32/u8/u16/u32 values are exact and can use the default formatter.
    fn write_i64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: i64) -> io::Result<()> {
        self.write_f64(writer, value as f64)
    }

    fn write_u64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: u64) -> io::Result<()> {
        self.write_f64(writer, value as f64)
    }

    fn write_i128<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: i128) -> io::Result<()> {
        self.write_f64(writer, value as f64)
    }

    fn write_u128<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: u128) -> io::Result<()> {
        self.write_f64(writer, value as f64)
    }

    fn write_number_str<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        value: &str,
    ) -> io::Result<()> {
        let number = value
            .parse::<f64>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.write_f64(writer, number)
    }
}

/// Compact serde JSON, but numeric values are encoded as JavaScript Numbers.
/// Strings (including numeric-looking strings) and object order are untouched.
/// This does not fix information already discarded by a typed input boundary,
/// nor allow non-finite numbers to be parsed by serde_json's Value model.
pub(crate) fn to_json_string_with_js_numbers<T: ?Sized + Serialize>(
    value: &T,
) -> serde_json::Result<String> {
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, JsNumberFormatter);
    value.serialize(&mut serializer)?;
    String::from_utf8(bytes)
        .map_err(|error| serde_json::Error::io(io::Error::new(io::ErrorKind::InvalidData, error)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_number_writer_rounds_integer_variants_and_widens_f32() {
        let values = (
            -9_007_199_254_740_993_i64,
            9_007_199_254_740_993_u64,
            -9_007_199_254_740_993_i128,
            9_007_199_254_740_993_u128,
            0.1_f32,
        );
        assert_eq!(to_json_string_with_js_numbers(&values).unwrap(),
            "[-9007199254740992,9007199254740992,-9007199254740992,9007199254740992,0.10000000149011612]");
    }

    #[test]
    fn json_number_writer_does_not_rewrite_strings_or_object_order() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"z":"-0.0 \n \" 1e+21","2":1e-7,"a":[1.0,"9007199254740993","1e400"]}"#,
        )
        .unwrap();
        assert_eq!(
            to_json_string_with_js_numbers(&value).unwrap(),
            r#"{"z":"-0.0 \n \" 1e+21","2":1e-7,"a":[1,"9007199254740993","1e400"]}"#
        );
    }

    #[test]
    fn json_number_writer_nonfinite_values_are_null_not_error_labels() {
        assert_eq!(js_number_string(f64::NAN), "NaN");
        assert_eq!(js_number_string(f64::INFINITY), "Infinity");
        assert_eq!(js_number_string(f64::NEG_INFINITY), "-Infinity");
        assert_eq!(
            to_json_string_with_js_numbers(&[f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.0])
                .unwrap(),
            "[null,null,null,0]"
        );
        let mut bytes = Vec::new();
        JsNumberFormatter
            .write_number_str(&mut bytes, "1000000000000000128")
            .unwrap();
        assert_eq!(bytes, b"1000000000000000100");
        assert!(JsNumberFormatter
            .write_number_str(&mut Vec::new(), "not numeric")
            .is_err());
    }
}
