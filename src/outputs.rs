/// Stable connector order, independent of the compositor's enumeration order.
/// Empty names are not persistent identities; the UI may use an anonymous
/// monitor only as a temporary last resort, never as a saved preference.
pub fn select_output(preferred: &str, connectors: &[String]) -> Option<String> {
    connectors
        .iter()
        .filter(|name| !name.is_empty())
        .find(|name| name.as_str() == preferred)
        .or_else(|| connectors.iter().filter(|name| !name.is_empty()).min())
        .cloned()
}

pub fn select_dialog_output(
    hint: &str,
    active: &[String],
    preferred: &str,
    available: &[String],
) -> Option<String> {
    if !hint.is_empty() && available.iter().any(|name| name == hint) {
        return Some(hint.into());
    }
    let on_active = active
        .iter()
        .filter(|name| !name.is_empty() && available.contains(name));
    on_active
        .clone()
        .find(|name| name.as_str() == preferred)
        .or_else(|| on_active.min())
        .cloned()
        .or_else(|| select_output(preferred, available))
}
