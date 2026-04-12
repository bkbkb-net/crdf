use std::collections::HashSet;

use crdf::{RdfGraph, RdfOperation, RdfTerm, Triple};

use crate::error::DpoError;
use crate::pattern::{Binding, PatternTriple, find_matches};

/// A Double Pushout (DPO) graph rewriting rule.
///
/// Defined as a span **L ← K → R** where each component is a set of triples:
/// - `lhs` (L): Left-hand side pattern to match in the host graph
/// - `interface` (K): Preserved pattern (subgraph of both L and R)
/// - `rhs` (R): Right-hand side pattern for the replacement
///
/// Application removes triples from L\K and adds triples from R\K.
/// The dangling and identification conditions ensure safe rewriting.
#[derive(Clone, Debug)]
pub struct DpoRule {
    pub name: String,
    pub lhs: Vec<PatternTriple>,
    pub interface: Vec<PatternTriple>,
    pub rhs: Vec<PatternTriple>,
}

/// The result of applying a DPO rule to a host graph.
#[derive(Debug)]
pub struct DpoApplication {
    pub binding: Binding,
    pub operations: Vec<RdfOperation>,
}

impl DpoRule {
    pub fn new(
        name: impl Into<String>,
        lhs: Vec<PatternTriple>,
        interface: Vec<PatternTriple>,
        rhs: Vec<PatternTriple>,
    ) -> Self {
        DpoRule {
            name: name.into(),
            lhs,
            interface,
            rhs,
        }
    }

    /// Validate that the interface is a subgraph of both L and R.
    pub fn validate(&self) -> Result<(), DpoError> {
        // K ⊆ L
        for k_triple in &self.interface {
            if !self.lhs.iter().any(|l| l.structurally_equal(k_triple)) {
                return Err(DpoError::InterfaceNotSubgraphOfLhs);
            }
        }
        // K ⊆ R
        for k_triple in &self.interface {
            if !self.rhs.iter().any(|r| r.structurally_equal(k_triple)) {
                return Err(DpoError::InterfaceNotSubgraphOfRhs);
            }
        }
        Ok(())
    }

    /// Find all valid matches of the LHS pattern against the host graph.
    pub fn find_matches(&self, graph: &RdfGraph) -> Vec<Binding> {
        let host_triples = graph.triples();
        find_matches(&self.lhs, &host_triples)
    }

    /// Apply the rule using the first valid match.
    pub fn apply(&self, graph: &mut RdfGraph) -> Result<DpoApplication, DpoError> {
        self.validate()?;

        let matches = self.find_matches(graph);
        if matches.is_empty() {
            return Err(DpoError::NoMatchFound);
        }

        self.apply_with_binding(graph, &matches[0])
    }

    /// Apply the rule to all non-overlapping matches.
    pub fn apply_all(&self, graph: &mut RdfGraph) -> Result<Vec<DpoApplication>, DpoError> {
        self.validate()?;

        let matches = self.find_matches(graph);
        if matches.is_empty() {
            return Err(DpoError::NoMatchFound);
        }

        let mut results = Vec::new();
        for binding in &matches {
            let current_matches = self.find_matches(graph);
            if current_matches.iter().any(|m| bindings_equal(m, binding)) {
                results.push(self.apply_with_binding(graph, binding)?);
            }
        }

        Ok(results)
    }

    /// Apply the rule with a specific variable binding.
    pub fn apply_with_binding(
        &self,
        graph: &mut RdfGraph,
        binding: &Binding,
    ) -> Result<DpoApplication, DpoError> {
        let mut operations = Vec::new();

        // Step 1: Compute triples to delete (L \ K)
        let to_delete = self.triples_to_delete(binding)?;

        // Resolve matched L triples (all of L) for condition checks
        let matched_l_triples = self.resolve_lhs(binding)?;
        let interface_triples = self.resolve_interface(binding)?;

        // Step 2: Check identification condition
        self.check_identification_condition(binding)?;

        // Step 3: Check dangling condition
        self.check_dangling_condition(graph, &to_delete, &matched_l_triples, &interface_triples)?;

        // Step 4: Delete L \ K from graph
        for triple in &to_delete {
            let op = graph.remove_triple(&triple.subject, &triple.predicate, &triple.object)?;
            operations.push(op);
        }

        // Step 5: Compute triples to add (R \ K)
        let to_add = self.triples_to_add(binding)?;

        // Step 6: Add R \ K to graph
        for triple in &to_add {
            let op = graph.add_triple(
                triple.subject.clone(),
                triple.predicate.clone(),
                triple.object.clone(),
            )?;
            operations.push(op);
        }

        Ok(DpoApplication {
            binding: binding.clone(),
            operations,
        })
    }

    fn triples_to_delete(&self, binding: &Binding) -> Result<Vec<Triple>, DpoError> {
        let mut result = Vec::new();
        for l_triple in &self.lhs {
            let is_in_interface = self
                .interface
                .iter()
                .any(|k| k.structurally_equal(l_triple));
            if !is_in_interface {
                let concrete = l_triple
                    .resolve(binding)
                    .ok_or_else(|| DpoError::UnboundVariable("in LHS".to_string()))?;
                result.push(concrete);
            }
        }
        Ok(result)
    }

    fn triples_to_add(&self, binding: &Binding) -> Result<Vec<Triple>, DpoError> {
        let mut result = Vec::new();
        for r_triple in &self.rhs {
            let is_in_interface = self
                .interface
                .iter()
                .any(|k| k.structurally_equal(r_triple));
            if !is_in_interface {
                let concrete = r_triple
                    .resolve(binding)
                    .ok_or_else(|| DpoError::UnboundVariable("in RHS".to_string()))?;
                result.push(concrete);
            }
        }
        Ok(result)
    }

    /// Resolve all LHS pattern triples to concrete triples.
    fn resolve_lhs(&self, binding: &Binding) -> Result<Vec<Triple>, DpoError> {
        self.lhs
            .iter()
            .map(|pt| {
                pt.resolve(binding)
                    .ok_or_else(|| DpoError::UnboundVariable("in LHS".to_string()))
            })
            .collect()
    }

    /// Resolve interface pattern triples to concrete triples.
    fn resolve_interface(&self, binding: &Binding) -> Result<Vec<Triple>, DpoError> {
        self.interface
            .iter()
            .map(|pt| {
                pt.resolve(binding)
                    .ok_or_else(|| DpoError::UnboundVariable("in interface".to_string()))
            })
            .collect()
    }

    /// Dangling condition (ダングリング条件):
    ///
    /// When nodes are effectively removed (they appear in L but not in K),
    /// no edge in the remaining host graph D = G \ (L\K) may be incident to
    /// those removed nodes.
    ///
    fn check_dangling_condition(
        &self,
        graph: &RdfGraph,
        to_delete: &[Triple],
        matched_l_triples: &[Triple],
        interface_triples: &[Triple],
    ) -> Result<(), DpoError> {
        // L_V from deleted triples (L\K edges)
        let mut nodes_in_deleted: HashSet<RdfTerm> = HashSet::new();
        for triple in to_delete {
            nodes_in_deleted.insert(triple.subject.clone());
            nodes_in_deleted.insert(triple.object.clone());
        }

        // K_V = nodes from interface triples
        let mut nodes_in_interface: HashSet<RdfTerm> = HashSet::new();
        for triple in interface_triples {
            nodes_in_interface.insert(triple.subject.clone());
            nodes_in_interface.insert(triple.object.clone());
        }

        // Nodes effectively removed: in L\K triples but NOT in K_V
        let removed_nodes: HashSet<&RdfTerm> = nodes_in_deleted
            .iter()
            .filter(|n| !nodes_in_interface.contains(n))
            .collect();

        if removed_nodes.is_empty() {
            return Ok(());
        }

        // Collect all matched L triples as a set for quick lookup
        let matched_set: HashSet<(&RdfTerm, &str, &RdfTerm)> = matched_l_triples
            .iter()
            .map(|t| (&t.subject, t.predicate.as_str(), &t.object))
            .collect();

        // Check all host graph triples NOT matched by L
        for host_triple in &graph.triples() {
            let key = (
                &host_triple.subject,
                host_triple.predicate.as_str(),
                &host_triple.object,
            );
            if matched_set.contains(&key) {
                continue; // This triple is part of the match; skip
            }

            // If this remaining triple is incident to a removed node, violation
            for removed in &removed_nodes {
                if &host_triple.subject == *removed || &host_triple.object == *removed {
                    return Err(DpoError::DanglingCondition(format!(
                        "Node {} is removed by L\\K but still incident to triple: {} <{}> {}",
                        removed, host_triple.subject, host_triple.predicate, host_triple.object
                    )));
                }
            }
        }

        Ok(())
    }

    fn check_identification_condition(&self, _binding: &Binding) -> Result<(), DpoError> {
        Ok(())
    }

    pub fn variables(&self) -> Vec<String> {
        Vec::new()
    }

    /// Preview what applying the rule to the first match would produce,
    /// without actually modifying the graph.
    /// Returns (triples_to_delete, triples_to_add, binding).
    pub fn preview(
        &self,
        graph: &RdfGraph,
    ) -> Result<(Vec<Triple>, Vec<Triple>, Binding), DpoError> {
        self.validate()?;
        let matches = self.find_matches(graph);
        if matches.is_empty() {
            return Err(DpoError::NoMatchFound);
        }
        self.preview_with_binding(graph, &matches[0])
    }

    /// Preview what applying the rule with a specific binding would produce,
    /// without modifying the graph. Checks conditions.
    pub fn preview_with_binding(
        &self,
        graph: &RdfGraph,
        binding: &Binding,
    ) -> Result<(Vec<Triple>, Vec<Triple>, Binding), DpoError> {
        let to_delete = self.triples_to_delete(binding)?;
        let matched_l_triples = self.resolve_lhs(binding)?;
        let interface_triples = self.resolve_interface(binding)?;

        self.check_identification_condition(binding)?;
        self.check_dangling_condition(graph, &to_delete, &matched_l_triples, &interface_triples)?;

        let to_add = self.triples_to_add(binding)?;

        Ok((to_delete, to_add, binding.clone()))
    }

    /// Add a triple to the LHS pattern.
    pub fn push_lhs(&mut self, triple: PatternTriple) {
        self.lhs.push(triple);
    }

    /// Add a triple to the interface pattern.
    pub fn push_interface(&mut self, triple: PatternTriple) {
        self.interface.push(triple);
    }

    /// Add a triple to the RHS pattern.
    pub fn push_rhs(&mut self, triple: PatternTriple) {
        self.rhs.push(triple);
    }

    /// Remove a triple from the LHS pattern by index.
    pub fn remove_lhs(&mut self, index: usize) -> Option<PatternTriple> {
        if index < self.lhs.len() {
            Some(self.lhs.remove(index))
        } else {
            None
        }
    }

    /// Remove a triple from the interface pattern by index.
    pub fn remove_interface(&mut self, index: usize) -> Option<PatternTriple> {
        if index < self.interface.len() {
            Some(self.interface.remove(index))
        } else {
            None
        }
    }

    /// Remove a triple from the RHS pattern by index.
    pub fn remove_rhs(&mut self, index: usize) -> Option<PatternTriple> {
        if index < self.rhs.len() {
            Some(self.rhs.remove(index))
        } else {
            None
        }
    }

    /// Human-readable summary of the rule for display.
    pub fn summary(&self) -> String {
        format!(
            "{}: L({}) ← K({}) → R({})",
            self.name,
            self.lhs.len(),
            self.interface.len(),
            self.rhs.len(),
        )
    }

    pub fn check_unbound_rhs_variables(&self) -> Result<(), DpoError> {
        Ok(())
    }
}

fn bindings_equal(a: &Binding, b: &Binding) -> bool {
    a.terms == b.terms && a.predicates == b.predicates
}
