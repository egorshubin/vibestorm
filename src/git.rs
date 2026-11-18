//! Git integration: status, commit, push/pull/fetch, branches, log graph, diffs.
//! All git commands run on background threads; results come back over a channel.

use crate::theme::{LANES, Palette, paint_check, tool_btn};
use crate::util::hidden_cmd;
use eframe::egui::{self, Align, Align2, Color32, FontId, Frame, Layout, Margin, RichText, Sense, Stroke, Ui, pos2, vec2};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
}

impl StatusKind {
    pub fn color(self, p: &Palette) -> Color32 {
        match self {
            StatusKind::Modified | StatusKind::Renamed => p.modified,
            StatusKind::Added => p.added,
            StatusKind::Deleted => p.deleted,
            StatusKind::Untracked => p.untracked,
            StatusKind::Conflict => p.conflict,
        }
    }
    pub fn letter(self) -> &'static str {
        match self {
            StatusKind::Modified => "M",
            StatusKind::Added => "A",
            StatusKind::Deleted => "D",
            StatusKind::Renamed => "R",
            StatusKind::Untracked => "U",
            StatusKind::Conflict => "!",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitFile {
    pub path: String,
    pub orig: Option<String>,
    pub kind: StatusKind,
}

#[derive(Clone, Default)]
pub struct StatusData {
    pub is_repo: bool,
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<GitFile>,
}

#[derive(Clone)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub refs: Vec<String>,
    pub subject: String,
    pub author: String,
    pub date: String,
}

#[derive(Clone)]
pub struct LogRow {
    pub graph: String,
    pub commit: Option<Commit>,
}

pub enum GitMsg {
    Status(StatusData),
    Log(Vec<LogRow>),
    Branches(Vec<String>, Vec<String>),
    Op { label: String, result: Result<String, String> },
    Diff { title: String, key: String, text: String },
    Details { hash: String, text: String, files: Vec<(String, String)> },
    CommitMsg(Result<String, String>),
}

pub enum GitUiAction {
    OpenDiff { title: String, key: String, text: String },
    OpenFile(PathBuf),
    ConfirmRollback(Vec<GitFile>),
}

pub struct GitState {
    pub root: PathBuf,
    pub status: StatusData,
    pub checked: HashSet<String>,
    pub status_map: HashMap<String, StatusKind>,
    pub dirty_dirs: HashSet<String>,
    pub log: Vec<LogRow>,
    pub local_branches: Vec<String>,
    pub remote_branches: Vec<String>,
    pub commit_msg: String,
    pub amend: bool,
    pub busy: Vec<String>,
    pub last_error: Option<String>,
    pub last_info: Option<String>,
    pub selected_commit: Option<String>,
    pub details: Option<(String, String, Vec<(String, String)>)>,
    pub generating_msg: bool,
    /// change selected in the Commit tree (shows its diff)
    pub sel_change: Option<String>,
    pub collapsed_groups: HashSet<&'static str>,
    status_inflight: bool,
    status_again: bool,
    commit_msg_pending_clear: bool,
    pending_ui: Vec<GitUiAction>,
    known_files: HashSet<String>,
    tx: Sender<GitMsg>,
    rx: Receiver<GitMsg>,
    ctx: egui::Context,
}

fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = hidden_cmd("git")
        .args(["-c", "core.quotepath=false", "-c", "color.ui=false"])
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("Cannot run git: {e}"))?;
    if out.status.success() {
        let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
        let err = String::from_utf8_lossy(&out.stderr);
        if s.trim().is_empty() && !err.trim().is_empty() {
            s = err.into_owned(); // push/pull report progress on stderr
        }
        Ok(s)
    } else {
        let mut e = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        if e.is_empty() {
            e = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        }
        Err(e)
    }
}

fn parse_status(s: &str) -> StatusData {
    let mut d = StatusData {
        is_repo: true,
        ..Default::default()
    };
    let mut toks = s.split('\0').peekable();
    while let Some(t) = toks.next() {
        if t.is_empty() {
            continue;
        }
        if let Some(b) = t.strip_prefix("## ") {
            // "main...origin/main [ahead 1, behind 2]" | "No commits yet on main" | "HEAD (no branch)"
            let (head, rest) = match b.find(" [") {
                Some(i) => (&b[..i], &b[i..]),
                None => (b, ""),
            };
            if let Some(n) = head.strip_prefix("No commits yet on ") {
                d.branch = n.to_owned();
            } else if let Some((l, r)) = head.split_once("...") {
                d.branch = l.to_owned();
                d.upstream = Some(r.to_owned());
            } else {
                d.branch = head.to_owned();
            }
            for part in rest.trim_matches(|c| c == ' ' || c == '[' || c == ']').split(", ") {
                if let Some(n) = part.strip_prefix("ahead ") {
                    d.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("behind ") {
                    d.behind = n.parse().unwrap_or(0);
                }
            }
            continue;
        }
        if t.len() < 4 {
            continue;
        }
        let x = t.as_bytes()[0] as char;
        let y = t.as_bytes()[1] as char;
        let path = t[3..].to_owned();
        let mut orig = None;
        if x == 'R' || x == 'C' {
            orig = toks.next().map(|s| s.to_owned());
        }
        let kind = match (x, y) {
            ('?', '?') => StatusKind::Untracked,
            ('!', '!') => continue,
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => StatusKind::Conflict,
            ('R', _) | ('C', _) => StatusKind::Renamed,
            ('A', _) => StatusKind::Added,
            ('D', _) | (_, 'D') => StatusKind::Deleted,
            _ => StatusKind::Modified,
        };
        d.files.push(GitFile { path, orig, kind });
    }
    d.files.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
    d
}

fn parse_log(s: &str) -> Vec<LogRow> {
    s.lines()
        .map(|line| match line.find('\x1f') {
            None => LogRow {
                graph: line.trim_end().to_owned(),
                commit: None,
            },
            Some(i) => {
                let graph = line[..i].trim_end().to_owned();
                let f: Vec<&str> = line[i + 1..].split('\x1f').collect();
                let get = |k: usize| f.get(k).copied().unwrap_or("").to_owned();
                let refs = get(2)
                    .split(", ")
                    .filter(|r| !r.is_empty())
                    .map(|r| r.to_owned())
                    .collect();
                LogRow {
                    graph,
                    commit: Some(Commit {
                        hash: get(0),
                        short: get(1),
                        refs,
                        subject: get(3),
                        author: get(4),
                        date: get(5),
                    }),
                }
            }
        })
        .collect()
}

impl GitState {
    pub fn new(root: PathBuf, ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let mut g = Self {
            root,
            status: StatusData::default(),
            checked: HashSet::new(),
            status_map: HashMap::new(),
            dirty_dirs: HashSet::new(),
            log: vec![],
            local_branches: vec![],
            remote_branches: vec![],
            commit_msg: String::new(),
            amend: false,
            busy: vec![],
            last_error: None,
            last_info: None,
            selected_commit: None,
            details: None,
            generating_msg: false,
            sel_change: None,
            collapsed_groups: HashSet::new(),
            status_inflight: false,
            status_again: false,
            commit_msg_pending_clear: false,
            pending_ui: vec![],
            known_files: HashSet::new(),
            tx,
            rx,
            ctx,
        };
        g.refresh_all();
        g
    }

    fn spawn(&self, f: impl FnOnce(&Path) -> GitMsg + Send + 'static) {
        let tx = self.tx.clone();
        let root = self.root.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let msg = f(&root);
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    pub fn refresh_status(&mut self) {
        if self.status_inflight {
            self.status_again = true;
            return;
        }
        self.status_inflight = true;
        self.spawn(|root| {
            match run_git(root, &["status", "--porcelain=v1", "-z", "-b", "--untracked-files=all"]) {
                Ok(s) => GitMsg::Status(parse_status(&s)),
                Err(_) => GitMsg::Status(StatusData::default()),
            }
        });
    }

    pub fn refresh_log(&mut self) {
        self.spawn(|root| {
            let r = run_git(
                root,
                &[
                    "log",
                    "--graph",
                    "--date-order",
                    "--all",
                    "-n",
                    "600",
                    "--date=relative",
                    "--pretty=format:\x1f%H\x1f%h\x1f%D\x1f%s\x1f%an\x1f%ad",
                ],
            );
            GitMsg::Log(r.map(|s| parse_log(&s)).unwrap_or_default())
        });
    }

    pub fn refresh_branches(&mut self) {
        self.spawn(|root| {
            let local = run_git(root, &["branch", "--format=%(refname:short)"]).unwrap_or_default();
            let remote = run_git(root, &["branch", "-r", "--format=%(refname:short)"]).unwrap_or_default();
            GitMsg::Branches(
                local.lines().map(|s| s.to_owned()).filter(|s| !s.is_empty()).collect(),
                remote
                    .lines()
                    .map(|s| s.to_owned())
                    .filter(|s| !s.is_empty() && !s.ends_with("/HEAD") && s.contains('/'))
                    .collect(),
            )
        });
    }

    pub fn refresh_all(&mut self) {
        self.refresh_status();
        self.refresh_log();
        self.refresh_branches();
    }

    /// Run an arbitrary git operation, then refresh everything.
    pub fn op(&mut self, label: &str, steps: Vec<Vec<String>>) {
        self.busy.push(label.to_owned());
        let label = label.to_owned();
        self.spawn(move |root| {
            let mut out = String::new();
            for step in steps {
                let args: Vec<&str> = step.iter().map(|s| s.as_str()).collect();
                match run_git(root, &args) {
                    Ok(s) => out.push_str(&s),
                    Err(e) => {
                        return GitMsg::Op {
                            label,
                            result: Err(e),
                        };
                    }
                }
            }
            GitMsg::Op {
                label,
                result: Ok(out),
            }
        });
    }

    pub fn fetch(&mut self) {
        self.op("Fetching", vec![vec!["fetch".into(), "--all".into(), "--prune".into()]]);
    }
    pub fn pull(&mut self) {
        self.op("Pulling", vec![vec!["pull".into()]]);
    }
    pub fn push(&mut self) {
        let args = if self.status.upstream.is_some() {
            vec!["push".into()]
        } else {
            vec!["push".into(), "-u".into(), "origin".into(), "HEAD".into()]
        };
        self.op("Pushing", vec![args]);
    }
    pub fn checkout(&mut self, branch: &str, remote: bool) {
        let name = if remote {
            branch.split_once('/').map(|(_, b)| b).unwrap_or(branch)
        } else {
            branch
        };
        self.op(&format!("Checking out {name}"), vec![vec!["checkout".into(), name.into()]]);
    }
    pub fn new_branch(&mut self, name: &str) {
        self.op(&format!("Creating {name}"), vec![vec!["checkout".into(), "-b".into(), name.into()]]);
    }

    pub fn commit(&mut self, and_push: bool) {
        let msg = self.commit_msg.trim().to_owned();
        if msg.is_empty() && !self.amend {
            self.last_error = Some("Commit message is empty".into());
            return;
        }
        let selected: Vec<GitFile> = self
            .status
            .files
            .iter()
            .filter(|f| self.checked.contains(&f.path))
            .cloned()
            .collect();
        if selected.is_empty() && !self.amend {
            self.last_error = Some("No files selected for commit".into());
            return;
        }
        let all = selected.len() == self.status.files.len();
        let mut steps: Vec<Vec<String>> = vec![];
        let mut commit: Vec<String> = vec!["commit".into()];
        if self.amend {
            commit.push("--amend".into());
            if msg.is_empty() {
                commit.push("--no-edit".into());
            }
        }
        if !msg.is_empty() {
            commit.push("-m".into());
            commit.push(msg);
        }
        if all {
            steps.push(vec!["add".into(), "-A".into()]);
        } else if !selected.is_empty() {
            let mut list = String::new();
            for f in &selected {
                list.push_str(&f.path);
                list.push('\0');
                if let Some(o) = &f.orig {
                    list.push_str(o);
                    list.push('\0');
                }
            }
            let file = std::env::temp_dir().join(format!("vibestorm-pathspec-{}.txt", std::process::id()));
            let _ = std::fs::write(&file, list);
            let spec = format!("--pathspec-from-file={}", file.display());
            steps.push(vec!["add".into(), "-A".into(), spec.clone(), "--pathspec-file-nul".into()]);
            commit.push(spec);
            commit.push("--pathspec-file-nul".into());
        }
        steps.push(commit);
        if and_push {
            if self.status.upstream.is_some() {
                steps.push(vec!["push".into()]);
            } else {
                steps.push(vec!["push".into(), "-u".into(), "origin".into(), "HEAD".into()]);
            }
        }
        self.op(if and_push { "Committing and pushing" } else { "Committing" }, steps);
        self.commit_msg_pending_clear = true;
    }

    pub fn rollback(&mut self, files: Vec<GitFile>) {
        let mut tracked: Vec<String> = vec![];
        for f in &files {
            if f.kind == StatusKind::Untracked || f.kind == StatusKind::Added && f.orig.is_none() {
                let _ = std::fs::remove_file(self.root.join(&f.path));
                if f.kind == StatusKind::Added {
                    tracked.push(f.path.clone());
                }
            } else {
                tracked.push(f.path.clone());
                if let Some(o) = &f.orig {
                    tracked.push(o.clone());
                }
            }
        }
        if tracked.is_empty() {
            self.refresh_status();
            return;
        }
        let mut args: Vec<String> = vec![
            "restore".into(),
            "--source=HEAD".into(),
            "--staged".into(),
            "--worktree".into(),
            "--".into(),
        ];
        args.extend(tracked);
        self.op("Rolling back", vec![args]);
    }

    pub fn show_file_diff(&self, f: &GitFile) {
        let path = f.path.clone();
        let kind = f.kind;
        self.spawn(move |root| {
            let text = if kind == StatusKind::Untracked {
                run_git(root, &["diff", "--no-index", "--", "/dev/null", &path])
                    .or_else(|e| if e.is_empty() { Ok(String::new()) } else { Err(e) })
            } else {
                run_git(root, &["diff", "HEAD", "--", &path])
                    .or_else(|_| run_git(root, &["diff", "--cached", "--", &path]))
            };
            // `git diff --no-index` exits 1 when files differ; its output then arrives as Err.
            let text = match text {
                Ok(t) => t,
                Err(e) => e,
            };
            GitMsg::Diff {
                title: format!("Δ {}", path.rsplit('/').next().unwrap_or(&path)),
                key: format!("diff:{path}"),
                text: if text.trim().is_empty() { "(no textual changes)".into() } else { text },
            }
        });
    }

    pub fn select_commit(&mut self, hash: &str) {
        self.selected_commit = Some(hash.to_owned());
        let h = hash.to_owned();
        self.spawn(move |root| {
            let text = run_git(
                root,
                &["show", "-s", "--format=%H%nAuthor: %an <%ae>%nDate:   %ad%n%n%B", &h],
            )
            .unwrap_or_else(|e| e);
            let files = run_git(root, &["show", "--name-status", "--format=", &h])
                .unwrap_or_default()
                .lines()
                .filter_map(|l| {
                    let mut it = l.split('\t');
                    let st = it.next()?.to_owned();
                    let p = it.last()?.to_owned();
                    Some((st, p))
                })
                .collect();
            GitMsg::Details { hash: h, text, files }
        });
    }

    pub fn show_commit_file_diff(&self, hash: &str, path: &str) {
        let (h, p) = (hash.to_owned(), path.to_owned());
        self.spawn(move |root| {
            let text = run_git(root, &["show", "--format=%h %s%n", &h, "--", &p]).unwrap_or_else(|e| e);
            GitMsg::Diff {
                title: format!("Δ {} @{}", p.rsplit('/').next().unwrap_or(&p), &h[..h.len().min(7)]),
                key: format!("show:{h}:{p}"),
                text,
            }
        });
    }

    /// Ask Claude (haiku, one-shot) to write a commit message from the diff.
    pub fn generate_message(&mut self) {
        if self.generating_msg {
            return;
        }
        self.generating_msg = true;
        let files: Vec<String> = self
            .status
            .files
            .iter()
            .filter(|f| self.checked.contains(&f.path))
            .map(|f| f.path.clone())
            .collect();
        self.spawn(move |root| {
            let mut args = vec!["diff", "HEAD", "--"];
            for f in &files {
                args.push(f);
            }
            let mut diff = run_git(root, &args).unwrap_or_default();
            if diff.trim().is_empty() {
                diff = format!("New/untracked files: {}", files.join(", "));
            }
            if diff.len() > 40_000 {
                let mut cut = 40_000;
                while !diff.is_char_boundary(cut) {
                    cut -= 1;
                }
                diff.truncate(cut);
                diff.push_str("\n…(truncated)");
            }
            let prompt = format!(
                "Write a git commit message for the following changes. Use a concise imperative summary line (max 72 chars), then optionally a blank line and a short bullet list. Output ONLY the commit message, no quotes or code fences.\n\n{diff}"
            );
            GitMsg::CommitMsg(crate::claude::one_shot(root, &prompt, "haiku"))
        });
    }

    /// Process background results. Returns true if the working tree may have changed.
    pub fn poll(&mut self) -> bool {
        let mut tree_changed = false;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                GitMsg::Status(s) => {
                    self.status_inflight = false;
                    // default: newly seen files are checked (like PhpStorm)
                    let paths: HashSet<String> = s.files.iter().map(|f| f.path.clone()).collect();
                    for p in &paths {
                        if !self.known_files.contains(p) {
                            self.checked.insert(p.clone());
                        }
                    }
                    self.checked.retain(|p| paths.contains(p));
                    self.known_files = paths;
                    self.status_map = s.files.iter().map(|f| (f.path.clone(), f.kind)).collect();
                    self.dirty_dirs.clear();
                    for f in &s.files {
                        let mut p = f.path.as_str();
                        while let Some(i) = p.rfind('/') {
                            p = &p[..i];
                            self.dirty_dirs.insert(p.to_owned());
                        }
                    }
                    self.status = s;
                    if self.status_again {
                        self.status_again = false;
                        self.refresh_status();
                    }
                }
                GitMsg::Log(l) => self.log = l,
                GitMsg::Branches(l, r) => {
                    self.local_branches = l;
                    self.remote_branches = r;
                }
                GitMsg::Op { label, result } => {
                    self.busy.retain(|b| b != &label);
                    match result {
                        Ok(out) => {
                            self.last_error = None;
                            let first = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                            self.last_info = Some(format!("{label}: done{}", if first.is_empty() { String::new() } else { format!(" — {first}") }));
                            if label.starts_with("Committing") && self.commit_msg_pending_clear {
                                self.commit_msg.clear();
                                self.amend = false;
                            }
                        }
                        Err(e) => {
                            self.last_error = Some(format!("{label} failed: {e}"));
                        }
                    }
                    self.commit_msg_pending_clear = false;
                    tree_changed = true;
                    self.refresh_all();
                }
                GitMsg::Diff { title, key, text } => {
                    self.pending_ui.push(GitUiAction::OpenDiff { title, key, text });
                }
                GitMsg::Details { hash, text, files } => {
                    if self.selected_commit.as_deref() == Some(hash.as_str()) {
                        self.details = Some((hash, text, files));
                    }
                }
                GitMsg::CommitMsg(r) => {
                    self.generating_msg = false;
                    match r {
                        Ok(m) => self.commit_msg = m.trim().to_owned(),
                        Err(e) => self.last_error = Some(format!("Generating message failed: {e}")),
                    }
                }
            }
        }
        tree_changed
    }

    pub fn take_ui_actions(&mut self) -> Vec<GitUiAction> {
        std::mem::take(&mut self.pending_ui)
    }

    // ------------------------------------------------------------------ UI

    /// The "Commit" tool window (left side).
    /// The left "Commit" tool window, laid out like JetBrains: toolbar, change tree
    /// with tri-state checkboxes, resizable commit-message box, Commit / Commit and Push.
    pub fn commit_ui(&mut self, ui: &mut Ui, pal: &Palette) -> Vec<GitUiAction> {
        let mut actions = vec![];
        if !self.status.is_repo {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("This folder is not a git repository.").color(pal.text_weak));
                if ui.button("Initialize repository").clicked() {
                    self.op("Initializing", vec![vec!["init".into()]]);
                }
            });
            return actions;
        }
        let checked_files: Vec<GitFile> =
            self.status.files.iter().filter(|f| self.checked.contains(&f.path)).cloned().collect();
        let selected: Option<GitFile> = self
            .sel_change
            .as_ref()
            .and_then(|s| self.status.files.iter().find(|f| &f.path == s))
            .cloned();

        // ---------------- toolbar
        egui::Panel::top(("commit_toolbar", self.root.clone()))
            .frame(Frame::NONE.inner_margin(Margin::symmetric(4, 3)))
            .show_separator_line(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 1.0;
                    if tool_btn(ui, pal, "⟳", "Refresh", true) {
                        self.refresh_all();
                    }
                    let rb = if checked_files.is_empty() { selected.clone().into_iter().collect() } else { checked_files.clone() };
                    if tool_btn(ui, pal, "↶", "Rollback… (checked files, or the selected one)", !rb.is_empty()) {
                        actions.push(GitUiAction::ConfirmRollback(rb));
                    }
                    if tool_btn(ui, pal, "⇄", "Show Diff (Ctrl+D)", selected.is_some()) {
                        if let Some(f) = &selected {
                            self.show_file_diff(f);
                        }
                    }
                    ui.add_space(6.0);
                    if tool_btn(ui, pal, "⊞", "Expand All", true) {
                        self.collapsed_groups.clear();
                    }
                    if tool_btn(ui, pal, "⊟", "Collapse All", true) {
                        self.collapsed_groups.insert("changes");
                        self.collapsed_groups.insert("unversioned");
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !self.status.branch.is_empty() {
                            ui.label(RichText::new(format!("⎇ {}", self.status.branch)).small().color(pal.text_weak));
                        }
                    });
                });
            });

        // ---------------- commit message + buttons (resizable from the top edge)
        egui::Panel::bottom(("commit_box", self.root.clone()))
            .resizable(true)
            .default_size(200.0)
            .min_size(140.0)
            .max_size((ui.available_height() - 80.0).max(150.0))
            .frame(Frame::NONE.inner_margin(Margin { left: 6, right: 6, top: 8, bottom: 6 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.amend, "Amend");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let gen_label = if self.generating_msg { "✨ Writing…" } else { "✨" };
                        if tool_btn(ui, pal, gen_label, "Generate commit message with Claude (Haiku) from the checked changes", !self.generating_msg) {
                            self.generate_message();
                        }
                    });
                });
                ui.add_space(2.0);
                let msg_id = egui::Id::new(("commit_msg", self.root.clone()));
                let focused = ui.memory(|m| m.has_focus(msg_id));
                let buttons_h = 34.0;
                let box_h = (ui.available_height() - buttons_h).max(50.0);
                Frame::NONE
                    .fill(pal.input_bg)
                    .stroke(Stroke::new(1.0, if focused { pal.accent } else { pal.border }))
                    .corner_radius(6)
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt(("commit_msg_scroll", self.root.clone()))
                            .max_height(box_h - 12.0)
                            .min_scrolled_height(box_h - 12.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.add_sized(
                                    vec2(ui.available_width(), box_h - 12.0),
                                    egui::TextEdit::multiline(&mut self.commit_msg)
                                        .id(msg_id)
                                        .frame(Frame::NONE)
                                        .hint_text("Commit Message"),
                                );
                            });
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let n = self.checked.len();
                    let busy = !self.busy.is_empty();
                    let can = !busy;
                    if ui
                        .add_enabled(
                            can,
                            egui::Button::new(RichText::new("  Commit  ").color(Color32::WHITE)).fill(pal.accent).corner_radius(4),
                        )
                        .on_hover_text(format!("Commit {n} file(s)  (Ctrl+Enter)"))
                        .clicked()
                    {
                        self.commit(false);
                    }
                    if ui.add_enabled(can, egui::Button::new("Commit and Push…").corner_radius(4)).clicked() {
                        self.commit(true);
                    }
                    if busy {
                        ui.label(RichText::new(format!("{}…", self.busy.join(", "))).small().color(pal.accent));
                    } else if n > 0 {
                        ui.label(RichText::new(format!("{n} selected")).small().color(pal.text_weak));
                    }
                });
            });

        // ---------------- change tree
        egui::CentralPanel::default().frame(Frame::NONE.inner_margin(Margin::symmetric(2, 2))).show(ui, |ui| {
            let (tracked, untracked): (Vec<GitFile>, Vec<GitFile>) =
                self.status.files.iter().cloned().partition(|f| f.kind != StatusKind::Untracked);
            if tracked.is_empty() && untracked.is_empty() {
                ui.add_space(14.0);
                ui.vertical_centered(|ui| ui.label(RichText::new("No local changes").color(pal.text_weak)));
                return;
            }
            let row_h = 22.0;
            let mut toggle_files: Vec<(String, bool)> = vec![];
            let mut toggle_group: Option<&'static str> = None;
            let mut select: Option<GitFile> = None;
            let mut open: Option<GitFile> = None;
            egui::ScrollArea::vertical().id_salt(("changes", self.root.clone())).auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (key, title, list) in [("changes", "Changes", &tracked), ("unversioned", "Unversioned Files", &untracked)] {
                    if list.is_empty() {
                        continue;
                    }
                    let n_checked = list.iter().filter(|f| self.checked.contains(&f.path)).count();
                    let state = if n_checked == 0 {
                        Some(false)
                    } else if n_checked == list.len() {
                        Some(true)
                    } else {
                        None
                    };
                    let open_group = !self.collapsed_groups.contains(key);
                    // group row
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                    let p = ui.painter();
                    if resp.hovered() {
                        p.rect_filled(rect, 3.0, pal.hover);
                    }
                    let mut x = rect.min.x + 4.0;
                    p.text(pos2(x + 5.0, rect.center().y), Align2::CENTER_CENTER, if open_group { "⌄" } else { "›" }, FontId::proportional(13.0), pal.text_weak);
                    x += 14.0;
                    let cb = egui::Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(13.0, 13.0));
                    paint_check(p, cb, state, pal);
                    x += 20.0;
                    let g = p.layout_no_wrap(title.to_owned(), FontId::proportional(13.0), pal.text);
                    let gw = g.size().x;
                    p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, pal.text);
                    let files_word = if list.len() == 1 { "file" } else { "files" };
                    p.text(pos2(x + gw + 8.0, rect.center().y), Align2::LEFT_CENTER, format!("{} {files_word}", list.len()), FontId::proportional(12.0), pal.text_weak);
                    if resp.clicked() {
                        let ptr = resp.interact_pointer_pos().unwrap_or_default();
                        if cb.expand(4.0).contains(ptr) {
                            let on = state != Some(true);
                            for f in list.iter() {
                                toggle_files.push((f.path.clone(), on));
                            }
                        } else {
                            toggle_group = Some(key);
                        }
                    }
                    if !open_group {
                        continue;
                    }
                    // file rows
                    for f in list.iter() {
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                        let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                        let is_sel = self.sel_change.as_deref() == Some(f.path.as_str());
                        if is_sel {
                            p.rect_filled(rect, 3.0, pal.selection);
                        } else if resp.hovered() {
                            p.rect_filled(rect, 3.0, pal.hover);
                        }
                        let mut x = rect.min.x + 22.0;
                        let c = self.checked.contains(&f.path);
                        let cb = egui::Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(13.0, 13.0));
                        paint_check(&p, cb, Some(c), pal);
                        x += 20.0;
                        let (dir, name) = match f.path.rfind('/') {
                            Some(i) => (&f.path[..i], &f.path[i + 1..]),
                            None => ("", f.path.as_str()),
                        };
                        let (badge, bcol) = crate::tree::file_badge(name);
                        let bg = p.layout_no_wrap(badge.to_owned(), FontId::proportional(9.5), bcol);
                        let bw = bg.size().x.max(12.0);
                        p.galley(pos2(x + (bw - bg.size().x) / 2.0, rect.center().y - bg.size().y / 2.0), bg, bcol);
                        x += bw + 6.0;
                        let col = f.kind.color(pal);
                        let ng = p.layout_no_wrap(name.to_owned(), FontId::proportional(13.0), col);
                        let nw = ng.size().x;
                        p.galley(pos2(x, rect.center().y - ng.size().y / 2.0), ng, col);
                        if !dir.is_empty() {
                            p.text(pos2(x + nw + 8.0, rect.center().y), Align2::LEFT_CENTER, dir, FontId::proportional(12.0), pal.text_weak);
                        }
                        if resp.clicked() {
                            let ptr = resp.interact_pointer_pos().unwrap_or_default();
                            if cb.expand(4.0).contains(ptr) {
                                toggle_files.push((f.path.clone(), !c));
                            } else {
                                select = Some(f.clone());
                            }
                        }
                        if resp.double_clicked() {
                            open = Some(f.clone());
                        }
                        resp.on_hover_text(format!("{}  ·  {}\nclick: diff · double-click: open", f.path, f.kind.letter()))
                            .context_menu(|ui| {
                                if ui.button("⇄  Show Diff").clicked() {
                                    select = Some(f.clone());
                                    ui.close();
                                }
                                if ui.button("Open File").clicked() {
                                    open = Some(f.clone());
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button("↶  Rollback…").clicked() {
                                    actions.push(GitUiAction::ConfirmRollback(vec![f.clone()]));
                                    ui.close();
                                }
                            });
                    }
                }
            });
            for (p, on) in toggle_files {
                if on {
                    self.checked.insert(p);
                } else {
                    self.checked.remove(&p);
                }
            }
            if let Some(k) = toggle_group {
                if !self.collapsed_groups.remove(k) {
                    self.collapsed_groups.insert(k);
                }
            }
            if let Some(f) = select {
                self.sel_change = Some(f.path.clone());
                self.show_file_diff(&f);
            }
            if let Some(f) = open {
                actions.push(GitUiAction::OpenFile(self.root.join(&f.path)));
            }
        });
        actions
    }

    /// The bottom "Git" tool window: log graph + commit details.
    pub fn log_ui(&mut self, ui: &mut Ui, pal: &Palette) -> Vec<GitUiAction> {
        let mut actions = vec![];
        if !self.status.is_repo {
            ui.label(RichText::new("Not a git repository").color(pal.text_weak));
            return actions;
        }
        let details_w = (ui.available_width() * 0.36).clamp(240.0, 520.0);
        let full = ui.available_rect_before_wrap();
        let left = egui::Rect::from_min_max(full.min, pos2(full.max.x - details_w, full.max.y));
        let right = egui::Rect::from_min_max(pos2(full.max.x - details_w + 6.0, full.min.y), full.max);

        let mut clicked: Option<String> = None;
        let mut checkout: Option<String> = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
            let row_h = 22.0;
            let cell = 8.0;
            let font = FontId::proportional(13.0);
            let small = FontId::proportional(11.5);
            egui::ScrollArea::vertical()
                .id_salt("git_log")
                .auto_shrink([false, false])
                .show_rows(ui, row_h, self.log.len(), |ui, range| {
                    for row in &self.log[range] {
                        let w = ui.available_width();
                        let (rect, resp) = ui.allocate_exact_size(vec2(w, row_h), Sense::click());
                        let p = ui.painter();
                        let sel = row
                            .commit
                            .as_ref()
                            .map(|c| self.selected_commit.as_deref() == Some(c.hash.as_str()))
                            .unwrap_or(false);
                        if sel {
                            p.rect_filled(rect, 3.0, pal.selection);
                        } else if resp.hovered() && row.commit.is_some() {
                            p.rect_filled(rect, 3.0, pal.hover);
                        }
                        // graph
                        let (top, bot) = (rect.min.y, rect.max.y);
                        let mid = rect.center().y;
                        for (ci, ch) in row.graph.chars().enumerate() {
                            let x = rect.min.x + 6.0 + ci as f32 * cell + cell * 0.5;
                            let lane = LANES[(ci / 2) % LANES.len()];
                            let s = Stroke::new(1.6, lane);
                            match ch {
                                '*' => {
                                    p.line_segment([pos2(x, top), pos2(x, bot)], s);
                                    p.circle_filled(pos2(x, mid), 4.2, lane);
                                    p.circle_filled(pos2(x, mid), 2.0, pal.panel_bg);
                                }
                                '|' => { p.line_segment([pos2(x, top), pos2(x, bot)], s); }
                                '/' => { p.line_segment([pos2(x + cell, top), pos2(x - cell, bot)], Stroke::new(1.6, LANES[((ci + 1) / 2) % LANES.len()])); }
                                '\\' => { p.line_segment([pos2(x - cell, top), pos2(x + cell, bot)], Stroke::new(1.6, LANES[(ci.saturating_sub(1) / 2) % LANES.len()])); }
                                '_' => { p.line_segment([pos2(x - cell * 0.5, bot), pos2(x + cell * 0.5, bot)], s); }
                                '-' | '.' => { p.line_segment([pos2(x - cell * 0.5, mid), pos2(x + cell * 0.5, mid)], s); }
                                _ => {}
                            }
                        }
                        let Some(c) = &row.commit else { continue };
                        let gw = row.graph.chars().count() as f32 * cell + 14.0;
                        let mut x = rect.min.x + gw.max(30.0);
                        for r in &c.refs {
                            let (txt, col) = if let Some(b) = r.strip_prefix("HEAD -> ") {
                                (b.to_owned(), pal.added)
                            } else if let Some(t) = r.strip_prefix("tag: ") {
                                (t.to_owned(), pal.warn)
                            } else if r.contains('/') {
                                (r.clone(), LANES[3])
                            } else {
                                (r.clone(), pal.modified)
                            };
                            let g = p.layout_no_wrap(txt, small.clone(), col);
                            let br = egui::Rect::from_min_size(pos2(x, mid - 8.0), vec2(g.size().x + 10.0, 16.0));
                            p.rect_filled(br, 4.0, col.gamma_multiply(0.18));
                            p.rect_stroke(br, 4.0, Stroke::new(1.0, col.gamma_multiply(0.6)), egui::StrokeKind::Inside);
                            p.galley(pos2(x + 5.0, mid - g.size().y / 2.0), g, col);
                            x += br.width() + 4.0;
                        }
                        let right_x = rect.max.x - 8.0;
                        let date_g = p.layout_no_wrap(c.date.clone(), small.clone(), pal.text_weak);
                        let author_g = p.layout_no_wrap(c.author.clone(), small.clone(), pal.text_weak);
                        let date_x = right_x - 110.0;
                        let author_x = date_x - 130.0;
                        let subj_max = (author_x - x - 10.0).max(20.0);
                        let subj = p.layout(c.subject.clone(), font.clone(), pal.text, f32::INFINITY);
                        let clip = egui::Rect::from_min_max(pos2(x, rect.min.y), pos2(x + subj_max, rect.max.y));
                        p.with_clip_rect(clip).galley(pos2(x + 2.0, mid - subj.size().y / 2.0), subj, pal.text);
                        p.with_clip_rect(egui::Rect::from_min_max(pos2(author_x, top), pos2(date_x - 6.0, bot)))
                            .galley(pos2(author_x, mid - author_g.size().y / 2.0), author_g, pal.text_weak);
                        p.galley(pos2(date_x, mid - date_g.size().y / 2.0), date_g, pal.text_weak);
                        if resp.clicked() {
                            clicked = Some(c.hash.clone());
                        }
                        resp.on_hover_text(format!("{}  {}", c.short, c.subject)).context_menu(|ui| {
                            if ui.button("Copy Revision Number").clicked() {
                                ui.ctx().copy_text(c.hash.clone());
                                ui.close();
                            }
                            if ui.button("Checkout Revision").clicked() {
                                checkout = Some(c.hash.clone());
                                ui.close();
                            }
                        });
                    }
                });
        });
        if let Some(h) = clicked {
            self.select_commit(&h);
        }
        if let Some(h) = checkout {
            self.op("Checking out", vec![vec!["checkout".into(), h]]);
        }

        // details pane
        ui.painter().line_segment(
            [pos2(right.min.x - 3.0, right.min.y), pos2(right.min.x - 3.0, right.max.y)],
            Stroke::new(1.0, pal.border),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
            egui::ScrollArea::vertical().id_salt("git_details").auto_shrink([false, false]).show(ui, |ui| {
                match &self.details {
                    None => {
                        ui.label(RichText::new("Select a commit to see details").color(pal.text_weak));
                    }
                    Some((hash, text, files)) => {
                        let hash = hash.clone();
                        for (st, path) in files.clone() {
                            ui.horizontal(|ui| {
                                let kind = match st.chars().next() {
                                    Some('A') => StatusKind::Added,
                                    Some('D') => StatusKind::Deleted,
                                    Some('R') => StatusKind::Renamed,
                                    _ => StatusKind::Modified,
                                };
                                ui.label(RichText::new(kind.letter()).monospace().color(kind.color(pal)));
                                if ui
                                    .add(egui::Label::new(RichText::new(&path).color(kind.color(pal))).sense(Sense::click()).truncate())
                                    .on_hover_text("Show diff")
                                    .clicked()
                                {
                                    self.show_commit_file_diff(&hash, &path);
                                }
                            });
                        }
                        ui.separator();
                        ui.label(RichText::new(text.clone()).monospace().size(12.0));
                    }
                }
            });
        });
        let _ = ui.allocate_rect(full, Sense::hover());
        actions.append(&mut self.take_ui_actions());
        actions
    }
}
