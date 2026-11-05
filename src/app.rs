use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{self, BufRead, BufReader},
    os::unix::{fs::MetadataExt, net::UnixListener},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute, terminal,
};

use crate::{
    book, config,
    config::Config,
    disguise::{self, Rng},
    progress::{Pos, Store},
    render::{self, now, Entry, Level, Line},
};

const MAX_BOSS: usize = 300;
const SAVE_EVERY: Duration = Duration::from_secs(2);
/// Modules the app itself logs under (picker, notices); they count toward the column width.
const OWN_MODULES: [&str; 2] = ["loader", "stats"];

struct Picker {
    items: Vec<PathBuf>,
    sel: usize,
}

struct App {
    cfg: Config,
    store: Store,
    rng: Rng,
    module_w: usize,
    cols: u16,
    rows: u16,

    book: Option<PathBuf>,
    paras: Vec<String>,
    lines: Vec<Line>,
    cur: usize,
    /// When each line was "logged", keyed by (para, start) so it survives redraws.
    stamps: HashMap<(usize, usize), i64>,

    boss: bool,
    boss_log: VecDeque<Entry>,
    next_boss_line: Instant,
    picker: Option<Picker>,
    notice: Option<(Entry, Instant)>,

    dirty: bool,
    unsaved: bool,
    last_save: Instant,
    quit: bool,
}

impl App {
    fn new(cfg: Config, store: Store, cols: u16, rows: u16) -> App {
        let module_w = cfg
            .modules
            .iter()
            .map(|m| m.chars().count())
            .chain(OWN_MODULES.iter().map(|m| m.len()))
            .max()
            .unwrap_or(4);
        App {
            cfg,
            store,
            rng: Rng::new(),
            module_w,
            cols,
            rows,
            book: None,
            paras: Vec::new(),
            lines: Vec::new(),
            cur: 0,
            stamps: HashMap::new(),
            boss: false,
            boss_log: VecDeque::new(),
            next_boss_line: Instant::now(),
            picker: None,
            notice: None,
            dirty: true,
            unsaved: false,
            last_save: Instant::now(),
            quit: false,
        }
    }

    fn text_width(&self) -> usize {
        (self.cols as usize)
            .saturating_sub(render::prefix_width(self.module_w) + 1)
            .max(10)
    }

    fn pos(&self) -> Option<Pos> {
        self.lines.get(self.cur).map(|l| Pos { para: l.para, offset: l.start })
    }

    fn rewrap(&mut self) {
        let pos = self.pos().unwrap_or_default();
        self.lines = render::wrap_book(&self.paras, self.text_width());
        self.cur = render::find_line(&self.lines, pos.para, pos.offset);
    }

    fn notify(&mut self, level: Level, module: &str, text: String, secs: u64) {
        let e = Entry { time: now(), level, module: module.into(), text };
        self.notice = Some((e, Instant::now() + Duration::from_secs(secs)));
        self.dirty = true;
    }

    fn save(&mut self) {
        if let (Some(path), Some(pos)) = (&self.book, self.pos()) {
            let key = path.to_string_lossy().into_owned();
            self.store.books.insert(key.clone(), pos);
            self.store.last = Some(key);
        }
        let _ = self.store.save();
        self.unsaved = false;
        self.last_save = Instant::now();
    }

    fn open_book(&mut self, path: &Path) {
        if self.book.is_some() {
            self.save();
        }
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match book::load(&path) {
            Ok(paras) => {
                let pos = self
                    .store
                    .books
                    .get(path.to_string_lossy().as_ref())
                    .copied()
                    .unwrap_or_default();
                self.paras = paras;
                self.lines = render::wrap_book(&self.paras, self.text_width());
                self.cur = render::find_line(&self.lines, pos.para, pos.offset);
                self.stamps.clear();
                self.book = Some(path);
                self.notice = None;
                self.save();
            }
            Err(e) => self.notify(Level::Warn, "loader", format!("{name}: {e:#}"), 8),
        }
        self.dirty = true;
    }

    fn library(&self) -> Vec<PathBuf> {
        fn collect(dir: &Path, depth: u8, items: &mut Vec<PathBuf>) {
            let Ok(rd) = fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if depth > 0 {
                        collect(&p, depth - 1, items);
                    }
                } else if book::is_book(&p) {
                    items.push(p.canonicalize().unwrap_or(p));
                }
            }
        }
        let mut items = Vec::new();
        collect(&config::expand(&self.cfg.library), 2, &mut items);
        items.extend(self.store.books.keys().map(PathBuf::from).filter(|p| p.exists()));
        items.sort();
        items.dedup();
        items
    }

    fn open_picker(&mut self) {
        let items = self.library();
        if items.is_empty() {
            let cfg = config::config_dir().join("config.toml");
            self.notify(
                Level::Warn,
                "loader",
                format!("no input in {} (set `library` in {})", self.cfg.library, cfg.display()),
                3600,
            );
            return;
        }
        let sel = self
            .book
            .as_ref()
            .and_then(|b| items.iter().position(|p| p == b))
            .unwrap_or(0);
        self.picker = Some(Picker { items, sel });
        self.dirty = true;
    }

    /// Lines per page: the whole pane.
    fn page(&self) -> usize {
        (self.rows as usize).max(1)
    }

    /// Turn `pages` whole pages (negative = back). Every row on screen is replaced, nothing
    /// from the previous page stays behind.
    fn step(&mut self, pages: isize) {
        if self.lines.is_empty() {
            return;
        }
        let page = self.page();
        let new = if pages > 0 {
            if self.cur + page >= self.lines.len() {
                self.notify(Level::Info, "stats", "queue drained, idle".into(), 3);
                return;
            }
            let last_start = (self.lines.len() - 1) / page * page;
            (self.cur + pages as usize * page).min(last_start.max(self.cur + page))
        } else {
            self.cur.saturating_sub(pages.unsigned_abs() * page)
        };
        if new == self.cur {
            return;
        }
        let t = now();
        for l in &self.lines[new..(new + page).min(self.lines.len())] {
            self.stamps.insert((l.para, l.start), t);
        }
        self.cur = new;
        self.notice = None;
        self.dirty = true;
        self.unsaved = true;
    }

    fn info(&mut self) {
        if self.lines.is_empty() {
            return;
        }
        let seen = (self.cur + self.page()).min(self.lines.len());
        let pct = seen as f64 * 100.0 / self.lines.len() as f64;
        let text = format!("processed {seen}/{} records ({pct:.1}%)", self.lines.len());
        self.notify(Level::Info, "stats", text, 4);
    }

    fn toggle_boss(&mut self) {
        self.boss = !self.boss;
        if self.boss {
            self.picker = None;
            let t = now();
            if self.boss_log.is_empty() {
                // Back-fill a screenful so the pane looks like it has been running a while.
                let mut back = 0;
                let mut fill = Vec::new();
                for _ in 0..self.rows {
                    back += self.rng.range(1, 6) as i64;
                    fill.push(disguise::fake_entry(&mut self.rng, &self.cfg.modules, t - back));
                }
                self.boss_log.extend(fill.into_iter().rev());
            } else {
                let e = disguise::fake_entry(&mut self.rng, &self.cfg.modules, t);
                self.boss_log.push_back(e);
            }
            self.schedule_boss_line();
        }
        self.dirty = true;
    }

    fn schedule_boss_line(&mut self) {
        self.next_boss_line = Instant::now() + Duration::from_millis(self.rng.range(1500, 5000));
    }

    fn tick(&mut self) {
        if self.boss && Instant::now() >= self.next_boss_line {
            let e = disguise::fake_entry(&mut self.rng, &self.cfg.modules, now());
            self.boss_log.push_back(e);
            while self.boss_log.len() > MAX_BOSS {
                self.boss_log.pop_front();
            }
            self.schedule_boss_line();
            self.dirty = true;
        }
        if self.notice.as_ref().is_some_and(|(_, until)| Instant::now() >= *until) {
            self.notice = None;
            self.dirty = true;
        }
        if self.unsaved && self.last_save.elapsed() >= SAVE_EVERY {
            self.save();
        }
    }

    fn command(&mut self, cmd: &str) {
        match cmd {
            "boss" => self.toggle_boss(),
            "quit" => self.quit = true,
            _ if self.boss || self.picker.is_some() => {}
            "next" => self.step(1),
            "prev" => self.step(-1),
            "info" => self.info(),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl_c = k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c');
        if ctrl_c {
            self.quit = true;
            return;
        }
        if k.code == KeyCode::Char('b') {
            self.toggle_boss();
            return;
        }
        if self.boss {
            // Anything but b / q is swallowed so a stray key never reveals the book.
            self.quit = k.code == KeyCode::Char('q');
            return;
        }
        if let Some(p) = &mut self.picker {
            match k.code {
                KeyCode::Down | KeyCode::Char('j') => p.sel = (p.sel + 1).min(p.items.len() - 1),
                KeyCode::Up | KeyCode::Char('k') => p.sel = p.sel.saturating_sub(1),
                KeyCode::Enter => {
                    let path = p.items[p.sel].clone();
                    self.picker = None;
                    self.open_book(&path);
                }
                KeyCode::Esc | KeyCode::Char('o') | KeyCode::Char('q') => self.picker = None,
                _ => {}
            }
            self.dirty = true;
            return;
        }
        match k.code {
            KeyCode::Char('j' | 'n' | ' ') | KeyCode::Enter | KeyCode::Down | KeyCode::PageDown => {
                self.step(1)
            }
            KeyCode::Char('k' | 'p') | KeyCode::Up | KeyCode::PageUp => self.step(-1),
            KeyCode::Char('J') => self.step(10),
            KeyCode::Char('K') => self.step(-10),
            KeyCode::Char('i') => self.info(),
            KeyCode::Char('o') => self.open_picker(),
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.rewrap();
        self.dirty = true;
    }

    fn reading_entries(&mut self, rows: usize) -> Vec<Entry> {
        if self.lines.is_empty() {
            return Vec::new();
        }
        let t = now();
        let end = (self.cur + rows).min(self.lines.len());
        (self.cur..end)
            .map(|i| {
                let l = &self.lines[i];
                let time = *self.stamps.entry((l.para, l.start)).or_insert(t);
                let (level, module) = disguise::tag(l.para, l.start, &self.cfg.modules);
                Entry { time, level, module, text: l.text.clone() }
            })
            .collect()
    }

    fn picker_entries(&self, p: &Picker, rows: usize) -> Vec<Entry> {
        let start = (p.sel + 1).saturating_sub(rows);
        let t = now();
        p.items
            .iter()
            .enumerate()
            .skip(start)
            .take(rows)
            .map(|(i, path)| {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let mark = if i == p.sel { '>' } else { ' ' };
                Entry { time: t, level: Level::Info, module: "loader".into(), text: format!("{mark} {name}") }
            })
            .collect()
    }

    fn draw(&mut self, out: &mut impl io::Write) -> io::Result<()> {
        let rows = self.rows as usize;
        let mut entries = if self.boss {
            self.boss_log.iter().skip(self.boss_log.len().saturating_sub(rows)).cloned().collect()
        } else if let Some(p) = &self.picker {
            self.picker_entries(p, rows)
        } else {
            self.reading_entries(rows)
        };
        if let (false, Some((e, _))) = (self.boss, &self.notice) {
            if entries.len() >= rows && !entries.is_empty() {
                entries.remove(0);
            }
            entries.push(e.clone());
        }
        render::draw(out, &entries, self.module_w, self.cols, self.rows)
    }
}

struct TermGuard;

fn restore_terminal() {
    let _ = execute!(io::stdout(), cursor::Show, terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
}

impl TermGuard {
    fn enter() -> io::Result<TermGuard> {
        terminal::enable_raw_mode()?;
        execute!(io::stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            prev(info);
        }));
        Ok(TermGuard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// The control socket the `next` / `prev` / `boss` actions write to. The newest reader wins;
/// on exit the file is removed only if it is still ours.
struct Control {
    listener: UnixListener,
    path: PathBuf,
    ino: u64,
}

impl Control {
    fn bind() -> Option<Control> {
        let path = config::socket_path();
        fs::create_dir_all(path.parent()?).ok()?;
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path).ok()?;
        listener.set_nonblocking(true).ok()?;
        let ino = fs::metadata(&path).ok()?.ino();
        Some(Control { listener, path, ino })
    }

    fn poll(&self, app: &mut App) {
        while let Ok((stream, _)) = self.listener.accept() {
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok() {
                app.command(line.trim());
            }
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        if fs::metadata(&self.path).is_ok_and(|m| m.ino() == self.ino) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn run(path: Option<PathBuf>) -> Result<()> {
    let cfg = config::load();
    let store = Store::load();
    let target = path.or_else(|| store.last.as_ref().map(PathBuf::from));

    let _term = TermGuard::enter()?;
    let (cols, rows) = terminal::size()?;
    let mut app = App::new(cfg, store, cols, rows);
    let control = Control::bind();
    match target {
        Some(p) => app.open_book(&p),
        None => app.open_picker(),
    }

    let mut out = io::stdout();
    while !app.quit {
        if app.dirty {
            app.draw(&mut out)?;
            app.dirty = false;
        }
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k),
                Event::Resize(c, r) => app.resize(c, r),
                _ => {}
            }
        }
        if let Some(c) = &control {
            c.poll(&mut app);
        }
        // A resize that lands before crossterm hooks SIGWINCH (herdr shrinks the pane right
        // after creating it) never shows up as an event, so check the size directly too.
        if let Ok((c, r)) = terminal::size() {
            if (c, r) != (app.cols, app.rows) {
                app.resize(c, r);
            }
        }
        app.tick();
    }
    app.save();
    Ok(())
}
