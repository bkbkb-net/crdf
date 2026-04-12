use crdf::{RdfTerm, Triple};
use std::collections::HashMap;

/// A term in a pattern — a concrete RDF term.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PatternTerm {
    Concrete(RdfTerm),
}

impl std::fmt::Display for PatternTerm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatternTerm::Concrete(term) => write!(f, "{}", term),
        }
    }
}

impl std::fmt::Display for PatternPredicate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatternPredicate::Concrete(iri) => {
                if let Some(frag) = iri.rfind('#').or_else(|| iri.rfind('/')) {
                    write!(f, "{}", &iri[frag + 1..])
                } else {
                    write!(f, "{}", iri)
                }
            }
        }
    }
}

impl std::fmt::Display for PatternTriple {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} —[{}]→ {}", self.subject, self.predicate, self.object)
    }
}

/// A predicate in a pattern — a concrete IRI.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PatternPredicate {
    Concrete(String),
}

/// A triple within a DPO pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternTriple {
    pub subject: PatternTerm,
    pub predicate: PatternPredicate,
    pub object: PatternTerm,
}

/// A binding produced by matching a pattern against a host graph.
#[derive(Clone, Debug, Default)]
pub struct Binding {
    pub terms: HashMap<String, RdfTerm>,
    pub predicates: HashMap<String, String>,
}

impl PatternTerm {
    pub fn concrete(term: RdfTerm) -> Self {
        PatternTerm::Concrete(term)
    }

    pub fn matches(&self, term: &RdfTerm, _binding: &mut Binding) -> bool {
        match self {
            PatternTerm::Concrete(t) => t == term,
        }
    }

    pub fn resolve(&self, _binding: &Binding) -> Option<RdfTerm> {
        match self {
            PatternTerm::Concrete(t) => Some(t.clone()),
        }
    }

    pub fn is_concrete(&self) -> bool {
        matches!(self, PatternTerm::Concrete(_))
    }
}

impl PatternPredicate {
    pub fn concrete(pred: impl Into<String>) -> Self {
        PatternPredicate::Concrete(pred.into())
    }

    pub fn matches(&self, pred: &str, _binding: &mut Binding) -> bool {
        match self {
            PatternPredicate::Concrete(p) => p == pred,
        }
    }

    pub fn resolve(&self, _binding: &Binding) -> Option<String> {
        match self {
            PatternPredicate::Concrete(p) => Some(p.clone()),
        }
    }

    pub fn is_concrete(&self) -> bool {
        matches!(self, PatternPredicate::Concrete(_))
    }
}

impl PatternTriple {
    pub fn new(subject: PatternTerm, predicate: PatternPredicate, object: PatternTerm) -> Self {
        PatternTriple {
            subject,
            predicate,
            object,
        }
    }

    /// Check if this pattern triple matches a concrete triple, extending the binding.
    pub fn matches_triple(&self, triple: &Triple, binding: &mut Binding) -> bool {
        let mut trial = binding.clone();
        if self.subject.matches(&triple.subject, &mut trial)
            && self.predicate.matches(&triple.predicate, &mut trial)
            && self.object.matches(&triple.object, &mut trial)
        {
            *binding = trial;
            true
        } else {
            false
        }
    }

    /// Resolve this pattern triple to a concrete triple using bindings.
    pub fn resolve(&self, binding: &Binding) -> Option<Triple> {
        let subject = self.subject.resolve(binding)?;
        let predicate = self.predicate.resolve(binding)?;
        let object = self.object.resolve(binding)?;
        Some(Triple::new(subject, predicate, object))
    }

    /// Check structural equality (same shape, same variables/concrete values).
    pub fn structurally_equal(&self, other: &PatternTriple) -> bool {
        self.subject == other.subject
            && self.predicate == other.predicate
            && self.object == other.object
    }
}

/// Find all valid matches of a set of pattern triples against host triples.
/// Each host triple is used at most once per match (injective on triples).
pub fn find_matches(patterns: &[PatternTriple], host_triples: &[Triple]) -> Vec<Binding> {
    let mut results = Vec::new();
    let used = vec![false; host_triples.len()];
    backtrack_match(
        patterns,
        0,
        host_triples,
        &used,
        &Binding::default(),
        &mut results,
    );
    results
}

fn backtrack_match(
    patterns: &[PatternTriple],
    idx: usize,
    host_triples: &[Triple],
    used: &[bool],
    current: &Binding,
    results: &mut Vec<Binding>,
) {
    if idx >= patterns.len() {
        results.push(current.clone());
        return;
    }

    for (i, triple) in host_triples.iter().enumerate() {
        if used[i] {
            continue;
        }
        let mut binding = current.clone();
        if patterns[idx].matches_triple(triple, &mut binding) {
            let mut new_used = used.to_vec();
            new_used[i] = true;
            backtrack_match(
                patterns,
                idx + 1,
                host_triples,
                &new_used,
                &binding,
                results,
            );
        }
    }
}
