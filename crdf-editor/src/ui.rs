use crdf::RdfTerm;
#[cfg(feature = "native-dialog")]
use crdf::{Literal, RdfFileFormat, RdfGraph};

use crate::app::CrdfEditorApp;
use crate::graph_view::NodeType;

#[derive(Default)]
pub struct SidePanel {
    pub new_subject: String,
    pub new_predicate: String,
    pub new_object: String,
    pub new_object_is_literal: bool,
    pub show_add_node: bool,
    pub new_node_type: NodeType,
    pub new_node_value: String,
    pub pending_edge: Option<(RdfTerm, RdfTerm)>,
    pub pending_edge_predicate: String,
    pub file_path: Option<std::path::PathBuf>,
}

pub fn draw_menu_bar(app: &mut CrdfEditorApp, ctx: &egui::Context) {
    #[allow(deprecated)]
    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |_ui| {
                #[cfg(feature = "native-dialog")]
                #[allow(unused_variables)]
                let ui = _ui;
                #[cfg(feature = "native-dialog")]
                {
                    if ui.button("📂 Open…").clicked() {
                        ui.close_menu();
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("CRDF (FlatBuffers)", &["crdf"])
                            .add_filter("N-Triples", &["nt"])
                            .pick_file()
                        {
                            let result = match path.extension().and_then(|e| e.to_str()) {
                                Some("crdf") => RdfGraph::read_flatbuffers_file(&path)
                                    .map_err(|e| e.to_string()),
                                _ => load_ntriples(&path),
                            };
                            match result {
                                Ok(graph) => {
                                    app.rdf_graph = graph;
                                    app.undo_manager.clear();
                                    app.interaction = Default::default();
                                    app.side_panel.pending_edge = None;
                                    app.side_panel.pending_edge_predicate.clear();
                                    app.node_positions.clear();
                                    app.ensure_node_positions();
                                    app.layout.running = true;
                                    app.layout.reset_temperature();
                                    app.side_panel.file_path = Some(path.clone());
                                    let time = ui.input(|i| i.time);
                                    app.set_status(format!("Loaded: {}", path.display()), time);
                                }
                                Err(e) => {
                                    let time = ui.input(|i| i.time);
                                    app.set_status(format!("Error: {e}"), time);
                                }
                            }
                        }
                    }
                    if ui.button("💾 Save").clicked() {
                        ui.close_menu();
                        let save_path = if let Some(ref existing) = app.side_panel.file_path {
                            Some(existing.clone())
                        } else {
                            rfd::FileDialog::new()
                                .add_filter("CRDF (FlatBuffers)", &["crdf"])
                                .add_filter("N-Triples", &["nt"])
                                .save_file()
                        };
                        if let Some(path) = save_path {
                            let format = format_for_path(&path);
                            match app.rdf_graph.write_rdf_file(&path, format) {
                                Ok(()) => {
                                    app.side_panel.file_path = Some(path.clone());
                                    let time = ui.input(|i| i.time);
                                    app.set_status(format!("Saved: {}", path.display()), time);
                                }
                                Err(e) => {
                                    let time = ui.input(|i| i.time);
                                    app.set_status(format!("Save error: {e}"), time);
                                }
                            }
                        }
                    }
                    if ui.button("💾 Save As…").clicked() {
                        ui.close_menu();
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("CRDF (FlatBuffers)", &["crdf"])
                            .add_filter("N-Triples", &["nt"])
                            .save_file()
                        {
                            let format = format_for_path(&path);
                            match app.rdf_graph.write_rdf_file(&path, format) {
                                Ok(()) => {
                                    app.side_panel.file_path = Some(path.clone());
                                    let time = ui.input(|i| i.time);
                                    app.set_status(format!("Saved: {}", path.display()), time);
                                }
                                Err(e) => {
                                    let time = ui.input(|i| i.time);
                                    app.set_status(format!("Save error: {e}"), time);
                                }
                            }
                        }
                    }
                } // #[cfg(feature = "native-dialog")]
            });

            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(app.undo_manager.can_undo(), egui::Button::new("↩ Undo"))
                    .clicked()
                {
                    ui.close_menu();
                    if app.undo() {
                        app.remove_orphan_positions();
                        let time = ui.input(|i| i.time);
                        app.set_status("Undo", time);
                    }
                }
                if ui
                    .add_enabled(app.undo_manager.can_redo(), egui::Button::new("↪ Redo"))
                    .clicked()
                {
                    ui.close_menu();
                    if app.redo() {
                        app.ensure_node_positions();
                        app.remove_orphan_positions();
                        let time = ui.input(|i| i.time);
                        app.set_status("Redo", time);
                    }
                }
            });

            ui.menu_button("View", |ui| {
                if ui
                    .button(if app.layout.running {
                        "⏸ Stop Layout"
                    } else {
                        "▶ Start Layout"
                    })
                    .clicked()
                {
                    ui.close_menu();
                    app.layout.running = !app.layout.running;
                    if app.layout.running {
                        app.layout.reset_temperature();
                    }
                }
                if ui.button("🔄 Reset Camera").clicked() {
                    ui.close_menu();
                    app.camera.offset = egui::Vec2::ZERO;
                    app.camera.zoom = 1.0;
                }
                ui.separator();
                if ui
                    .button(if app.dpo_panel.open {
                        "🔀 Hide DPO Panel"
                    } else {
                        "🔀 Show DPO Panel"
                    })
                    .clicked()
                {
                    ui.close_menu();
                    app.dpo_panel.open = !app.dpo_panel.open;
                }
            });

            // Show current file name in the menu bar
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(ref path) = app.side_panel.file_path
                    && let Some(name) = path.file_name().and_then(|n| n.to_str())
                {
                    ui.label(
                        egui::RichText::new(name)
                            .small()
                            .color(egui::Color32::from_gray(160)),
                    );
                }
            });
        });
    });
}

pub fn draw_side_panel(app: &mut CrdfEditorApp, ctx: &egui::Context) {
    #[allow(deprecated)]
    egui::SidePanel::left("side_panel")
        .min_width(280.0)
        .max_width(400.0)
        .show(ctx, |ui| {
            ui.heading("crdf Editor");
            ui.separator();

            // Graph stats
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Triples: {}  |  Nodes: {}",
                    app.rdf_graph.len(),
                    app.node_positions.len()
                ));
            });

            ui.separator();

            // Add triple form
            ui.collapsing("➕ Add Triple", |ui| {
                ui.label("Subject (IRI):");
                ui.text_edit_singleline(&mut app.side_panel.new_subject);

                ui.label("Predicate (IRI):");
                ui.text_edit_singleline(&mut app.side_panel.new_predicate);

                ui.horizontal(|ui| {
                    ui.label("Object:");
                    ui.checkbox(&mut app.side_panel.new_object_is_literal, "Literal");
                });
                ui.text_edit_singleline(&mut app.side_panel.new_object);

                if ui.button("Add Triple").clicked() {
                    let subject = RdfTerm::iri(&app.side_panel.new_subject);
                    let predicate = app.side_panel.new_predicate.clone();
                    let object = if app.side_panel.new_object_is_literal {
                        RdfTerm::literal(&app.side_panel.new_object)
                    } else {
                        RdfTerm::iri(&app.side_panel.new_object)
                    };

                    match app.add_triple(subject, &predicate, object) {
                        Ok(_) => {
                            let time = ui.input(|i| i.time);
                            app.set_status("Triple added", time);
                            app.side_panel.new_subject.clear();
                            app.side_panel.new_predicate.clear();
                            app.side_panel.new_object.clear();
                        }
                        Err(e) => {
                            let time = ui.input(|i| i.time);
                            app.set_status(format!("Error: {e}"), time);
                        }
                    }
                }
            });

            ui.separator();

            // Pending edge dialog (from drag-and-drop)
            if let Some((ref from, ref to)) = app.side_panel.pending_edge.clone() {
                ui.group(|ui| {
                    ui.heading("🔗 New Edge");
                    ui.label(format!("From: {}", short_label(from)));
                    ui.label(format!("To: {}", short_label(to)));
                    ui.label("Predicate (IRI):");
                    let response =
                        ui.text_edit_singleline(&mut app.side_panel.pending_edge_predicate);

                    ui.horizontal(|ui| {
                        let enter_pressed =
                            response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("✅ Create").clicked() || enter_pressed {
                            let predicate = app.side_panel.pending_edge_predicate.clone();
                            if !predicate.is_empty() {
                                match app.add_triple(from.clone(), &predicate, to.clone()) {
                                    Ok(_) => {
                                        let time = ui.input(|i| i.time);
                                        app.set_status("Edge created", time);
                                    }
                                    Err(e) => {
                                        let time = ui.input(|i| i.time);
                                        app.set_status(format!("Error: {e}"), time);
                                    }
                                }
                                app.side_panel.pending_edge = None;
                                app.side_panel.pending_edge_predicate.clear();
                            }
                        }
                        if ui.button("❌ Cancel").clicked() {
                            app.side_panel.pending_edge = None;
                            app.side_panel.pending_edge_predicate.clear();
                        }
                    });
                });
                ui.separator();
            }

            // Add standalone node dialog
            if app.side_panel.show_add_node {
                ui.group(|ui| {
                    ui.heading("➕ Add Node");
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut app.side_panel.new_node_type,
                            NodeType::Iri,
                            "IRI",
                        );
                        ui.selectable_value(
                            &mut app.side_panel.new_node_type,
                            NodeType::Literal,
                            "Literal",
                        );
                        ui.selectable_value(
                            &mut app.side_panel.new_node_type,
                            NodeType::BlankNode,
                            "BlankNode",
                        );
                    });
                    ui.label("Value:");
                    ui.text_edit_singleline(&mut app.side_panel.new_node_value);
                    ui.horizontal(|ui| {
                        if ui.button("✅ Add").clicked() {
                            let _term = match app.side_panel.new_node_type {
                                NodeType::Iri => RdfTerm::iri(&app.side_panel.new_node_value),
                                NodeType::Literal => {
                                    RdfTerm::literal(&app.side_panel.new_node_value)
                                }
                                NodeType::BlankNode => {
                                    RdfTerm::blank_node(&app.side_panel.new_node_value)
                                }
                            };
                            // Note: standalone nodes require at least one triple,
                            // so we just prepare the node value for the user
                            app.side_panel.new_subject = app.side_panel.new_node_value.clone();
                            app.side_panel.show_add_node = false;
                            app.side_panel.new_node_value.clear();
                            let time = ui.input(|i| i.time);
                            app.set_status("Set as subject — add a triple to place the node", time);
                        }
                        if ui.button("❌ Cancel").clicked() {
                            app.side_panel.show_add_node = false;
                            app.side_panel.new_node_value.clear();
                        }
                    });
                });
                ui.separator();
            }

            // Selected node inspector
            if let Some(ref selected) = app.interaction.selected_node.clone() {
                ui.group(|ui| {
                    ui.heading("🔍 Inspector");
                    ui.label(format!("Type: {}", node_type_str(selected)));
                    match selected {
                        RdfTerm::Iri(iri) => {
                            ui.label("IRI:");
                            let mut iri_display = iri.clone();
                            ui.text_edit_singleline(&mut iri_display);
                        }
                        RdfTerm::BlankNode(id) => {
                            ui.label(format!("ID: _:{id}"));
                        }
                        RdfTerm::Literal(lit) => {
                            ui.label(format!("Value: \"{}\"", lit.value()));
                            ui.label(format!("Datatype: {}", lit.datatype()));
                            if let Some(lang) = lit.language() {
                                ui.label(format!("Language: {lang}"));
                            }
                        }
                    }

                    ui.separator();
                    ui.label("Outgoing triples:");
                    let out_triples = app.rdf_graph.triples_for_subject(selected);
                    for t in &out_triples {
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "—[{}]→ {}",
                                short_predicate(&t.predicate),
                                short_label(&t.object)
                            ));
                            if ui.small_button("🗑").clicked() {
                                let _ = app.remove_triple(&t.subject, &t.predicate, &t.object);
                                app.remove_orphan_positions();
                            }
                        });
                    }

                    ui.label("Incoming triples:");
                    let in_triples = app.rdf_graph.triples_for_object(selected);
                    for t in &in_triples {
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "{} —[{}]→",
                                short_label(&t.subject),
                                short_predicate(&t.predicate),
                            ));
                            if ui.small_button("🗑").clicked() {
                                let _ = app.remove_triple(&t.subject, &t.predicate, &t.object);
                                app.remove_orphan_positions();
                            }
                        });
                    }
                });

                // Clear selected node if it became orphaned
                if !app.node_positions.contains_key(selected) {
                    app.interaction.selected_node = None;
                }
            }

            ui.separator();

            // Triple list
            ui.collapsing("📋 All Triples", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        let triples = app.rdf_graph.triples();
                        for t in &triples {
                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "{} —[{}]→ {}",
                                    short_label(&t.subject),
                                    short_predicate(&t.predicate),
                                    short_label(&t.object),
                                ));
                                if ui.small_button("🗑").clicked() {
                                    let _ = app.remove_triple(&t.subject, &t.predicate, &t.object);
                                    app.remove_orphan_positions();
                                }
                            });
                        }
                        if triples.is_empty() {
                            ui.label("(empty)");
                        }
                    });
            });
        });
}

pub fn draw_status_bar(app: &mut CrdfEditorApp, ctx: &egui::Context) {
    #[allow(deprecated)]
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            if let Some((ref msg, created_at)) = app.status_message {
                let now = ui.input(|i| i.time);
                if now - created_at < 5.0 {
                    ui.label(msg);
                } else {
                    app.status_message = None;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("Zoom: {:.0}%", app.camera.zoom * 100.0));
                if app.layout.running {
                    ui.label("⚡ Layout running");
                }
            });
        });
    });
}

pub fn draw_operations_panel(app: &mut CrdfEditorApp, ctx: &egui::Context) {
    #[allow(deprecated)]
    egui::SidePanel::right("operations_panel")
        .min_width(280.0)
        .max_width(400.0)
        .show(ctx, |ui| {
            ui.heading("⚙ CRDT State (2P2P-Graph)");
            ui.separator();

            let va = app.rdf_graph.all_vertices_added();
            let vr = app.rdf_graph.all_vertices_removed();
            let ea = app.rdf_graph.all_edges_added();
            let er = app.rdf_graph.all_edges_removed();

            ui.label(format!(
                "V_A: {}  V_R: {}  E_A: {}  E_R: {}",
                va.len(),
                vr.len(),
                ea.len(),
                er.len(),
            ));

            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    // V_A — Vertices Added
                    let va_header = format!("📥 V_A — Vertices Added ({})", va.len());
                    ui.collapsing(va_header, |ui| {
                        for v in va {
                            let id_short = &v.id.to_string()[..8];
                            let time = format_uuid_time(&v.id);
                            let removed = vr.iter().any(|r| r.add_vertex_id == v.id);
                            ui.horizontal(|ui| {
                                let label_text =
                                    format!("{} [{}] {}", id_short, time, short_term(&v.term));
                                if removed {
                                    ui.label(
                                        egui::RichText::new(format!("🪦 {label_text}"))
                                            .small()
                                            .strikethrough()
                                            .color(egui::Color32::from_gray(100)),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new(format!("● {label_text}"))
                                            .small()
                                            .color(egui::Color32::from_rgb(76, 175, 80)),
                                    );
                                }
                            });
                        }
                        if va.is_empty() {
                            ui.label("(empty)");
                        }
                    });

                    // V_R — Vertices Removed
                    let vr_header = format!("📤 V_R — Vertices Removed ({})", vr.len());
                    ui.collapsing(vr_header, |ui| {
                        for v in vr {
                            let id_short = &v.id.to_string()[..8];
                            let time = format_uuid_time(&v.id);
                            let ref_short = &v.add_vertex_id.to_string()[..8];
                            ui.label(
                                egui::RichText::new(format!(
                                    "● {id_short} [{time}] removes ← {ref_short}"
                                ))
                                .small()
                                .color(egui::Color32::from_rgb(244, 67, 54)),
                            );
                        }
                        if vr.is_empty() {
                            ui.label("(empty)");
                        }
                    });

                    ui.separator();

                    // E_A — Edges Added
                    let ea_header = format!("📥 E_A — Edges Added ({})", ea.len());
                    ui.collapsing(ea_header, |ui| {
                        for e in ea {
                            let id_short = &e.id.to_string()[..8];
                            let time = format_uuid_time(&e.id);
                            let src_short = &e.source.to_string()[..8];
                            let tgt_short = &e.target.to_string()[..8];
                            let pred = short_predicate(&e.predicate);
                            let removed = er.iter().any(|r| r.add_edge_id == e.id);
                            let text =
                                format!("{id_short} [{time}] {src_short}→{tgt_short} [{pred}]");
                            ui.horizontal(|ui| {
                                if removed {
                                    ui.label(
                                        egui::RichText::new(format!("🪦 {text}"))
                                            .small()
                                            .strikethrough()
                                            .color(egui::Color32::from_gray(100)),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new(format!("● {text}"))
                                            .small()
                                            .color(egui::Color32::from_rgb(66, 135, 245)),
                                    );
                                }
                            });
                        }
                        if ea.is_empty() {
                            ui.label("(empty)");
                        }
                    });

                    // E_R — Edges Removed
                    let er_header = format!("📤 E_R — Edges Removed ({})", er.len());
                    ui.collapsing(er_header, |ui| {
                        for e in er {
                            let id_short = &e.id.to_string()[..8];
                            let time = format_uuid_time(&e.id);
                            let ref_short = &e.add_edge_id.to_string()[..8];
                            ui.label(
                                egui::RichText::new(format!(
                                    "● {id_short} [{time}] removes ← {ref_short}"
                                ))
                                .small()
                                .color(egui::Color32::from_rgb(255, 152, 0)),
                            );
                        }
                        if er.is_empty() {
                            ui.label("(empty)");
                        }
                    });
                });
        });
}

fn format_uuid_time(id: &crdf::Uuid) -> String {
    if let Some(ts) = id.get_timestamp() {
        let (secs, _nanos) = ts.to_unix();
        // Convert Unix seconds to YYYY-MM-DD HH:MM:SS (UTC)
        const SECS_PER_DAY: u64 = 86400;
        let days = secs / SECS_PER_DAY;
        let rem = secs % SECS_PER_DAY;
        let hours = rem / 3600;
        let minutes = (rem % 3600) / 60;
        let seconds = rem % 60;

        let mut year = 1970i64;
        let mut remaining_days = days as i64;
        loop {
            let diy = if is_leap_year(year) { 366 } else { 365 };
            if remaining_days < diy {
                break;
            }
            remaining_days -= diy;
            year += 1;
        }
        let month_days: [i64; 12] = if is_leap_year(year) {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };
        let mut month = 1;
        for &md in &month_days {
            if remaining_days < md {
                break;
            }
            remaining_days -= md;
            month += 1;
        }
        let day = remaining_days + 1;
        format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}:{seconds:02}")
    } else {
        "—".to_string()
    }
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn short_term(term: &RdfTerm) -> String {
    match term {
        RdfTerm::Iri(iri) => {
            if let Some(hash) = iri.rfind('#') {
                iri[hash + 1..].to_string()
            } else if let Some(slash) = iri.rfind('/') {
                iri[slash + 1..].to_string()
            } else if iri.len() > 30 {
                format!("{}…", &iri[..27])
            } else {
                iri.clone()
            }
        }
        RdfTerm::BlankNode(id) => format!("_:{id}"),
        RdfTerm::Literal(lit) => {
            let val = lit.value();
            if val.len() > 20 {
                format!("\"{}…\"", &val[..17])
            } else {
                format!("\"{val}\"")
            }
        }
    }
}

fn short_label(term: &RdfTerm) -> String {
    match term {
        RdfTerm::Iri(iri) => {
            if let Some(hash) = iri.rfind('#') {
                iri[hash + 1..].to_string()
            } else if let Some(slash) = iri.rfind('/') {
                iri[slash + 1..].to_string()
            } else {
                iri.clone()
            }
        }
        RdfTerm::BlankNode(id) => format!("_:{id}"),
        RdfTerm::Literal(lit) => {
            let val = lit.value();
            if val.len() > 20 {
                format!("\"{}...\"", &val[..17])
            } else {
                format!("\"{val}\"")
            }
        }
    }
}

fn short_predicate(predicate: &str) -> String {
    if let Some(hash) = predicate.rfind('#') {
        predicate[hash + 1..].to_string()
    } else if let Some(slash) = predicate.rfind('/') {
        predicate[slash + 1..].to_string()
    } else {
        predicate.to_string()
    }
}

fn node_type_str(term: &RdfTerm) -> &'static str {
    match term {
        RdfTerm::Iri(_) => "IRI",
        RdfTerm::BlankNode(_) => "BlankNode",
        RdfTerm::Literal(_) => "Literal",
    }
}

#[cfg(feature = "native-dialog")]
fn format_for_path(path: &std::path::Path) -> RdfFileFormat {
    match path.extension().and_then(|e| e.to_str()) {
        Some("crdf") => RdfFileFormat::FlatBuffers,
        _ => RdfFileFormat::NTriples,
    }
}

#[cfg(feature = "native-dialog")]
fn load_ntriples(path: &std::path::Path) -> Result<RdfGraph, String> {
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut graph = RdfGraph::new();

    for (line_num, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        match parse_ntriple_line(line) {
            Some((subject, predicate, object)) => {
                graph
                    .add_triple(subject, &predicate, object)
                    .map_err(|e| format!("Line {}: {e}", line_num + 1))?;
            }
            None => {
                return Err(format!("Line {}: Parse error: {line}", line_num + 1));
            }
        }
    }

    Ok(graph)
}

#[cfg(feature = "native-dialog")]
fn parse_ntriple_line(line: &str) -> Option<(RdfTerm, String, RdfTerm)> {
    let line = line.trim();
    let line = line.strip_suffix('.')?;
    let line = line.trim();

    let (subject, rest) = parse_term(line)?;
    let rest = rest.trim_start();
    let (predicate_term, rest) = parse_iri(rest)?;
    let rest = rest.trim_start();
    let (object, _rest) = parse_term(rest)?;

    let subject_rdf = match subject {
        ParsedTerm::Iri(iri) => RdfTerm::iri(iri),
        ParsedTerm::BlankNode(id) => RdfTerm::blank_node(id),
        ParsedTerm::Literal(_, _, _) => return None,
    };

    let object_rdf = match object {
        ParsedTerm::Iri(iri) => RdfTerm::iri(iri),
        ParsedTerm::BlankNode(id) => RdfTerm::blank_node(id),
        ParsedTerm::Literal(val, dt, lang) => {
            let mut lit = Literal::new(val);
            if let Some(lang) = lang {
                lit = lit.with_language(lang).ok()?;
            } else if let Some(dt) = dt {
                lit = lit.with_datatype(dt).ok()?;
            }
            RdfTerm::from(lit)
        }
    };

    Some((subject_rdf, predicate_term, object_rdf))
}

#[cfg(feature = "native-dialog")]
enum ParsedTerm {
    Iri(String),
    BlankNode(String),
    Literal(String, Option<String>, Option<String>),
}

#[cfg(feature = "native-dialog")]
fn parse_term(input: &str) -> Option<(ParsedTerm, &str)> {
    let input = input.trim_start();
    if input.starts_with('<') {
        let (iri, rest) = parse_iri(input)?;
        Some((ParsedTerm::Iri(iri), rest))
    } else if let Some(rest) = input.strip_prefix("_:") {
        let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        let id = &rest[..end];
        Some((ParsedTerm::BlankNode(id.to_string()), &rest[end..]))
    } else if input.starts_with('"') {
        parse_literal(input)
    } else {
        None
    }
}

#[cfg(feature = "native-dialog")]
fn parse_iri(input: &str) -> Option<(String, &str)> {
    if !input.starts_with('<') {
        return None;
    }
    let end = input.find('>')?;
    let iri = &input[1..end];
    Some((iri.to_string(), &input[end + 1..]))
}

#[cfg(feature = "native-dialog")]
fn parse_literal(input: &str) -> Option<(ParsedTerm, &str)> {
    if !input.starts_with('"') {
        return None;
    }

    let mut chars = input[1..].char_indices();
    let mut value = String::new();
    let mut end_pos = 0;

    while let Some((i, ch)) = chars.next() {
        match ch {
            '\\' => {
                if let Some((_, escaped)) = chars.next() {
                    match escaped {
                        'n' => value.push('\n'),
                        't' => value.push('\t'),
                        'r' => value.push('\r'),
                        '"' => value.push('"'),
                        '\\' => value.push('\\'),
                        _ => {
                            value.push('\\');
                            value.push(escaped);
                        }
                    }
                }
            }
            '"' => {
                end_pos = i + 2; // +1 for the opening quote offset, +1 for the closing quote
                break;
            }
            _ => value.push(ch),
        }
    }

    let rest = &input[end_pos..];

    if let Some(rest) = rest.strip_prefix("^^") {
        let (dt_iri, rest) = parse_iri(rest)?;
        Some((ParsedTerm::Literal(value, Some(dt_iri), None), rest))
    } else if let Some(rest) = rest.strip_prefix('@') {
        let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        let lang = &rest[..end];
        Some((
            ParsedTerm::Literal(value, None, Some(lang.to_string())),
            &rest[end..],
        ))
    } else {
        Some((ParsedTerm::Literal(value, None, None), rest))
    }
}
