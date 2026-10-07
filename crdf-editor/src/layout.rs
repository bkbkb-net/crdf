use std::collections::HashMap;

use crdf::{RdfTerm, Triple};
use egui::Pos2;

pub struct ForceLayout {
    pub running: bool,
    temperature: f32,
    iterations: usize,
}

const REPULSION: f32 = 8000.0;
const ATTRACTION: f32 = 0.01;
const DAMPING: f32 = 0.95;
const MIN_DISTANCE: f32 = 30.0;
const INITIAL_TEMP: f32 = 10.0;
const COOLING: f32 = 0.995;
const STEPS_PER_FRAME: usize = 3;

impl Default for ForceLayout {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
struct EdgeLayoutParams {
    target_distance: f32,
    strength: f32,
}

fn edge_layout_params(predicate: &str, object: &RdfTerm) -> EdgeLayoutParams {
    let short = predicate
        .rsplit(['#', '/', ':'])
        .next()
        .unwrap_or(predicate);

    match short {
        // RDF's own descriptive properties are satellites of the subject.
        "type" | "label" | "comment" => EdgeLayoutParams {
            target_distance: 75.0,
            strength: ATTRACTION * 4.0,
        },
        // Any other literal is an attribute, and sits close to its resource.
        _ if matches!(object, RdfTerm::Literal(_)) => EdgeLayoutParams {
            target_distance: 90.0,
            strength: ATTRACTION * 3.0,
        },
        _ => EdgeLayoutParams {
            target_distance: 200.0,
            strength: ATTRACTION,
        },
    }
}

impl ForceLayout {
    pub fn new() -> Self {
        Self {
            running: false,
            temperature: INITIAL_TEMP,
            iterations: 0,
        }
    }

    pub fn reset_temperature(&mut self) {
        self.temperature = INITIAL_TEMP;
        self.iterations = 0;
    }

    pub fn step(&mut self, positions: &mut HashMap<RdfTerm, Pos2>, triples: &[Triple]) {
        if positions.len() < 2 {
            return;
        }

        for _ in 0..STEPS_PER_FRAME {
            self.single_step(positions, triples);
        }

        self.temperature *= COOLING;
        self.iterations += 1;

        if self.temperature < 0.01 || self.iterations > 2000 {
            self.running = false;
        }
    }

    fn single_step(&self, positions: &mut HashMap<RdfTerm, Pos2>, triples: &[Triple]) {
        let terms: Vec<RdfTerm> = positions.keys().cloned().collect();
        let mut forces: HashMap<RdfTerm, egui::Vec2> = HashMap::new();
        for t in &terms {
            forces.insert(t.clone(), egui::Vec2::ZERO);
        }

        // Repulsive forces between all node pairs
        for i in 0..terms.len() {
            for j in (i + 1)..terms.len() {
                let pos_i = positions[&terms[i]];
                let pos_j = positions[&terms[j]];
                let delta = pos_i - pos_j;
                let dist = delta.length().max(MIN_DISTANCE);
                let force = REPULSION / (dist * dist);
                let dir = delta / dist;

                *forces.get_mut(&terms[i]).unwrap() += dir * force;
                *forces.get_mut(&terms[j]).unwrap() -= dir * force;
            }
        }

        // Attractive forces along edges. RDF graphs mix very different
        // relationships: property literals should sit close to their resource,
        // while relations between resources need more room. Weighting edges
        // makes the layout read more like the data model instead of a hairball.
        for triple in triples {
            if let (Some(&src), Some(&tgt)) = (
                positions.get(&triple.subject),
                positions.get(&triple.object),
            ) {
                let edge = edge_layout_params(triple.predicate.as_str(), &triple.object);
                let delta = tgt - src;
                let dist = delta.length().max(MIN_DISTANCE);
                let force = edge.strength * (dist - edge.target_distance);
                let dir = delta / dist;

                *forces.get_mut(&triple.subject).unwrap() += dir * force;
                *forces.get_mut(&triple.object).unwrap() -= dir * force;
            }
        }

        // Apply forces with temperature
        for term in &terms {
            let f = forces[term];
            let f_len = f.length();
            if f_len > 0.0 {
                let capped = f / f_len * f_len.min(self.temperature);
                let pos = positions.get_mut(term).unwrap();
                *pos += (capped * DAMPING).to_pos2().to_vec2();
            }
        }
    }
}
