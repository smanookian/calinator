//! Contrast test: every color pair Calinator draws is readable, in every
//! built-in theme and every Omarchy theme installed on this machine,
//! including event blocks in any calendar color.

use std::path::PathBuf;

use calinator::app::theme::{calendar_color, event_colors};
use musinator_themes::{Theme, builtin, contrast, contrast_report, omarchy};

/// Calendar colors to try: our palette, Google's, and extremes.
const CAL_COLORS: [&str; 16] = [
    "#89b4fa", "#a6e3a1", "#f38ba8", "#fab387", "#f9e2af", "#cba6f7", "#94e2d5", "#b4befe", "#039be5", "#7986cb", "#33b679", "#e67c73",
    "#000000", "#ffffff", "#ffff00", "#0000ff",
];

fn check(t: &Theme, bad: &mut Vec<String>) {
    let mut pairs: Vec<(String, f64, f64)> = contrast_report(t).into_iter().map(|(p, r, m)| (p.to_string(), r, m)).collect();
    // The now line.
    pairs.push(("error/bg (now line)".into(), contrast(t.error, t.bg), 3.0));
    for col in CAL_COLORS {
        for sel in [false, true] {
            let (bg, fg) = event_colors(t, col, sel);
            pairs.push((format!("event text {col} selected={sel}"), contrast(fg, bg), 4.5));
        }
        pairs.push((format!("calendar dot {col}/bg"), contrast(calendar_color(t, col), t.bg), 3.0));
    }
    for (pair, ratio, min) in pairs {
        if ratio + 1e-9 < min {
            bad.push(format!("{} ({}): {pair} {ratio:.2} < {min}", t.label, t.id));
        }
    }
}

/// Omarchy theme dirs: system themes, user themes, and the active one.
fn omarchy_dirs() -> Vec<PathBuf> {
    let home = std::env::home_dir().unwrap_or_default();
    let roots = [PathBuf::from("/usr/share/omarchy/themes"), home.join(".config/omarchy/themes"), home.join(".local/share/omarchy/themes")];
    let mut dirs: Vec<PathBuf> =
        roots.iter().filter_map(|r| std::fs::read_dir(r).ok()).flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.extend(omarchy::theme_dir());
    dirs
}

#[test]
fn every_builtin_theme_passes() {
    let mut bad = Vec::new();
    let themes = builtin();
    assert!(themes.len() >= 20);
    for t in &themes {
        check(t, &mut bad);
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn every_installed_omarchy_theme_passes() {
    let mut bad = Vec::new();
    let mut n = 0;
    for dir in omarchy_dirs() {
        if let Some(t) = omarchy::load(&dir) {
            check(&t, &mut bad);
            n += 1;
        }
    }
    eprintln!("checked {n} Omarchy themes");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
