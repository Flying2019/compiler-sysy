pub(crate) fn sanitize_ident(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
