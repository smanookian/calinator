//! Accounts: list, add (Google, iCloud, CalDAV) and remove; the first-run
//! welcome; the list of sync conflicts. Checking a login runs on a thread,
//! so the screen never waits for the network.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::App;
use super::dialogs::{Dialog, draw_input, frame};
use super::draw::{c, fill, fit, put, put_right};
use super::input::Input;
use crate::accounts::{self, Account, ICLOUD_PASSWORD_PAGE, ICLOUD_URL, Kind};
use crate::edit::Changes;
use crate::model::new_id;
use crate::oauth;
use crate::sync::Provider;
use crate::sync::caldav::CalDav;

/// Kinds offered when adding, in order.
const KINDS: [Kind; 3] = [Kind::Google, Kind::ICloud, Kind::CalDav];

pub struct Form {
    kind: Kind,
    /// CalDAV: address, user, password. iCloud: Apple ID, password.
    inputs: Vec<Input>,
    sel: usize,
    err: Option<String>,
}

impl Form {
    fn new(kind: Kind) -> Self {
        let n = if kind == Kind::CalDav { 3 } else { 2 };
        Form { kind, inputs: vec![Input::default(); n], sel: 0, err: None }
    }

    fn labels(&self) -> &'static [&'static str] {
        match self.kind {
            Kind::CalDav => &["Server address", "User name", "Password"],
            _ => &["Apple ID (email)", "App password"],
        }
    }

    fn is_password(&self, i: usize) -> bool {
        i + 1 == self.inputs.len()
    }

    /// (url, user, password, shown name).
    fn values(&self) -> Result<(String, String, String, String), String> {
        let v: Vec<String> = self.inputs.iter().map(|i| i.text.trim().to_string()).collect();
        match self.kind {
            Kind::CalDav => {
                let url = if v[0].contains("://") { v[0].clone() } else { format!("https://{}", v[0]) };
                if v[0].is_empty() || v[1].is_empty() || v[2].is_empty() {
                    return Err("Fill in all three fields".into());
                }
                let host = url.split("://").nth(1).and_then(|r| r.split(['/', ':']).next()).unwrap_or("CalDAV").to_string();
                Ok((url, v[1].clone(), v[2].clone(), host))
            }
            _ => {
                if !v[0].contains('@') {
                    return Err("Enter your Apple ID email".into());
                }
                if v[1].is_empty() {
                    return Err("Enter the app-specific password".into());
                }
                // Apple shows these as xxxx-xxxx-xxxx-xxxx; spaces sneak in when pasted.
                Ok((ICLOUD_URL.into(), v[0].clone(), v[1].replace(' ', ""), format!("iCloud ({})", v[0])))
            }
        }
    }
}

pub enum Page {
    /// First run: how to start.
    Welcome {
        sel: usize,
    },
    List {
        sel: usize,
        confirm: bool,
    },
    Pick {
        sel: usize,
    },
    Form(Form),
    /// Checking the login on a thread.
    Checking(Form, Receiver<Result<Account, String>>),
    GoogleLater,
    /// Google's sign-in page is open in the browser.
    GoogleWaiting {
        url: String,
        cancel: Arc<AtomicBool>,
        rx: Receiver<Result<Account, String>>,
        err: Option<String>,
    },
}

pub struct AccountsUi {
    pub page: Page,
}

impl Default for AccountsUi {
    fn default() -> Self {
        AccountsUi { page: Page::List { sel: 0, confirm: false } }
    }
}

impl AccountsUi {
    pub fn welcome() -> Self {
        AccountsUi { page: Page::Welcome { sel: 0 } }
    }
}

/// Connects, lists calendars and saves the password in the keyring. Runs on a thread.
fn check(kind: Kind, url: String, user: String, password: String, name: String) -> Result<Account, String> {
    let mut p = CalDav::new(&url, &user, &password).map_err(|e| e.to_string())?;
    let cals = p.calendars().map_err(|e| e.to_string())?;
    if cals.is_empty() {
        return Err("Signed in, but found no calendars there".into());
    }
    let addresses = p.my_addresses().unwrap_or_default();
    let account = Account { id: new_id(), kind, name, url, username: user, addresses, ..Default::default() };
    accounts::set_secret(&account.id, &password)?;
    Ok(account)
}

pub fn key(app: &mut App, mut ui: AccountsUi, k: KeyEvent) -> Option<Dialog> {
    let esc = k.code == KeyCode::Esc;
    let down = matches!(k.code, KeyCode::Down | KeyCode::Char('j'));
    let up = matches!(k.code, KeyCode::Up | KeyCode::Char('k'));
    ui.page = match ui.page {
        Page::Welcome { mut sel } => {
            if esc || (k.code == KeyCode::Enter && sel == 3) {
                let _ = app.store.set_meta("welcomed", "1");
                return None;
            }
            if down {
                sel = (sel + 1) % 4;
            } else if up {
                sel = (sel + 3) % 4;
            } else if k.code == KeyCode::Enter {
                let _ = app.store.set_meta("welcomed", "1");
                return Some(Dialog::Accounts(AccountsUi { page: start(KINDS[sel]) }));
            }
            Page::Welcome { sel }
        }
        Page::List { mut sel, confirm } => {
            let n = app.accounts.len();
            if esc {
                return None;
            }
            match k.code {
                KeyCode::Char('a') | KeyCode::Enter if n == 0 || k.code == KeyCode::Char('a') => Page::Pick { sel: 0 },
                KeyCode::Char('s') => {
                    app.request_sync();
                    app.flash("Syncing…");
                    Page::List { sel, confirm: false }
                }
                KeyCode::Char('x') | KeyCode::Delete if n > 0 => {
                    if confirm {
                        let a = app.accounts[sel.min(n - 1)].clone();
                        let _ = app.store.delete_account(&a.id);
                        let _ = accounts::delete_secret(&a.id);
                        app.reload();
                        app.flash(format!("Removed {} (nothing was deleted on the server)", a.name));
                        Page::List { sel: 0, confirm: false }
                    } else {
                        Page::List { sel, confirm: true }
                    }
                }
                _ if down && n > 0 => {
                    sel = (sel + 1) % n;
                    Page::List { sel, confirm: false }
                }
                _ if up && n > 0 => {
                    sel = (sel + n - 1) % n;
                    Page::List { sel, confirm: false }
                }
                _ => Page::List { sel, confirm: false },
            }
        }
        Page::Pick { mut sel } => {
            if esc {
                Page::List { sel: 0, confirm: false }
            } else if down {
                sel = (sel + 1) % KINDS.len();
                Page::Pick { sel }
            } else if up {
                sel = (sel + KINDS.len() - 1) % KINDS.len();
                Page::Pick { sel }
            } else if k.code == KeyCode::Enter {
                start(KINDS[sel])
            } else {
                Page::Pick { sel }
            }
        }
        Page::GoogleLater => {
            if esc || k.code == KeyCode::Enter {
                Page::Pick { sel: 0 }
            } else {
                Page::GoogleLater
            }
        }
        Page::GoogleWaiting { url, cancel, rx, err } => {
            if esc {
                cancel.store(true, Ordering::Relaxed);
                Page::Pick { sel: 0 }
            } else if k.code == KeyCode::Enter && err.is_some() {
                google_start()
            } else {
                Page::GoogleWaiting { url, cancel, rx, err }
            }
        }
        Page::Form(mut f) => match k.code {
            KeyCode::Esc => Page::Pick { sel: KINDS.iter().position(|x| *x == f.kind).unwrap_or(0) },
            KeyCode::Tab | KeyCode::Down => {
                f.sel = (f.sel + 1) % f.inputs.len();
                Page::Form(f)
            }
            KeyCode::BackTab | KeyCode::Up => {
                f.sel = (f.sel + f.inputs.len() - 1) % f.inputs.len();
                Page::Form(f)
            }
            KeyCode::F(2) | KeyCode::Char('o')
                if f.kind == Kind::ICloud && k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                app.open_link(ICLOUD_PASSWORD_PAGE);
                Page::Form(f)
            }
            KeyCode::Enter => match f.values() {
                Err(e) => {
                    f.err = Some(e);
                    Page::Form(f)
                }
                Ok((url, user, pass, name)) => {
                    let (tx, rx) = channel();
                    let kind = f.kind;
                    std::thread::spawn(move || {
                        let _ = tx.send(check(kind, url, user, pass, name));
                    });
                    Page::Checking(f, rx)
                }
            },
            _ => {
                f.inputs[f.sel].key(&k);
                f.err = None;
                Page::Form(f)
            }
        },
        // Keys wait while checking; esc gives up waiting (the check finishes quietly).
        Page::Checking(f, rx) => {
            if esc {
                Page::Form(f)
            } else {
                Page::Checking(f, rx)
            }
        }
    };
    Some(Dialog::Accounts(ui))
}

fn start(kind: Kind) -> Page {
    match kind {
        Kind::Google => google_start(),
        k => Page::Form(Form::new(k)),
    }
}

/// Opens Google's sign-in page and waits for it on a thread.
fn google_start() -> Page {
    let Some(app) = oauth::google_app() else { return Page::GoogleLater };
    let pending = match oauth::begin(&app) {
        Ok(p) => p,
        Err(e) => {
            let (_, rx) = channel();
            return Page::GoogleWaiting { url: String::new(), cancel: Arc::default(), rx, err: Some(e) };
        }
    };
    let url = pending.url().to_string();
    oauth::open_browser(&url);
    let cancel: Arc<AtomicBool> = Arc::default();
    let (tx, rx) = channel();
    let stop = cancel.clone();
    std::thread::spawn(move || {
        let result = pending.wait(oauth::LOGIN_TIMEOUT, &stop).and_then(|t| {
            let email = t.email.clone().unwrap_or_else(|| "Google".into());
            let a = Account {
                id: new_id(),
                kind: Kind::Google,
                name: email.clone(),
                username: email.clone(),
                addresses: t.email.into_iter().collect(),
                ..Default::default()
            };
            accounts::set_secret(&a.id, &t.refresh)?;
            Ok(a)
        });
        let _ = tx.send(result);
    });
    Page::GoogleWaiting { url, cancel, rx, err: None }
}

/// Called every tick: finishes a login check.
pub fn poll(app: &mut App, ui: &mut AccountsUi) {
    if let Page::GoogleWaiting { rx, err, .. } = &mut ui.page {
        let Ok(result) = rx.try_recv() else { return };
        match result {
            Ok(a) => {
                match app.store.add_account(&a) {
                    Ok(None) => {}
                    Ok(Some(existing)) => {
                        // Same Google account again: keep it, with the new sign-in.
                        let old = app
                            .accounts
                            .iter()
                            .find(|x| x.kind == a.kind && x.username.eq_ignore_ascii_case(&a.username))
                            .map(|x| x.id.clone());
                        if let (Some(old), Ok(Some(token))) = (old, accounts::get_secret(&a.id)) {
                            let _ = accounts::set_secret(&old, &token);
                        }
                        let _ = accounts::delete_secret(&a.id);
                        ui.page = Page::List { sel: 0, confirm: false };
                        app.request_sync();
                        app.flash(format!("Signed in to {existing} again"));
                        return;
                    }
                    Err(e) => {
                        *err = Some(format!("Could not save: {e}"));
                        return;
                    }
                }
                ui.page = Page::List { sel: 0, confirm: false };
                app.reload();
                app.request_sync();
                app.flash(format!("Added {}. Syncing your calendars…", a.name));
            }
            Err(e) => *err = Some(e),
        }
        return;
    }
    let Page::Checking(_, rx) = &ui.page else { return };
    let Ok(result) = rx.try_recv() else { return };
    let Page::Checking(mut f, _) = std::mem::replace(&mut ui.page, Page::List { sel: 0, confirm: false }) else { return };
    match result {
        Ok(a) => {
            match app.store.add_account(&a) {
                Ok(None) => {}
                Ok(Some(existing)) => {
                    let _ = accounts::delete_secret(&a.id);
                    f.err = Some(format!("{existing} is already added"));
                    ui.page = Page::Form(f);
                    return;
                }
                Err(e) => {
                    f.err = Some(format!("Could not save: {e}"));
                    ui.page = Page::Form(f);
                    return;
                }
            }
            app.reload();
            app.request_sync();
            app.flash(format!("Added {}. Syncing your calendars…", a.name));
        }
        Err(e) => {
            f.err = Some(e);
            ui.page = Page::Form(f);
        }
    }
}

pub fn draw(f: &mut Frame, app: &App, ui: &AccountsUi) {
    let t = &app.theme;
    let fg = Style::default().fg(c(t.fg)).bg(c(t.dialog_bg));
    let muted = Style::default().fg(c(t.muted)).bg(c(t.dialog_bg));
    let accent = Style::default().fg(c(t.accent)).bg(c(t.dialog_bg));
    let pick = |f: &mut Frame, r: Rect, y: u16, text: &str, active: bool| {
        let bg = if active { t.selection } else { t.dialog_bg };
        fill(f.buffer_mut(), Rect { x: r.x, y, width: r.width, height: 1 }, bg);
        put(f.buffer_mut(), r.x, y, if active { "→" } else { " " }, 1, Style::default().fg(c(t.marker)).bg(c(bg)));
        put(
            f.buffer_mut(),
            r.x + 2,
            y,
            text,
            r.width.saturating_sub(2),
            Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg)),
        );
    };
    match &ui.page {
        Page::Welcome { sel } => {
            let r = frame(f, app, 64, 13, "Welcome to Calinator", "enter pick  esc skip");
            put(f.buffer_mut(), r.x, r.y, "Add an online calendar, so changes sync both ways", r.width, fg);
            put(f.buffer_mut(), r.x, r.y + 1, "with your phone and the web. You can add more later (@).", r.width, muted);
            let items = ["Google", "iCloud", "Other (CalDAV: Fastmail, Nextcloud…)", "Only on this computer"];
            for (i, it) in items.iter().enumerate() {
                pick(f, r, r.y + 3 + i as u16, it, i == *sel);
            }
        }
        Page::List { sel, confirm } => {
            let h = (app.accounts.len() as u16 * 2 + 7).clamp(8, 24);
            let r = frame(f, app, 70, h, "Accounts", "a add  x remove  s sync  esc close");
            if app.accounts.is_empty() {
                put(f.buffer_mut(), r.x, r.y, "No online accounts yet. Press a to add one.", r.width, muted);
            }
            for (i, a) in app.accounts.iter().enumerate() {
                let y = r.y + i as u16 * 2;
                pick(f, r, y, &format!("{} · {}", a.name, a.kind.label()), i == *sel);
                let status = match (&a.error, a.last_sync) {
                    (Some(e), _) => format!("⚠ {e}"),
                    (None, Some(t)) => format!("● synced {}", crate::tz::local(t, app.zone).format("%b %-d %H:%M")),
                    (None, None) => "↻ not synced yet".into(),
                };
                put(f.buffer_mut(), r.x + 2, y + 1, &fit(&status, r.width as usize - 2), r.width.saturating_sub(2), muted);
            }
            if *confirm && let Some(a) = app.accounts.get(*sel) {
                put(
                    f.buffer_mut(),
                    r.x,
                    r.y + r.height,
                    &format!("Press x again to remove {} from this computer", a.name),
                    r.width,
                    accent,
                );
            }
        }
        Page::Pick { sel } => {
            let r = frame(f, app, 56, 9, "Add an account", "enter pick  esc back");
            for (i, k) in KINDS.iter().enumerate() {
                let label = match k {
                    Kind::Google => "Google",
                    Kind::ICloud => "iCloud",
                    Kind::CalDav => "Other (CalDAV: Fastmail, Nextcloud, Radicale…)",
                };
                pick(f, r, r.y + i as u16, label, i == *sel);
            }
        }
        Page::GoogleLater => {
            let r = frame(f, app, 64, 8, "Google", "esc back");
            put(f.buffer_mut(), r.x, r.y, "Google sign-in is not set up in this build.", r.width, fg);
            put(f.buffer_mut(), r.x, r.y + 1, "It needs the Calinator Google app's client ID.", r.width, muted);
        }
        Page::GoogleWaiting { url, err, .. } => {
            let r = frame(f, app, 76, 11, "Sign in with Google", "esc cancel");
            put(f.buffer_mut(), r.x, r.y, "Your browser opened Google's sign-in page.", r.width, fg);
            put(f.buffer_mut(), r.x, r.y + 1, "Sign in and allow Calinator to see and edit your calendars.", r.width, muted);
            put(f.buffer_mut(), r.x, r.y + 2, "(A “Google hasn't verified this app” page is expected in the beta:", r.width, muted);
            put(f.buffer_mut(), r.x, r.y + 3, " click Advanced → Go to Calinator.)", r.width, muted);
            put(f.buffer_mut(), r.x, r.y + 5, "No browser? Open this link:", r.width, muted);
            put(f.buffer_mut(), r.x, r.y + 6, &fit(url, r.width as usize), r.width, accent);
            match err {
                Some(e) => put(
                    f.buffer_mut(),
                    r.x,
                    r.y + 8,
                    &format!("⚠ {e}"),
                    r.width,
                    Style::default().fg(c(musinator_themes::legible(t.error, t.dialog_bg, 4.5))).bg(c(t.dialog_bg)),
                ),
                None => put(f.buffer_mut(), r.x, r.y + 8, "Waiting for Google…", r.width, accent),
            };
        }
        Page::Form(form) | Page::Checking(form, _) => {
            let checking = matches!(ui.page, Page::Checking(..));
            let icloud = form.kind == Kind::ICloud;
            let guide: &[&str] = if icloud {
                &[
                    "iCloud needs an app-specific password (not your Apple password):",
                    "1. Open account.apple.com (ctrl+o opens it) and sign in.",
                    "2. Sign-In and Security → App-Specific Passwords → +.",
                    "3. Name it Calinator, then copy the password here.",
                ]
            } else {
                &["The address of your CalDAV server, e.g. caldav.fastmail.com", "or https://cloud.example.com/remote.php/dav"]
            };
            let h = guide.len() as u16 + form.inputs.len() as u16 + 8;
            let title = if icloud { "Add iCloud" } else { "Add a CalDAV account" };
            let r = frame(f, app, 72, h, title, "enter add  tab next  esc back");
            for (i, l) in guide.iter().enumerate() {
                put(f.buffer_mut(), r.x, r.y + i as u16, l, r.width, if i == 0 { fg } else { muted });
            }
            let y0 = r.y + guide.len() as u16 + 1;
            for (i, label) in form.labels().iter().enumerate() {
                let y = y0 + i as u16;
                let active = i == form.sel && !checking;
                put(f.buffer_mut(), r.x, y, label, 18, if active { accent.add_modifier(Modifier::BOLD) } else { muted });
                let shown = if form.is_password(i) {
                    Input::new(&"•".repeat(form.inputs[i].text.chars().count()))
                } else {
                    form.inputs[i].clone()
                };
                draw_input(f, app, r.x + 19, y, r.width.saturating_sub(19), &shown, active);
            }
            let note_y = y0 + form.inputs.len() as u16 + 1;
            if checking {
                put(f.buffer_mut(), r.x, note_y, "Checking… (the password goes to the system keyring)", r.width, accent);
            } else if let Some(e) = &form.err {
                put(
                    f.buffer_mut(),
                    r.x,
                    note_y,
                    &fit(&format!("⚠ {e}"), r.width as usize),
                    r.width,
                    Style::default().fg(c(musinator_themes::legible(t.error, t.dialog_bg, 4.5))).bg(c(t.dialog_bg)),
                );
            } else {
                put(f.buffer_mut(), r.x, note_y, "The password is kept in the system keyring, not in Calinator's files.", r.width, muted);
            }
        }
    }
}

// ---------------- conflicts

pub fn conflicts_key(app: &mut App, sel: usize, k: KeyEvent) -> Option<Dialog> {
    let list = app.store.conflicts().unwrap_or_default();
    if k.code == KeyCode::Esc || list.is_empty() {
        return None;
    }
    let sel = sel.min(list.len() - 1);
    match k.code {
        KeyCode::Down | KeyCode::Char('j') => Some(Dialog::Conflicts { sel: (sel + 1) % list.len() }),
        KeyCode::Up | KeyCode::Char('k') => Some(Dialog::Conflicts { sel: (sel + list.len() - 1) % list.len() }),
        KeyCode::Char('d') | KeyCode::Char('x') => {
            let _ = app.store.delete_conflict(&list[sel].id);
            app.reload();
            Some(Dialog::Conflicts { sel })
        }
        KeyCode::Char('u') | KeyCode::Enter => {
            // Use mine: put this computer's version back; it syncs as a new edit.
            let c = &list[sel];
            let now: Vec<String> = app.events.iter().filter(|e| e.uid == c.uid && e.calendar == c.calendar).map(|e| e.id.clone()).collect();
            let keep: Vec<String> = c.mine.iter().map(|e| e.id.clone()).collect();
            let delete = now.into_iter().filter(|id| !keep.contains(id)).collect();
            let upsert = c.mine.iter().map(|e| crate::model::Event { calendar: c.calendar.clone(), ..e.clone() }).collect();
            app.commit(Changes::new(upsert, delete), None, "Your version is back");
            let _ = app.store.delete_conflict(&c.id);
            app.reload();
            Some(Dialog::Conflicts { sel })
        }
        _ => Some(Dialog::Conflicts { sel }),
    }
}

pub fn draw_conflicts(f: &mut Frame, app: &App, sel: usize) {
    let t = &app.theme;
    let list = app.store.conflicts().unwrap_or_default();
    let h = (list.len() as u16 * 3 + 6).clamp(8, 26);
    let r = frame(f, app, 76, h, "Sync conflicts", "enter use mine  d dismiss  esc close");
    if list.is_empty() {
        put(f.buffer_mut(), r.x, r.y, "No conflicts. Nothing was lost.", r.width, Style::default().fg(c(t.muted)).bg(c(t.dialog_bg)));
        return;
    }
    for (i, cf) in list.iter().enumerate() {
        let y = r.y + i as u16 * 3;
        if y + 2 > r.y + r.height + 1 {
            break;
        }
        let active = i == sel.min(list.len() - 1);
        let bg = if active { t.selection } else { t.dialog_bg };
        fill(f.buffer_mut(), Rect { x: r.x, y, width: r.width, height: 2 }, bg);
        let st = Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg));
        put(f.buffer_mut(), r.x + 1, y, &cf.title, r.width.saturating_sub(20), st.add_modifier(Modifier::BOLD));
        put_right(f.buffer_mut(), r.x + r.width, y, &crate::tz::local(cf.at, app.zone).format("%b %-d %H:%M").to_string(), 16, st);
        put(f.buffer_mut(), r.x + 1, y + 1, &fit(&cf.note, r.width as usize - 2), r.width.saturating_sub(2), st);
    }
}
