use crdf::{RdfGraph, RdfTerm};

use crate::error::DpoError;
use crate::pattern::{PatternPredicate, PatternTerm, PatternTriple};
use crate::rule::DpoRule;
use crate::vocab::*;

impl DpoRule {
    /// Serialize this DPO rule into RDF triples and store them in the given graph.
    ///
    /// Uses the DPO vocabulary (`http://crdf.bkbkb.net/dpo#`) to represent
    /// the rule structure.
    pub fn to_rdf(&self, graph: &mut RdfGraph, rule_iri: &str) -> Result<(), DpoError> {
        let rule_term = RdfTerm::iri(rule_iri);

        // Rule type and name
        graph.add_triple(rule_term.clone(), RDF_TYPE, RdfTerm::iri(DPO_RULE))?;
        graph.add_triple(rule_term.clone(), DPO_NAME, RdfTerm::literal(&self.name))?;

        // LHS
        let lhs_iri = format!("{}/lhs", rule_iri);
        pattern_to_rdf(graph, &lhs_iri, &self.lhs)?;
        graph.add_triple(rule_term.clone(), DPO_LHS, RdfTerm::iri(&lhs_iri))?;

        // Interface
        let iface_iri = format!("{}/interface", rule_iri);
        pattern_to_rdf(graph, &iface_iri, &self.interface)?;
        graph.add_triple(rule_term.clone(), DPO_INTERFACE, RdfTerm::iri(&iface_iri))?;

        // RHS
        let rhs_iri = format!("{}/rhs", rule_iri);
        pattern_to_rdf(graph, &rhs_iri, &self.rhs)?;
        graph.add_triple(rule_term, DPO_RHS, RdfTerm::iri(&rhs_iri))?;

        Ok(())
    }

    /// Load a DPO rule from RDF triples stored in the given graph.
    pub fn from_rdf(graph: &RdfGraph, rule_iri: &str) -> Result<Self, DpoError> {
        let rule_term = RdfTerm::iri(rule_iri);

        // Name
        let name_objs = graph.objects_for_subject_predicate(&rule_term, DPO_NAME);
        let name = name_objs
            .first()
            .and_then(|t| t.as_literal())
            .map(|l| l.value().to_string())
            .unwrap_or_default();

        // LHS
        let lhs_objs = graph.objects_for_subject_predicate(&rule_term, DPO_LHS);
        let lhs_iri = lhs_objs
            .first()
            .and_then(|t| t.as_iri())
            .ok_or_else(|| DpoError::RdfFormat("Missing LHS".to_string()))?
            .to_string();
        let lhs = pattern_from_rdf(graph, &lhs_iri)?;

        // Interface
        let iface_objs = graph.objects_for_subject_predicate(&rule_term, DPO_INTERFACE);
        let iface_iri = iface_objs
            .first()
            .and_then(|t| t.as_iri())
            .ok_or_else(|| DpoError::RdfFormat("Missing interface".to_string()))?
            .to_string();
        let interface = pattern_from_rdf(graph, &iface_iri)?;

        // RHS
        let rhs_objs = graph.objects_for_subject_predicate(&rule_term, DPO_RHS);
        let rhs_iri = rhs_objs
            .first()
            .and_then(|t| t.as_iri())
            .ok_or_else(|| DpoError::RdfFormat("Missing RHS".to_string()))?
            .to_string();
        let rhs = pattern_from_rdf(graph, &rhs_iri)?;

        Ok(DpoRule {
            name,
            lhs,
            interface,
            rhs,
        })
    }
}

fn pattern_to_rdf(
    graph: &mut RdfGraph,
    pattern_iri: &str,
    triples: &[PatternTriple],
) -> Result<(), DpoError> {
    let pattern_term = RdfTerm::iri(pattern_iri);
    graph.add_triple(pattern_term.clone(), RDF_TYPE, RdfTerm::iri(DPO_PATTERN))?;

    for (i, pt) in triples.iter().enumerate() {
        let triple_iri = format!("{}/t{}", pattern_iri, i);
        let triple_term = RdfTerm::iri(&triple_iri);

        graph.add_triple(
            triple_term.clone(),
            RDF_TYPE,
            RdfTerm::iri(DPO_PATTERN_TRIPLE),
        )?;
        graph.add_triple(pattern_term.clone(), DPO_TRIPLE, triple_term.clone())?;

        // Subject
        let subj_term =
            pattern_term_to_rdf(graph, &format!("{}/subject", triple_iri), &pt.subject)?;
        graph.add_triple(triple_term.clone(), DPO_SUBJECT, subj_term)?;

        // Predicate
        let pred_term =
            pattern_predicate_to_rdf(graph, &format!("{}/predicate", triple_iri), &pt.predicate)?;
        graph.add_triple(triple_term.clone(), DPO_PREDICATE, pred_term)?;

        // Object
        let obj_term = pattern_term_to_rdf(graph, &format!("{}/object", triple_iri), &pt.object)?;
        graph.add_triple(triple_term, DPO_OBJECT, obj_term)?;
    }

    Ok(())
}

fn pattern_term_to_rdf(
    _graph: &mut RdfGraph,
    _iri: &str,
    term: &PatternTerm,
) -> Result<RdfTerm, DpoError> {
    match term {
        PatternTerm::Concrete(rdf_term) => Ok(rdf_term.clone()),
    }
}

fn pattern_predicate_to_rdf(
    _graph: &mut RdfGraph,
    _iri: &str,
    pred: &PatternPredicate,
) -> Result<RdfTerm, DpoError> {
    match pred {
        PatternPredicate::Concrete(p) => Ok(RdfTerm::iri(p)),
    }
}

fn pattern_from_rdf(graph: &RdfGraph, pattern_iri: &str) -> Result<Vec<PatternTriple>, DpoError> {
    let pattern_term = RdfTerm::iri(pattern_iri);
    let triple_objs = graph.objects_for_subject_predicate(&pattern_term, DPO_TRIPLE);

    let mut pattern_triples = Vec::new();
    for triple_term in &triple_objs {
        let triple_iri = triple_term
            .as_iri()
            .ok_or_else(|| DpoError::RdfFormat("Pattern triple should be an IRI".to_string()))?;
        let triple_rdf_term = RdfTerm::iri(triple_iri);

        // Subject
        let subj_objs = graph.objects_for_subject_predicate(&triple_rdf_term, DPO_SUBJECT);
        let subject = subj_objs
            .first()
            .ok_or_else(|| DpoError::RdfFormat("Missing subject".to_string()))?;
        let subject = rdf_to_pattern_term(graph, subject)?;

        // Predicate
        let pred_objs = graph.objects_for_subject_predicate(&triple_rdf_term, DPO_PREDICATE);
        let predicate = pred_objs
            .first()
            .ok_or_else(|| DpoError::RdfFormat("Missing predicate".to_string()))?;
        let predicate = rdf_to_pattern_predicate(graph, predicate)?;

        // Object
        let obj_objs = graph.objects_for_subject_predicate(&triple_rdf_term, DPO_OBJECT);
        let object = obj_objs
            .first()
            .ok_or_else(|| DpoError::RdfFormat("Missing object".to_string()))?;
        let object = rdf_to_pattern_term(graph, object)?;

        pattern_triples.push(PatternTriple::new(subject, predicate, object));
    }

    Ok(pattern_triples)
}

fn rdf_to_pattern_term(_graph: &RdfGraph, term: &RdfTerm) -> Result<PatternTerm, DpoError> {
    Ok(PatternTerm::Concrete(term.clone()))
}

fn rdf_to_pattern_predicate(
    _graph: &RdfGraph,
    term: &RdfTerm,
) -> Result<PatternPredicate, DpoError> {
    if let Some(iri) = term.as_iri() {
        return Ok(PatternPredicate::Concrete(iri.to_string()));
    }
    Err(DpoError::RdfFormat("Predicate must be an IRI".to_string()))
}
