//! Bounded content-only readers. AVB-0039, AVB-0038 and AVB-0013; no cached _TTEXT authority.
use super::*;
use crate::source::TextKind;
mod properties;
pub(super) use properties::properties;

pub(super) const MAX_TEXT_BYTES: usize = 32 * 1024;
const MAX_DATABASE_BYTES: usize = 1024 * 1024;
pub(super) fn kind(node: &Node) -> Option<TextKind> {
    if node.class != *b"TKFX" {
        return None;
    }
    match node.get("effect_id").and_then(Value::as_str)? {
        "E4FE520A-B815-447F-A969-2D5E1C5F6C14" => Some(TextKind::SubCap),
        "E4EEBB07-AFAA-4122-BFA8-0A838BB40A8C" => Some(TextKind::TitlerPlus),
        _ => None,
    }
}
/// Locate exact full parameter/type identities, never labels, comments or suffixes.
pub(super) fn body<'a>(
    ctx: &Ctx<'a>,
    node: &Node,
    kind: TextKind,
) -> Result<&'a [u8], &'static str> {
    let (parameter, user_type) = match kind {
        TextKind::SubCap => (
            "961a0231335b514595e0317fddf547df",
            "4af0193369ac2545b9e82206362fd233",
        ),
        TextKind::TitlerPlus => (
            "ac14a667b629fe4ab9cc8107a5dd3468",
            "aa2c2fc09372d611b4e40030658a6504",
        ),
    };
    let list = ctx
        .deref(node.get("param_list"))
        .filter(|n| n.class == *b"PRLS")
        .ok_or("text parameter list missing")?;
    let Some(Value::RefList(items)) = list.get("items") else {
        return Err("text parameter list malformed");
    };
    let mut found = None;
    for reference in items {
        let p = ctx
            .deref(Some(reference))
            .filter(|n| n.class == *b"PRIT")
            .ok_or("text parameter reference malformed")?;
        let Some(Value::Uuid(id)) = p.get("uuid") else {
            return Err("text parameter identity missing");
        };
        if file_order_hex(id) != parameter {
            continue;
        }
        if found.is_some() {
            return Err("duplicate text content parameter");
        }
        if !matches!(
            p.get("control_track"),
            Some(Value::Ref(0)) | Some(Value::Null)
        ) {
            return Err("animated text content is not interpreted");
        }
        let v = ctx
            .deref(p.get("value"))
            .filter(|n| n.class == *b"AVUP")
            .ok_or("text content is not AVUP")?;
        if !matches!(v.get("uuid"), Some(Value::Uuid(u)) if file_order_hex(u) == user_type) {
            return Err("unqualified text AVUP type");
        }
        let Some(Value::Bytes(bytes)) = v.get("data") else {
            return Err("text content bytes missing");
        };
        if bytes.len() > MAX_DATABASE_BYTES {
            return Err("text database exceeds 1 MiB");
        }
        found = Some(bytes.as_slice());
    }
    found.ok_or("text content parameter missing")
}
pub(super) fn decode(bytes: &[u8], kind: TextKind) -> Result<String, &'static str> {
    match kind {
        TextKind::SubCap => {
            let text = bytes
                .strip_suffix(&[0])
                .ok_or("SubCap text lacks its terminal NUL")?;
            // AVB-0038 ASCII plus the User's native Unicode/multiline control.
            // Strict UTF-8; never guess UTF-16 from a symbol name or substitute
            // replacement characters for undecodable legacy bytes.
            let text = std::str::from_utf8(text).map_err(|_| "SubCap text is not strict UTF-8")?;
            if text.len() > MAX_TEXT_BYTES || text.chars().any(|c| c.is_control() && c != '\n') {
                return Err("SubCap text contains unsupported controls or exceeds 32 KiB");
            }
            Ok(text.to_owned())
        }
        TextKind::TitlerPlus => titler(bytes),
    }
}
fn titler(bytes: &[u8]) -> Result<String, &'static str> {
    if bytes.len() > MAX_DATABASE_BYTES {
        return Err("text database exceeds 1 MiB");
    }
    let body = std::str::from_utf8(bytes).map_err(|_| "Titler+ database is not strict UTF-8")?;
    if !body.starts_with("Version = AvidAVX1003000000\n")
        && !body.starts_with("Version = AvidAVX1003000000\r\n")
    {
        return Err("unqualified Titler+ database version");
    }
    let mut stack: Vec<&str> = Vec::new();
    let (mut scenes, mut layers, mut flows, mut static_flows, mut characters, mut codes) =
        (0, 0, 0, 0, 0, 0);
    let mut out = String::new();
    let mut flow_text = None;
    for line in body.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "}" {
            if stack.pop().is_none() {
                return Err("unbalanced Titler+ database");
            }
            continue;
        }
        let (key, value) = line
            .split_once(" = ")
            .ok_or("malformed Titler+ database line")?;
        if value == "{" {
            if stack.len() >= 64 {
                return Err("Titler+ database nesting exceeds 64");
            }
            if stack.is_empty() && key == "Scene" {
                scenes += 1;
            }
            if stack.first() == Some(&"Scene") {
                if key == "Layer" {
                    layers += 1;
                }
                if key == "Flow" {
                    flows += 1;
                }
                if key == "Character" {
                    characters += 1;
                }
            }
            stack.push(key);
        } else if stack.first() == Some(&"Scene") {
            if key == "Text" && stack.last() == Some(&"Flow") {
                if flow_text.is_some() {
                    return Err("multiple Titler+ text flows are not prepared");
                }
                flow_text = Some(unquote_flow_text(value)?);
            }
            if stack.last() == Some(&"Flow") && key == "Mode" {
                if value != "Static" {
                    return Err("scrolling or rolling Titler+ text is not interpreted");
                }
                static_flows += 1;
            }
            if key == "Code" {
                if stack.last() != Some(&"Character") || !stack.contains(&"Flow") {
                    return Err("unqualified Titler+ character ownership");
                }
                let code = if value == "0xA" {
                    "\n"
                } else if value == "{ }" {
                    " "
                } else {
                    value
                };
                if code.chars().count() != 1 || code.chars().any(|c| c.is_control() && c != '\n') {
                    return Err("unqualified Titler+ character code");
                }
                if out
                    .len()
                    .checked_add(code.len())
                    .is_none_or(|n| n > MAX_TEXT_BYTES)
                {
                    return Err("Titler+ text exceeds 32 KiB");
                }
                out.push_str(code);
                codes += 1;
            }
        }
    }
    if !stack.is_empty()
        || scenes != 1
        || layers != 1
        || flows != 1
        || static_flows != 1
        || characters != codes
    {
        return Err("Titler+ requires one complete static text flow in one layer");
    }
    match (codes, flow_text) {
        (0, Some(text)) => out = text,
        (0, None) => return Err("Titler+ has no qualified text content"),
        (_, Some(_)) => {
            return Err("mixed Titler+ Text and Character representations are not interpreted")
        }
        (_, None) => {}
    }
    // The retained native fixtures carry a terminal paragraph marker.
    if !out.ends_with('\n') {
        return Err("Titler+ terminal paragraph marker missing");
    }
    out.pop();
    Ok(out)
}
/// Native single-flow Text carrier: brace-quoted UTF-8 with escaped paragraph
/// separators. Only observed escapes are admitted; no general Tcl evaluation.
fn unquote_flow_text(value: &str) -> Result<String, &'static str> {
    let body = value
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or("Titler+ Text is not brace quoted")?;
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        let c = if c == '\\' {
            match chars.next() {
                Some('n') => '\n',
                _ => return Err("unqualified Titler+ Text escape"),
            }
        } else {
            c
        };
        if c == '{' || c == '}' || (c.is_control() && c != '\n') {
            return Err("unqualified Titler+ Text character");
        }
        if out
            .len()
            .checked_add(c.len_utf8())
            .is_none_or(|n| n > MAX_TEXT_BYTES)
        {
            return Err("Titler+ text exceeds 32 KiB");
        }
        out.push(c);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn title(codes: &str) -> String {
        format!("Version = AvidAVX1003000000\nScene = {{\nLayer = {{\nFlow = {{\nMode = Static\n{codes}Character = {{\nCode = 0xA\n}}\n}}\n}}\n}}\n")
    }
    #[test]
    fn subcap_utf8_keeps_native_paragraphs_and_refuses_encoding_guesses() {
        assert_eq!(
            decode(b"Caption 123\0", TextKind::SubCap).unwrap(),
            "Caption 123"
        );
        assert_eq!(
            decode(
                "SubCap Alpha café\nSecond line 123\0".as_bytes(),
                TextKind::SubCap
            )
            .unwrap(),
            "SubCap Alpha café\nSecond line 123"
        );
        for b in [
            b"missing NUL".as_slice(),
            b"a\0b\0",
            b"a\rb\0",
            b"caf\xe9\0",
            b"a\0\0",
        ] {
            assert!(decode(b, TextKind::SubCap).is_err());
        }
    }
    #[test]
    fn title_uses_literal_codes_preserving_spaces_and_internal_lines() {
        let s = title("Character = {\nCode = Z\n}\nCharacter = {\nCode = { }\n}\nCharacter = {\nCode = 0xA\n}\nCharacter = {\nCode = é\n}\n");
        assert_eq!(decode(s.as_bytes(), TextKind::TitlerPlus).unwrap(), "Z \né");
        for bad in [
            s.replace("Mode = Static", "Mode = Roll"),
            s.replace("Code = Z", "Code = 0x41"),
            s.replace("Code = é", "Code = {}"),
            s.replace("Version = AvidAVX1003000000", "Version = AvidAVX999"),
            format!("{s}}}"),
        ] {
            assert!(decode(bad.as_bytes(), TextKind::TitlerPlus).is_err());
        }
    }
    #[test]
    fn native_flow_text_keeps_paragraphs_but_refuses_ambiguous_carriers() {
        let s="Version = AvidAVX1003000000\nScene = {\nLayer = {\nFlow = {\nMode = Static\nText = {Title Beta café\\nSecond line 456\\n}\n}\n}\n}\n";
        assert_eq!(
            decode(s.as_bytes(), TextKind::TitlerPlus).unwrap(),
            "Title Beta café\nSecond line 456"
        );
        assert!(decode(
            s.replace("café\\n", "café\\t").as_bytes(),
            TextKind::TitlerPlus
        )
        .is_err());
        let mixed = s.replace("Text =", "Character = {\nCode = A\n}\nText =");
        assert!(decode(mixed.as_bytes(), TextKind::TitlerPlus).is_err());
    }
}
