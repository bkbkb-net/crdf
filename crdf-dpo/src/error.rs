use thiserror::Error;

#[derive(Debug, Error)]
pub enum DpoError {
    #[error("No match found for LHS pattern in host graph")]
    NoMatchFound,

    #[error("Dangling condition violated: {0}")]
    DanglingCondition(String),

    #[error("Identification condition violated: {0}")]
    IdentificationCondition(String),

    #[error("Interface is not a subpattern of LHS")]
    InterfaceNotSubgraphOfLhs,

    #[error("Interface is not a subpattern of RHS")]
    InterfaceNotSubgraphOfRhs,

    #[error("Variable '{0}' is unbound")]
    UnboundVariable(String),

    #[error("Invalid rule: {0}")]
    InvalidRule(String),

    #[error("CRDF error: {0}")]
    Crdf(#[from] crdf::CrdfError),

    #[error("RDF format error: {0}")]
    RdfFormat(String),
}
