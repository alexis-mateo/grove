use std::process::Command;

/// Police utilisée si fontconfig ne répond pas.
const FALLBACK: &str = "DejaVu Sans Mono";

/// Nom de famille concret pour `family`.
///
/// GPUI ne résout pas les noms génériques (`monospace`) et retombe sans le dire
/// sur une police proportionnelle : on demande à fontconfig la police réelle.
pub fn resolve_family(family: &str) -> String {
    if !matches!(family, "monospace" | "mono") {
        return family.to_owned();
    }
    Command::new("fc-match")
        .args(["-f", "%{family[0]}", "monospace"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| FALLBACK.to_owned())
}
