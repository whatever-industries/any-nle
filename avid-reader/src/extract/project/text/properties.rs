//! AVB-0013: exact 1-based named cache bindings. Preserve values and owners;
//! do not flatten character overrides or claim these are rendered geometry.
type Property<'a> = (&'static str, usize, &'a str, &'a str);
fn selected(name: &str) -> bool {
    [
        "Transform.",
        "Text.Font.",
        "Text.Kerning",
        "Text.Underline",
        "Manager.",
        "Shadow.",
        "Material.Main.",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}
pub(in crate::extract::project) fn properties(
    bytes: &[u8],
) -> Result<Vec<Property<'_>>, &'static str> {
    let body = std::str::from_utf8(bytes).map_err(|_| "title properties are not UTF-8")?;
    if !body.starts_with("Version = AvidAVX1003000000\n")
        && !body.starts_with("Version = AvidAVX1003000000\r\n")
    {
        return Err("unqualified title property database version");
    }
    let mut stack: Vec<&str> = Vec::new();
    let mut blocks: Vec<(&str, Option<&str>)> = Vec::new();
    let mut lists: Vec<Vec<(usize, &str)>> = Vec::new();
    let mut owners = vec![("default", 1usize)];
    let mut fields = Vec::new();
    let (mut flow_count, mut refs) = (0, 0);
    for line in body.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "}" {
            if stack.pop().is_none() {
                return Err("unbalanced title properties");
            }
            continue;
        }
        let (key, value) = line
            .split_once(" = ")
            .ok_or("malformed title property line")?;
        if value == "{" {
            if stack.len() >= 64 {
                return Err("title property nesting exceeds 64");
            }
            if stack.as_slice() == ["PropCache"] {
                if key == "List" {
                    if lists.len() >= 1024 {
                        return Err("title property list count exceeds 1024");
                    }
                    lists.push(Vec::new());
                } else {
                    if blocks.len() >= 4096 {
                        return Err("title property block count exceeds 4096");
                    }
                    blocks.push((key, None));
                }
            }
            if stack.first() == Some(&"Scene") && key == "Flow" {
                flow_count += 1;
                if flow_count > 1 {
                    return Err("native title metadata requires one flow");
                }
            }
            stack.push(key);
            continue;
        }
        if stack.len() == 2 && stack[0] == "PropCache" {
            if stack[1] == "List" && key == "Prop" {
                refs += 1;
                if refs > 8192 {
                    return Err("title property reference count exceeds 8192");
                }
                let (number, name) = value
                    .split_once(':')
                    .ok_or("malformed title property reference")?;
                let number = number
                    .parse::<usize>()
                    .map_err(|_| "malformed title property index")?;
                lists
                    .last_mut()
                    .ok_or("title property list missing")?
                    .push((number, name));
            } else if key == "Value" {
                let b = blocks.last_mut().ok_or("title property block missing")?;
                // Unselected compound controls (for example Text.Scroll) can
                // legitimately contain repeated Value fields. Do not interpret
                // them as scalar appearance properties.
                if b.1.is_some() && selected(b.0) {
                    return Err("duplicate selected title property value");
                }
                b.1 = Some(value);
            }
        }
        if stack.first() == Some(&"Scene") {
            let scope = match stack.last().copied() {
                Some("Flow") => Some("flow"),
                Some("Paragraph") => Some("paragraph"),
                Some("Character") => Some("character"),
                Some("Layer") => Some("layer"),
                _ => None,
            };
            if let Some(scope) = scope {
                if key == "PropList" {
                    let n = value
                        .parse::<usize>()
                        .map_err(|_| "malformed scene property list")?;
                    if n > 0 && !owners.contains(&(scope, n)) {
                        if owners.len() >= 32 {
                            return Err("too many distinct title property owners");
                        }
                        owners.push((scope, n));
                    }
                } else if scope == "flow" && ["FixedWidth", "FixedHeight", "Mode"].contains(&key) {
                    if fields
                        .iter()
                        .any(|&(s, n, k, _)| s == scope && n == 0 && k == key)
                    {
                        return Err("duplicate title flow field");
                    }
                    fields.push((scope, 0, key, value));
                }
            }
        }
    }
    if !stack.is_empty() || flow_count != 1 {
        return Err("native title metadata requires one flow");
    }
    for list in &lists {
        let mut names = std::collections::HashSet::new();
        for &(number, name) in list {
            if number == 0
                || blocks.get(number - 1).is_none_or(|b| b.0 != name)
                || !names.insert(name)
            {
                return Err("title property cache identity or uniqueness mismatch");
            }
        }
    }
    for (scope, n) in owners {
        let list = lists
            .get(n - 1)
            .ok_or("scene title property list out of range")?;
        for &(number, name) in list {
            if selected(name) {
                let value = blocks[number - 1]
                    .1
                    .ok_or("selected title property is not scalar")?;
                if name.len() > 256 || value.len() > 1024 || fields.len() >= 512 {
                    return Err("native title metadata exceeds bounds");
                }
                fields.push((scope, n, name, value));
            }
        }
    }
    Ok(fields)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn title_properties_keep_owner_lists_without_flattening_inheritance() {
        let b=b"Version = AvidAVX1003000000\nPropCache = {\nTransform.Position.X = {\nValue = 0\n}\nTransform.Position.X = {\nValue = 0.008\n}\nList = {\nProp = 1:Transform.Position.X\n}\nList = {\nProp = 2:Transform.Position.X\n}\n}\nScene = {\nLayer = {\nFlow = {\nPropList = 2\nFixedWidth = False\n}\n}\n}\n";
        let p = properties(b).unwrap();
        assert!(p.contains(&("default", 1, "Transform.Position.X", "0")));
        assert!(p.contains(&("flow", 2, "Transform.Position.X", "0.008")));
        let s = std::str::from_utf8(b).unwrap();
        for bad in [
            s.replace("2:Transform.Position.X", "0:Transform.Position.X"),
            s.replace("2:Transform.Position.X", "2:Transform.Position.Y"),
            s.replace("PropList = 2", "PropList = 3"),
            s.replace("Value = 0.008", "Value = 0.008\nValue = 0.009"),
        ] {
            assert!(properties(bad.as_bytes()).is_err());
        }
        let compound = s
            .replace(
                "PropCache = {",
                "PropCache = {\nText.Scroll = {\nValue = 0\nValue = 1\n}",
            )
            .replace("Prop = 1:", "Prop = 2:")
            .replace(
                "Prop = 2:Transform.Position.X\n}\n}\nScene",
                "Prop = 3:Transform.Position.X\n}\n}\nScene",
            );
        assert!(properties(compound.as_bytes()).is_ok());
    }
}
