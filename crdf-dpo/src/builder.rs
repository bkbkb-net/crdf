use crate::error::DpoError;
use crate::pattern::{PatternPredicate, PatternTerm, PatternTriple};
use crate::rule::DpoRule;
use crdf::RdfTerm;

/// Fluent builder for constructing DPO rules.
///
/// # Example
/// ```ignore
/// let rule = DpoRuleBuilder::new("relocate")
///     .lhs_triple(iri("http://example.org/Alice"), pred("http://example.org/livesIn"), iri("http://example.org/Tokyo"))
///     .interface_triple(iri("http://example.org/Alice"), pred("http://example.org/livesIn"), iri("http://example.org/Tokyo"))
///     .rhs_triple(iri("http://example.org/Alice"), pred("http://example.org/livesIn"), iri("http://example.org/Osaka"))
///     .build()?;
/// ```
pub struct DpoRuleBuilder {
    name: String,
    lhs: Vec<PatternTriple>,
    interface: Vec<PatternTriple>,
    rhs: Vec<PatternTriple>,
}

impl DpoRuleBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            lhs: Vec::new(),
            interface: Vec::new(),
            rhs: Vec::new(),
        }
    }

    /// Add a triple to the LHS (L) pattern.
    pub fn lhs_triple(
        mut self,
        subject: PatternTerm,
        predicate: PatternPredicate,
        object: PatternTerm,
    ) -> Self {
        self.lhs
            .push(PatternTriple::new(subject, predicate, object));
        self
    }

    /// Add a triple to the interface (K) pattern.
    pub fn interface_triple(
        mut self,
        subject: PatternTerm,
        predicate: PatternPredicate,
        object: PatternTerm,
    ) -> Self {
        self.interface
            .push(PatternTriple::new(subject, predicate, object));
        self
    }

    /// Add a triple to the RHS (R) pattern.
    pub fn rhs_triple(
        mut self,
        subject: PatternTerm,
        predicate: PatternPredicate,
        object: PatternTerm,
    ) -> Self {
        self.rhs
            .push(PatternTriple::new(subject, predicate, object));
        self
    }

    /// Build the DPO rule, validating that K ⊆ L and K ⊆ R.
    pub fn build(self) -> Result<DpoRule, DpoError> {
        let rule = DpoRule::new(self.name, self.lhs, self.interface, self.rhs);
        rule.validate()?;
        rule.check_unbound_rhs_variables()?;
        Ok(rule)
    }

    /// Build without validation (useful when constructing incrementally in a UI).
    pub fn build_unchecked(self) -> DpoRule {
        DpoRule::new(self.name, self.lhs, self.interface, self.rhs)
    }
}

/// Shorthand: create a concrete IRI PatternTerm.
pub fn iri(value: impl Into<String>) -> PatternTerm {
    PatternTerm::Concrete(RdfTerm::iri(value))
}

/// Shorthand: create a concrete literal PatternTerm.
pub fn literal(value: impl Into<String>) -> PatternTerm {
    PatternTerm::Concrete(RdfTerm::literal(value))
}

/// Shorthand: create a concrete IRI PatternPredicate.
pub fn pred(value: impl Into<String>) -> PatternPredicate {
    PatternPredicate::Concrete(value.into())
}
