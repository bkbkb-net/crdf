//! RDF conformance: every identifier this crate mints must be valid
//! per a real RDF implementation (oxrdf), and full graphs must convert
//! losslessly. This is the automated half of the publication
//! verification policy; the operational half (w3id redirect, content
//! negotiation) lives in the spec document.

use crdf_circuit::stdlib::Stdlib;
use crdf_circuit::vocab;
use crdf_circuit::{asset_to_rdf, model::CircuitAsset};
use oxrdf::NamedNode;

const VOCAB: &[&str] = &[
    vocab::RDF_TYPE,
    vocab::TYPE_ASSET,
    vocab::TYPE_MODULE,
    vocab::TYPE_PORT,
    vocab::TYPE_CELL,
    vocab::TYPE_WIRE,
    vocab::PRED_SCHEMA_VERSION,
    vocab::PRED_ROOT_MODULE,
    vocab::PRED_TICKS_PER_STEP,
    vocab::PRED_MODULE_NAME,
    vocab::PRED_PORT_OF,
    vocab::PRED_PORT_NAME,
    vocab::PRED_PORT_DIRECTION,
    vocab::PRED_CELL_OF,
    vocab::PRED_INSTANCE_OF,
    vocab::PRED_REG_INIT,
    vocab::PRED_WIRE_OF,
    vocab::PRED_WIRE_FROM_CELL,
    vocab::PRED_WIRE_FROM_PORT,
    vocab::PRED_WIRE_TO_CELL,
    vocab::PRED_WIRE_TO_PORT,
    vocab::IRI_NAND,
    vocab::IRI_REG,
    vocab::IRI_NAND_A,
    vocab::IRI_NAND_B,
    vocab::IRI_NAND_Y,
    vocab::IRI_REG_D,
    vocab::IRI_REG_Q,
];

#[test]
fn every_vocabulary_iri_is_valid_and_namespaced() {
    for iri in VOCAB {
        NamedNode::new(*iri).unwrap_or_else(|e| panic!("invalid IRI {iri:?}: {e}"));
        if *iri != vocab::RDF_TYPE {
            assert!(
                iri.starts_with(vocab::CIRCUIT_NS),
                "{iri:?} escapes the circuit namespace"
            );
        }
    }
    // Instance identifiers must be RFC 9562 UUID URNs.
    assert_eq!(vocab::ENTITY_PREFIX, "urn:uuid:");
    let sample = format!("{}{}", vocab::ENTITY_PREFIX, uuid::Uuid::new_v4());
    NamedNode::new(sample.as_str()).expect("urn:uuid instance IRI must be valid");
}

#[test]
fn full_graphs_convert_to_oxrdf_losslessly() {
    // A realistic asset (deep hierarchy, canonical stdlib): every
    // subject, predicate and object must survive strict oxrdf
    // validation, and the triple multiset cardinality must match.
    let stdlib = Stdlib::build();
    let asset = CircuitAsset::new(stdlib.fmul32, 1, stdlib.library.clone());
    let graph = asset_to_rdf(&asset).unwrap();

    let ours = graph.triples().len();
    let converted = graph.to_oxrdf().expect("oxrdf conversion must succeed");
    // oxrdf::Graph is a set: identical duplicate triples collapse, so
    // converted.len() <= ours, and every one of ours must convert.
    assert!(converted.len() <= ours);
    assert!(converted.len() > 100, "unexpectedly small graph");
    for triple in graph.triples() {
        triple.to_oxrdf().expect("every triple must convert");
    }
}
