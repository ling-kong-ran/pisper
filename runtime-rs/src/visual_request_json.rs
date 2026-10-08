//! 视觉 PUT 请求的 JSON.parse 边界：严格 JSON 语法、UTF-16 错误位置及响应字符串清洗。
//!
//! serde_json 无法表达孤立代理项和非有限数字。视觉偏好只以 JS 字符串强制转换
//! 消费 model，所以代理项按响应契约清洗，正负无穷以对应字符串保存。该表示不能
//! 用于需要保留 JS 数字类型的其他请求接口。
use serde_json::{Map, Number, Value};

/// 解析完整的严格 JSON 文档，不施加比请求体边界更小的深度或数字长度限制。
pub(crate) fn parse(input: &str) -> Result<Value, String> {
    Parser {
        source: input.encode_utf16().collect(),
        position: 0,
        frames: Vec::new(),
    }
    .document()
}

/// Value 的默认析构递归访问容器；请求边界必须用此方法释放任意深度的输入。
pub(crate) fn dispose(value: Value) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(values) => pending.extend(values),
            Value::Object(values) => pending.extend(values.into_iter().map(|(_, value)| value)),
            _ => {}
        }
    }
}

/// 视觉偏好消费的 String(model || '')；数组连接与输入析构均不使用 Rust 调用栈。
/// Node 的数组连接可能受 V8 可用调用栈影响而抛 RangeError；这里没有人为深度阈值。
pub(crate) fn checked_string_like_model(value: &Value) -> Result<String, String> {
    if matches!(value, Value::Null | Value::Bool(false))
        || matches!(value, Value::Number(value) if value.as_f64() == Some(0.0))
    {
        return Ok(String::new());
    }
    enum Part<'a> {
        Value(&'a Value),
        Comma,
    }
    let mut pending = vec![Part::Value(value)];
    let mut output = String::new();
    while let Some(part) = pending.pop() {
        match part {
            Part::Comma => output.push(','),
            Part::Value(value) => match value {
                Value::Null => {}
                Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
                Value::String(value) => output.push_str(value),
                Value::Number(value) => output.push_str(&number_string(value)?),
                Value::Array(values) => {
                    for (index, value) in values.iter().enumerate().rev() {
                        pending.push(Part::Value(value));
                        if index > 0 {
                            pending.push(Part::Comma);
                        }
                    }
                }
                Value::Object(values) => {
                    if values.contains_key("toString") {
                        return Err("Cannot convert object to primitive value".into());
                    }
                    output.push_str("[object Object]");
                }
            },
        }
    }
    Ok(output)
}

fn number_string(value: &Number) -> Result<String, String> {
    let number = value.as_f64().unwrap_or_default();
    if number == 0.0 {
        return Ok("0".into());
    }
    // serde 的最短十进制采用偶数舍入；Rust Display 在 .25 一类等距情况
    // 可选择不同末位，不能用于 JS 模型 ID 字符串（例如 900719925474099.2）。
    let text = Number::from_f64(number)
        .ok_or("Cannot convert number to string")?
        .to_string();
    let (sign, text) = if let Some(text) = text.strip_prefix('-') {
        ("-", text)
    } else {
        ("", text.as_str())
    };
    let (mantissa, exponent) = if let Some((mantissa, exponent)) = text.split_once('e') {
        (
            mantissa,
            exponent
                .parse::<i32>()
                .map_err(|_| "Cannot convert number to string")?,
        )
    } else {
        (text, 0)
    };
    let point = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
    let mut digits = mantissa.replace('.', "");
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    let length = digits.len() as i32;
    let mut output = String::from(sign);
    // ECMA Number::toString 的小数点与指数布局边界为 1e-6 和 1e21。
    if length <= point && point <= 21 {
        output.push_str(&digits);
        output.extend(std::iter::repeat_n('0', (point - length) as usize));
    } else if point > 0 && point <= 21 {
        let point = point as usize;
        output.push_str(&digits[..point]);
        output.push('.');
        output.push_str(&digits[point..]);
    } else if point > -6 && point <= 0 {
        output.push_str("0.");
        output.extend(std::iter::repeat_n('0', (-point) as usize));
        output.push_str(&digits);
    } else {
        output.push_str(&digits[..1]);
        if digits.len() > 1 {
            output.push('.');
            output.push_str(&digits[1..]);
        }
        output.push('e');
        if point > 0 {
            output.push('+');
        }
        output.push_str(&(point - 1).to_string());
    }
    Ok(output)
}

#[derive(Clone, Copy)]
enum Stage {
    ObjectKey { first: bool },
    ObjectColon,
    ObjectValue,
    ObjectWaiting,
    ObjectComma,
    ArrayValue { first: bool },
    ArrayWaiting,
    ArrayComma,
}

enum Container {
    Object {
        values: Map<String, Value>,
        key: String,
    },
    Array(Vec<Value>),
}

struct Frame {
    container: Container,
    stage: Stage,
}

impl Frame {
    fn value(self) -> Value {
        match self.container {
            Container::Object { values, .. } => Value::Object(values),
            Container::Array(values) => Value::Array(values),
        }
    }
}

struct Parser {
    source: Vec<u16>,
    position: usize,
    frames: Vec<Frame>,
}

impl Drop for Parser {
    fn drop(&mut self) {
        // 语法失败时栈内仍可能持有很深的已完成子树。
        for frame in self.frames.drain(..) {
            dispose(frame.value());
        }
    }
}

impl Parser {
    fn document(&mut self) -> Result<Value, String> {
        let mut completed = self.value()?;
        loop {
            if let Some(value) = completed.take() {
                let Some(frame) = self.frames.last_mut() else {
                    self.whitespace();
                    if self.current().is_some() {
                        dispose(value);
                        return Err(self.at("Unexpected non-whitespace character after JSON"));
                    }
                    return Ok(value);
                };
                match &mut frame.container {
                    Container::Object { values, key } => {
                        if let Some(previous) = values.insert(std::mem::take(key), value) {
                            // 重复属性采用最后一个值，并安全释放被覆盖的子树。
                            dispose(previous);
                        }
                        frame.stage = Stage::ObjectComma;
                    }
                    Container::Array(values) => {
                        values.push(value);
                        frame.stage = Stage::ArrayComma;
                    }
                }
            }

            let Some(stage) = self.frames.last().map(|frame| frame.stage) else {
                // 首个值是标量时已在上方返回；容器则一直保留一个 frame。
                return Err(self.unexpected());
            };
            match stage {
                Stage::ObjectKey { first } => {
                    self.whitespace();
                    if first && self.current() == Some(b'}' as u16) {
                        self.position += 1;
                        completed = self.close();
                        continue;
                    }
                    if self.current() != Some(b'"' as u16) {
                        return Err(self.at(if first {
                            "Expected property name or '}'"
                        } else {
                            "Expected double-quoted property name"
                        }));
                    }
                    let key = self.string()?;
                    if let Some(Frame {
                        container: Container::Object { key: target, .. },
                        stage,
                    }) = self.frames.last_mut()
                    {
                        *target = key;
                        *stage = Stage::ObjectColon;
                    }
                }
                Stage::ObjectColon => {
                    self.whitespace();
                    if self.current() != Some(b':' as u16) {
                        return Err(self.at("Expected ':' after property name"));
                    }
                    self.position += 1;
                    self.stage(Stage::ObjectValue);
                }
                Stage::ObjectValue => {
                    self.stage(Stage::ObjectWaiting);
                    completed = self.value()?;
                }
                Stage::ArrayValue { first } => {
                    self.whitespace();
                    if first && self.current() == Some(b']' as u16) {
                        self.position += 1;
                        completed = self.close();
                        continue;
                    }
                    self.stage(Stage::ArrayWaiting);
                    completed = self.value()?;
                }
                Stage::ObjectComma | Stage::ArrayComma => {
                    self.whitespace();
                    let object = matches!(stage, Stage::ObjectComma);
                    let close = if object { b'}' } else { b']' } as u16;
                    if self.current() == Some(close) {
                        self.position += 1;
                        completed = self.close();
                    } else if self.current() == Some(b',' as u16) {
                        self.position += 1;
                        self.stage(if object {
                            Stage::ObjectKey { first: false }
                        } else {
                            Stage::ArrayValue { first: false }
                        });
                    } else {
                        return Err(self.at(if object {
                            "Expected ',' or '}' after property value"
                        } else {
                            "Expected ',' or ']' after array element"
                        }));
                    }
                }
                // 等待状态下面必有正在解析的子容器，不会成为当前栈顶。
                Stage::ObjectWaiting | Stage::ArrayWaiting => return Err(self.unexpected()),
            }
        }
    }

    fn stage(&mut self, stage: Stage) {
        if let Some(frame) = self.frames.last_mut() {
            frame.stage = stage;
        }
    }

    fn close(&mut self) -> Option<Value> {
        self.frames.pop().map(Frame::value)
    }

    fn current(&self) -> Option<u16> {
        self.source.get(self.position).copied()
    }

    fn whitespace(&mut self) {
        while matches!(self.current(), Some(0x20 | 0x09 | 0x0a | 0x0d)) {
            self.position += 1;
        }
    }

    fn value(&mut self) -> Result<Option<Value>, String> {
        self.whitespace();
        match self.current() {
            Some(0x22) => self.string().map(|value| Some(Value::String(value))),
            Some(0x2d | 0x30..=0x39) => self.number().map(Some),
            Some(0x74) => self.literal("true", Value::Bool(true)).map(Some),
            Some(0x66) => self.literal("false", Value::Bool(false)).map(Some),
            Some(0x6e) => self.literal("null", Value::Null).map(Some),
            Some(0x7b) => {
                self.position += 1;
                self.frames.push(Frame {
                    container: Container::Object {
                        values: Map::new(),
                        key: String::new(),
                    },
                    stage: Stage::ObjectKey { first: true },
                });
                Ok(None)
            }
            Some(0x5b) => {
                self.position += 1;
                self.frames.push(Frame {
                    container: Container::Array(Vec::new()),
                    stage: Stage::ArrayValue { first: true },
                });
                Ok(None)
            }
            _ => Err(self.unexpected()),
        }
    }

    fn literal(&mut self, literal: &str, value: Value) -> Result<Value, String> {
        for unit in literal.bytes() {
            if self.current() != Some(unit as u16) {
                return Err(self.unexpected());
            }
            self.position += 1;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.position;
        if self.current() == Some(0x2d) {
            self.position += 1;
        }
        match self.current() {
            Some(0x30) => {
                self.position += 1;
                if matches!(self.current(), Some(0x30..=0x39)) {
                    return Err(self.at("Unexpected number"));
                }
            }
            Some(0x31..=0x39) => self.digits(),
            _ => return Err(self.at("No number after minus sign")),
        }
        if self.current() == Some(0x2e) {
            self.position += 1;
            if !matches!(self.current(), Some(0x30..=0x39)) {
                return Err(self.at("Unterminated fractional number"));
            }
            self.digits();
        }
        if matches!(self.current(), Some(0x65 | 0x45)) {
            self.position += 1;
            if matches!(self.current(), Some(0x2b | 0x2d)) {
                self.position += 1;
            }
            if !matches!(self.current(), Some(0x30..=0x39)) {
                return Err(self.at("Exponent part is missing a number"));
            }
            self.digits();
        }
        let token = String::from_utf16_lossy(&self.source[start..self.position]);
        // JSON.parse 的全部数字采用 IEEE-754；不能保留 serde 的超安全整数精度。
        let number = token
            .parse::<f64>()
            .map_err(|_| self.at("Unexpected number"))?;
        if number.is_infinite() {
            Ok(Value::String(if number.is_sign_negative() {
                "-Infinity".into()
            } else {
                "Infinity".into()
            }))
        } else if number == 0.0 {
            Ok(Value::Number(Number::from(0)))
        } else {
            Number::from_f64(number)
                .map(Value::Number)
                .ok_or_else(|| self.at("Unexpected number"))
        }
    }

    fn digits(&mut self) {
        while matches!(self.current(), Some(0x30..=0x39)) {
            self.position += 1;
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.position += 1;
        let mut units = Vec::new();
        loop {
            match self.current() {
                None => return Err(self.at("Unterminated string")),
                Some(0x22) => {
                    self.position += 1;
                    // 成对代理项保留；孤立项与 release 响应的 U+FFFD 清洗一致。
                    return Ok(String::from_utf16_lossy(&units));
                }
                Some(0x5c) => {
                    self.position += 1;
                    let escaped = match self.current() {
                        Some(0x22) => 0x22,
                        Some(0x5c) => 0x5c,
                        Some(0x2f) => 0x2f,
                        Some(0x62) => 0x08,
                        Some(0x66) => 0x0c,
                        Some(0x6e) => 0x0a,
                        Some(0x72) => 0x0d,
                        Some(0x74) => 0x09,
                        Some(0x75) => {
                            let mut unit = 0;
                            for _ in 0..4 {
                                self.position += 1;
                                let digit = match self.current() {
                                    Some(value @ 0x30..=0x39) => value - 0x30,
                                    Some(value @ 0x41..=0x46) => value - 0x41 + 10,
                                    Some(value @ 0x61..=0x66) => value - 0x61 + 10,
                                    _ => return Err(self.at("Bad Unicode escape")),
                                };
                                unit = unit * 16 + digit;
                            }
                            unit
                        }
                        Some(0x100..=0xffff) | None => return Err(self.unexpected()),
                        _ => return Err(self.at("Bad escaped character")),
                    };
                    units.push(escaped);
                    self.position += 1;
                }
                Some(0x00..=0x1f) => return Err(self.at("Bad control character in string literal")),
                Some(unit) => {
                    units.push(unit);
                    self.position += 1;
                }
            }
        }
    }

    fn at(&self, message: &str) -> String {
        let mut line = 1;
        let mut line_start = 0;
        let mut cursor = 0;
        while cursor < self.position {
            if self.source[cursor] == 0x0d
                && cursor + 1 < self.position
                && self.source[cursor + 1] == 0x0a
            {
                cursor += 1;
            }
            if matches!(self.source[cursor], 0x0d | 0x0a) {
                line += 1;
                line_start = cursor + 1;
            }
            cursor += 1;
        }
        let context = if message == "Unexpected non-whitespace character after JSON" {
            ""
        } else {
            " in JSON"
        };
        format!(
            "{message}{context} at position {} (line {line} column {})",
            self.position,
            self.position - line_start + 1
        )
    }

    fn unexpected(&self) -> String {
        let Some(unit) = self.current() else {
            return "Unexpected end of JSON input".into();
        };
        if matches!(unit, 0x2d | 0x30..=0x39) {
            return self.at("Unexpected number");
        }
        if unit == 0x22 {
            return self.at("Unexpected string");
        }
        let original = String::from_utf16_lossy(&self.source);
        // V8 对这些完整的非 JSON 输入有固定诊断；只改变错误文案，不接受它们。
        if matches!(
            original.as_str(),
            "undefined" | "NaN" | "Infinity" | "[object Object]"
        ) {
            return format!("\"{original}\" is not valid JSON");
        }
        let token = String::from_utf16_lossy(&[unit]);
        let length = self.source.len();
        let (start, end, before, after) = if length < 21 {
            (0, length, "", "")
        } else if self.position < 10 {
            (0, self.position + 10, "", "...")
        } else if self.position < length - 10 {
            (self.position - 10, self.position + 10, "...", "...")
        } else {
            (self.position - 10, length, "...", "")
        };
        let snippet = String::from_utf16_lossy(&self.source[start..end]);
        format!("Unexpected token '{token}', {before}\"{snippet}\"{after} is not valid JSON")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node24_diagnostics_preserve_grammar_cursor_and_utf16_locations() {
        let cases = [
            ("", "Unexpected end of JSON input"),
            (
                "{",
                "Expected property name or '}' in JSON at position 1 (line 1 column 2)",
            ),
            (
                "{\"x\":0,}",
                "Expected double-quoted property name in JSON at position 7 (line 1 column 8)",
            ),
            (
                "{\"x\" 0}",
                "Expected ':' after property name in JSON at position 5 (line 1 column 6)",
            ),
            (
                "{\"x\":0",
                "Expected ',' or '}' after property value in JSON at position 6 (line 1 column 7)",
            ),
            (
                "[0 1]",
                "Expected ',' or ']' after array element in JSON at position 3 (line 1 column 4)",
            ),
            ("[0,]", "Unexpected token ']', \"[0,]\" is not valid JSON"),
            (
                "01",
                "Unexpected number in JSON at position 1 (line 1 column 2)",
            ),
            (
                "-",
                "No number after minus sign in JSON at position 1 (line 1 column 2)",
            ),
            (
                "1.",
                "Unterminated fractional number in JSON at position 2 (line 1 column 3)",
            ),
            (
                "1e+",
                "Exponent part is missing a number in JSON at position 3 (line 1 column 4)",
            ),
            (
                "\"x",
                "Unterminated string in JSON at position 2 (line 1 column 3)",
            ),
            ("\"x\\", "Unexpected end of JSON input"),
            (
                "\"\\x\"",
                "Bad escaped character in JSON at position 2 (line 1 column 3)",
            ),
            (
                "\"\\u12z4\"",
                "Bad Unicode escape in JSON at position 5 (line 1 column 6)",
            ),
            (
                "\"x\n\"",
                "Bad control character in string literal in JSON at position 2 (line 1 column 3)",
            ),
            (
                "{\r\n \"x\":0,\r\n}",
                "Expected double-quoted property name in JSON at position 12 (line 3 column 1)",
            ),
            (
                "[\"😀\" 0]",
                "Expected ',' or ']' after array element in JSON at position 6 (line 1 column 7)",
            ),
            (
                "{}\r\nx",
                "Unexpected non-whitespace character after JSON at position 4 (line 2 column 1)",
            ),
            ("NaN", "\"NaN\" is not valid JSON"),
            (
                "x                    ",
                "Unexpected token 'x', \"x         \"... is not valid JSON",
            ),
            (
                "          x          ",
                "Unexpected token 'x', ...\"          x         \"... is not valid JSON",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(parse(input).unwrap_err(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn strict_json_does_not_accept_non_json_whitespace_or_number_extensions() {
        for input in [
            "\u{feff}{}",
            "\u{a0}{}",
            "\u{2028}null",
            "/*x*/null",
            "undefined",
            "Infinity",
            "-Infinity",
            "+1",
            ".1",
            "0x10",
            "[1,]",
            "{x:0}",
            "{\"x\":0,}",
            "\"\\v\"",
        ] {
            assert!(parse(input).is_err(), "input: {input:?}");
        }
        for input in [
            " \r\n\t null \r\n",
            "[true,false,null,-0,0.1,1E+2]",
            "{\"x\":{\"y\":[]}}",
            "{\"model\":1e999}",
            "{\"model\":\"\\ud800\"}",
        ] {
            dispose(parse(input).unwrap());
        }
    }

    #[test]
    fn model_coercion_preserves_ieee754_lone_surrogates_and_array_join() {
        let cases = [
            ("null", ""),
            ("false", ""),
            ("-0", ""),
            ("true", "true"),
            ("1e-999", ""),
            ("1e999", "Infinity"),
            ("-1e999", "-Infinity"),
            ("9007199254740993", "9007199254740992"),
            ("900719925474099.3", "900719925474099.2"),
            ("1000000000000000128", "1000000000000000100"),
            ("1e21", "1e+21"),
            ("1e-6", "0.000001"),
            ("1e-7", "1e-7"),
            ("\"\\ud800\\ud800\\udc00\\udc00\"", "�𐀀�"),
            ("\"\\\\ud800\"", "\\ud800"),
            (
                "[null,false,0,1e999,-1e999,[null,\"x\"],{}]",
                ",false,0,Infinity,-Infinity,,x,[object Object]",
            ),
            ("{\"valueOf\":null}", "[object Object]"),
        ];
        for (input, expected) in cases {
            let value = parse(input).unwrap();
            assert_eq!(
                checked_string_like_model(&value).unwrap(),
                expected,
                "input: {input:?}"
            );
            dispose(value);
        }
        for input in ["{\"toString\":null}", "[{\"toString\":\"x\"}]"] {
            let value = parse(input).unwrap();
            assert_eq!(
                checked_string_like_model(&value).unwrap_err(),
                "Cannot convert object to primitive value"
            );
            dispose(value);
        }
    }

    #[test]
    fn deep_ignored_values_duplicate_properties_and_errors_are_stack_safe() {
        let depth = 20_000;
        let array = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        let object = format!("{{\"ignored\":{array},\"model\":\"x\"}}");
        let input = parse(&object).unwrap();
        assert_eq!(checked_string_like_model(&input["model"]).unwrap(), "x");
        dispose(input);
        dispose(parse(&format!("{{\"x\":{array},\"x\":0}}")).unwrap());
        assert_eq!(
            parse(&format!("[{array},")).unwrap_err(),
            "Unexpected end of JSON input"
        );
        assert!(parse(&format!("{array} x")).is_err());
        let model = parse(&array).unwrap();
        assert_eq!(checked_string_like_model(&model).unwrap(), "0");
        dispose(model);
    }
}
