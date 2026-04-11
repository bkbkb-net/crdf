use egui::{Pos2, Vec2};

pub struct Camera {
    pub offset: Vec2,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl Camera {
    pub fn world_to_screen(&self, world: Pos2, canvas_center: Pos2) -> Pos2 {
        canvas_center + (world.to_vec2() + self.offset) * self.zoom
    }

    pub fn screen_to_world(&self, screen: Pos2, canvas_center: Pos2) -> Pos2 {
        ((screen - canvas_center) / self.zoom - self.offset).to_pos2()
    }

    pub fn handle_input(&mut self, ui: &egui::Ui, response: &egui::Response) {
        // Zoom with scroll wheel
        let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll_delta != 0.0 {
            let zoom_factor = 1.0 + scroll_delta * 0.002;
            self.zoom = (self.zoom * zoom_factor).clamp(0.1, 5.0);
        }

        // Pan with middle mouse button or right mouse button
        if response.dragged_by(egui::PointerButton::Middle)
            || (response.dragged_by(egui::PointerButton::Secondary)
                && ui.input(|i| !i.pointer.button_down(egui::PointerButton::Primary)))
        {
            self.offset += response.drag_delta() / self.zoom;
        }
    }
}

pub fn draw_grid(painter: &egui::Painter, camera: &Camera, rect: egui::Rect) {
    let grid_spacing = 50.0 * camera.zoom;
    if grid_spacing < 5.0 {
        return;
    }

    let color = egui::Color32::from_gray(40);
    let center = rect.center();

    let offset_x = (camera.offset.x * camera.zoom + center.x) % grid_spacing;
    let offset_y = (camera.offset.y * camera.zoom + center.y) % grid_spacing;

    let mut x = rect.left() + offset_x % grid_spacing;
    while x < rect.right() {
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            egui::Stroke::new(0.5, color),
        );
        x += grid_spacing;
    }

    let mut y = rect.top() + offset_y % grid_spacing;
    while y < rect.bottom() {
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            egui::Stroke::new(0.5, color),
        );
        y += grid_spacing;
    }
}
