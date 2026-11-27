//! Options > Plugin List: the folders to search, scanning (each file is opened in its own
//! short-lived process, so a crashing plugin can't take the host down) and the known plugins.

use crate::app::App;
use crate::plugins::{self, Scan, ScanMsg};
use crate::subwin;
use egui::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

#[derive(Default)]
pub struct PluginListState {
    pub scan: Option<Scan>,
    pub current: String,
    pub done: usize,
    pub total: usize,
    pub found: usize,
    pub failures: Vec<(String, String)>,
    pub selected: Option<usize>,
    pub filter: String,
    pub finished: Option<String>,
}

enum Act {
    AddFolder(bool),
    RemoveFolder(bool, usize),
    ScanNew,
    ScanAll,
    AddFiles,
    Remove,
    Clear,
}

impl App {
    fn start_plugin_scan(&mut self, files: Vec<PathBuf>) {
        let ctx = self.egui_ctx.clone();
        let Some(st) = self.dlg.plugins.as_mut() else { return };
        if files.is_empty() {
            st.finished = Some("No new plugin files were found in the plugin folders.".into());
            return;
        }
        st.total = files.len();
        st.done = 0;
        st.found = 0;
        st.failures.clear();
        st.finished = None;
        st.current.clear();
        st.scan = Some(plugins::start_scan(files, move || ctx.request_repaint()));
    }

    fn poll_scan(&mut self) {
        let Some(st) = self.dlg.plugins.as_mut() else { return };
        let Some(scan) = &st.scan else { return };
        let (mut found, mut done) = (Vec::new(), false);
        while let Ok(m) = scan.rx.try_recv() {
            match m {
                ScanMsg::Scanning(f, i, t) => {
                    st.current = f;
                    st.done = i;
                    st.total = t;
                }
                ScanMsg::Found(list) => {
                    st.found += list.len();
                    found.extend(list);
                }
                ScanMsg::Failed(f, e) => st.failures.push((f, e)),
                ScanMsg::Done => done = true,
            }
        }
        if done {
            let cancelled = scan.cancel.load(Ordering::Relaxed);
            st.scan = None;
            st.finished = Some(format!(
                "{}: {} plugin(s) found{}.",
                if cancelled { "Scan cancelled" } else { "Scan finished" },
                st.found,
                if st.failures.is_empty() { String::new() } else { format!(", {} file(s) could not be loaded", st.failures.len()) }
            ));
        }
        if !found.is_empty() {
            for p in found {
                self.settings.plugins.retain(|o| !o.same_plugin(&p));
                self.settings.plugins.push(p);
            }
            self.settings.plugins.sort_by_key(|p| p.name.to_lowercase());
            self.settings.save();
        }
    }

    pub fn plugin_list_window(&mut self, ctx: &Context) {
        self.poll_scan();
        let (mut open, mut act) = (true, None);
        let Some(st) = self.dlg.plugins.as_mut() else { return };
        let s = &self.settings;
        let spec = subwin::Spec::new(subwin::id("plugins"), "Available Plugins", subwin::Kind::fill(vec2(680.0, 470.0), vec2(420.0, 260.0), true).scrolled());
        let close = subwin::show(ctx, spec, |ui| {
            CollapsingHeader::new("Plugin folders").default_open(s.plugins.is_empty()).show(ui, |ui| {
                for (title, list, vst3) in [("VST folders", &s.vst2_folders, false), ("VST3 folders", &s.vst3_folders, true)] {
                    ui.strong(title);
                    for (k, f) in list.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button("Remove").clicked() {
                                act = Some(Act::RemoveFolder(vst3, k));
                            }
                            ui.label(f);
                        });
                    }
                    if ui.button(if vst3 { "Add a VST3 folder..." } else { "Add a VST folder..." }).clicked() {
                        act = Some(Act::AddFolder(vst3));
                    }
                    ui.add_space(4.0);
                }
            });
            let scanning = st.scan.is_some();
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!scanning, Button::new("Scan for new plugins")).clicked() {
                    act = Some(Act::ScanNew);
                }
                if ui.add_enabled(!scanning, Button::new("Rescan everything")).clicked() {
                    act = Some(Act::ScanAll);
                }
                if ui.add_enabled(!scanning, Button::new("Add plugin files...")).clicked() {
                    act = Some(Act::AddFiles);
                }
                if ui.add_enabled(st.selected.is_some() && !scanning, Button::new("Remove from list")).clicked() {
                    act = Some(Act::Remove);
                }
                if ui.add_enabled(!s.plugins.is_empty() && !scanning, Button::new("Clear list")).clicked() {
                    act = Some(Act::Clear);
                }
            });
            if let Some(scan) = &st.scan {
                ui.horizontal(|ui| {
                    let f = if st.total > 0 { st.done as f32 / st.total as f32 } else { 0.0 };
                    ui.add(ProgressBar::new(f).desired_width(380.0).text(format!("{} of {}", (st.done + 1).min(st.total), st.total)));
                    if ui.button("Cancel").clicked() {
                        scan.cancel.store(true, Ordering::Relaxed);
                    }
                });
                ui.label(RichText::new(format!("Scanning {}", st.current)).small());
            } else if let Some(f) = &st.finished {
                ui.label(f);
            }
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.add(TextEdit::singleline(&mut st.filter).desired_width(200.0));
                ui.label(format!("{} plugin(s) known", s.plugins.len()));
            });
            ui.separator();
            let filter = st.filter.to_lowercase();
            ScrollArea::both().id_salt("plugin_table").max_height(250.0).auto_shrink([false, true]).show(ui, |ui| {
                Grid::new("plugin_grid").num_columns(5).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                    for h in ["Name", "Format", "Category", "Manufacturer", "File"] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for (k, p) in s.plugins.iter().enumerate() {
                        if !filter.is_empty() && ![&p.name, &p.manufacturer, &p.category].iter().any(|t| t.to_lowercase().contains(&filter)) {
                            continue;
                        }
                        if ui.selectable_label(st.selected == Some(k), &p.name).clicked() {
                            st.selected = Some(k);
                        }
                        ui.label(&p.format);
                        ui.label(p.menu_category());
                        ui.label(&p.manufacturer);
                        ui.label(plugins::file_name(&p.file)).on_hover_text(&p.file);
                        ui.end_row();
                    }
                });
            });
            if !st.failures.is_empty() {
                CollapsingHeader::new(format!("{} file(s) could not be loaded", st.failures.len())).show(ui, |ui| {
                    ScrollArea::vertical().id_salt("plugin_failures").max_height(110.0).show(ui, |ui| {
                        for (f, e) in &st.failures {
                            ui.label(RichText::new(format!("{f}: {e}")).small());
                        }
                    });
                });
            }
        });
        open &= !close;
        match act {
            Some(Act::AddFolder(vst3)) => {
                if let Some(d) = subwin::pick_folder(rfd::FileDialog::new()) {
                    let d = d.display().to_string();
                    let list = if vst3 { &mut self.settings.vst3_folders } else { &mut self.settings.vst2_folders };
                    if !list.iter().any(|x| x.eq_ignore_ascii_case(&d)) {
                        list.push(d);
                        self.settings.save();
                    }
                }
            }
            Some(Act::RemoveFolder(vst3, k)) => {
                let list = if vst3 { &mut self.settings.vst3_folders } else { &mut self.settings.vst2_folders };
                if k < list.len() {
                    list.remove(k);
                    self.settings.save();
                }
            }
            Some(Act::ScanNew) => {
                let known = &self.settings.plugins;
                let files: Vec<PathBuf> = plugins::candidates(&self.settings.vst2_folders, &self.settings.vst3_folders)
                    .into_iter()
                    .filter(|f| !known.iter().any(|p| Path::new(&p.file) == f.as_path() || p.file.eq_ignore_ascii_case(&f.display().to_string())))
                    .collect();
                self.start_plugin_scan(files);
            }
            Some(Act::ScanAll) => {
                self.settings.plugins.retain(|p| Path::new(&p.file).exists());
                let files = plugins::candidates(&self.settings.vst2_folders, &self.settings.vst3_folders);
                self.start_plugin_scan(files);
            }
            Some(Act::AddFiles) => {
                if let Some(files) = subwin::pick_files(rfd::FileDialog::new().add_filter("VST and VST3 plugins", &["dll", "vst3"])) {
                    self.start_plugin_scan(files);
                }
            }
            Some(Act::Remove) => {
                if let Some(k) = self.dlg.plugins.as_mut().and_then(|st| st.selected.take()) {
                    if k < self.settings.plugins.len() {
                        self.settings.plugins.remove(k);
                        self.settings.save();
                    }
                }
            }
            Some(Act::Clear) => {
                self.settings.plugins.clear();
                if let Some(st) = self.dlg.plugins.as_mut() {
                    st.selected = None;
                }
                self.settings.save();
            }
            None => {}
        }
        if !open {
            if let Some(scan) = self.dlg.plugins.as_ref().and_then(|st| st.scan.as_ref()) {
                scan.cancel.store(true, Ordering::Relaxed);
            }
            self.dlg.plugins = None;
        }
    }
}
