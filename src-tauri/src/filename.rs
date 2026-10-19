use chrono::{DateTime, Local};

pub const DEFAULT_TEMPLATE: &str = "PureShot {date} {time}";
const VARIABLES: &[&str] = &[
    "date", "time", "year", "month", "day", "hour", "minute", "second",
    "timestamp", "width", "height", "type", "id",
];

pub fn validate(template: &str) -> Result<(), String> {
    if template.trim().is_empty() || template.chars().count() > 120 {
        return Err("Use a filename template between 1 and 120 characters.".into());
    }
    let mut remaining = template;
    while let Some(start) = remaining.find('{') {
        let literal = &remaining[..start];
        validate_literal(literal)?;
        let end = remaining[start..].find('}').ok_or("Missing closing } in filename variable.")? + start;
        let variable = &remaining[start + 1..end];
        if !VARIABLES.contains(&variable) {
            return Err(format!("Unknown filename variable: {{{variable}}}"));
        }
        remaining = &remaining[end + 1..];
    }
    validate_literal(remaining)
}

fn validate_literal(literal: &str) -> Result<(), String> {
    if literal.chars().any(|ch| ch.is_control() || "<>:\"/\\|?*}".contains(ch)) {
        return Err("Filename cannot contain < > : \" / \\ | ? * or unmatched braces.".into());
    }
    Ok(())
}

pub fn render(
    template: &str,
    created_at: i64,
    width: u32,
    height: u32,
    capture_type: &str,
    id: &str,
) -> Result<String, String> {
    validate(template)?;
    let stamp = DateTime::from_timestamp_millis(created_at)
        .ok_or("Invalid screenshot date.")?.with_timezone(&Local);
    let values = [
        ("date", stamp.format("%Y-%m-%d").to_string()),
        ("time", stamp.format("%H-%M-%S").to_string()),
        ("year", stamp.format("%Y").to_string()),
        ("month", stamp.format("%m").to_string()),
        ("day", stamp.format("%d").to_string()),
        ("hour", stamp.format("%H").to_string()),
        ("minute", stamp.format("%M").to_string()),
        ("second", stamp.format("%S").to_string()),
        ("timestamp", created_at.to_string()),
        ("width", width.to_string()),
        ("height", height.to_string()),
        ("type", capture_type.to_string()),
        ("id", id.to_string()),
    ];
    let mut result = template.to_string();
    for (key, value) in values {
        result = result.replace(&format!("{{{key}}}"), &value);
    }
    let result = result.trim().trim_end_matches(['.', ' ']).to_string();
    validate_literal(&result)?;
    if result.is_empty() || result.chars().count() > 200 {
        return Err("Generated filename must be between 1 and 200 characters.".into());
    }
    let base = result.split('.').next().unwrap_or("").trim_end().to_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            base.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
            })
        });
    if reserved {
        return Err("This filename is reserved by Windows. Choose another template.".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_metadata_and_repeated_variables() {
        assert_eq!(render("{type}_{width}x{height}_{timestamp}_{width}", 0, 1920, 1080, "area", "0-area").unwrap(), "area_1920x1080_0_1920");
    }

    #[test]
    fn rejects_invalid_templates_and_windows_names() {
        for template in ["", "{unknown}", "{date", "date}", "../capture", "C:\\capture", "CON", "nul.png", "LPT1", "..."] {
            assert!(render(template, 0, 1, 1, "area", "0-area").is_err(), "{template}");
        }
    }

    #[test]
    fn default_has_safe_date_and_time() {
        let name = render(DEFAULT_TEMPLATE, 0, 1, 1, "area", "0-area").unwrap();
        assert!(name.starts_with("PureShot "));
        assert!(!name.contains('{'));
        assert!(!name.contains(':'));
    }
}
