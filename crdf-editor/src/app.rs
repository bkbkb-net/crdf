use std::collections::HashMap;

use crdf::{RdfGraph, RdfTerm, UndoManager};
use egui::Pos2;

use crate::canvas::Camera;
use crate::dpo_ui::DpoPanel;
use crate::graph_view::Interaction;
use crate::layout::ForceLayout;
use crate::ui::SidePanel;

pub struct CrdfEditorApp {
    pub rdf_graph: RdfGraph,
    pub undo_manager: UndoManager,
    pub node_positions: HashMap<RdfTerm, Pos2>,
    pub camera: Camera,
    pub interaction: Interaction,
    pub layout: ForceLayout,
    pub side_panel: SidePanel,
    pub dpo_panel: DpoPanel,
    pub status_message: Option<(String, f64)>,
}

impl Default for CrdfEditorApp {
    fn default() -> Self {
        Self::new()
    }
}

impl CrdfEditorApp {
    pub fn new() -> Self {
        Self {
            rdf_graph: RdfGraph::new(),
            undo_manager: UndoManager::new(),
            node_positions: HashMap::new(),
            camera: Camera::default(),
            interaction: Interaction::default(),
            layout: ForceLayout::new(),
            side_panel: SidePanel::default(),
            dpo_panel: DpoPanel::default(),
            status_message: None,
        }
    }

    /// Seed initial positions for any node we haven't placed yet, using
    /// a typed-lane hierarchical layout:
    ///
    /// * Nodes are bucketed by `rdf:type` (literals/blank nodes get their
    ///   own buckets).
    /// * Buckets with many nodes (literals are the usual offender) are
    ///   wrapped into a grid of `MAX_ROWS_PER_COLUMN` rows × N sub-
    ///   columns so they don't render as a single 100-tall column.
    /// * Small buckets (≤ `SMALL_CLUSTER_THRESHOLD` nodes) are folded
    ///   into a shared "misc" block packed in the same wrapping grid,
    ///   so a project with 8 distinct 1-node types doesn't render as a
    ///   single row of disconnected dots.
    pub fn ensure_node_positions(&mut self) {
        let subjects = self.rdf_graph.subjects();
        let objects = self.rdf_graph.objects();

        let mut all_terms: Vec<RdfTerm> = Vec::new();
        for s in &subjects {
            if !self.node_positions.contains_key(*s) {
                all_terms.push((*s).clone());
            }
        }
        for o in &objects {
            if !self.node_positions.contains_key(*o) && !all_terms.contains(o) {
                all_terms.push((*o).clone());
            }
        }
        if all_terms.is_empty() {
            return;
        }

        const COLUMN_SPACING: f32 = 240.0;
        const ROW_SPACING: f32 = 64.0;
        // Caps a cluster's column height so an all-literals project
        // doesn't paint a 100-row tower nobody can scroll.
        const MAX_ROWS_PER_COLUMN: usize = 12;
        // Extra horizontal padding between two distinct clusters so the
        // type boundary stays visible after wrapping.
        const CLUSTER_GAP: f32 = 80.0;
        // Buckets at or below this size are merged into the "misc"
        // block instead of each claiming their own column.
        const SMALL_CLUSTER_THRESHOLD: usize = 2;

        // ── Step 1: bucket by rdf:type / literal / blank / namespace ──
        let mut clusters: std::collections::BTreeMap<String, Vec<RdfTerm>> =
            std::collections::BTreeMap::new();
        for term in all_terms {
            let bucket = cluster_key_for_term(&term, &self.rdf_graph);
            clusters.entry(bucket).or_default().push(term);
        }
        for nodes in clusters.values_mut() {
            nodes.sort_by_key(sort_key_for_term);
        }

        // ── Step 2: separate big clusters from a shared "misc" pool ──
        let mut blocks: Vec<Vec<RdfTerm>> = Vec::new();
        let mut misc: Vec<RdfTerm> = Vec::new();
        for (_key, nodes) in clusters {
            if nodes.len() > SMALL_CLUSTER_THRESHOLD {
                blocks.push(nodes);
            } else {
                misc.extend(nodes);
            }
        }
        if !misc.is_empty() {
            blocks.push(misc);
        }
        if blocks.is_empty() {
            return;
        }

        // ── Step 3: each block wraps to ceil(n / MAX_ROWS) sub-columns ──
        let block_widths: Vec<usize> = blocks
            .iter()
            .map(|nodes| nodes.len().div_ceil(MAX_ROWS_PER_COLUMN).max(1))
            .collect();
        let total_sub_columns: usize = block_widths.iter().sum();
        let gap_count = blocks.len().saturating_sub(1) as f32;
        let total_width =
            (total_sub_columns as f32 - 1.0).max(0.0) * COLUMN_SPACING + gap_count * CLUSTER_GAP;
        let mut current_x = -total_width * 0.5;

        // ── Step 4: place each block left-to-right ──
        for (nodes, sub_cols) in blocks.into_iter().zip(block_widths) {
            let nodes_len = nodes.len();
            for (i, term) in nodes.into_iter().enumerate() {
                let sub_col = i / MAX_ROWS_PER_COLUMN;
                let row = i % MAX_ROWS_PER_COLUMN;
                let x = current_x + sub_col as f32 * COLUMN_SPACING;
                let rows_in_sub_col = if sub_col + 1 == sub_cols {
                    nodes_len - sub_col * MAX_ROWS_PER_COLUMN
                } else {
                    MAX_ROWS_PER_COLUMN
                };
                let top_y = -((rows_in_sub_col as f32 - 1.0) * ROW_SPACING * 0.5);
                let y = top_y + row as f32 * ROW_SPACING;
                self.node_positions.insert(term, Pos2::new(x, y));
            }
            current_x += sub_cols as f32 * COLUMN_SPACING + CLUSTER_GAP;
        }
    }

    pub fn set_external_graph(&mut self, graph: RdfGraph) {
        self.rdf_graph = graph;
        self.undo_manager.clear();
        self.side_panel.pending_edge = None;
        self.side_panel.pending_edge_predicate.clear();
        self.side_panel.file_path = None;
        self.remove_orphan_positions();
        self.ensure_node_positions();
    }

    pub fn show_graph_view(&mut self, ui: &mut egui::Ui) {
        self.ensure_node_positions();

        if self.layout.running {
            let triples = self.rdf_graph.triples();
            self.layout.step(&mut self.node_positions, &triples);
            ui.ctx().request_repaint();
        }

        crate::graph_view::draw_graph_in_ui(self, ui);
    }

    pub fn set_status(&mut self, msg: impl Into<String>, time: f64) {
        self.status_message = Some((msg.into(), time));
    }

    /// Adds a triple to the graph (tracked by undo manager).
    pub fn add_triple(
        &mut self,
        subject: RdfTerm,
        predicate: &str,
        object: RdfTerm,
    ) -> Result<(), crdf::CrdfError> {
        self.undo_manager
            .add_triple(&mut self.rdf_graph, subject, predicate, object)?;
        Ok(())
    }

    /// Removes a triple from the graph (tracked by undo manager).
    pub fn remove_triple(
        &mut self,
        subject: &RdfTerm,
        predicate: &str,
        object: &RdfTerm,
    ) -> Result<(), crdf::CrdfError> {
        self.undo_manager
            .remove_triple(&mut self.rdf_graph, subject, predicate, object)?;
        Ok(())
    }

    /// Undoes the last action. Returns true if an action was undone.
    pub fn undo(&mut self) -> bool {
        matches!(self.undo_manager.undo(&mut self.rdf_graph), Ok(Some(_)))
    }

    /// Redoes the last undone action. Returns true if an action was redone.
    pub fn redo(&mut self) -> bool {
        matches!(self.undo_manager.redo(&mut self.rdf_graph), Ok(Some(_)))
    }

    pub fn remove_orphan_positions(&mut self) {
        let subjects: std::collections::HashSet<_> =
            self.rdf_graph.subjects().into_iter().cloned().collect();
        let objects: std::collections::HashSet<_> =
            self.rdf_graph.objects().into_iter().cloned().collect();
        self.node_positions
            .retain(|term, _| subjects.contains(term) || objects.contains(term));
    }
}

/// Bucket key driving the hierarchical seed layout. Resources cluster by
/// their `rdf:type` value (so all `Person`s share a column, all
/// `Document`s share another, …); literals and blank nodes get their own clusters
/// so they don't clutter the typed columns.
fn cluster_key_for_term(term: &RdfTerm, graph: &RdfGraph) -> String {
    match term {
        RdfTerm::Literal(_) => "~literal".to_string(),
        RdfTerm::BlankNode(_) => "~blank".to_string(),
        RdfTerm::Iri(iri) => {
            // Find the rdf:type object for this subject, if any. The
            // type IRI's local name becomes the column label.
            for triple in graph.triples() {
                if let RdfTerm::Iri(subj) = &triple.subject
                    && subj == iri
                    && (triple.predicate.as_str().ends_with("#type")
                        || triple.predicate.as_str().ends_with("/type"))
                    && let RdfTerm::Iri(obj) = &triple.object
                {
                    return local_name(obj).to_string();
                }
            }
            // Untyped IRIs: fall back to the URI path prefix so resources
            // from the same namespace still cluster together.
            namespace_prefix(iri)
        }
    }
}

/// Stable secondary key used for ordering nodes inside a cluster.
fn sort_key_for_term(term: &RdfTerm) -> String {
    match term {
        RdfTerm::Iri(iri) => iri.clone(),
        RdfTerm::BlankNode(id) => format!("_:{id}"),
        RdfTerm::Literal(lit) => format!("\"{}\"", lit.value()),
    }
}

fn local_name(iri: &str) -> &str {
    if let Some(idx) = iri.rfind(['#', '/']) {
        &iri[idx + 1..]
    } else {
        iri
    }
}

fn namespace_prefix(iri: &str) -> String {
    if let Some(idx) = iri.rfind(['#', '/']) {
        iri[..=idx].to_string()
    } else {
        iri.to_string()
    }
}

impl eframe::App for CrdfEditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.ensure_node_positions();

        if self.layout.running {
            let triples = self.rdf_graph.triples();
            self.layout.step(&mut self.node_positions, &triples);
            ctx.request_repaint();
        }

        crate::ui::draw_menu_bar(self, ui);
        crate::ui::draw_side_panel(self, ui);
        crate::ui::draw_operations_panel(self, ui);
        crate::ui::draw_status_bar(self, ui);
        crate::graph_view::draw_graph(self, ui);
        crate::dpo_ui::draw_dpo_panel(self, &ctx);
    }
}
