//! A JSON reader for the files the daemon meets: Volumio's configuration
//! store (`{ "key": { "type": "string", "value": "..." } }`), the plugin's
//! lines over the control socket, and the catalogue it writes itself. The
//! whole of RFC 8259 is read; nothing is written here but by `Value::to_json`.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    /// `text` as one JSON value, with nothing but whitespace after it.
    pub fn parse(text: &str) -> Result<Value, Error> {
        let mut p = Parser {
            s: text.as_bytes(),
            i: 0,
            depth: 0,
        };
        p.skip_ws();
        let v = p.value()?;
        p.skip_ws();
        if p.i != p.s.len() {
            return Err(p.err("trailing characters after the value"));
        }
        Ok(v)
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(m) => m.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The value as JSON text, on one line, keys in order.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => write_number(*n, out),
            Value::String(s) => write_string(s, out),
            Value::Array(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Value::Object(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// A JSON number: integers without a fraction, the rest as Rust prints them.
fn write_number(n: f64, out: &mut String) {
    if !n.is_finite() {
        out.push_str("null");
    } else if n.fract() == 0.0 && n.abs() < 1e15 {
        out.push_str(&format!("{}", n as i64));
    } else {
        out.push_str(&format!("{n}"));
    }
}

/// A JSON string literal: quotes, backslashes and control characters escaped.
pub fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Byte offset into the text.
    pub at: usize,
    pub what: &'static str,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "json: {} at byte {}", self.what, self.at)
    }
}

impl std::error::Error for Error {}

/// Nesting beyond this is refused rather than recursed into.
const MAX_DEPTH: usize = 128;

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, what: &'static str) -> Error {
        Error { at: self.i, what }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.i += 1;
        }
    }

    fn expect(&mut self, b: u8, what: &'static str) -> Result<(), Error> {
        if self.peek() == Some(b) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err(what))
        }
    }

    fn value(&mut self) -> Result<Value, Error> {
        match self.peek() {
            None => Err(self.err("unexpected end of text")),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') => self.literal(b"true", Value::Bool(true)),
            Some(b'f') => self.literal(b"false", Value::Bool(false)),
            Some(b'n') => self.literal(b"null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn literal(&mut self, word: &[u8], v: Value) -> Result<Value, Error> {
        if self.s[self.i..].starts_with(word) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("unknown literal"))
        }
    }

    fn enter(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.err("nested too deep"));
        }
        Ok(())
    }

    fn object(&mut self) -> Result<Value, Error> {
        self.enter()?;
        self.i += 1;
        let mut m = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::Object(m));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected a string key"));
            }
            let k = self.string()?;
            self.skip_ws();
            self.expect(b':', "expected ':' after the key")?;
            self.skip_ws();
            let v = self.value()?;
            // A key written twice: the later wins, as every reader does.
            m.insert(k, v);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Value::Object(m));
                }
                _ => return Err(self.err("expected ',' or '}' in an object")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, Error> {
        self.enter()?;
        self.i += 1;
        let mut a = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::Array(a));
        }
        loop {
            self.skip_ws();
            a.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Value::Array(a));
                }
                _ => return Err(self.err("expected ',' or ']' in an array")),
            }
        }
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        match self.peek() {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(self.err("expected a digit")),
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("expected a digit after '.'"));
            }
            self.digits();
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.i += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.i += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("expected a digit in the exponent"));
            }
            self.digits();
        }
        // The grammar above admits only what f64's parser accepts.
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| self.err("number"))?;
        match text.parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(Value::Number(n)),
            _ => Err(Error {
                at: start,
                what: "number out of range",
            }),
        }
    }

    fn digits(&mut self) {
        while let Some(b'0'..=b'9') = self.peek() {
            self.i += 1;
        }
    }

    fn string(&mut self) -> Result<String, Error> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let b = self.peek().ok_or_else(|| self.err("unterminated string"))?;
            match b {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.i += 1;
                    let e = self.peek().ok_or_else(|| self.err("unterminated escape"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let c = if (0xD800..0xDC00).contains(&hi) {
                                // A surrogate pair: the low half must follow as another escape.
                                if !self.s[self.i..].starts_with(b"\\u") {
                                    return Err(self.err("lone high surrogate"));
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("bad low surrogate"));
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err(self.err("lone low surrogate"));
                            } else {
                                hi
                            };
                            out.push(char::from_u32(c).ok_or_else(|| self.err("bad code point"))?);
                        }
                        _ => {
                            return Err(Error {
                                at: self.i - 1,
                                what: "unknown escape",
                            })
                        }
                    }
                }
                0..=0x1F => return Err(self.err("control character in a string")),
                _ => {
                    // One UTF-8 sequence, however long; the text was valid UTF-8 to begin with.
                    let start = self.i;
                    self.i += 1;
                    while let Some(0x80..=0xBF) = self.peek() {
                        self.i += 1;
                    }
                    out.push_str(
                        std::str::from_utf8(&self.s[start..self.i])
                            .map_err(|_| self.err("bad UTF-8"))?,
                    );
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        let end = self.i + 4;
        let slice = self
            .s
            .get(self.i..end)
            .ok_or_else(|| self.err("short \\u escape"))?;
        let text = std::str::from_utf8(slice).map_err(|_| self.err("bad \\u escape"))?;
        let v = u32::from_str_radix(text, 16).map_err(|_| self.err("bad \\u escape"))?;
        self.i = end;
        Ok(v)
    }
}

/// Volumio's configuration store: every key holds `{ "type", "value" }`,
/// and the value is read leniently, as the plugin may have stored a number
/// as a string or a switch as `"true"`.
pub struct Store(pub Value);

impl Store {
    pub fn parse(text: &str) -> Result<Store, Error> {
        Value::parse(text).map(Store)
    }

    fn value(&self, key: &str) -> Option<&Value> {
        let entry = self.0.get(key)?;
        Some(entry.get("value").unwrap_or(entry))
    }

    pub fn string(&self, key: &str) -> Option<String> {
        match self.value(key)? {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(Value::Number(*n).to_json()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        match self.value(key)? {
            Value::Number(n) => Some(*n),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn boolean(&self, key: &str) -> Option<bool> {
        match self.value(key)? {
            Value::Bool(b) => Some(*b),
            Value::String(s) => match s.trim() {
                "true" | "1" | "on" => Some(true),
                "false" | "0" | "off" => Some(false),
                _ => None,
            },
            Value::Number(n) => Some(*n != 0.0),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(t: &str) -> Value {
        Value::parse(t).unwrap_or_else(|e| panic!("{t}: {e}"))
    }

    #[test]
    fn scalars_and_whitespace() {
        assert_eq!(parse(" null "), Value::Null);
        assert_eq!(parse("true"), Value::Bool(true));
        assert_eq!(parse("false"), Value::Bool(false));
        assert_eq!(parse("0"), Value::Number(0.0));
        assert_eq!(parse("-12.5e2"), Value::Number(-1250.0));
        assert_eq!(parse("1E-2"), Value::Number(0.01));
        assert_eq!(parse("\"a\""), Value::String("a".into()));
    }

    #[test]
    fn strings_with_every_escape_and_utf8() {
        assert_eq!(
            parse(r#""a\"b\\c\/d\b\f\n\r\t""#).as_str().unwrap(),
            "a\"b\\c/d\u{8}\u{c}\n\r\t"
        );
        assert_eq!(parse(r#""\u00e9\u4e2d""#).as_str().unwrap(), "é中");
        assert_eq!(parse(r#""\ud83c\udfb5""#).as_str().unwrap(), "🎵");
        assert_eq!(
            parse("\"ünïcödé 🎵 直接\"").as_str().unwrap(),
            "ünïcödé 🎵 直接"
        );
    }

    #[test]
    fn containers_nest_and_the_later_key_wins() {
        let v = parse(r#"{"a":[1,2,{"b":null}],"c":{"d":"e"},"a":true}"#);
        assert_eq!(v.get("a"), Some(&Value::Bool(true)));
        assert_eq!(
            v.get("c").and_then(|c| c.get("d")).and_then(Value::as_str),
            Some("e")
        );
        assert_eq!(parse("[]"), Value::Array(vec![]));
        assert_eq!(parse("{ }"), Value::Object(BTreeMap::new()));
        assert_eq!(parse("[ [ [ ] ] ]").as_array().unwrap().len(), 1);
    }

    #[test]
    fn what_is_refused_and_where() {
        let bad = [
            ("", 0, "unexpected end of text"),
            ("{", 1, "expected a string key"),
            ("[1,]", 3, "unexpected character"),
            ("{\"a\":1,}", 7, "expected a string key"),
            ("01", 1, "trailing characters after the value"),
            ("1.", 2, "expected a digit after '.'"),
            ("-", 1, "expected a digit"),
            ("\"abc", 4, "unterminated string"),
            ("\"\\x\"", 2, "unknown escape"),
            ("\"\u{1}\"", 1, "control character in a string"),
            ("nul", 0, "unknown literal"),
            ("\"\\ud800\"", 7, "lone high surrogate"),
            ("tru e", 0, "unknown literal"),
            ("{\"a\" 1}", 5, "expected ':' after the key"),
            ("1e999", 0, "number out of range"),
        ];
        for (text, at, what) in bad {
            let e = Value::parse(text).expect_err(text);
            assert_eq!((e.at, e.what), (at, what), "{text:?}");
        }
        let deep = "[".repeat(200) + &"]".repeat(200);
        assert_eq!(Value::parse(&deep).unwrap_err().what, "nested too deep");
    }

    #[test]
    fn writes_what_it_read() {
        for text in [
            r#"{"a":[1,2.5,-3,true,null,"x\"y\n"],"b":{"c":"é🎵"}}"#,
            r#"[0,1e21,-0.001,123456789012345]"#,
            r#""\u0001\u001f""#,
        ] {
            let v = parse(text);
            assert_eq!(parse(&v.to_json()), v, "{text}");
        }
        assert_eq!(parse("1e21").to_json(), "1000000000000000000000");
        assert_eq!(parse("2.50").to_json(), "2.5");
        assert_eq!(parse("\"\\u0001\"").to_json(), "\"\\u0001\"");
    }

    #[test]
    fn the_store_reads_volumio_values_leniently() {
        let s = Store::parse(
            r#"{
            "connection": {"type": "string", "value": "spi_waveshare"},
            "gpio_dc": {"type": "number", "value": 25},
            "as_text": {"type": "string", "value": " 27 "},
            "show_lifecycle": {"type": "boolean", "value": true},
            "flag_text": {"type": "string", "value": "false"},
            "flag_num": {"type": "number", "value": 1},
            "bare": 7,
            "junk": {"type": "string", "value": [1]}
        }"#,
        )
        .unwrap();
        assert_eq!(s.string("connection").as_deref(), Some("spi_waveshare"));
        assert_eq!(s.number("gpio_dc"), Some(25.0));
        assert_eq!(s.number("as_text"), Some(27.0));
        assert_eq!(s.string("gpio_dc").as_deref(), Some("25"));
        assert_eq!(s.boolean("show_lifecycle"), Some(true));
        assert_eq!(s.boolean("flag_text"), Some(false));
        assert_eq!(s.boolean("flag_num"), Some(true));
        assert_eq!(s.number("bare"), Some(7.0));
        assert_eq!(s.string("junk"), None);
        assert_eq!(s.string("missing"), None);
    }
}
