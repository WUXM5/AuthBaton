// AuthManager 新增（cc-same 里没有这个文件）：按成员改写 JSON 对象，保留不变成员的原始字节。
//! A JSON object edited member by member, keeping the bytes of the members that stay.
//!
//! cc-same builds serde_json with `preserve_order`, so a file it rewrites keeps its keys in their
//! order. This workspace does not (the feature is shared, and it would change how auth-service
//! writes JSON), and a `serde_json::Map` sorts its keys. Where only a few members of a file that
//! Claude or the user wrote change, these helpers keep the others exactly as written: order,
//! spacing, number formatting. Callers check the result against the value they meant
//! ([`reads_as`]) and fall back to writing that value the plain way when the two differ.

use serde::Serialize as _;
use serde_json::{Map, Value};

/// The top level of a JSON object, as written.
pub struct Object<'a> {
    /// Whitespace after `{`.
    open: &'a [u8],
    /// Between two members: `,` and its whitespace.
    sep: Option<&'a [u8]>,
    /// Whitespace before `}`.
    close: &'a [u8],
    /// Whitespace after `}` (a final newline).
    tail: &'a [u8],
    /// Between a key and its value: `:` and its whitespace.
    colon: Option<&'a [u8]>,
    pub members: Vec<Member<'a>>,
}

/// One member of an [`Object`].
pub struct Member<'a> {
    pub key: String,
    /// `"key": value`, exactly as written.
    pub raw: &'a [u8],
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn skip_ws(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && is_ws(s[i]) {
        i += 1;
    }
    i
}

/// Where the string starting at `i` (a `"`) ends, just past its closing quote.
fn string_end(s: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < s.len() {
        match s[j] {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// Where the value starting at `i` ends.
fn value_end(s: &[u8], i: usize) -> Option<usize> {
    match *s.get(i)? {
        b'"' => string_end(s, i),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            while j < s.len() {
                match s[j] {
                    b'"' => {
                        j = string_end(s, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth = depth.checked_sub(1)?;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while j < s.len() && !is_ws(s[j]) && !matches!(s[j], b',' | b'}' | b']') {
                j += 1;
            }
            (j > i).then_some(j)
        }
    }
}

/// The top level of the JSON object `s`; `None` when it is not one (or starts with whitespace,
/// or has anything but whitespace after it).
pub fn parse(s: &[u8]) -> Option<Object<'_>> {
    if s.first() != Some(&b'{') {
        return None;
    }
    let first = skip_ws(s, 1);
    let mut obj = Object { open: &s[1..first], sep: None, close: &[], tail: &[], colon: None, members: Vec::new() };
    if s.get(first) == Some(&b'}') {
        obj.close = obj.open;
        obj.tail = &s[first + 1..];
    } else {
        let mut i = first;
        loop {
            let start = i;
            if s.get(i) != Some(&b'"') {
                return None;
            }
            let key_end = string_end(s, i)?;
            let key: String = serde_json::from_slice(&s[i..key_end]).ok()?;
            let colon = skip_ws(s, key_end);
            if s.get(colon) != Some(&b':') {
                return None;
            }
            let value = skip_ws(s, colon + 1);
            obj.colon.get_or_insert(&s[key_end..value]);
            let end = value_end(s, value)?;
            obj.members.push(Member { key, raw: &s[start..end] });
            let next = skip_ws(s, end);
            match s.get(next) {
                Some(b',') => {
                    let after = skip_ws(s, next + 1);
                    obj.sep.get_or_insert(&s[end..after]);
                    i = after;
                }
                Some(b'}') => {
                    obj.close = &s[end..next];
                    obj.tail = &s[next + 1..];
                    break;
                }
                _ => return None,
            }
        }
    }
    obj.tail.iter().all(|b| is_ws(*b)).then_some(obj)
}

impl Object<'_> {
    /// An object holding `members` (each `"key": value`), spaced the way this one is.
    pub fn rebuild<'m>(&self, members: impl IntoIterator<Item = &'m [u8]>) -> Vec<u8> {
        let default_sep = [b",".as_slice(), self.open].concat();
        let sep = self.sep.unwrap_or(&default_sep);
        let mut out = vec![b'{'];
        let mut any = false;
        for m in members {
            out.extend_from_slice(if any { sep } else { self.open });
            out.extend_from_slice(m);
            any = true;
        }
        if any {
            out.extend_from_slice(self.close);
        }
        out.push(b'}');
        out.extend_from_slice(self.tail);
        out
    }

    /// `"key": value` for a new member, written the way this object writes its members: with its
    /// colon, and a nested value indented one level in from the member's own line.
    pub fn member(&self, key: &str, value: &Value) -> Vec<u8> {
        let pretty = self.open.contains(&b'\n');
        let colon = self.colon.unwrap_or(if pretty { b": ".as_slice() } else { b":".as_slice() });
        let indent = self.open.rsplit(|b| *b == b'\n').next().unwrap_or_default();
        let mut out = serde_json::to_vec(key).unwrap();
        out.extend_from_slice(colon);
        if pretty && !indent.is_empty() && (value.is_object() || value.is_array()) {
            let mut text = Vec::new();
            let fmt = serde_json::ser::PrettyFormatter::with_indent(indent);
            value.serialize(&mut serde_json::Serializer::with_formatter(&mut text, fmt)).unwrap();
            for b in text {
                out.push(b);
                if b == b'\n' {
                    out.extend_from_slice(indent);
                }
            }
        } else {
            out.extend(serde_json::to_vec(value).unwrap());
        }
        out
    }
}

/// `bytes`, read as JSON, is the object `want`.
pub fn reads_as(bytes: &[u8], want: &Map<String, Value>) -> bool {
    matches!(serde_json::from_slice::<Value>(bytes), Ok(Value::Object(m)) if m == *want)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn same_again(text: &str) {
        let obj = parse(text.as_bytes()).unwrap();
        assert_eq!(obj.rebuild(obj.members.iter().map(|m| m.raw)), text.as_bytes(), "{text}");
    }

    #[test]
    fn an_object_rebuilt_from_its_members_is_byte_for_byte_the_same() {
        same_again(r#"{"z":1,"a":{"q":[1,2,{"x":"}\"]"}]},"m":1.50,"e":"é"}"#);
        same_again("{\n\t\"darkMode\": \"dark\",\n\t\"nested\": {\n\t\t\"b\": 1\n\t}\n}");
        same_again("{\n  \"model\": \"opus\",\n  \"env\": {}\n}\n");
        same_again("{}");
        same_again("{}\n");
        assert!(parse(b" {}").is_none());
        assert!(parse(b"{} x").is_none());
        assert!(parse(b"[1]").is_none());
    }

    #[test]
    fn members_keep_their_order_when_some_go_and_others_come() {
        let text = "{\n\t\"zeta\": 1,\n\t\"oauth:tokenCache\": \"x\",\n\t\"alpha\": [\n\t\t1\n\t]\n}";
        let obj = parse(text.as_bytes()).unwrap();
        let added = obj.member("lastKnownAccountUuid", &json!({"a": [1]}));
        let kept = obj.members.iter().filter(|m| m.key != "oauth:tokenCache").map(|m| m.raw);
        let out = obj.rebuild(kept.chain([added.as_slice()]));
        let expected = "{\n\t\"zeta\": 1,\n\t\"alpha\": [\n\t\t1\n\t],\n\t\"lastKnownAccountUuid\": {\n\t\t\"a\": [\n\t\t\t1\n\t\t]\n\t}\n}";
        assert_eq!(String::from_utf8(out.clone()).unwrap(), expected);
        let want = json!({"zeta": 1, "alpha": [1], "lastKnownAccountUuid": {"a": [1]}});
        assert!(reads_as(&out, want.as_object().unwrap()));
        let compact = parse(br#"{"b":2}"#).unwrap();
        assert_eq!(compact.member("a", &json!([1, 2])), br#""a":[1,2]"#);
    }
}
