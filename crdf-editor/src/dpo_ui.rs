use crdf::RdfTerm;
use crdf_dpo::{Binding, DpoRule, PatternPredicate, PatternTerm, PatternTriple};

use crate::app::CrdfEditorApp;

/// Which section of a DPO rule we are adding a triple to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PatternSection {
    Lhs,
    Interface,
    Rhs,
}

/// Term kind selector for UI input fields.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum TermKind {
    #[default]
    Iri,
    Literal,
}

/// State for the "add pattern triple" form.
#[derive(Default)]
struct TripleForm {
    subj_kind: TermKind,
    subj_value: String,
    pred_kind: TermKind,
    pred_value: String,
    obj_kind: TermKind,
    obj_value: String,
}

impl TripleForm {
    fn clear(&mut self) {
        self.subj_value.clear();
        self.pred_value.clear();
        self.obj_value.clear();
    }

    fn to_pattern_triple(&self) -> Option<PatternTriple> {
        if self.subj_value.is_empty() || self.pred_value.is_empty() || self.obj_value.is_empty() {
            return None;
        }
        let subject = match self.subj_kind {
            TermKind::Iri => PatternTerm::Concrete(RdfTerm::iri(&self.subj_value)),
            TermKind::Literal => return None, // subject cannot be literal
        };
        let predicate = match self.pred_kind {
            TermKind::Iri => PatternPredicate::Concrete(self.pred_value.clone()),
            TermKind::Literal => return None, // predicate cannot be literal
        };
        let object = match self.obj_kind {
            TermKind::Iri => PatternTerm::Concrete(RdfTerm::iri(&self.obj_value)),
            TermKind::Literal => PatternTerm::Concrete(RdfTerm::literal(&self.obj_value)),
        };
        Some(PatternTriple::new(subject, predicate, object))
    }
}

/// Cached preview result from `DpoRule::preview`.
struct PreviewResult {
    to_delete: Vec<crdf::Triple>,
    to_add: Vec<crdf::Triple>,
    binding: Binding,
}

/// UI state for the DPO rewriting panel.
#[derive(Default)]
pub struct DpoPanel {
    pub open: bool,
    rules: Vec<DpoRule>,
    selected: Option<usize>,
    new_rule_name: String,
    triple_form: TripleForm,
    adding_to: Option<PatternSection>,
    match_count: Option<usize>,
    preview: Option<PreviewResult>,
    last_error: Option<String>,
}

/// Draw the DPO rewriting window (floating, toggled from menu bar).
pub fn draw_dpo_panel(app: &mut CrdfEditorApp, ctx: &egui::Context) {
    if !app.dpo_panel.open {
        return;
    }

    let mut open = app.dpo_panel.open;
    egui::Window::new("🔀 DPO Graph Rewriting")
        .open(&mut open)
        .default_width(520.0)
        .default_height(440.0)
        .resizable(true)
        .show(ctx, |ui| {
            draw_dpo_content(app, ui);
        });
    app.dpo_panel.open = open;
}

fn draw_dpo_content(app: &mut CrdfEditorApp, ui: &mut egui::Ui) {
    // ── Rule list & creation ──
    ui.horizontal(|ui| {
        ui.heading("Rules");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("➕ New Rule").clicked() && !app.dpo_panel.new_rule_name.is_empty() {
                let rule = DpoRule::new(
                    app.dpo_panel.new_rule_name.clone(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                );
                app.dpo_panel.rules.push(rule);
                app.dpo_panel.selected = Some(app.dpo_panel.rules.len() - 1);
                app.dpo_panel.new_rule_name.clear();
                app.dpo_panel.invalidate_cache();
            }
            ui.text_edit_singleline(&mut app.dpo_panel.new_rule_name)
                .on_hover_text("Enter new rule name");
        });
    });

    if app.dpo_panel.rules.is_empty() {
        ui.label("No rules defined. Enter a name and click ➕ New Rule.");
        return;
    }

    // Rule selector
    ui.horizontal_wrapped(|ui| {
        let rules_snapshot: Vec<(usize, String)> = app
            .dpo_panel
            .rules
            .iter()
            .enumerate()
            .map(|(i, r)| (i, r.summary()))
            .collect();
        for (i, summary) in &rules_snapshot {
            let selected = app.dpo_panel.selected == Some(*i);
            if ui.selectable_label(selected, summary).clicked() {
                app.dpo_panel.selected = Some(*i);
                app.dpo_panel.invalidate_cache();
            }
        }
    });

    // Delete button
    if let Some(sel) = app.dpo_panel.selected {
        ui.horizontal(|ui| {
            if ui.button("🗑 Delete Rule").clicked() {
                app.dpo_panel.rules.remove(sel);
                app.dpo_panel.selected = if app.dpo_panel.rules.is_empty() {
                    None
                } else {
                    Some(sel.min(app.dpo_panel.rules.len() - 1))
                };
                app.dpo_panel.invalidate_cache();
            }
        });
    }

    ui.separator();

    // ── Rule editor ──
    let Some(sel) = app.dpo_panel.selected else {
        ui.label("Select a rule to edit.");
        return;
    };
    if sel >= app.dpo_panel.rules.len() {
        app.dpo_panel.selected = None;
        return;
    }

    // Name
    ui.horizontal(|ui| {
        ui.label("Name:");
        ui.text_edit_singleline(&mut app.dpo_panel.rules[sel].name);
    });

    // Validation status
    let validation = app.dpo_panel.rules[sel].validate();
    let unbound = app.dpo_panel.rules[sel].check_unbound_rhs_variables();
    match (&validation, &unbound) {
        (Ok(()), Ok(())) => {
            ui.colored_label(egui::Color32::from_rgb(76, 175, 80), "✓ Rule valid");
        }
        _ => {
            if let Err(e) = &validation {
                ui.colored_label(egui::Color32::from_rgb(244, 67, 54), format!("✗ {e}"));
            }
            if let Err(e) = &unbound {
                ui.colored_label(egui::Color32::from_rgb(244, 67, 54), format!("✗ {e}"));
            }
        }
    }

    // Variables overview
    let vars = app.dpo_panel.rules[sel].variables();
    if !vars.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label("Variables:");
            for v in &vars {
                ui.code(format!("?{v}"));
            }
        });
    }

    ui.separator();

    // ── L / K / R sections ──
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .max_height(200.0)
        .show(ui, |ui| {
            draw_pattern_section(
                ui,
                "L (LHS — match pattern)",
                &app.dpo_panel.rules[sel].lhs.clone(),
                PatternSection::Lhs,
                &mut app.dpo_panel,
                sel,
            );

            draw_pattern_section(
                ui,
                "K (Interface — preserved)",
                &app.dpo_panel.rules[sel].interface.clone(),
                PatternSection::Interface,
                &mut app.dpo_panel,
                sel,
            );

            draw_pattern_section(
                ui,
                "R (RHS — replacement)",
                &app.dpo_panel.rules[sel].rhs.clone(),
                PatternSection::Rhs,
                &mut app.dpo_panel,
                sel,
            );
        });

    ui.separator();

    // ── Add triple form ──
    if let Some(section) = app.dpo_panel.adding_to {
        let section_label = match section {
            PatternSection::Lhs => "L",
            PatternSection::Interface => "K",
            PatternSection::Rhs => "R",
        };
        ui.group(|ui| {
            ui.heading(format!("➕ Add Triple to {section_label}"));
            draw_triple_form(ui, &mut app.dpo_panel.triple_form);

            ui.horizontal(|ui| {
                if ui.button("✅ Add").clicked()
                    && let Some(pt) = app.dpo_panel.triple_form.to_pattern_triple()
                {
                    match section {
                        PatternSection::Lhs => app.dpo_panel.rules[sel].push_lhs(pt),
                        PatternSection::Interface => app.dpo_panel.rules[sel].push_interface(pt),
                        PatternSection::Rhs => app.dpo_panel.rules[sel].push_rhs(pt),
                    }
                    app.dpo_panel.triple_form.clear();
                    app.dpo_panel.adding_to = None;
                    app.dpo_panel.invalidate_cache();
                }
                if ui.button("❌ Cancel").clicked() {
                    app.dpo_panel.adding_to = None;
                    app.dpo_panel.triple_form.clear();
                }
            });
        });
    }

    ui.separator();

    // ── Actions ──
    ui.horizontal_wrapped(|ui| {
        // Find matches
        if ui.button("🔍 Find Matches").clicked() {
            let count = app.dpo_panel.rules[sel].find_matches(&app.rdf_graph).len();
            app.dpo_panel.match_count = Some(count);
            let time = ui.input(|i| i.time);
            app.set_status(format!("DPO: {count} match(es) found"), time);
        }

        // Preview
        if ui.button("👁 Preview").clicked() {
            match app.dpo_panel.rules[sel].preview(&app.rdf_graph) {
                Ok((to_delete, to_add, binding)) => {
                    app.dpo_panel.preview = Some(PreviewResult {
                        to_delete,
                        to_add,
                        binding,
                    });
                    app.dpo_panel.last_error = None;
                }
                Err(e) => {
                    app.dpo_panel.last_error = Some(e.to_string());
                    app.dpo_panel.preview = None;
                }
            }
        }

        // Apply first match
        if ui.button("▶ Apply").clicked() {
            match app.dpo_panel.rules[sel].apply(&mut app.rdf_graph) {
                Ok(result) => {
                    let ops = result.operations.len();
                    app.dpo_panel.invalidate_cache();
                    app.ensure_node_positions();
                    app.remove_orphan_positions();
                    let time = ui.input(|i| i.time);
                    app.set_status(format!("DPO applied: {ops} operation(s)"), time);
                }
                Err(e) => {
                    app.dpo_panel.last_error = Some(e.to_string());
                    let time = ui.input(|i| i.time);
                    app.set_status(format!("DPO error: {e}"), time);
                }
            }
        }

        // Apply all
        if ui.button("▶▶ Apply All").clicked() {
            match app.dpo_panel.rules[sel].apply_all(&mut app.rdf_graph) {
                Ok(results) => {
                    let total_ops: usize = results.iter().map(|r| r.operations.len()).sum();
                    app.dpo_panel.invalidate_cache();
                    app.ensure_node_positions();
                    app.remove_orphan_positions();
                    let time = ui.input(|i| i.time);
                    app.set_status(
                        format!(
                            "DPO applied to {} match(es): {total_ops} operation(s)",
                            results.len()
                        ),
                        time,
                    );
                }
                Err(e) => {
                    app.dpo_panel.last_error = Some(e.to_string());
                    let time = ui.input(|i| i.time);
                    app.set_status(format!("DPO error: {e}"), time);
                }
            }
        }
    });

    // Match count
    if let Some(count) = app.dpo_panel.match_count {
        ui.label(format!("Matches: {count}"));
    }

    // Error display
    if let Some(ref err) = app.dpo_panel.last_error {
        ui.colored_label(
            egui::Color32::from_rgb(244, 67, 54),
            format!("Error: {err}"),
        );
    }

    // Preview display
    if let Some(ref preview) = app.dpo_panel.preview {
        ui.group(|ui| {
            ui.heading("Preview");

            if !preview.to_delete.is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(244, 67, 54),
                    format!("Remove {} triple(s):", preview.to_delete.len()),
                );
                for t in &preview.to_delete {
                    ui.horizontal(|ui| {
                        ui.label("  ");
                        ui.colored_label(
                            egui::Color32::from_rgb(244, 67, 54),
                            format!(
                                "− {} <{}> {}",
                                short_term(&t.subject),
                                short_pred(&t.predicate),
                                short_term(&t.object)
                            ),
                        );
                    });
                }
            }

            if !preview.to_add.is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(76, 175, 80),
                    format!("Add {} triple(s):", preview.to_add.len()),
                );
                for t in &preview.to_add {
                    ui.horizontal(|ui| {
                        ui.label("  ");
                        ui.colored_label(
                            egui::Color32::from_rgb(76, 175, 80),
                            format!(
                                "+ {} <{}> {}",
                                short_term(&t.subject),
                                short_pred(&t.predicate),
                                short_term(&t.object)
                            ),
                        );
                    });
                }
            }

            // Binding
            if !preview.binding.terms.is_empty() || !preview.binding.predicates.is_empty() {
                ui.collapsing("Binding", |ui| {
                    for (var, val) in &preview.binding.terms {
                        ui.label(format!("?{var} = {val}"));
                    }
                    for (var, val) in &preview.binding.predicates {
                        ui.label(format!("?{var} = <{val}>"));
                    }
                });
            }
        });
    }
}

fn draw_pattern_section(
    ui: &mut egui::Ui,
    heading: &str,
    triples: &[PatternTriple],
    section: PatternSection,
    panel: &mut DpoPanel,
    rule_idx: usize,
) {
    ui.collapsing(format!("{heading} ({})", triples.len()), |ui| {
        // Triples
        let mut triple_to_remove = None;
        for (i, pt) in triples.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{pt}"));
                if ui.small_button("🗑").clicked() {
                    triple_to_remove = Some(i);
                }
            });
        }

        if let Some(idx) = triple_to_remove {
            match section {
                PatternSection::Lhs => {
                    panel.rules[rule_idx].remove_lhs(idx);
                }
                PatternSection::Interface => {
                    panel.rules[rule_idx].remove_interface(idx);
                }
                PatternSection::Rhs => {
                    panel.rules[rule_idx].remove_rhs(idx);
                }
            }
            panel.invalidate_cache();
        }

        if triples.is_empty() {
            ui.label("(empty)");
        }

        if ui.button("➕ Add Triple").clicked() {
            panel.adding_to = Some(section);
            panel.triple_form.clear();
        }
    });
}

fn draw_triple_form(ui: &mut egui::Ui, form: &mut TripleForm) {
    // Subject
    ui.horizontal(|ui| {
        ui.label("Subject:");
        ui.selectable_value(&mut form.subj_kind, TermKind::Iri, "IRI");
    });
    ui.text_edit_singleline(&mut form.subj_value);

    // Predicate
    ui.horizontal(|ui| {
        ui.label("Predicate:");
        ui.selectable_value(&mut form.pred_kind, TermKind::Iri, "IRI");
    });
    ui.text_edit_singleline(&mut form.pred_value);

    // Object
    ui.horizontal(|ui| {
        ui.label("Object:");
        ui.selectable_value(&mut form.obj_kind, TermKind::Iri, "IRI");
        ui.selectable_value(&mut form.obj_kind, TermKind::Literal, "Literal");
    });
    ui.text_edit_singleline(&mut form.obj_value);
}

impl DpoPanel {
    fn invalidate_cache(&mut self) {
        self.match_count = None;
        self.preview = None;
        self.last_error = None;
    }
}

fn short_term(term: &RdfTerm) -> String {
    match term {
        RdfTerm::Iri(iri) => {
            if let Some(frag) = iri.rfind('#').or_else(|| iri.rfind('/')) {
                iri[frag + 1..].to_string()
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

fn short_pred(pred: &str) -> String {
    if let Some(frag) = pred.rfind('#').or_else(|| pred.rfind('/')) {
        pred[frag + 1..].to_string()
    } else {
        pred.to_string()
    }
}
