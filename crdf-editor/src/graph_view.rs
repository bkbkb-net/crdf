use crdf::RdfTerm;
use egui::{Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect, Stroke, Vec2};

use crate::app::CrdfEditorApp;

#[derive(Default)]
pub struct Interaction {
    pub selected_node: Option<RdfTerm>,
    pub dragging_node: Option<RdfTerm>,
    pub drag_offset: Vec2,
    pub creating_edge_from: Option<RdfTerm>,
    pub edge_drag_target: Option<Pos2>,
    pub hovered_node: Option<RdfTerm>,
}

const NODE_MIN_WIDTH: f32 = 120.0;
const NODE_HEIGHT: f32 = 40.0;
const NODE_ROUNDING: f32 = 8.0;
const PORT_RADIUS: f32 = 6.0;
const EDGE_THICKNESS: f32 = 2.0;
const ARROW_SIZE: f32 = 10.0;

fn node_color(term: &RdfTerm) -> Color32 {
    match term {
        RdfTerm::Iri(_) => Color32::from_rgb(66, 135, 245),
        RdfTerm::BlankNode(_) => Color32::from_rgb(158, 158, 158),
        RdfTerm::Literal(_) => Color32::from_rgb(76, 175, 80),
    }
}

fn node_label(term: &RdfTerm) -> String {
    match term {
        RdfTerm::Iri(iri) => {
            // Show local name or last path segment
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

fn predicate_label(predicate: &str) -> String {
    if let Some(hash) = predicate.rfind('#') {
        predicate[hash + 1..].to_string()
    } else if let Some(slash) = predicate.rfind('/') {
        predicate[slash + 1..].to_string()
    } else {
        predicate.to_string()
    }
}

fn node_type_label(term: &RdfTerm) -> &'static str {
    match term {
        RdfTerm::Iri(_) => "IRI",
        RdfTerm::BlankNode(_) => "BNode",
        RdfTerm::Literal(_) => "Literal",
    }
}

fn measure_node_width(label: &str, zoom: f32) -> f32 {
    let char_width = 8.0 * zoom;
    let padding = 40.0 * zoom;
    let text_width = label.len() as f32 * char_width + padding;
    text_width.max(NODE_MIN_WIDTH * zoom)
}

pub fn draw_graph(app: &mut CrdfEditorApp, ui: &mut egui::Ui) {
    egui::CentralPanel::default().show(ui, |ui| {
        draw_graph_in_ui(app, ui);
    });
}

pub fn draw_graph_in_ui(app: &mut CrdfEditorApp, ui: &mut egui::Ui) {
    let (response, painter) =
        ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
    let canvas_rect = response.rect;
    let canvas_center = canvas_rect.center();

    // Handle camera
    app.camera.handle_input(ui, &response);

    // Draw grid
    crate::canvas::draw_grid(&painter, &app.camera, canvas_rect);

    let zoom = app.camera.zoom;
    let triples = app.rdf_graph.triples();

    // Draw edges
    for triple in &triples {
        let Some(&src_world) = app.node_positions.get(&triple.subject) else {
            continue;
        };
        let Some(&tgt_world) = app.node_positions.get(&triple.object) else {
            continue;
        };

        let src_screen = app.camera.world_to_screen(src_world, canvas_center);
        let tgt_screen = app.camera.world_to_screen(tgt_world, canvas_center);

        draw_edge(
            &painter,
            src_screen,
            tgt_screen,
            &triple.predicate,
            zoom,
            false,
        );
    }

    // Draw edge being created
    if let Some(ref from_term) = app.interaction.creating_edge_from
        && let Some(&from_world) = app.node_positions.get(from_term)
    {
        let from_screen = app.camera.world_to_screen(from_world, canvas_center);
        if let Some(target_pos) = app.interaction.edge_drag_target {
            draw_edge_line(&painter, from_screen, target_pos, zoom, true);
        }
    }

    // Collect nodes for hit testing (front to back)
    let mut node_rects: Vec<(RdfTerm, Rect, Pos2)> = Vec::new();
    let subjects = app.rdf_graph.subjects();
    let objects = app.rdf_graph.objects();

    let mut all_terms: std::collections::HashSet<&RdfTerm> = std::collections::HashSet::new();
    for s in &subjects {
        all_terms.insert(s);
    }
    for o in &objects {
        all_terms.insert(o);
    }

    for term in &all_terms {
        if let Some(&world_pos) = app.node_positions.get(*term) {
            let screen_pos = app.camera.world_to_screen(world_pos, canvas_center);
            let label = node_label(term);
            let w = measure_node_width(&label, zoom);
            let h = NODE_HEIGHT * zoom;
            let rect = Rect::from_center_size(screen_pos, Vec2::new(w, h));
            node_rects.push(((*term).clone(), rect, screen_pos));
        }
    }

    // Draw nodes
    for (term, rect, screen_pos) in &node_rects {
        let is_selected = app.interaction.selected_node.as_ref() == Some(term);
        let is_hovered = app.interaction.hovered_node.as_ref() == Some(term);
        draw_node(
            &painter,
            term,
            *rect,
            *screen_pos,
            zoom,
            is_selected,
            is_hovered,
        );

        // Draw output port (right side)
        let port_pos = Pos2::new(rect.right(), rect.center().y);
        let port_r = PORT_RADIUS * zoom;
        let port_color = if is_hovered {
            Color32::from_rgb(255, 200, 50)
        } else {
            Color32::from_gray(180)
        };
        painter.circle(
            port_pos,
            port_r,
            port_color,
            Stroke::new(1.0, Color32::WHITE),
        );

        // Draw input port (left side)
        let in_port_pos = Pos2::new(rect.left(), rect.center().y);
        painter.circle(
            in_port_pos,
            port_r,
            port_color,
            Stroke::new(1.0, Color32::WHITE),
        );
    }

    // Handle interactions
    let pointer_pos = ui.input(|i| i.pointer.hover_pos());

    // Update hover state
    app.interaction.hovered_node = None;
    if let Some(pos) = pointer_pos {
        for (term, rect, _) in node_rects.iter().rev() {
            if rect.contains(pos) {
                app.interaction.hovered_node = Some(term.clone());
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                break;
            }
        }
    }

    // Node dragging
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(pos) = pointer_pos
    {
        // Check if starting edge creation from port
        for (term, rect, _) in node_rects.iter().rev() {
            let port_pos = Pos2::new(rect.right(), rect.center().y);
            let port_r = PORT_RADIUS * zoom + 4.0;
            if (pos - port_pos).length() < port_r {
                app.interaction.creating_edge_from = Some(term.clone());
                break;
            }
        }

        // If not creating edge, start dragging node
        if app.interaction.creating_edge_from.is_none() {
            for (term, rect, _) in node_rects.iter().rev() {
                if rect.contains(pos) {
                    let world_pos = app.node_positions.get(term).copied().unwrap();
                    let click_world = app.camera.screen_to_world(pos, canvas_center);
                    app.interaction.dragging_node = Some(term.clone());
                    app.interaction.drag_offset = world_pos - click_world;
                    app.interaction.selected_node = Some(term.clone());
                    break;
                }
            }
        }
    }

    if response.dragged_by(egui::PointerButton::Primary)
        && let Some(pos) = pointer_pos
    {
        if let Some(ref dragging) = app.interaction.dragging_node {
            let world_pos =
                app.camera.screen_to_world(pos, canvas_center) + app.interaction.drag_offset;
            if let Some(p) = app.node_positions.get_mut(dragging) {
                *p = world_pos;
            }
        }
        if app.interaction.creating_edge_from.is_some() {
            app.interaction.edge_drag_target = Some(pos);
        }
    }

    if response.drag_stopped_by(egui::PointerButton::Primary) {
        // Complete edge creation
        if let Some(from_term) = app.interaction.creating_edge_from.take() {
            if let Some(pos) = pointer_pos {
                for (term, rect, _) in node_rects.iter().rev() {
                    let in_port_pos = Pos2::new(rect.left(), rect.center().y);
                    let in_port_r = PORT_RADIUS * zoom + 8.0;
                    let on_port = (pos - in_port_pos).length() < in_port_r;
                    let on_node = rect.contains(pos);
                    if (on_port || on_node) && *term != from_term {
                        // Set up the side panel for edge creation
                        app.side_panel.pending_edge = Some((from_term.clone(), term.clone()));
                        app.side_panel.pending_edge_predicate = String::new();
                        break;
                    }
                }
            }
            app.interaction.edge_drag_target = None;
        }
        app.interaction.dragging_node = None;
    }

    // Click to select/deselect
    if response.clicked()
        && let Some(pos) = pointer_pos
    {
        let mut clicked_node = false;
        for (term, rect, _) in node_rects.iter().rev() {
            if rect.contains(pos) {
                app.interaction.selected_node = Some(term.clone());
                clicked_node = true;
                break;
            }
        }
        if !clicked_node {
            app.interaction.selected_node = None;
        }
    }

    // Undo: Ctrl+Z
    if ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Z) && !i.modifiers.shift)
        && app.undo()
    {
        app.remove_orphan_positions();
        let time = ui.input(|i| i.time);
        app.set_status("Undo", time);
    }

    // Redo: Ctrl+Y or Ctrl+Shift+Z
    if ui.input(|i| {
        i.modifiers.ctrl
            && (i.key_pressed(egui::Key::Y) || (i.modifiers.shift && i.key_pressed(egui::Key::Z)))
    }) && app.redo()
    {
        app.ensure_node_positions();
        app.remove_orphan_positions();
        let time = ui.input(|i| i.time);
        app.set_status("Redo", time);
    }

    // Delete key removes all triples for the selected node
    if ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        && let Some(ref selected) = app.interaction.selected_node.clone()
    {
        let subj_triples = app.rdf_graph.triples_for_subject(selected);
        for t in &subj_triples {
            let _ = app.remove_triple(&t.subject, &t.predicate, &t.object);
        }
        let obj_triples = app.rdf_graph.triples_for_object(selected);
        for t in &obj_triples {
            let _ = app.remove_triple(&t.subject, &t.predicate, &t.object);
        }
        app.remove_orphan_positions();
        app.interaction.selected_node = None;
        let time = ui.input(|i| i.time);
        app.set_status("Node removed", time);
    }

    // Context menu
    response.context_menu(|ui| {
        if let Some(ref hovered) = app.interaction.hovered_node {
            ui.label(format!("Node: {}", node_label(hovered)));
            ui.separator();
            if ui.button("🗑 Remove all triples for this node").clicked() {
                let term = hovered.clone();
                let triples = app.rdf_graph.triples_for_subject(&term);
                for t in &triples {
                    let _ = app.remove_triple(&t.subject, &t.predicate, &t.object);
                }
                let triples_obj = app.rdf_graph.triples_for_object(&term);
                for t in &triples_obj {
                    let _ = app.remove_triple(&t.subject, &t.predicate, &t.object);
                }
                app.remove_orphan_positions();
                if app.interaction.selected_node.as_ref() == Some(&term) {
                    app.interaction.selected_node = None;
                }
                ui.close();
            }
        } else {
            if ui.button("🔗 Add IRI Node").clicked() {
                app.side_panel.show_add_node = true;
                app.side_panel.new_node_type = NodeType::Iri;
                ui.close();
            }
            if ui.button("📝 Add Literal Node").clicked() {
                app.side_panel.show_add_node = true;
                app.side_panel.new_node_type = NodeType::Literal;
                ui.close();
            }
            if ui.button("📐 Toggle Auto Layout").clicked() {
                app.layout.running = !app.layout.running;
                if app.layout.running {
                    app.layout.reset_temperature();
                }
                ui.close();
            }
        }
    });
}

fn draw_node(
    painter: &egui::Painter,
    term: &RdfTerm,
    rect: Rect,
    _center: Pos2,
    zoom: f32,
    selected: bool,
    hovered: bool,
) {
    let base_color = node_color(term);
    let fill_color = if hovered {
        lighten(base_color, 30)
    } else {
        base_color
    };

    let stroke = if selected {
        Stroke::new(3.0 * zoom, Color32::from_rgb(255, 215, 0))
    } else {
        Stroke::new(1.0 * zoom, lighten(base_color, 60))
    };

    let rounding = CornerRadius::same((NODE_ROUNDING * zoom).round() as u8);

    // Shadow
    let shadow_rect = rect.translate(Vec2::new(2.0 * zoom, 2.0 * zoom));
    painter.rect_filled(shadow_rect, rounding, Color32::from_black_alpha(40));

    // Fill
    painter.rect(
        rect,
        rounding,
        fill_color,
        stroke,
        egui::StrokeKind::Outside,
    );

    // Type badge
    let type_label = node_type_label(term);
    let badge_font = FontId::proportional(9.0 * zoom);
    let badge_pos = Pos2::new(rect.left() + 6.0 * zoom, rect.top() + 4.0 * zoom);
    painter.text(
        badge_pos,
        egui::Align2::LEFT_TOP,
        type_label,
        badge_font,
        Color32::from_white_alpha(160),
    );

    // Label
    let label = node_label(term);
    let font = FontId::proportional(13.0 * zoom);
    painter.text(
        rect.center() + Vec2::new(0.0, 2.0 * zoom),
        egui::Align2::CENTER_CENTER,
        label,
        font,
        Color32::WHITE,
    );
}

fn draw_edge(
    painter: &egui::Painter,
    src: Pos2,
    tgt: Pos2,
    predicate: &str,
    zoom: f32,
    is_preview: bool,
) {
    let color = if is_preview {
        Color32::from_rgb(255, 200, 50)
    } else {
        Color32::from_gray(160)
    };

    // Bezier control points
    let dist = (tgt.x - src.x).abs().max(80.0 * zoom);
    let cp_offset = dist * 0.4;
    let cp1 = Pos2::new(src.x + cp_offset, src.y);
    let cp2 = Pos2::new(tgt.x - cp_offset, tgt.y);

    // Draw cubic bezier via line segments
    let segments = 32;
    let mut points = Vec::with_capacity(segments + 1);
    for i in 0..=segments {
        let t = i as f32 / segments as f32;
        let p = cubic_bezier(src, cp1, cp2, tgt, t);
        points.push(p);
    }

    let thickness = EDGE_THICKNESS * zoom;
    for i in 0..points.len() - 1 {
        painter.line_segment([points[i], points[i + 1]], Stroke::new(thickness, color));
    }

    // Arrow head at target
    let last = points[points.len() - 1];
    let prev = points[points.len() - 2];
    let dir = (last - prev).normalized();
    let arrow = ARROW_SIZE * zoom;
    let perp = Vec2::new(-dir.y, dir.x);
    let arrow_p1 = last - dir * arrow + perp * arrow * 0.4;
    let arrow_p2 = last - dir * arrow - perp * arrow * 0.4;
    painter.add(egui::Shape::convex_polygon(
        vec![last, arrow_p1, arrow_p2],
        color,
        Stroke::NONE,
    ));

    // Predicate label at midpoint
    let mid = cubic_bezier(src, cp1, cp2, tgt, 0.5);
    let label = predicate_label(predicate);
    let font = FontId::proportional(11.0 * zoom);

    // Background for label
    let galley = painter.layout_no_wrap(label.clone(), font.clone(), color);
    let text_size = galley.size();
    let label_rect = Rect::from_center_size(
        mid + Vec2::new(0.0, -10.0 * zoom),
        text_size + Vec2::new(8.0, 4.0) * zoom,
    );
    painter.rect_filled(
        label_rect,
        CornerRadius::same(3),
        Color32::from_black_alpha(180),
    );

    painter.text(
        mid + Vec2::new(0.0, -10.0 * zoom),
        egui::Align2::CENTER_CENTER,
        label,
        font,
        Color32::from_rgb(220, 220, 220),
    );
}

fn draw_edge_line(painter: &egui::Painter, src: Pos2, tgt: Pos2, zoom: f32, _is_preview: bool) {
    let color = Color32::from_rgb(255, 200, 50);
    let thickness = EDGE_THICKNESS * zoom;
    painter.line_segment([src, tgt], Stroke::new(thickness, color));

    let dir = (tgt - src).normalized();
    let arrow = ARROW_SIZE * zoom;
    let perp = Vec2::new(-dir.y, dir.x);
    let arrow_p1 = tgt - dir * arrow + perp * arrow * 0.4;
    let arrow_p2 = tgt - dir * arrow - perp * arrow * 0.4;
    painter.add(egui::Shape::convex_polygon(
        vec![tgt, arrow_p1, arrow_p2],
        color,
        Stroke::NONE,
    ));
}

fn cubic_bezier(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let u = 1.0 - t;
    let tt = t * t;
    let uu = u * u;
    let uuu = uu * u;
    let ttt = tt * t;

    let x = uuu * p0.x + 3.0 * uu * t * p1.x + 3.0 * u * tt * p2.x + ttt * p3.x;
    let y = uuu * p0.y + 3.0 * uu * t * p1.y + 3.0 * u * tt * p2.y + ttt * p3.y;
    Pos2::new(x, y)
}

fn lighten(color: Color32, amount: u8) -> Color32 {
    Color32::from_rgb(
        color.r().saturating_add(amount),
        color.g().saturating_add(amount),
        color.b().saturating_add(amount),
    )
}

// Re-export for use in ui.rs
#[derive(Clone, Debug, PartialEq, Default)]
pub enum NodeType {
    #[default]
    Iri,
    Literal,
    BlankNode,
}
