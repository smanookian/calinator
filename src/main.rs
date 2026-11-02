use std::path::PathBuf;

use calinator::store::{Store, data_dir};
use calinator::{cli, daemon, doctor};

fn fail(msg: &str) -> ! {
    eprintln!("calinator: {msg}");
    std::process::exit(1);
}

fn main() {
    let mut db: Option<PathBuf> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--db" => db = Some(args.next().map(PathBuf::from).unwrap_or_else(|| fail("--db needs a file"))),
            _ => rest.push(a),
        }
    }
    let custom_db = db.is_some();
    let db = db.unwrap_or_else(|| data_dir().join("calinator.db"));
    let (cmd, args) = match rest.split_first() {
        Some((c, a)) => (c.as_str(), a),
        None => ("", &[][..]),
    };
    let mut opening: Option<String> = None;
    match cmd {
        "" => {}
        "-h" | "--help" | "help" => return println!("{}", cli::HELP),
        "-V" | "--version" | "version" => return println!("calinator {}", env!("CARGO_PKG_VERSION")),
        "--bar" | "bar" => {
            if let Err(e) = cli::bar(&db) {
                fail(&e);
            }
            return;
        }
        "doctor" => {
            let checks = doctor::run(&db);
            for c in &checks {
                println!("{} {:<17} {}", if c.ok { "✓" } else { "⚠" }, c.name, c.detail);
            }
            if checks.iter().any(|c| !c.ok) {
                std::process::exit(1);
            }
            return;
        }
        "daemon" => {
            let r = if args.iter().any(|a| a == "--install") {
                daemon::install(custom_db.then_some(db.as_path()))
            } else if args.iter().any(|a| a == "--uninstall") {
                daemon::uninstall()
            } else {
                daemon::run(&db)
            };
            match r {
                Ok(m) => return println!("{m}"),
                Err(e) => fail(&e),
            }
        }
        // A .ics file or webcal link opened from the desktop: import, then
        // open the app showing what happened.
        c if c.ends_with(".ics") || c.starts_with("webcal") || c.starts_with("file://") => {
            opening = Some(match cli::import(&db, &rest) {
                Ok(m) => m,
                Err(e) => format!("⚠ {e}"),
            });
        }
        c => match cli::run(&db, c, args) {
            Some(Ok(())) => return,
            Some(Err(e)) => fail(&e),
            None => fail(&format!("unknown command “{c}”. See calinator --help")),
        },
    }
    let store = match Store::open(&db).and_then(|s| s.ensure_calendar().map(|()| s)) {
        Ok(s) => s,
        Err(e) => fail(&format!("cannot open the calendar file {}: {e}", db.display())),
    };
    if let Err(e) = calinator::app::run(store, &db, opening) {
        fail(&e.to_string());
    }
}
