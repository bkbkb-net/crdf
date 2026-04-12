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

        for (i, term) in all_terms.into_iter().enumerate() {
            let angle = i as f32 * 2.4;
            let radius = 150.0 + (i as f32 * 30.0);
            let pos = Pos2::new(angle.cos() * radius, angle.sin() * radius);
            self.node_positions.insert(term, pos);
        }
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
        match self.undo_manager.undo(&mut self.rdf_graph) {
            Ok(Some(_)) => true,
            _ => false,
        }
    }

    /// Redoes the last undone action. Returns true if an action was redone.
    pub fn redo(&mut self) -> bool {
        match self.undo_manager.redo(&mut self.rdf_graph) {
            Ok(Some(_)) => true,
            _ => false,
        }
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

impl eframe::App for CrdfEditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ensure_node_positions();

        if self.layout.running {
            let triples = self.rdf_graph.triples();
            self.layout.step(&mut self.node_positions, &triples);
            ctx.request_repaint();
        }

        crate::ui::draw_menu_bar(self, ctx);
        crate::ui::draw_side_panel(self, ctx);
        crate::dpo_ui::draw_dpo_panel(self, ctx);
        crate::ui::draw_operations_panel(self, ctx);
        crate::ui::draw_status_bar(self, ctx);
        crate::graph_view::draw_graph(self, ctx);
    }

    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // We override update() for custom panel layout
    }
}
