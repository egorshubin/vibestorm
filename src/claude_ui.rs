//! The Claude Code tool window, styled after the Claude Code VS Code extension:
//! conversation header with history, a timeline of messages and tool calls with
//! inline diffs, and a rounded input box holding mode / model / attach / send.

use crate::claude::{self, Chat, Item, MODELS, MODES, PermState};
use crate::theme::{Palette, hex, tool_btn};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Frame, Id, Key, Layout, Margin, Modifiers, RichText, Sense, Stroke, Ui, pos2,
    vec2,
};
use serde_json::Value;
use std::path::PathBuf;

/// Claude brand orange
pub const CLAUDE: Color32 = hex(0xD97757);

pub enum ChatAction {
    OpenFile(PathBuf),
    /// new default permission mode for every project (saved in settings)
    SetMode(String),
    /// new default model for every project (saved in settings)
    SetModel(String),
    /// start a fresh conversation in its own tab
    NewSession,
    /// show a stored conversation (in its tab if one is open, else in a new one)
    OpenSession(String),
}

enum Act {
    Allow(usize, bool, Option<&'static str>),
    Deny(usize),
    Pick(usize, String, String, bool),
    Submit(usize, bool),
    Open(PathBuf),
}

fn spark(t: f64) -> &'static str {
    const F: [&str; 6] = ["·", "✢", "✳", "✶", "✻", "✽"];
    F[((t * 7.0) as usize) % F.len()]
}

fn mode_label(mode: &str) -> String {
    let icon = match mode {
        "acceptEdits" => "⏵⏵",
        "plan" => "⏸",
        "bypassPermissions" => "⚠",
        "auto" => "⏵",
        _ => "✋",
    };
    let name = MODES.iter().find(|m| m.0 == mode).map(|m| m.1).unwrap_or(mode);
    format!("{icon} {name}")
}

/// A small rounded "chip" button used in the input footer.
fn chip(ui: &mut Ui, pal: &Palette, text: &str, tip: &str) -> egui::Response {
    let font = FontId::proportional(12.0);
    let g = ui.painter().layout_no_wrap(text.to_owned(), font, pal.text_weak);
    let size = vec2(g.size().x + 14.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let col = if resp.hovered() { pal.text } else { pal.text_weak };
    if resp.hovered() {
        ui.painter().rect_filled(rect, 11.0, pal.hover);
    }
    ui.painter().galley_with_override_text_color(rect.center() - g.size() / 2.0, g, col);
    resp.on_hover_text(tip)
}

fn code_block(ui: &mut Ui, pal: &Palette, label: &str, text: &str, max_lines: usize) {
    let mut s: String = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    let total = text.lines().count();
    if total > max_lines {
        s.push_str(&format!("\n… {} more lines", total - max_lines));
    }
    ui.horizontal_top(|ui| {
        ui.add_sized(vec2(30.0, 16.0), egui::Label::new(RichText::new(label).small().color(pal.text_weak)));
        Frame::NONE
            .fill(ui.visuals().code_bg_color)
            .corner_radius(4)
            .inner_margin(Margin::symmetric(6, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(RichText::new(s).monospace().size(11.5).color(pal.text)).wrap().selectable(true));
            });
    });
}

/// Claude's markdown. egui_commonmark lays out every styled run as its own label and
/// the wrapping row centres them, which lifts monospace `code` above the text line;
/// bottom alignment puts Segoe UI and the monospace font on the same baseline.
fn markdown(ui: &mut Ui, cache: &mut egui_commonmark::CommonMarkCache, text: &str) {
    ui.scope(|ui| {
        ui.style_mut().override_text_valign = Some(Align::BOTTOM);
        egui_commonmark::CommonMarkViewer::new().show(ui, cache, text);
    });
}

/// Line diff by trimming the common head/tail (good enough for Edit tool hunks).
fn diff_lines<'a>(old: &'a str, new: &'a str) -> Vec<(char, &'a str)> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] {
        suf += 1;
    }
    let mut out = vec![];
    for l in &a[pre.saturating_sub(2)..pre] {
        out.push((' ', *l));
    }
    for l in &a[pre..a.len() - suf] {
        out.push(('-', *l));
    }
    for l in &b[pre..b.len() - suf] {
        out.push(('+', *l));
    }
    for l in a[a.len() - suf..].iter().take(2) {
        out.push((' ', *l));
    }
    out
}

fn diff_view(ui: &mut Ui, pal: &Palette, file: &str, hunks: &[(&str, &str)]) {
    Frame::NONE
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(6)
        .inner_margin(Margin::same(0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            Frame::NONE.inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
                ui.label(RichText::new(file).small().monospace().color(pal.text_weak));
            });
            let font = FontId::monospace(11.5);
            let row_h = 16.0;
            let mut shown = 0;
            for (i, (old, new)) in hunks.iter().enumerate() {
                if i > 0 {
                    ui.add_space(3.0);
                }
                for (k, line) in diff_lines(old, new) {
                    if shown >= 60 {
                        break;
                    }
                    shown += 1;
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                    let (bg, sign_col) = match k {
                        '-' => (Some(pal.diff_del_bg), pal.deleted),
                        '+' => (Some(pal.diff_add_bg), pal.added),
                        _ => (None, pal.text_weak),
                    };
                    if let Some(bg) = bg {
                        p.rect_filled(rect, 0.0, bg);
                    }
                    p.text(pos2(rect.min.x + 6.0, rect.center().y), Align2::LEFT_CENTER, k.to_string(), font.clone(), sign_col);
                    p.text(pos2(rect.min.x + 20.0, rect.center().y), Align2::LEFT_CENTER, line, font.clone(), pal.text);
                }
            }
            if shown >= 60 {
                ui.label(RichText::new("   …").small().color(pal.text_weak));
            }
            ui.add_space(3.0);
        });
}

/// Expanded body of a tool call.
fn tool_body(ui: &mut Ui, pal: &Palette, name: &str, input: &Value, result: &Option<(String, bool)>, cwd: &std::path::Path) {
    let file = || {
        let p = input["file_path"].as_str().unwrap_or("");
        PathBuf::from(p).strip_prefix(cwd).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or(p.to_owned())
    };
    match name {
        "Edit" => diff_view(
            ui,
            pal,
            &file(),
            &[(input["old_string"].as_str().unwrap_or(""), input["new_string"].as_str().unwrap_or(""))],
        ),
        "MultiEdit" => {
            let hunks: Vec<(&str, &str)> = input["edits"]
                .as_array()
                .map(|a| a.iter().map(|e| (e["old_string"].as_str().unwrap_or(""), e["new_string"].as_str().unwrap_or(""))).collect())
                .unwrap_or_default();
            diff_view(ui, pal, &file(), &hunks);
        }
        "Write" => diff_view(ui, pal, &file(), &[("", input["content"].as_str().unwrap_or(""))]),
        "Bash" | "PowerShell" => code_block(ui, pal, "IN", input["command"].as_str().unwrap_or(""), 30),
        "Read" | "Glob" | "Grep" | "TodoWrite" => {}
        _ => code_block(ui, pal, "IN", &serde_json::to_string_pretty(input).unwrap_or_default(), 30),
    }
    if let Some((r, is_err)) = result {
        if !matches!(name, "Edit" | "MultiEdit" | "Write" | "TodoWrite") || *is_err {
            code_block(ui, pal, if *is_err { "ERR" } else { "OUT" }, r, if name == "Read" { 12 } else { 30 });
        }
    }
}

pub fn show(
    ui: &mut Ui,
    chat: &mut Chat,
    pal: &Palette,
    last_session: Option<String>,
    active_file: Option<String>,
) -> Vec<ChatAction> {
    let mut out = vec![];
    let mut acts: Vec<Act> = vec![];
    let now = ui.input(|i| i.time);
    let cwd = chat.cwd.clone();
    let uid = chat.uid;

    // ---------------- header: conversation title + history + new
    egui::Panel::top("claude_header")
        .frame(Frame::NONE.inner_margin(Margin::symmetric(8, 5)))
        .show_separator_line(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("✻").color(CLAUDE).size(16.0));
                let title = chat.title().unwrap_or_else(|| "New conversation".into());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 1.0;
                    if tool_btn(ui, pal, "✚", "New session", true) {
                        out.push(ChatAction::NewSession);
                    }
                    let hist = ui.menu_button(RichText::new("🕘").size(13.0), |ui| {
                        ui.set_min_width(320.0);
                        let stale = chat.history_at.is_none_or(|t| t.elapsed().as_secs() > 5);
                        if stale {
                            chat.history = claude::list_sessions(&cwd);
                            chat.history_at = Some(std::time::Instant::now());
                        }
                        ui.label(RichText::new("Past conversations").small().strong().color(pal.text_weak));
                        if chat.history.is_empty() {
                            ui.label(RichText::new("No saved conversations for this folder").color(pal.text_weak));
                        }
                        let mut pick = None;
                        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                            for s in &chat.history {
                                let current = chat.holds_session(&s.id);
                                let label = format!("{}\n{}", s.title, crate::util::fmt_ago(s.modified));
                                if ui.add(egui::Button::selectable(current, label).wrap()).clicked() {
                                    pick = Some(s.id.clone());
                                    ui.close();
                                }
                            }
                        });
                        ui.separator();
                        if ui
                            .add_enabled(last_session.is_some(), egui::Button::new("⟲  Resume the last conversation"))
                            .clicked()
                        {
                            pick = last_session.clone();
                            ui.close();
                        }
                        if let Some(id) = pick {
                            out.push(ChatAction::OpenSession(id));
                        }
                    });
                    hist.response.on_hover_text("Past conversations");
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(title).strong()).truncate());
                    });
                });
            });
            if !chat.todos.is_empty() && chat.todos.iter().any(|t| t.status != "completed") {
                ui.add_space(2.0);
                for t in &chat.todos {
                    let (icon, col) = match t.status.as_str() {
                        "completed" => ("☑", pal.success),
                        "in_progress" => ("◐", CLAUDE),
                        _ => ("☐", pal.text_weak),
                    };
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.label(RichText::new(icon).color(col));
                        let mut rt = RichText::new(&t.content).small();
                        if t.status == "completed" {
                            rt = rt.strikethrough().color(pal.text_weak);
                        } else if t.status == "in_progress" {
                            rt = rt.color(pal.text);
                        }
                        ui.add(egui::Label::new(rt).wrap());
                    });
                }
            }
        });

    // ---------------- input box
    egui::Panel::bottom("claude_input")
        .frame(Frame::NONE.inner_margin(Margin { left: 8, right: 8, top: 4, bottom: 8 }))
        .show_separator_line(false)
        .show(ui, |ui| {
            let input_id = chat.input_id();
            if std::mem::take(&mut chat.focus_input) {
                ui.memory_mut(|m| m.request_focus(input_id));
            }
            let focused = ui.memory(|m| m.has_focus(input_id));
            // Enter sends; Shift+Enter falls through to the text box as a new line
            let mut send = focused && !ui.input(|i| i.modifiers.shift) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
            if focused && chat.busy && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                chat.interrupt();
            }
            if focused {
                let pasted: Vec<std::sync::Arc<egui::ColorImage>> = ui.input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::PasteImage(img) => Some(img.clone()),
                            _ => None,
                        })
                        .collect()
                });
                for img in pasted {
                    let png = claude::color_image_to_png(&img);
                    match claude::prepare_image(ui.ctx(), "pasted.png", png) {
                        Ok(a) => chat.attachments.push(a),
                        Err(e) => chat.items.push(Item::Error(e)),
                    }
                }
            }
            if chat.busy {
                let secs = chat.busy_since.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(spark(now)).color(CLAUDE).size(14.0));
                    ui.label(RichText::new("Working…").color(CLAUDE));
                    ui.label(
                        RichText::new(format!("{}  ·  esc to interrupt", crate::util::fmt_duration(secs)))
                            .small()
                            .color(pal.text_weak),
                    );
                });
                ui.add_space(2.0);
            }
            Frame::NONE
                .fill(pal.input_bg)
                .stroke(Stroke::new(1.0, if focused { CLAUDE.gamma_multiply(0.8) } else { pal.border }))
                .corner_radius(10)
                .inner_margin(Margin { left: 10, right: 8, top: 8, bottom: 6 })
                .show(ui, |ui| {
                    if !chat.attachments.is_empty() {
                        let mut remove = None;
                        ui.horizontal_wrapped(|ui| {
                            for (i, a) in chat.attachments.iter().enumerate() {
                                let r = ui.add(egui::Image::from_texture((a.thumb.id(), vec2(48.0, 48.0))).corner_radius(6).sense(Sense::click()));
                                if r.on_hover_text(format!("{}  ·  click to remove", a.name)).clicked() {
                                    remove = Some(i);
                                }
                            }
                        });
                        if let Some(i) = remove {
                            chat.attachments.remove(i);
                        }
                        ui.add_space(4.0);
                    }
                    egui::ScrollArea::vertical().id_salt(("claude_input_scroll", uid)).max_height(220.0).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut chat.input)
                                .id(input_id)
                                .frame(Frame::NONE)
                                .hint_text("Ask Claude to edit, explain or fix…")
                                .desired_rows(2)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        // permission mode
                        let r = chip(ui, pal, &mode_label(&chat.mode), "Permission mode");
                        egui::Popup::menu(&r).show(|ui| {
                            for (id, label, tip) in MODES {
                                let cur = chat.mode == *id;
                                if ui.add(egui::Button::selectable(cur, mode_label(id))).on_hover_text(format!("{label}: {tip}")).clicked() {
                                    chat.set_mode(id);
                                    out.push(ChatAction::SetMode(id.to_string()));
                                }
                            }
                        });
                        // model
                        let mname = MODELS.iter().find(|m| m.0 == chat.model).map(|m| m.1).unwrap_or(&chat.model).to_owned();
                        let r = chip(ui, pal, &format!("{mname} ▾"), "Model");
                        egui::Popup::menu(&r).show(|ui| {
                            for (id, label) in MODELS {
                                if ui.add(egui::Button::selectable(chat.model == *id, *label)).clicked() {
                                    chat.set_model(id);
                                    out.push(ChatAction::SetModel(id.to_string()));
                                }
                            }
                        });
                        if let Some(f) = &active_file {
                            let name = f.rsplit('/').next().unwrap_or(f);
                            if chip(ui, pal, &format!("@ {name}"), &format!("Mention {f} in the message")).clicked() {
                                if !chat.input.is_empty() && !chat.input.ends_with(' ') {
                                    chat.input.push(' ');
                                }
                                chat.input.push_str(&format!("@{f} "));
                                chat.focus_input = true;
                            }
                        }
                        if chip(ui, pal, "📎", "Attach images (or paste with Ctrl+V)").clicked() {
                            crate::util::tall_file_dialog();
                            if let Some(files) = rfd::FileDialog::new()
                                .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
                                .pick_files()
                            {
                                for f in files {
                                    let name = f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                                    match std::fs::read(&f).map_err(|e| e.to_string()).and_then(|b| claude::prepare_image(ui.ctx(), &name, b)) {
                                        Ok(a) => chat.attachments.push(a),
                                        Err(e) => chat.items.push(Item::Error(e)),
                                    }
                                }
                            }
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            // round send / stop button
                            let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                            let can = !chat.input.trim().is_empty() || !chat.attachments.is_empty();
                            let p = ui.painter();
                            if chat.busy && !can {
                                p.circle_filled(rect.center(), 13.0, if resp.hovered() { pal.error } else { pal.button_bg });
                                p.rect_filled(egui::Rect::from_center_size(rect.center(), vec2(9.0, 9.0)), 1.5, pal.text);
                                if resp.on_hover_text("Stop (Esc)").clicked() {
                                    chat.interrupt();
                                }
                            } else {
                                let fill = if can { if resp.hovered() { CLAUDE.gamma_multiply(1.15) } else { CLAUDE } } else { pal.button_bg };
                                p.circle_filled(rect.center(), 13.0, fill);
                                p.text(rect.center(), Align2::CENTER_CENTER, "↑", FontId::proportional(15.0), Color32::WHITE);
                                if can && resp.on_hover_text("Send (Enter) · Shift+Enter for a new line").clicked() {
                                    send = true;
                                }
                            }
                            if let Some(s) = &chat.stats {
                                ui.add_space(6.0);
                                ui.label(RichText::new(s).small().color(pal.text_weak));
                            }
                        });
                    });
                });
            if send {
                chat.send();
                ui.memory_mut(|m| m.request_focus(input_id));
            }
        });

    // ---------------- conversation
    egui::CentralPanel::default()
        .frame(Frame::NONE.inner_margin(Margin::symmetric(10, 6)))
        .show(ui, |ui| {
            let Chat { items, md_cache, live_text, busy, .. } = chat;
            egui::ScrollArea::vertical()
                .id_salt(("claude_msgs", uid))
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if items.is_empty() && !*busy {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("✻").size(36.0).color(CLAUDE));
                            ui.add_space(4.0);
                            ui.label(RichText::new("What should we build?").size(16.0).strong());
                            ui.add_space(2.0);
                            ui.label(RichText::new(cwd.display().to_string()).small().color(pal.text_weak));
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new(
                                    "Each project has its own Claude agent running in parallel.\nYou get a notification when a task is done or needs you.\n🕘 opens past conversations.",
                                )
                                .small()
                                .color(pal.text_weak),
                            );
                        });
                    }
                    for (idx, item) in items.iter_mut().enumerate() {
                        match item {
                            Item::User { text, thumbs } => {
                                ui.add_space(8.0);
                                Frame::NONE
                                    .fill(pal.user_bubble)
                                    .stroke(Stroke::new(1.0, pal.border))
                                    .corner_radius(8)
                                    .inner_margin(Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        if !thumbs.is_empty() {
                                            ui.horizontal_wrapped(|ui| {
                                                for t in thumbs.iter() {
                                                    ui.add(egui::Image::from_texture((t.id(), vec2(72.0, 72.0))).corner_radius(4));
                                                }
                                            });
                                        }
                                        if !text.is_empty() {
                                            ui.add(egui::Label::new(RichText::new(text.as_str())).wrap().selectable(true));
                                        }
                                    });
                                ui.add_space(6.0);
                            }
                            Item::Assistant(text) => {
                                ui.horizontal_top(|ui| {
                                    ui.label(RichText::new("●").size(10.0).color(pal.text));
                                    ui.vertical(|ui| {
                                        markdown(ui, md_cache, text);
                                    });
                                });
                                ui.add_space(4.0);
                            }
                            Item::Thinking(text) => {
                                egui::CollapsingHeader::new(RichText::new("✻ Thinking").small().italics().color(pal.text_weak))
                                    .id_salt(("think", idx))
                                    .show(ui, |ui| {
                                        ui.add(egui::Label::new(RichText::new(text.as_str()).small().italics().color(pal.text_weak)).wrap());
                                    });
                            }
                            Item::Tool { name, input, result, sub, .. } => {
                                let col = match result {
                                    None => CLAUDE.gamma_multiply(0.55 + 0.45 * ((now * 4.0).sin() as f32 * 0.5 + 0.5)),
                                    Some((_, true)) => pal.error,
                                    Some((_, false)) => pal.success,
                                };
                                let summary = claude::tool_summary(name, input, &cwd);
                                let open_id = Id::new(("tool_open", uid, idx));
                                // edits start expanded (inline diff), like in VS Code
                                let default_open = matches!(name.as_str(), "Edit" | "MultiEdit" | "Write");
                                let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(default_open);
                                let indent = if *sub { 18.0 } else { 0.0 };
                                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
                                let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                                if resp.hovered() {
                                    p.rect_filled(rect, 4.0, pal.hover);
                                }
                                let x = rect.min.x + 4.0 + indent;
                                p.circle_filled(pos2(x + 4.0, rect.center().y), 3.5, col);
                                let ng = p.layout_no_wrap(name.clone(), FontId::proportional(13.0), pal.text);
                                let nw = ng.size().x;
                                p.galley(pos2(x + 14.0, rect.center().y - ng.size().y / 2.0), ng, pal.text);
                                p.text(
                                    pos2(x + 22.0 + nw, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    &summary,
                                    FontId::monospace(11.5),
                                    pal.text_weak,
                                );
                                let file = claude::tool_file_path(name, input);
                                let hover = if file.is_some() { "Click: details · double-click: open file" } else { "Click: details" };
                                let resp = resp.on_hover_text(hover);
                                if resp.clicked() {
                                    open = !open;
                                    ui.data_mut(|d| d.insert_temp(open_id, open));
                                }
                                if resp.double_clicked() {
                                    if let Some(f) = file {
                                        acts.push(Act::Open(f));
                                    }
                                }
                                if open {
                                    ui.horizontal(|ui| {
                                        ui.add_space(14.0 + indent);
                                        ui.vertical(|ui| tool_body(ui, pal, name, input, result, &cwd));
                                    });
                                    ui.add_space(4.0);
                                }
                            }
                            Item::Permission { tool, input, state, .. } => {
                                let pending = *state == PermState::Pending;
                                ui.add_space(4.0);
                                Frame::NONE
                                    .fill(pal.card)
                                    .stroke(Stroke::new(1.0, if pending { CLAUDE } else { pal.border }))
                                    .corner_radius(8)
                                    .inner_margin(Margin::same(10))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        let plan = tool == "ExitPlanMode";
                                        let title = if plan {
                                            "Here is Claude's plan. Ready to code?".to_owned()
                                        } else {
                                            match tool.as_str() {
                                                "Edit" | "MultiEdit" | "Write" => format!("Make this edit to {}?", claude::tool_summary(tool, input, &cwd)),
                                                "Bash" | "PowerShell" => "Run this command?".to_owned(),
                                                _ => format!("Allow {tool}?"),
                                            }
                                        };
                                        ui.label(RichText::new(title).strong());
                                        ui.add_space(4.0);
                                        if plan {
                                            markdown(ui, md_cache, input["plan"].as_str().unwrap_or(""));
                                        } else {
                                            tool_body(ui, pal, tool, input, &None, &cwd);
                                        }
                                        ui.add_space(4.0);
                                        match state {
                                            PermState::Pending => {
                                                let opts: Vec<(&str, Act)> = if plan {
                                                    vec![
                                                        ("Yes, and auto-accept edits", Act::Allow(idx, false, Some("acceptEdits"))),
                                                        ("Yes, and manually approve edits", Act::Allow(idx, false, Some("manual"))),
                                                        ("No, keep planning", Act::Deny(idx)),
                                                    ]
                                                } else {
                                                    vec![
                                                        ("Yes", Act::Allow(idx, false, None)),
                                                        ("Yes, and don't ask again this session", Act::Allow(idx, true, None)),
                                                        ("No, tell Claude what to do differently", Act::Deny(idx)),
                                                    ]
                                                };
                                                for (n, (label, act)) in opts.into_iter().enumerate() {
                                                    let txt = RichText::new(format!("{}  {label}", n + 1));
                                                    let b = egui::Button::new(if n == 0 { txt.color(Color32::WHITE) } else { txt })
                                                        .fill(if n == 0 { CLAUDE } else { pal.button_bg })
                                                        .min_size(vec2(ui.available_width(), 26.0))
                                                        .corner_radius(6);
                                                    if ui.add(b).clicked() {
                                                        acts.push(act);
                                                    }
                                                }
                                            }
                                            PermState::Allowed => {
                                                ui.label(RichText::new("✔ Allowed").small().color(pal.success));
                                            }
                                            PermState::Denied => {
                                                ui.label(RichText::new("✖ Denied").small().color(pal.error));
                                            }
                                        }
                                    });
                                ui.add_space(4.0);
                            }
                            Item::Question { input, answers, state, .. } => {
                                let pending = *state == PermState::Pending;
                                Frame::NONE
                                    .fill(pal.card)
                                    .stroke(Stroke::new(1.0, if pending { CLAUDE } else { pal.border }))
                                    .corner_radius(8)
                                    .inner_margin(Margin::same(10))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        if let Some(qs) = input["questions"].as_array() {
                                            for q in qs {
                                                let qt = q["question"].as_str().unwrap_or("").to_owned();
                                                let multi = q["multiSelect"].as_bool().unwrap_or(false);
                                                ui.add(egui::Label::new(RichText::new(&qt).strong()).wrap());
                                                if let Some(opts) = q["options"].as_array() {
                                                    for o in opts {
                                                        let label = o["label"].as_str().unwrap_or("").to_owned();
                                                        let chosen = answers.get(&qt).map(|a| a.split(", ").any(|x| x == label)).unwrap_or(false);
                                                        let desc = o["description"].as_str().unwrap_or("");
                                                        let r = ui.add_enabled(
                                                            pending,
                                                            egui::Button::selectable(chosen, format!("{}  {label}", if chosen { "●" } else { "○" }))
                                                                .min_size(vec2(ui.available_width(), 24.0)),
                                                        );
                                                        if r.on_hover_text(desc).clicked() {
                                                            acts.push(Act::Pick(idx, qt.clone(), label.clone(), multi));
                                                        }
                                                    }
                                                }
                                                ui.add_space(4.0);
                                            }
                                        }
                                        if pending {
                                            ui.horizontal(|ui| {
                                                if ui.add(egui::Button::new(RichText::new("Submit").color(Color32::WHITE)).fill(CLAUDE)).clicked() {
                                                    acts.push(Act::Submit(idx, true));
                                                }
                                                if ui.button("Skip").clicked() {
                                                    acts.push(Act::Submit(idx, false));
                                                }
                                            });
                                        }
                                    });
                            }
                            Item::Result { text, is_error } => {
                                if *is_error {
                                    ui.add(egui::Label::new(RichText::new(format!("✖ {text}")).small().color(pal.error)).wrap());
                                } else {
                                    ui.label(RichText::new(format!("✔ {text}")).small().color(pal.text_weak));
                                }
                                ui.add_space(6.0);
                            }
                            Item::System(t) => {
                                ui.label(RichText::new(t.as_str()).small().italics().color(pal.text_weak));
                            }
                            Item::Error(t) => {
                                ui.add(egui::Label::new(RichText::new(t.as_str()).small().color(pal.error)).wrap());
                            }
                        }
                    }
                    if *busy && !live_text.is_empty() {
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new("●").size(10.0).color(pal.text));
                            ui.vertical(|ui| {
                                markdown(ui, md_cache, live_text);
                            });
                        });
                    }
                    ui.add_space(8.0);
                });
        });

    for a in acts {
        match a {
            Act::Allow(i, always, mode) => chat.respond_permission(i, true, always, mode),
            Act::Deny(i) => {
                chat.respond_permission(i, false, false, None);
                chat.focus_input = true;
            }
            Act::Pick(i, q, label, multi) => {
                if let Some(ans) = chat.question_answers_mut(i) {
                    if multi {
                        let mut cur: Vec<String> = ans
                            .get(&q)
                            .map(|s| s.split(", ").filter(|x| !x.is_empty()).map(|x| x.to_owned()).collect())
                            .unwrap_or_default();
                        if let Some(p) = cur.iter().position(|x| *x == label) {
                            cur.remove(p);
                        } else {
                            cur.push(label);
                        }
                        ans.insert(q, cur.join(", "));
                    } else {
                        ans.insert(q, label);
                    }
                }
            }
            Act::Submit(i, ok) => chat.respond_question(i, ok),
            Act::Open(p) => out.push(ChatAction::OpenFile(p)),
        }
    }
    out
}
