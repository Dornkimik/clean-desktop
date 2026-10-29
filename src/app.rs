use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use egui::load::SizedTexture;
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Key, Label, Layout, Margin, Modifiers,
    Rect, Response, RichText, Sense, Stroke, StrokeKind, TextStyle, TextWrapMode, TextureHandle,
    TextureOptions, Ui, UiBuilder, WidgetText, pos2, vec2,
};
use egui_phosphor::regular as icon;

use crate::preview::{FolderSummary, Loader, Preview};
use crate::scan::{self, Entry, Kind, SortMode};
use crate::theme::*;
use crate::util::{age_days, format_date, format_size, relative_time};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    Keep,
    Delete,
}

struct Item {
    entry: Entry,
    decision: Option<Decision>,
}

enum Shown {
    Image {
        texture: TextureHandle,
        width: u32,
        height: u32,
    },
    Data(Preview),
}

struct Report {
    moved: usize,
    freed: u64,
    failed: Vec<(String, String)>,
}

enum Screen {
    Review,
    Summary,
    Applying,
    Report(Report),
}

type ApplyResult = Vec<(PathBuf, u64, Result<(), String>)>;

enum Action {
    Decide(Decision),
    SetDecision(usize, Decision),
    Undo,
    Select(usize),
    Next,
    Prev,
    Open,
    Reveal,
    RevealPath(PathBuf),
    Finish,
    BackToReview,
    Apply,
    Rescan,
    SetSort(SortMode),
    Quit,
}

pub struct App {
    root: PathBuf,
    items: Vec<Item>,
    current: usize,
    /// Undo stack: the item and the decision it had before.
    history: Vec<(PathBuf, Option<Decision>)>,
    previews: HashMap<PathBuf, Shown>,
    loader: Loader,
    screen: Screen,
    sort: SortMode,
    scan_error: Option<String>,
    scroll_to_current: bool,
    toast: Option<(String, f64)>,
    apply_rx: Option<Receiver<ApplyResult>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply(&cc.egui_ctx);
        let mut app = Self {
            root: scan::default_root(),
            items: Vec::new(),
            current: 0,
            history: Vec::new(),
            previews: HashMap::new(),
            loader: Loader::new(),
            screen: Screen::Review,
            sort: SortMode::OldestFirst,
            scan_error: None,
            scroll_to_current: true,
            toast: None,
            apply_rx: None,
        };
        app.rescan();
        app
    }

    // ---------------------------------------------------------------- state

    fn rescan(&mut self) {
        let previous: HashMap<PathBuf, Decision> = self
            .items
            .iter()
            .filter_map(|i| Some((i.entry.path.clone(), i.decision?)))
            .collect();
        match scan::scan(&self.root) {
            Ok(entries) => {
                self.items = entries
                    .into_iter()
                    .map(|entry| Item {
                        decision: previous.get(&entry.path).copied(),
                        entry,
                    })
                    .collect();
                self.scan_error = None;
            }
            Err(e) => {
                self.items.clear();
                self.scan_error = Some(format!("Couldn't read {}: {e}", self.root.display()));
            }
        }
        for path in self.previews.keys().cloned().collect::<Vec<_>>() {
            self.loader.forget(&path);
        }
        self.previews.clear();
        self.history.clear();
        self.current = 0;
        self.sort_items();
        self.current = self.first_undecided().unwrap_or(0);
        self.screen = Screen::Review;
        self.scroll_to_current = true;
    }

    fn sort_items(&mut self) {
        let current = self.current_path();
        scan::sort_entries(&mut self.items, self.sort, |i| &i.entry);
        if let Some(idx) = current.and_then(|p| self.index_of(&p)) {
            self.current = idx;
        }
    }

    fn current_path(&self) -> Option<PathBuf> {
        self.items.get(self.current).map(|i| i.entry.path.clone())
    }

    fn index_of(&self, path: &Path) -> Option<usize> {
        self.items.iter().position(|i| i.entry.path == path)
    }

    fn first_undecided(&self) -> Option<usize> {
        self.items.iter().position(|i| i.decision.is_none())
    }

    fn next_undecided(&self, from: usize) -> Option<usize> {
        let n = self.items.len();
        (1..=n)
            .map(|offset| (from + offset) % n)
            .find(|&i| self.items[i].decision.is_none())
    }

    fn count(&self, decision: Option<Decision>) -> usize {
        self.items.iter().filter(|i| i.decision == decision).count()
    }

    fn decide(&mut self, decision: Decision) {
        let Some(item) = self.items.get_mut(self.current) else {
            return;
        };
        self.history.push((item.entry.path.clone(), item.decision));
        item.decision = Some(decision);
        match self.next_undecided(self.current) {
            Some(next) => {
                self.current = next;
                self.scroll_to_current = true;
            }
            None => self.screen = Screen::Summary,
        }
    }

    fn undo(&mut self) {
        let Some((path, previous)) = self.history.pop() else {
            return;
        };
        if let Some(idx) = self.index_of(&path) {
            self.items[idx].decision = previous;
            self.current = idx;
            self.scroll_to_current = true;
        }
        self.screen = Screen::Review;
    }

    fn show_toast(&mut self, ctx: &egui::Context, message: impl Into<String>) {
        let now = ctx.input(|i| i.time);
        self.toast = Some((message.into(), now + 3.0));
    }

    fn open_path(&mut self, ctx: &egui::Context, path: &Path) {
        if let Err(e) = open::that_detached(path) {
            self.show_toast(ctx, format!("Couldn't open: {e}"));
        }
    }

    fn reveal_path(&mut self, ctx: &egui::Context, path: &Path) {
        #[cfg(windows)]
        let result = std::process::Command::new("explorer")
            .arg("/select,")
            .arg(path)
            .spawn()
            .map(|_| ());
        #[cfg(not(windows))]
        let result = open::that_detached(path.parent().unwrap_or(path));
        if let Err(e) = result {
            self.show_toast(ctx, format!("Couldn't open the file manager: {e}"));
        }
    }

    fn start_apply(&mut self, ctx: &egui::Context) {
        let targets: Vec<(PathBuf, u64)> = self
            .items
            .iter()
            .filter(|i| i.decision == Some(Decision::Delete))
            .map(|i| (i.entry.path.clone(), i.entry.size.unwrap_or(0)))
            .collect();
        if targets.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        self.apply_rx = Some(rx);
        self.screen = Screen::Applying;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let results: ApplyResult = targets
                .into_iter()
                .map(|(path, size)| {
                    let result = trash::delete(&path).map_err(|e| e.to_string());
                    (path, size, result)
                })
                .collect();
            let _ = tx.send(results);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        for (path, preview) in self.loader.poll() {
            let shown = match preview {
                Preview::Image {
                    image,
                    width,
                    height,
                } => Shown::Image {
                    texture: ctx.load_texture(
                        path.to_string_lossy(),
                        image,
                        TextureOptions::LINEAR,
                    ),
                    width,
                    height,
                },
                other => {
                    if let Preview::Folder(summary) = &other
                        && let Some(item) = self.items.iter_mut().find(|i| i.entry.path == path) {
                            item.entry.size = Some(summary.total_size);
                        }
                    Shown::Data(other)
                }
            };
            self.previews.insert(path, shown);
        }

        let results = self.apply_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(results) = results {
            self.apply_rx = None;
            let mut report = Report {
                moved: 0,
                freed: 0,
                failed: Vec::new(),
            };
            let mut removed = HashSet::new();
            for (path, size, result) in results {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match result {
                    Ok(()) => {
                        report.moved += 1;
                        report.freed += size;
                        self.previews.remove(&path);
                        removed.insert(path);
                    }
                    Err(e) => report.failed.push((name, e)),
                }
            }
            self.items.retain(|i| !removed.contains(&i.entry.path));
            self.history.clear();
            self.current = self.first_undecided().unwrap_or(0);
            self.scroll_to_current = true;
            self.screen = Screen::Report(report);
        }
    }

    fn ensure_previews(&mut self, ctx: &egui::Context) {
        let len = self.items.len();
        if len == 0 {
            return;
        }
        for item in &self.items[self.current..(self.current + 3).min(len)] {
            let path = &item.entry.path;
            if !self.previews.contains_key(path) && !self.loader.is_pending(path) {
                self.loader.request(ctx, &item.entry);
            }
        }
        // Textures are the only heavy thing: keep just the ones near the cursor.
        let window = self.current.saturating_sub(4)..(self.current + 6).min(len);
        let nearby: HashSet<&PathBuf> = self.items[window].iter().map(|i| &i.entry.path).collect();
        self.previews
            .retain(|path, shown| !matches!(shown, Shown::Image { .. }) || nearby.contains(path));
    }

    fn keyboard(&mut self, ctx: &egui::Context) -> Vec<Action> {
        // Keys drive the app; never let a focused button swallow them.
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        let mut actions = Vec::new();
        ctx.input_mut(|i| {
            let none = Modifiers::NONE;
            match self.screen {
                Screen::Review if !self.items.is_empty() => {
                    if i.consume_key(Modifiers::COMMAND, Key::Z)
                        || i.consume_key(none, Key::Backspace)
                        || i.consume_key(none, Key::U)
                    {
                        actions.push(Action::Undo);
                    }
                    if i.consume_key(none, Key::K) || i.consume_key(none, Key::ArrowRight) {
                        actions.push(Action::Decide(Decision::Keep));
                    }
                    if i.consume_key(none, Key::D)
                        || i.consume_key(none, Key::Delete)
                        || i.consume_key(none, Key::ArrowLeft)
                    {
                        actions.push(Action::Decide(Decision::Delete));
                    }
                    if i.consume_key(none, Key::ArrowDown) || i.consume_key(none, Key::J) {
                        actions.push(Action::Next);
                    }
                    if i.consume_key(none, Key::ArrowUp) {
                        actions.push(Action::Prev);
                    }
                    if i.consume_key(none, Key::Enter) || i.consume_key(none, Key::O) {
                        actions.push(Action::Open);
                    }
                    if i.consume_key(none, Key::E) {
                        actions.push(Action::Reveal);
                    }
                    if i.consume_key(none, Key::F) {
                        actions.push(Action::Finish);
                    }
                }
                Screen::Summary => {
                    if i.consume_key(none, Key::Escape) || i.consume_key(none, Key::Backspace) {
                        actions.push(Action::BackToReview);
                    }
                    if i.consume_key(Modifiers::COMMAND, Key::Z) {
                        actions.push(Action::Undo);
                    }
                    if i.consume_key(Modifiers::COMMAND, Key::Enter) {
                        actions.push(Action::Apply);
                    }
                }
                _ => {}
            }
            if i.consume_key(none, Key::F5) {
                actions.push(Action::Rescan);
            }
        });
        actions
    }

    fn run(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Decide(d) => self.decide(d),
            Action::SetDecision(idx, d) => {
                if let Some(item) = self.items.get_mut(idx) {
                    self.history.push((item.entry.path.clone(), item.decision));
                    item.decision = Some(d);
                }
            }
            Action::Undo => self.undo(),
            Action::Select(idx) => {
                self.current = idx;
                self.screen = Screen::Review;
            }
            Action::Next => {
                if self.current + 1 < self.items.len() {
                    self.current += 1;
                    self.scroll_to_current = true;
                }
            }
            Action::Prev => {
                if self.current > 0 {
                    self.current -= 1;
                    self.scroll_to_current = true;
                }
            }
            Action::Open => {
                if let Some(path) = self.current_path() {
                    self.open_path(ctx, &path);
                }
            }
            Action::Reveal => {
                if let Some(path) = self.current_path() {
                    self.reveal_path(ctx, &path);
                }
            }
            Action::RevealPath(path) => self.reveal_path(ctx, &path),
            Action::Finish => self.screen = Screen::Summary,
            Action::BackToReview => self.screen = Screen::Review,
            Action::Apply => self.start_apply(ctx),
            Action::Rescan => self.rescan(),
            Action::SetSort(mode) => {
                self.sort = mode;
                self.sort_items();
                self.scroll_to_current = true;
            }
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    // ---------------------------------------------------------------- hints

    fn hints(&self, item: &Item, shown: Option<&Shown>) -> Vec<(Color32, &'static str, String)> {
        let e = &item.entry;
        let mut out = Vec::new();
        let mut last_change = e.modified;

        match shown {
            Some(Shown::Data(Preview::Shortcut {
                target: Some(_),
                target_exists: false,
                ..
            })) => out.push((
                DELETE,
                icon::LINK_BREAK,
                "Broken shortcut — its target no longer exists".to_owned(),
            )),
            Some(Shown::Data(Preview::Folder(f))) => {
                if f.files == 0 && f.folders == 0 {
                    out.push((WARN, icon::FOLDER_OPEN, "Empty folder".to_owned()));
                } else if f.files == 0 {
                    out.push((WARN, icon::FOLDER_OPEN, "Contains only empty folders".to_owned()));
                }
                if let Some((t, _)) = f.newest {
                    last_change = Some(t);
                }
            }
            _ => {}
        }
        if !e.is_dir && e.size == Some(0) {
            out.push((WARN, icon::FILE_DASHED, "Empty file (0 bytes)".to_owned()));
        }
        if let Some(days) = last_change.and_then(age_days) {
            if days >= 365 {
                let years = days / 365;
                let unit = if years == 1 { "year" } else { "years" };
                out.push((WARN, icon::CLOCK, format!("Not changed in over {years} {unit}")));
            } else if days >= 90 {
                out.push((MUTED, icon::CLOCK, format!("Not changed in {} months", days / 30)));
            }
        }
        if looks_like_copy(&e.name, e.is_dir) {
            out.push((WARN, icon::STACK, "Looks like a duplicate copy".to_owned()));
        }
        let lower = e.name.to_lowercase();
        if e.kind == Kind::Program
            && (e.extension == "msi" || ["setup", "install", "installer"].iter().any(|w| lower.contains(w)))
        {
            out.push((
                WARN,
                icon::APP_WINDOW,
                "Looks like an installer — rarely needed after installing".to_owned(),
            ));
        }
        if e.kind == Kind::Archive {
            let stem = Path::new(&e.name)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if self
                .items
                .iter()
                .any(|i| i.entry.is_dir && i.entry.name.eq_ignore_ascii_case(&stem))
            {
                out.push((
                    ACCENT,
                    icon::FOLDER_SIMPLE,
                    format!("A folder named “{stem}” is also here — already extracted?"),
                ));
            }
        }
        if let Some(size) = e.size.filter(|&s| s >= 1 << 30) {
            out.push((ACCENT, icon::HARD_DRIVE, format!("Large — takes up {}", format_size(size))));
        }
        out
    }

    // ---------------------------------------------------------------- ui

    fn top_bar(&self, ui: &mut Ui, actions: &mut Vec<Action>, review: bool) {
        egui::Panel::top("top")
            .exact_size(60.0)
            .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(20, 0)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(icon::BROOM).size(22.0).color(ACCENT));
                    ui.label(RichText::new("Clean Desktop").font(FontId::new(17.0, heading_family())));
                    ui.add_space(10.0);
                    let path_label = |ui: &mut Ui| {
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(
                                Label::new(RichText::new(self.root.display().to_string()).size(12.5).color(FAINT))
                                    .truncate(),
                            )
                            .on_hover_text(format!(
                                "{}\n\nStart the app with a folder path as argument to clean another folder.",
                                self.root.display()
                            ));
                        });
                    };

                    if !review || self.items.is_empty() {
                        path_label(ui);
                        return;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let to_delete = self.count(Some(Decision::Delete));
                        let label = if to_delete > 0 {
                            format!("Clean up {to_delete}  {}", icon::ARROW_RIGHT)
                        } else {
                            format!("Finish  {}", icon::ARROW_RIGHT)
                        };
                        let finish = egui::Button::new(RichText::new(label).color(BG).strong())
                            .fill(ACCENT)
                            .corner_radius(8)
                            .min_size(vec2(0.0, 32.0));
                        if ui.add(finish).on_hover_text("Review what will be deleted  (F)").clicked() {
                            actions.push(Action::Finish);
                        }
                        ui.add_space(16.0);

                        let total = self.items.len();
                        let decided = total - self.count(None);
                        ui.label(RichText::new(format!("{decided} / {total}")).color(MUTED));
                        ui.add(
                            egui::ProgressBar::new(decided as f32 / total.max(1) as f32)
                                .desired_width(110.0)
                                .desired_height(5.0)
                                .fill(ACCENT)
                                .corner_radius(3),
                        );
                        ui.add_space(16.0);

                        let mut sort = self.sort;
                        egui::ComboBox::from_id_salt("sort")
                            .selected_text(RichText::new(format!("{}  {}", icon::FUNNEL_SIMPLE, sort.label())).color(MUTED))
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                for mode in SortMode::ALL {
                                    ui.selectable_value(&mut sort, mode, mode.label());
                                }
                            });
                        if sort != self.sort {
                            actions.push(Action::SetSort(sort));
                        }
                        ui.add_space(16.0);
                        path_label(ui);
                    });
                });
            });
    }

    fn review_ui(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        self.top_bar(ui, actions, true);

        if self.items.is_empty() {
            egui::CentralPanel::no_frame()
                .frame(Frame::new().fill(BG))
                .show(ui, |ui| {
                    centered_block(ui, "empty", |ui| {
                        if let Some(err) = &self.scan_error {
                            ui.label(RichText::new(icon::WARNING).size(56.0).color(WARN));
                            ui.add_space(8.0);
                            ui.label(RichText::new("Couldn't read the folder").heading());
                            ui.label(RichText::new(err).color(MUTED));
                        } else {
                            ui.label(RichText::new(icon::SPARKLE).size(64.0).color(ACCENT));
                            ui.add_space(8.0);
                            ui.label(RichText::new("Your desktop is clean").heading());
                            ui.label(RichText::new("There's nothing left to review here.").color(MUTED));
                        }
                        ui.add_space(16.0);
                        if ui.add(secondary_button(icon::ARROW_CLOCKWISE, "Scan again", "F5")).clicked() {
                            actions.push(Action::Rescan);
                        }
                    });
                });
            return;
        }

        egui::Panel::bottom("actions")
            .exact_size(86.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(20, 18)))
            .show(ui, |ui| self.action_bar(ui, actions));

        egui::Panel::left("list")
            .resizable(true)
            .default_size(280.0)
            .size_range(210.0..=460.0)
            .frame(Frame::new().fill(SURFACE).inner_margin(Margin::symmetric(10, 14)))
            .show(ui, |ui| self.sidebar(ui, actions));

        egui::Panel::right("info")
            .resizable(false)
            .exact_size(330.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(BG).inner_margin(Margin {
                left: 8,
                right: 20,
                top: 20,
                bottom: 0,
            }))
            .show(ui, |ui| self.info_panel(ui, actions));

        egui::CentralPanel::no_frame()
            .frame(Frame::new().fill(BG).inner_margin(Margin {
                left: 20,
                right: 12,
                top: 20,
                bottom: 0,
            }))
            .show(ui, |ui| self.preview_card(ui, actions));
    }

    fn sidebar(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("ITEMS").size(11.0).color(FAINT).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(6.0);
                let keep = self.count(Some(Decision::Keep));
                let delete = self.count(Some(Decision::Delete));
                ui.label(RichText::new(format!("{} {delete}", icon::TRASH)).size(12.0).color(DELETE.gamma_multiply(0.85)));
                ui.label(RichText::new(format!("{} {keep}", icon::CHECK)).size(12.0).color(KEEP.gamma_multiply(0.85)));
            });
        });
        ui.add_space(6.0);

        egui::Panel::bottom("legend")
            .show_separator_line(false)
            .frame(Frame::new().inner_margin(Margin {
                left: 0,
                right: 0,
                top: 10,
                bottom: 0,
            }))
            .show(ui, |ui| {
                Frame::new()
                    .fill(BG.gamma_multiply(0.6))
                    .corner_radius(CornerRadius::same(RADIUS))
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 3.0;
                        for (keys, what) in [
                            ("→  K", "Keep"),
                            ("←  D  Del", "Delete"),
                            ("↑  ↓", "Previous / next"),
                            ("Enter  E", "Open / show in Explorer"),
                            ("Ctrl+Z", "Undo"),
                        ] {
                            ui.horizontal(|ui| {
                                ui.add_sized(
                                    vec2(74.0, 16.0),
                                    Label::new(RichText::new(keys).size(11.5).color(MUTED).monospace()),
                                );
                                ui.label(RichText::new(what).size(11.5).color(FAINT));
                            });
                        }
                    });
            });

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (idx, item) in self.items.iter().enumerate() {
                let response = self.sidebar_row(ui, idx, item);
                if response.clicked() {
                    actions.push(Action::Select(idx));
                }
            }
        });
    }

    fn sidebar_row(&self, ui: &mut Ui, idx: usize, item: &Item) -> Response {
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
        let selected = idx == self.current;
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 8, SURFACE_3);
            painter.rect_filled(
                Rect::from_min_size(pos2(rect.left(), rect.top() + 8.0), vec2(3.0, rect.height() - 16.0)),
                2,
                ACCENT,
            );
        } else if response.hovered() {
            painter.rect_filled(rect, 8, SURFACE_2);
        }

        let e = &item.entry;
        let deleted = item.decision == Some(Decision::Delete);
        let icon_color = if deleted { FAINT } else { kind_color(e.kind) };
        painter.text(
            pos2(rect.left() + 14.0, rect.center().y),
            Align2::LEFT_CENTER,
            e.kind.icon(),
            FontId::proportional(16.0),
            icon_color,
        );
        let (status, status_color) = match item.decision {
            Some(Decision::Keep) => (icon::CHECK, KEEP),
            Some(Decision::Delete) => (icon::TRASH, DELETE),
            None => ("", MUTED),
        };
        painter.text(
            pos2(rect.right() - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            status,
            FontId::proportional(14.0),
            status_color,
        );

        let mut text = RichText::new(&e.name).size(13.5).color(match item.decision {
            _ if selected => TEXT,
            Some(Decision::Delete) => FAINT,
            Some(Decision::Keep) => MUTED,
            None => TEXT.gamma_multiply(0.9),
        });
        if deleted {
            text = text.strikethrough();
        }
        let max_width = rect.width() - 40.0 - 30.0;
        let galley = WidgetText::from(text).into_galley(ui, Some(TextWrapMode::Truncate), max_width, TextStyle::Body);
        let pos = pos2(rect.left() + 38.0, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(pos, galley, TEXT);

        if selected && self.scroll_to_current {
            response.scroll_to_me(Some(Align::Center));
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn action_bar(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let rect = ui.available_rect_before_wrap();
        let decision = self.items.get(self.current).and_then(|i| i.decision);

        let (button_width, gap) = (210.0, 14.0);
        let center = Rect::from_center_size(rect.center(), vec2(button_width * 2.0 + gap, rect.height()));
        ui.scope_builder(
            UiBuilder::new().max_rect(center).layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = gap;
                let size = vec2(button_width, 48.0);
                let delete = decision_button(ui, icon::TRASH, "Delete", "D", DELETE, decision == Some(Decision::Delete), size);
                if delete.on_hover_text("Mark for deletion  (D / ← / Del)").clicked() {
                    actions.push(Action::Decide(Decision::Delete));
                }
                let keep = decision_button(ui, icon::CHECK, "Keep", "K", KEEP, decision == Some(Decision::Keep), size);
                if keep.on_hover_text("Keep it  (K / →)").clicked() {
                    actions.push(Action::Decide(Decision::Keep));
                }
            },
        );

        ui.scope_builder(
            UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)),
            |ui| {
                let undo = ui.add_enabled(
                    !self.history.is_empty(),
                    secondary_button(icon::ARROW_U_UP_LEFT, "Undo", "Ctrl+Z"),
                );
                if undo.clicked() {
                    actions.push(Action::Undo);
                }
            },
        );

        ui.scope_builder(
            UiBuilder::new().max_rect(rect).layout(Layout::right_to_left(Align::Center)),
            |ui| {
                if ui
                    .add(secondary_button(icon::FOLDER_OPEN, "Show", "E"))
                    .on_hover_text("Show in Explorer")
                    .clicked()
                {
                    actions.push(Action::Reveal);
                }
                if ui
                    .add(secondary_button(icon::ARROW_SQUARE_OUT, "Open", "Enter"))
                    .on_hover_text("Open with the default app")
                    .clicked()
                {
                    actions.push(Action::Open);
                }
            },
        );
    }

    fn info_panel(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let Some(item) = self.items.get(self.current) else {
            return;
        };
        let e = &item.entry;
        let shown = self.previews.get(&e.path);

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                icon_badge(ui, e.kind.icon(), kind_color(e.kind));
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.add_space(3.0);
                    ui.label(RichText::new(e.kind.label()).color(MUTED).size(12.5));
                    let detail = match (&e.size, e.is_dir) {
                        (Some(s), _) => format_size(*s),
                        (None, true) => "Calculating size…".to_owned(),
                        (None, false) => String::new(),
                    };
                    ui.label(RichText::new(detail).color(TEXT).strong());
                });
            });
            ui.add_space(12.0);
            ui.add(
                Label::new(RichText::new(&e.name).font(FontId::new(19.0, heading_family())).color(TEXT)).wrap(),
            );
            ui.add_space(6.0);

            if let Some(decision) = item.decision {
                let (color, glyph, text) = match decision {
                    Decision::Keep => (KEEP, icon::CHECK, "Marked to keep"),
                    Decision::Delete => (DELETE, icon::TRASH, "Marked for deletion"),
                };
                pill(ui, color, &format!("{glyph}  {text}"));
                ui.add_space(6.0);
            }

            let hints = self.hints(item, shown);
            if !hints.is_empty() {
                ui.add_space(4.0);
                for (color, glyph, text) in hints {
                    Frame::new()
                        .fill(color.gamma_multiply(0.10))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(10, 7))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(glyph).color(color));
                                ui.label(RichText::new(text).color(color.lerp_to_gamma(TEXT, 0.35)).size(13.0));
                            });
                        });
                    ui.add_space(2.0);
                }
            }

            ui.add_space(14.0);
            let separator_rect = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover()).0;
            ui.painter().rect_filled(separator_rect, 0, BORDER);
            ui.add_space(14.0);

            // Type-specific facts first: they matter most for the decision.
            match shown {
                Some(Shown::Image { width, height, .. }) => {
                    let mp = (*width as f64 * *height as f64) / 1_000_000.0;
                    field(ui, "Dimensions", &format!("{width} × {height} px"), Some(format!("{mp:.1} megapixels")));
                }
                Some(Shown::Data(Preview::Text { lines, truncated, .. })) => {
                    let more = if *truncated { "+" } else { "" };
                    let noun = if *lines == 1 && !truncated { "line" } else { "lines" };
                    field(ui, "Length", &format!("{lines}{more} {noun}"), None);
                }
                Some(Shown::Data(Preview::Folder(f))) => {
                    let mut contents = format!("{} files · {} folders", f.files, f.folders);
                    if f.incomplete {
                        contents.push_str(" (stopped counting)");
                    }
                    field(ui, "Contents", &contents, Some(format!("{} items at the top level", f.child_count)));
                    if let Some((t, name)) = &f.newest {
                        field(ui, "Latest change inside", &relative_time(*t), Some(name.clone()));
                    }
                }
                Some(Shown::Data(Preview::Shortcut { target, target_exists, .. })) => {
                    let (value, sub) = match target {
                        Some(t) => (
                            t.display().to_string(),
                            Some(if *target_exists { "Target exists".to_owned() } else { "Target is missing".to_owned() }),
                        ),
                        None => ("Special location (not a file path)".to_owned(), None),
                    };
                    field(ui, "Points to", &value, sub);
                }
                Some(Shown::Data(Preview::WebLink { url })) => field(ui, "URL", url, None),
                _ => {}
            }

            let time_field = |ui: &mut Ui, label: &str, time: Option<std::time::SystemTime>| {
                if let Some(t) = time {
                    field(ui, label, &relative_time(t), Some(format_date(t)));
                }
            };
            time_field(ui, "Modified", e.modified);
            time_field(ui, "Created", e.created);
            time_field(ui, "Last accessed", e.accessed);
            if let Some(parent) = e.path.parent() {
                field(ui, "Location", &parent.display().to_string(), None);
            }
            if e.read_only {
                field(ui, "Attributes", "Read-only", None);
            }

            if let Some(Shown::Data(Preview::Shortcut { target: Some(t), target_exists: true, .. })) = shown
                && ui.add(secondary_button(icon::FOLDER_OPEN, "Show target", "")).clicked() {
                    actions.push(Action::RevealPath(t.clone()));
                }
        });
    }

    fn preview_card(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let Some(item) = self.items.get(self.current) else {
            return;
        };
        let stroke = match item.decision {
            Some(Decision::Keep) => Stroke::new(1.5, KEEP.gamma_multiply(0.6)),
            Some(Decision::Delete) => Stroke::new(1.5, DELETE.gamma_multiply(0.6)),
            None => Stroke::new(1.0, BORDER),
        };
        Frame::new()
            .fill(SURFACE)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::same(18))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                let e = &item.entry;
                match self.previews.get(&e.path) {
                    None => centered_block(ui, "loading", |ui| {
                        ui.add(egui::Spinner::new().size(28.0).color(MUTED));
                        ui.add_space(8.0);
                        let text = if e.is_dir { "Looking inside…" } else { "Loading preview…" };
                        ui.label(RichText::new(text).color(MUTED));
                    }),
                    Some(Shown::Image { texture, width, .. }) => image_preview(ui, texture, *width, actions),
                    Some(Shown::Data(preview)) => match preview {
                        Preview::Text { text, truncated, .. } => text_preview(ui, text, *truncated),
                        Preview::Folder(summary) => folder_preview(ui, summary),
                        Preview::Shortcut {
                            target,
                            target_exists,
                            target_is_dir,
                            arguments,
                            description,
                        } => centered_block(ui, "shortcut", |ui| {
                            let (glyph, color) = if target.is_some() && !target_exists {
                                (icon::LINK_BREAK, DELETE)
                            } else {
                                (icon::LINK, kind_color(Kind::Shortcut))
                            };
                            big_icon(ui, glyph, color);
                            ui.label(RichText::new("Shortcut to").color(MUTED));
                            ui.add_space(2.0);
                            let target_text = target
                                .as_ref()
                                .map(|t| t.display().to_string())
                                .unwrap_or_else(|| "a special location".into());
                            ui.add(Label::new(RichText::new(target_text).size(16.0).color(TEXT)).wrap());
                            ui.add_space(10.0);
                            match (target.is_some(), *target_exists) {
                                (true, true) => pill(
                                    ui,
                                    KEEP,
                                    &format!("{}  {} exists", icon::CHECK, if *target_is_dir { "Folder" } else { "Target" }),
                                ),
                                (true, false) => pill(ui, DELETE, &format!("{}  Target is missing", icon::WARNING)),
                                _ => {}
                            }
                            if let Some(args) = arguments {
                                ui.add_space(8.0);
                                ui.label(RichText::new(format!("Arguments: {args}")).color(MUTED).size(12.5));
                            }
                            if let Some(desc) = description {
                                ui.add_space(4.0);
                                ui.label(RichText::new(desc).color(MUTED).size(12.5).italics());
                            }
                        }),
                        Preview::WebLink { url } => centered_block(ui, "weblink", |ui| {
                            big_icon(ui, icon::GLOBE, kind_color(Kind::WebLink));
                            ui.label(RichText::new("Web link").color(MUTED));
                            ui.add_space(2.0);
                            ui.add(Label::new(RichText::new(url).size(16.0).color(TEXT)).wrap());
                            ui.add_space(12.0);
                            if ui.add(secondary_button(icon::ARROW_SQUARE_OUT, "Open in browser", "Enter")).clicked() {
                                actions.push(Action::Open);
                            }
                        }),
                        Preview::Unavailable(reason) => centered_block(ui, "unavailable", |ui| {
                            big_icon(ui, e.kind.icon(), kind_color(e.kind));
                            if !e.extension.is_empty() {
                                pill(ui, MUTED, &format!(".{}", e.extension.to_uppercase()));
                                ui.add_space(6.0);
                            }
                            ui.label(RichText::new(reason).color(MUTED));
                            ui.add_space(12.0);
                            if ui.add(secondary_button(icon::ARROW_SQUARE_OUT, "Open", "Enter")).clicked() {
                                actions.push(Action::Open);
                            }
                        }),
                        Preview::Image { .. } => {}
                    },
                }
            });
    }

    fn summary_ui(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        self.top_bar(ui, actions, false);
        egui::CentralPanel::no_frame()
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(28)))
            .show(ui, |ui| {
                let avail = ui.available_rect_before_wrap();
                let width = avail.width().min(680.0);
                let column = Rect::from_min_size(pos2(avail.center().x - width / 2.0, avail.top()), vec2(width, avail.height()));
                ui.scope_builder(UiBuilder::new().max_rect(column), |ui| {
                    let marked: Vec<(usize, &Item)> = self
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(_, i)| i.decision == Some(Decision::Delete))
                        .collect();
                    let total: u64 = marked.iter().filter_map(|(_, i)| i.entry.size).sum();
                    let unknown = marked.iter().any(|(_, i)| i.entry.size.is_none());
                    let kept = self.count(Some(Decision::Keep));
                    let open = self.count(None);

                    ui.add_space(12.0);
                    if marked.is_empty() {
                        ui.label(RichText::new("Nothing marked for deletion").heading());
                        ui.add_space(4.0);
                        ui.label(RichText::new("Everything stays where it is.").color(MUTED));
                    } else {
                        ui.label(RichText::new("Ready to clean up").heading());
                        ui.add_space(4.0);
                        let plus = if unknown { "+" } else { "" };
                        let noun = if marked.len() == 1 { "item" } else { "items" };
                        ui.label(
                            RichText::new(format!(
                                "{} {noun} · {}{plus} will move to the Recycle Bin",
                                marked.len(),
                                format_size(total)
                            ))
                            .color(MUTED)
                            .size(15.0),
                        );
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        pill(ui, KEEP, &format!("{}  {kept} kept", icon::CHECK));
                        if open > 0 {
                            pill(ui, MUTED, &format!("{}  {open} not reviewed — will stay", icon::CIRCLE));
                        }
                    });
                    ui.add_space(18.0);

                    if !marked.is_empty() {
                        Frame::new()
                            .fill(SURFACE)
                            .stroke(Stroke::new(1.0, BORDER))
                            .corner_radius(CornerRadius::same(12))
                            .inner_margin(Margin::symmetric(8, 8))
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .max_height((ui.available_height() - 120.0).max(120.0))
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        for (idx, item) in &marked {
                                            let e = &item.entry;
                                            ui.horizontal(|ui| {
                                                ui.set_min_height(34.0);
                                                ui.add_space(6.0);
                                                ui.label(RichText::new(e.kind.icon()).size(16.0).color(kind_color(e.kind)));
                                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                                    let keep = egui::Button::new(
                                                        RichText::new(format!("{}  Keep", icon::ARROW_U_UP_LEFT)).size(12.5).color(MUTED),
                                                    )
                                                    .fill(Color32::TRANSPARENT)
                                                    .corner_radius(6);
                                                    if ui.add(keep).on_hover_text("Keep this one instead").clicked() {
                                                        actions.push(Action::SetDecision(*idx, Decision::Keep));
                                                    }
                                                    let size = e.size.map(format_size).unwrap_or_else(|| "—".into());
                                                    ui.label(RichText::new(size).color(MUTED).size(13.0));
                                                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                                        ui.add(Label::new(RichText::new(&e.name).color(TEXT)).truncate());
                                                    });
                                                });
                                            });
                                        }
                                    });
                            });
                        ui.add_space(18.0);
                    }

                    ui.horizontal(|ui| {
                        if ui.add(secondary_button(icon::ARROW_LEFT, "Back to review", "Esc")).clicked() {
                            actions.push(Action::BackToReview);
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if marked.is_empty() {
                                if ui.add(secondary_button(icon::X, "Quit", "")).clicked() {
                                    actions.push(Action::Quit);
                                }
                            } else {
                                let apply = egui::Button::new(
                                    RichText::new(format!("{}   Move {} to Recycle Bin", icon::TRASH, marked.len()))
                                        .color(BG)
                                        .strong(),
                                )
                                .fill(DELETE)
                                .corner_radius(10)
                                .min_size(vec2(0.0, 40.0))
                                .shortcut_text(RichText::new("Ctrl+Enter").color(BG.gamma_multiply(0.7)).size(12.0));
                                if ui.add(apply).clicked() {
                                    actions.push(Action::Apply);
                                }
                            }
                        });
                    });
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("Nothing is deleted permanently — you can restore items from the Recycle Bin.")
                            .color(FAINT)
                            .size(12.5),
                    );
                });
            });
    }

    fn applying_ui(&self, ui: &mut Ui) {
        egui::CentralPanel::no_frame().frame(Frame::new().fill(BG)).show(ui, |ui| {
            centered_block(ui, "applying", |ui| {
                ui.add(egui::Spinner::new().size(32.0).color(ACCENT));
                ui.add_space(10.0);
                ui.label(RichText::new("Moving items to the Recycle Bin…").color(MUTED));
            });
        });
    }

    fn report_ui(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let Screen::Report(report) = &self.screen else {
            return;
        };
        self.top_bar(ui, actions, false);
        egui::CentralPanel::no_frame().frame(Frame::new().fill(BG)).show(ui, |ui| {
            centered_block(ui, "report", |ui| {
                ui.set_max_width(560.0);
                let (glyph, color) = if report.failed.is_empty() {
                    (icon::CHECK_CIRCLE, KEEP)
                } else {
                    (icon::WARNING, WARN)
                };
                ui.label(RichText::new(glyph).size(64.0).color(color));
                ui.add_space(6.0);
                ui.label(RichText::new("All done").heading());
                let noun = if report.moved == 1 { "item" } else { "items" };
                ui.label(
                    RichText::new(format!(
                        "Moved {} {noun} to the Recycle Bin · {} freed",
                        report.moved,
                        format_size(report.freed)
                    ))
                    .color(MUTED)
                    .size(15.0),
                );
                if !report.failed.is_empty() {
                    ui.add_space(12.0);
                    ui.label(RichText::new(format!("{} couldn't be moved:", report.failed.len())).color(WARN));
                    for (name, err) in report.failed.iter().take(8) {
                        ui.label(RichText::new(format!("{name} — {err}")).color(MUTED).size(12.5));
                    }
                }
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    let open = self.count(None);
                    // Centre the button row under the text.
                    let width = if open > 0 { 440.0 } else { 260.0 };
                    ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                    if open > 0
                        && ui
                            .add(secondary_button(icon::ARROW_RIGHT, &format!("Continue ({open} left)"), ""))
                            .clicked()
                    {
                        actions.push(Action::BackToReview);
                    }
                    if ui.add(secondary_button(icon::ARROW_CLOCKWISE, "Scan again", "F5")).clicked() {
                        actions.push(Action::Rescan);
                    }
                    if ui.add(secondary_button(icon::X, "Quit", "")).clicked() {
                        actions.push(Action::Quit);
                    }
                });
            });
        });
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some((message, until)) = &self.toast else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if now > *until {
            self.toast = None;
            return;
        }
        egui::Area::new(egui::Id::new("toast"))
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -110.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(SURFACE_3)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| ui.label(RichText::new(message.as_str()).color(TEXT)));
            });
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((until - now).max(0.05)));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        let mut actions = self.keyboard(&ctx);
        match self.screen {
            Screen::Review => self.review_ui(ui, &mut actions),
            Screen::Summary => self.summary_ui(ui, &mut actions),
            Screen::Applying => self.applying_ui(ui),
            Screen::Report(_) => self.report_ui(ui, &mut actions),
        }
        self.toast_ui(&ctx);
        self.scroll_to_current = false;
        for action in actions {
            self.run(action, &ctx);
        }
        self.ensure_previews(&ctx);
    }
}

// -------------------------------------------------------------------- widgets

fn kind_color(kind: Kind) -> Color32 {
    match kind {
        Kind::Folder => Color32::from_rgb(0x7A, 0xA7, 0xFF),
        Kind::Image => Color32::from_rgb(0xF0, 0x7C, 0xC4),
        Kind::Video => Color32::from_rgb(0xA7, 0x8B, 0xFA),
        Kind::Audio => Color32::from_rgb(0xFB, 0x92, 0x3C),
        Kind::Document | Kind::Text => Color32::from_rgb(0x60, 0xA5, 0xFA),
        Kind::Spreadsheet => Color32::from_rgb(0x4A, 0xDE, 0x80),
        Kind::Presentation => Color32::from_rgb(0xF9, 0x73, 0x16),
        Kind::Pdf => Color32::from_rgb(0xF8, 0x71, 0x71),
        Kind::Archive => Color32::from_rgb(0xFA, 0xCC, 0x15),
        Kind::Code => Color32::from_rgb(0x2D, 0xD4, 0xBF),
        Kind::Program => Color32::from_rgb(0xC0, 0x84, 0xFC),
        Kind::Shortcut | Kind::WebLink => Color32::from_rgb(0x38, 0xBD, 0xF8),
        Kind::Other => MUTED,
    }
}

fn looks_like_copy(name: &str, is_dir: bool) -> bool {
    let stem = if is_dir {
        name.to_lowercase()
    } else {
        Path::new(name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    if stem.ends_with(" - copy")
        || stem.contains(" - copy (")
        || stem.ends_with(" - kopie")
        || stem.starts_with("copy of ")
    {
        return true;
    }
    stem.rfind(" (")
        .and_then(|open| stem[open + 2..].strip_suffix(')'))
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Vertically and horizontally centres a block whose height is learned from the previous frame.
fn centered_block(ui: &mut Ui, salt: &str, add: impl FnOnce(&mut Ui)) {
    let id = ui.id().with(salt);
    let available = ui.available_height();
    let last: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
    ui.add_space(((available - last) / 2.0).max(0.0));
    let height = ui.vertical_centered(add).response.rect.height();
    if (height - last).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(id, height));
        ui.ctx().request_repaint();
    }
}

fn big_icon(ui: &mut Ui, glyph: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(112.0, 112.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 52.0, color.gamma_multiply(0.10));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(52.0), color);
    ui.add_space(10.0);
}

fn icon_badge(ui: &mut Ui, glyph: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(42.0, 42.0), Sense::hover());
    ui.painter().rect_filled(rect, 10, color.gamma_multiply(0.14));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(21.0), color);
}

fn pill(ui: &mut Ui, color: Color32, text: &str) {
    let galley = WidgetText::from(RichText::new(text).color(color).size(12.5)).into_galley(
        ui,
        Some(TextWrapMode::Extend),
        f32::INFINITY,
        TextStyle::Body,
    );
    let padding = vec2(10.0, 4.0);
    let (rect, _) = ui.allocate_exact_size(galley.size() + padding * 2.0, Sense::hover());
    ui.painter().rect_filled(rect, 255, color.gamma_multiply(0.12));
    ui.painter().galley(rect.min + padding, galley, color);
}

fn field(ui: &mut Ui, label: &str, value: &str, sub: Option<String>) {
    ui.label(RichText::new(label.to_uppercase()).size(10.5).color(FAINT).strong());
    ui.add_space(1.0);
    ui.add(Label::new(RichText::new(value).color(TEXT)).wrap());
    if let Some(sub) = sub {
        ui.add(Label::new(RichText::new(sub).size(12.0).color(MUTED)).wrap());
    }
    ui.add_space(12.0);
}

fn secondary_button<'a>(glyph: &str, label: &str, shortcut: &str) -> egui::Button<'a> {
    let button = egui::Button::new(RichText::new(format!("{glyph}  {label}")).color(TEXT.gamma_multiply(0.85)))
        .fill(SURFACE_2)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(10)
        .min_size(vec2(0.0, 38.0));
    if shortcut.is_empty() {
        button
    } else {
        button.shortcut_text(RichText::new(shortcut.to_owned()).size(11.5).color(FAINT))
    }
}

fn decision_button(
    ui: &mut Ui,
    glyph: &str,
    label: &str,
    key: &str,
    color: Color32,
    active: bool,
    size: egui::Vec2,
) -> Response {
    let text_color = if active { BG } else { color };
    let button = egui::Button::new(RichText::new(format!("{glyph}   {label}")).size(16.0).color(text_color).strong())
        .fill(if active { color } else { color.gamma_multiply(0.10) })
        .stroke(Stroke::new(1.0, color.gamma_multiply(if active { 1.0 } else { 0.35 })))
        .corner_radius(12)
        .min_size(size)
        .shortcut_text(RichText::new(key.to_owned()).size(12.0).color(text_color.gamma_multiply(0.6)));
    let response = ui.add(button);
    if response.hovered() && !active {
        ui.painter()
            .rect_stroke(response.rect, 12, Stroke::new(1.5, color.gamma_multiply(0.8)), StrokeKind::Inside);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn image_preview(ui: &mut Ui, texture: &TextureHandle, original_width: u32, actions: &mut Vec<Action>) {
    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, Sense::click());
    let tex = texture.size_vec2();
    let fit = (rect.width() / tex.x).min(rect.height() / tex.y);
    // Never blow tiny images up beyond 3×, and don't upscale past the original resolution otherwise.
    let max_scale = if tex.max_elem() < 256.0 { 3.0 } else { (original_width as f32 / tex.x).max(1.0) };
    let size = tex * fit.min(max_scale);
    let image_rect = Rect::from_center_size(rect.center(), size);

    // Checkerboard so transparent images stay readable.
    let painter = ui.painter_at(image_rect);
    let cell = 12.0;
    let (cols, rows) = ((size.x / cell).ceil() as i32, (size.y / cell).ceil() as i32);
    if cols * rows < 40_000 {
        painter.rect_filled(image_rect, 0, SURFACE_2);
        for y in 0..rows {
            for x in (y % 2..cols).step_by(2) {
                let min = image_rect.min + vec2(x as f32 * cell, y as f32 * cell);
                painter.rect_filled(Rect::from_min_size(min, vec2(cell, cell)), 0, SURFACE_3);
            }
        }
    }
    egui::Image::from_texture(SizedTexture::from_handle(texture)).paint_at(ui, image_rect);

    if response.on_hover_text("Double-click to open").double_clicked() {
        actions.push(Action::Open);
    }
}

fn text_preview(ui: &mut Ui, text: &str, truncated: bool) {
    if truncated {
        ui.label(RichText::new("Showing the beginning of the file").size(12.0).color(FAINT));
        ui.add_space(4.0);
    }
    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
        ui.add(
            Label::new(RichText::new(text).monospace().color(Color32::from_rgb(0xC9, 0xCD, 0xD6)))
                .wrap_mode(TextWrapMode::Extend),
        );
    });
}

fn folder_preview(ui: &mut Ui, f: &FolderSummary) {
    // Stat tiles
    let gap = 10.0;
    let tile_width = (ui.available_width() - gap * 3.0) / 4.0;
    let newest = f.newest.as_ref().map(|(t, _)| relative_time(*t)).unwrap_or_else(|| "—".into());
    let tiles = [
        ("Total size", format_size(f.total_size)),
        ("Files", f.files.to_string()),
        ("Folders", f.folders.to_string()),
        ("Last change", newest),
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (label, value) in tiles {
            Frame::new()
                .fill(SURFACE_2)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(tile_width - 28.0);
                        ui.label(RichText::new(label).size(11.5).color(MUTED));
                        ui.add(Label::new(RichText::new(value).font(FontId::new(19.0, heading_family())).color(TEXT)).truncate());
                    });
                });
        }
    });
    ui.add_space(14.0);

    // Breakdown by kind
    if f.total_size > 0 {
        let palette_total = f.total_size as f32;
        let bar = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover()).0;
        ui.painter().rect_filled(bar, 4, SURFACE_3);
        let mut x = bar.left();
        let painter = ui.painter_at(bar);
        for (kind, _, bytes) in f.kinds.iter().take(6) {
            let w = bar.width() * (*bytes as f32 / palette_total);
            if w < 1.0 {
                continue;
            }
            painter.rect_filled(Rect::from_min_size(pos2(x, bar.top()), vec2(w, bar.height())), 0, kind_color(*kind));
            x += w;
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            for (kind, count, bytes) in f.kinds.iter().take(6) {
                let pct = *bytes as f32 / palette_total * 100.0;
                ui.label(RichText::new("●").color(kind_color(*kind)).size(11.0));
                ui.add_space(-12.0);
                ui.label(
                    RichText::new(format!("{} {count} · {} · {pct:.0}%", kind.label(), format_size(*bytes)))
                        .size(12.0)
                        .color(MUTED),
                );
            }
        });
        ui.add_space(14.0);
    }

    // Top-level contents
    ui.horizontal(|ui| {
        ui.label(RichText::new("CONTENTS").size(11.0).color(FAINT).strong());
        if f.child_count > f.children.len() {
            ui.label(RichText::new(format!("showing {} of {}", f.children.len(), f.child_count)).size(11.0).color(FAINT));
        }
    });
    ui.add_space(4.0);
    if f.children.is_empty() {
        ui.label(RichText::new("This folder is empty.").color(MUTED));
        return;
    }
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, child) in f.children.iter().enumerate() {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
            if i % 2 == 1 {
                ui.painter().rect_filled(rect, 6, SURFACE_2.gamma_multiply(0.5));
            }
            ui.painter().text(
                pos2(rect.left() + 10.0, rect.center().y),
                Align2::LEFT_CENTER,
                child.kind.icon(),
                FontId::proportional(15.0),
                kind_color(child.kind),
            );
            let size_text = child.size.map(format_size).unwrap_or_default();
            ui.painter().text(
                pos2(rect.right() - 10.0, rect.center().y),
                Align2::RIGHT_CENTER,
                &size_text,
                FontId::proportional(12.5),
                MUTED,
            );
            let galley = WidgetText::from(RichText::new(&child.name).size(13.5).color(TEXT))
                .into_galley(ui, Some(TextWrapMode::Truncate), rect.width() - 130.0, TextStyle::Body);
            ui.painter()
                .galley(pos2(rect.left() + 34.0, rect.center().y - galley.size().y / 2.0), galley, TEXT);
        }
    });
}
