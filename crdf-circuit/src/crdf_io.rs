//! CRDF persistence for circuit assets.
//!
//! Vocabulary (schema version 1) — with
//! `PREFIX circuit: <https://bkbkb.net/crdf/circuit#>` (see
//! [`crate::vocab`]); instances are `urn:uuid:<uuid>`:
//!
//! ```text
//! urn:uuid:<A>  rdf:type circuit:Asset
//!     circuit:schemaVersion  "1"
//!     circuit:rootModule     urn:uuid:<M>
//!     circuit:ticksPerStep   "1".."64"
//! urn:uuid:<M>  rdf:type circuit:Module
//!     circuit:moduleName     literal
//! urn:uuid:<P>  rdf:type circuit:Port
//!     circuit:portOf         urn:uuid:<M>
//!     circuit:portName       literal
//!     circuit:portDirection  "in" | "out"
//! urn:uuid:<C>  rdf:type circuit:Cell
//!     circuit:cellOf         urn:uuid:<M>
//!     circuit:instanceOf     circuit:nand | circuit:reg | urn:uuid:<M'>
//!     circuit:regInit        "true" | "false"      (reg cells only)
//! urn:uuid:<W>  rdf:type circuit:Wire
//!     circuit:wireOf         urn:uuid:<M>
//!     circuit:wireFromCell   urn:uuid:<C>          (absent = own module port)
//!     circuit:wireFromPort   circuit:nand-a|-b|-y | circuit:reg-d|-q | urn:uuid:<P>
//!     circuit:wireToCell / circuit:wireToPort      (same shape)
//! ```
//!
//! Entity kinds are carried by `rdf:type`, not by the identifier;
//! loading therefore keys resources by their UUID within each typed
//! table.
//!
//! Wires are immutable resources: editors change an endpoint by
//! deleting the wire and creating a new one, so concurrent CRDT edits
//! can never merge into a half-rewired connection. A CRDT merge can
//! still produce structurally invalid circuits (two drivers, dangling
//! references, conflicting property values); the loader reports those
//! as errors instead of guessing, and callers are expected to keep the
//! last valid compiled program.
//!
//! Determinism is semantic: load order is normalised by sorting every
//! resource kind by UUID, so a load → save round trip is stable even
//! though the CRDT container carries history.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crdf::{RdfFileFormat, RdfGraph, RdfOperation, RdfTerm};
use uuid::Uuid;

use crate::block_dag::{BlockDag, BlockInterface, BlockMetadata, RoleTag};
use crate::model::{
    AssetId, Cell, CellId, CellKind, CircuitAsset, Endpoint, Module, ModuleId, ModuleLibrary, Port,
    PortDirection, PortId, PortRef, Wire, WireId, canonical_uuid,
};
use crate::types::Type;
use crate::vocab::*;

pub const CIRCUIT_CRDF_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CircuitCrdfError {
    #[error("failed to add triple: {0}")]
    AddTriple(String),
    #[error("failed to remove triple: {0}")]
    RemoveTriple(String),
    #[error("no circuit:Asset vertex in graph")]
    MissingAsset,
    #[error("more than one circuit:Asset vertex in graph")]
    MultipleAssets,
    #[error("{subject} is missing {predicate}")]
    MissingValue { subject: String, predicate: String },
    #[error("{subject} has conflicting values for {predicate} (unresolved CRDT merge?)")]
    ConflictingValues { subject: String, predicate: String },
    #[error("malformed IRI or literal {value:?} for {predicate}")]
    BadValue { predicate: String, value: String },
    #[error("{subject} references {reference}, which is not in the graph")]
    DanglingReference { subject: String, reference: String },
    #[error("wire {wire} endpoint is malformed: {reason}")]
    BadEndpoint { wire: String, reason: String },
    #[error("unsupported circuit schema version {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("file I/O: {0}")]
    File(String),
}

type Result<T> = std::result::Result<T, CircuitCrdfError>;

pub(crate) fn iri(prefix: &str, id: Uuid) -> RdfTerm {
    RdfTerm::iri(format!("{prefix}{id}"))
}

pub(crate) fn add_triple(
    graph: &mut RdfGraph,
    subject: RdfTerm,
    predicate: &str,
    object: RdfTerm,
) -> Result<RdfOperation> {
    graph
        .add_triple(subject, predicate, object)
        .map_err(|e| CircuitCrdfError::AddTriple(format!("{e:?}")))
}

fn parse_uuid_iri(term: &RdfTerm, prefix: &str, predicate: &str) -> Result<Uuid> {
    let text = term.as_iri().unwrap_or_default();
    text.strip_prefix(prefix)
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or_else(|| CircuitCrdfError::BadValue {
            predicate: predicate.to_string(),
            value: format!("{term:?}"),
        })
}

/// Serialises an asset (root + tick rate + full module closure) to a
/// fresh RDF graph.
pub fn asset_to_rdf(asset: &CircuitAsset) -> Result<RdfGraph> {
    let mut rdf = RdfGraph::new();
    write_asset_into(&mut rdf, asset)?;
    Ok(rdf)
}

/// Writes an asset (envelope + the transitive module closure of its
/// root) into an existing graph. A graph may hold any number of
/// assets; module definitions are shared vertices, and rewriting an
/// identical module produces only duplicate identical triples (which
/// the loader collapses).
pub fn write_asset_into(graph: &mut RdfGraph, asset: &CircuitAsset) -> Result<()> {
    write_asset_into_with_operations(graph, asset).map(|_| ())
}

/// Writes an asset into an existing graph and returns every CRDT operation
/// required to reproduce the import on another replica, in causal order.
///
/// The operation list is empty when the identical asset is already present.
/// As with [`write_asset_into`], validation and conflict detection complete
/// before the caller's graph is changed.
pub fn write_asset_into_with_operations(
    graph: &mut RdfGraph,
    asset: &CircuitAsset,
) -> Result<Vec<RdfOperation>> {
    let mut candidate = RdfGraph::new();
    write_asset_fresh(&mut candidate, asset)?;
    merge_graph_atomically(graph, &candidate)
}

/// Builds one asset in a fresh graph. Callers that merge into an
/// existing document go through [`write_asset_into`], which performs
/// conflict checks and commits the complete import atomically.
fn write_asset_fresh(graph: &mut RdfGraph, asset: &CircuitAsset) -> Result<()> {
    // Validate the complete dependency closure before writing even the
    // asset envelope. A missing module must not leave a partial asset in
    // the destination graph.
    let reachable = reachable_modules(asset)?;

    let asset_subject = iri(ASSET_PREFIX, asset.id.0);
    add_triple(
        graph,
        asset_subject.clone(),
        RDF_TYPE,
        RdfTerm::iri(TYPE_ASSET),
    )?;
    add_triple(
        graph,
        asset_subject.clone(),
        PRED_SCHEMA_VERSION,
        RdfTerm::literal(CIRCUIT_CRDF_SCHEMA_VERSION.to_string()),
    )?;
    add_triple(
        graph,
        asset_subject.clone(),
        PRED_ROOT_MODULE,
        iri(MODULE_PREFIX, asset.root.0),
    )?;
    add_triple(
        graph,
        asset_subject,
        PRED_TICKS_PER_STEP,
        RdfTerm::literal(asset.ticks_per_step.to_string()),
    )?;

    // Serialise only the transitive module closure of the root, so a
    // standalone asset does not drag along unrelated library modules.
    for module in asset.library.modules() {
        if !reachable.contains(&module.id) {
            continue;
        }
        write_module_fresh(graph, module)?;
    }
    Ok(())
}

/// Writes one module definition (with its ports, cells and wires) into
/// a graph.
pub fn write_module_into(graph: &mut RdfGraph, module: &Module) -> Result<()> {
    write_module_into_with_operations(graph, module).map(|_| ())
}

/// Writes one module into an existing graph and returns the CRDT operations
/// needed to reproduce the import on another replica, in causal order.
pub fn write_module_into_with_operations(
    graph: &mut RdfGraph,
    module: &Module,
) -> Result<Vec<RdfOperation>> {
    let mut candidate = RdfGraph::new();
    write_module_fresh(&mut candidate, module)?;
    merge_graph_atomically(graph, &candidate)
}

/// Writes a module's typed **bus interface**: for each input and
/// output bus, a `circuit:Bus` node linking the module to the bus name,
/// direction and encoded [`Type`]. Bus node ids are derived
/// deterministically from the module id, direction and bus name, so
/// re-writing the same interface is idempotent and CRDT-merge-safe. The
/// underlying 1-bit ports are unchanged — this is pure annotation.
pub fn write_block_interface_into(graph: &mut RdfGraph, interface: &BlockInterface) -> Result<()> {
    let mut candidate = RdfGraph::new();
    write_interface_fresh(&mut candidate, interface)?;
    merge_graph_atomically(graph, &candidate).map(|_| ())
}

fn write_interface_fresh(graph: &mut RdfGraph, interface: &BlockInterface) -> Result<()> {
    let module_subject = iri(MODULE_PREFIX, interface.module.0);
    write_bus_nodes(graph, &module_subject, interface.module.0, interface)?;
    Ok(())
}

/// Writes the `circuit:Bus` nodes for a typed interface, owned by
/// `owner_subject` (a module or a block instance). Bus node ids are
/// derived deterministically from `owner_uuid`, direction and name, so
/// re-writing the same interface is idempotent and merge-safe.
fn write_bus_nodes(
    graph: &mut RdfGraph,
    owner_subject: &RdfTerm,
    owner_uuid: Uuid,
    interface: &BlockInterface,
) -> Result<()> {
    for (direction, buses) in [
        (DIRECTION_IN, &interface.inputs),
        (DIRECTION_OUT, &interface.outputs),
    ] {
        for (name, ty, role) in buses {
            let subject = iri(
                ENTITY_PREFIX,
                canonical_uuid(&format!("bus/{owner_uuid}/{direction}/{name}")),
            );
            add_triple(graph, subject.clone(), RDF_TYPE, RdfTerm::iri(TYPE_BUS))?;
            add_triple(graph, subject.clone(), PRED_BUS_OF, owner_subject.clone())?;
            add_triple(
                graph,
                subject.clone(),
                PRED_BUS_NAME,
                RdfTerm::literal(name.clone()),
            )?;
            add_triple(
                graph,
                subject.clone(),
                PRED_BUS_DIRECTION,
                RdfTerm::literal(direction),
            )?;
            add_triple(
                graph,
                subject.clone(),
                PRED_BUS_TYPE,
                RdfTerm::literal(ty.encode()),
            )?;
            add_triple(
                graph,
                subject,
                PRED_BUS_ROLE,
                RdfTerm::literal(role.as_tag()),
            )?;
        }
    }
    Ok(())
}

/// A list of typed buses (`(name, type, role)`), one side of an interface.
type TypedBuses = Vec<(String, Type, RoleTag)>;

/// Reads the typed buses owned by `owner`, split into `(inputs, outputs)`,
/// each sorted by name (bus order is not semantically significant).
fn read_buses_owned_by(
    properties: &Properties,
    owner: &RdfTerm,
) -> Result<(TypedBuses, TypedBuses)> {
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for subject in properties.subjects_of_type_owned_by(TYPE_BUS, PRED_BUS_OF, owner) {
        let name =
            literal_text(properties.one(subject, PRED_BUS_NAME)?, PRED_BUS_NAME)?.to_string();
        let direction = literal_text(
            properties.one(subject, PRED_BUS_DIRECTION)?,
            PRED_BUS_DIRECTION,
        )?;
        let type_text = literal_text(properties.one(subject, PRED_BUS_TYPE)?, PRED_BUS_TYPE)?;
        let ty = Type::decode(type_text).map_err(|e| CircuitCrdfError::BadValue {
            predicate: PRED_BUS_TYPE.to_string(),
            value: format!("{type_text:?}: {e}"),
        })?;
        // A bus without a role claims none. A role is kept exactly as
        // written: the engine does not interpret it.
        let role = match properties.optional(subject, PRED_BUS_ROLE)? {
            Some(term) => RoleTag::new(literal_text(term, PRED_BUS_ROLE)?),
            None => RoleTag::RAW,
        };
        match direction {
            DIRECTION_IN => inputs.push((name, ty, role)),
            DIRECTION_OUT => outputs.push((name, ty, role)),
            other => {
                return Err(CircuitCrdfError::BadValue {
                    predicate: PRED_BUS_DIRECTION.to_string(),
                    value: other.to_string(),
                });
            }
        }
    }
    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    outputs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((inputs, outputs))
}

/// Reads a module's typed bus interface, or `None` when the graph carries
/// no bus annotations for it.
pub fn read_block_interface(graph: &RdfGraph, module: ModuleId) -> Result<Option<BlockInterface>> {
    let properties = Properties::new(graph);
    let owner = iri(MODULE_PREFIX, module.0);
    let (inputs, outputs) = read_buses_owned_by(&properties, &owner)?;
    if inputs.is_empty() && outputs.is_empty() {
        return Ok(None);
    }
    Ok(Some(BlockInterface {
        module,
        inputs,
        outputs,
    }))
}

/// Serialises a high-level **block-DAG**: a `circuit:BlockGraph`
/// node with one `circuit:BlockInstance` per block (carrying its index,
/// module and typed buses), one `circuit:Connection` per wire, and one
/// `circuit:Exposed` per external port. This preserves the *editable*
/// structure, not just the compiled bit circuit.
pub fn write_block_dag_into(graph: &mut RdfGraph, dag: &BlockDag) -> Result<()> {
    let mut candidate = RdfGraph::new();
    write_block_dag_fresh(&mut candidate, dag)?;
    if graph.all_vertices_added().is_empty() {
        // A graph nothing was ever written to, so the candidate is the result.
        // Adding it again fact by fact doubled the cost of every save of a
        // patch, which starts from a fresh graph. Only a fresh one: a graph
        // whose facts were all removed still has a history to keep. The
        // merge's one check still holds.
        single_valued(&candidate)?;
        *graph = candidate;
        return Ok(());
    }
    merge_graph_atomically(graph, &candidate).map(|_| ())
}

/// The identity of something a block graph names, derived from the
/// graph's identity and what the thing is — never minted, so an unchanged
/// graph writes the same facts every time.
fn scoped(dag: &BlockDag, kind: &str, parts: &[&str]) -> RdfTerm {
    // Length-prefix fields: names and tags may themselves contain '/'.
    let key: String = parts
        .iter()
        .map(|part| format!("{}:{part}", part.len()))
        .collect();
    iri(
        ENTITY_PREFIX,
        canonical_uuid(&format!("{kind}/{}/{key}", dag.id())),
    )
}

fn write_block_dag_fresh(graph: &mut RdfGraph, dag: &BlockDag) -> Result<()> {
    let mut ids = HashSet::new();
    ids.insert(dag.id());
    for metadata in dag.block_metadata() {
        if !ids.insert(metadata.id) {
            return Err(CircuitCrdfError::BadValue {
                predicate: PRED_BLOCK_INDEX.to_string(),
                value: format!("duplicate block/graph identity {}", metadata.id),
            });
        }
    }
    let graph_subject = iri(ENTITY_PREFIX, dag.id());
    // A block by its persistent identity, not its place in the list, so
    // reordering blocks renames nothing that points at them.
    let block = |index: usize| {
        dag.block_metadata().get(index).map_or_else(
            || format!("index-{index}"),
            |metadata| metadata.id.to_string(),
        )
    };
    add_triple(
        graph,
        graph_subject.clone(),
        RDF_TYPE,
        RdfTerm::iri(TYPE_BLOCK_GRAPH),
    )?;

    for (index, (iface, metadata)) in dag.blocks().iter().zip(dag.block_metadata()).enumerate() {
        let inst_uuid = metadata.id;
        let subject = iri(ENTITY_PREFIX, inst_uuid);
        add_triple(
            graph,
            subject.clone(),
            RDF_TYPE,
            RdfTerm::iri(TYPE_BLOCK_INSTANCE),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_MEMBER_OF,
            graph_subject.clone(),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_BLOCK_INDEX,
            RdfTerm::literal(index.to_string()),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_BLOCK_MODULE,
            iri(MODULE_PREFIX, iface.module.0),
        )?;
        if let Some(label) = &metadata.label {
            add_triple(
                graph,
                subject.clone(),
                PRED_BLOCK_LABEL,
                RdfTerm::literal(label.clone()),
            )?;
        }
        if let Some([x, y]) = metadata.position {
            add_triple(
                graph,
                subject.clone(),
                PRED_BLOCK_POSITION_X,
                RdfTerm::literal(x.to_string()),
            )?;
            add_triple(
                graph,
                subject.clone(),
                PRED_BLOCK_POSITION_Y,
                RdfTerm::literal(y.to_string()),
            )?;
        }
        if let Some(style) = &metadata.widget_style {
            add_triple(
                graph,
                subject.clone(),
                PRED_BLOCK_WIDGET_STYLE,
                RdfTerm::literal(style.clone()),
            )?;
        }
        if let Some(value) = metadata.manual_value {
            add_triple(
                graph,
                subject.clone(),
                PRED_BLOCK_MANUAL_VALUE,
                RdfTerm::literal(value.to_string()),
            )?;
        }
        if let Some(value) = metadata.default_value {
            add_triple(
                graph,
                subject.clone(),
                PRED_BLOCK_DEFAULT_VALUE,
                RdfTerm::literal(value.to_string()),
            )?;
        }
        if let Some(slot) = metadata.control_slot {
            add_triple(
                graph,
                subject.clone(),
                PRED_BLOCK_CONTROL_SLOT,
                RdfTerm::literal(slot.to_string()),
            )?;
        }
        if let Some(view) = &metadata.view {
            add_triple(
                graph,
                subject.clone(),
                PRED_BLOCK_VIEW,
                RdfTerm::literal(view.clone()),
            )?;
        }
        if metadata.docked {
            // Written only when true, so a graph nobody has docked
            // anything in carries no trace of the feature at all.
            add_triple(
                graph,
                subject.clone(),
                PRED_BLOCK_DOCKED,
                RdfTerm::literal("true"),
            )?;
        }
        write_bus_nodes(graph, &subject, inst_uuid, iface)?;
    }

    for (index, (name, [x, y])) in dag.boundary_positions().iter().enumerate() {
        let subject = scoped(dag, "boundary", &[name]);
        add_triple(
            graph,
            subject.clone(),
            PRED_BOUNDARY_INDEX,
            RdfTerm::literal(index.to_string()),
        )?;
        add_triple(
            graph,
            subject.clone(),
            RDF_TYPE,
            RdfTerm::iri(TYPE_BOUNDARY_NODE),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_MEMBER_OF,
            graph_subject.clone(),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_BOUNDARY_NAME,
            RdfTerm::literal(name.clone()),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_BOUNDARY_POSITION_X,
            RdfTerm::literal(x.to_string()),
        )?;
        add_triple(
            graph,
            subject,
            PRED_BOUNDARY_POSITION_Y,
            RdfTerm::literal(y.to_string()),
        )?;
    }

    for (from, to) in dag.connections() {
        let subject = scoped(
            dag,
            "connection",
            &[&block(from.block), &from.bus, &block(to.block), &to.bus],
        );
        add_triple(
            graph,
            subject.clone(),
            RDF_TYPE,
            RdfTerm::iri(TYPE_CONNECTION),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_MEMBER_OF,
            graph_subject.clone(),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_CONN_FROM_BLOCK,
            RdfTerm::literal(from.block.to_string()),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_CONN_FROM_BUS,
            RdfTerm::literal(from.bus.clone()),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_CONN_TO_BLOCK,
            RdfTerm::literal(to.block.to_string()),
        )?;
        add_triple(
            graph,
            subject,
            PRED_CONN_TO_BUS,
            RdfTerm::literal(to.bus.clone()),
        )?;
    }

    let exposed = dag
        .exposed_inputs()
        .iter()
        .map(|e| (DIRECTION_IN, e))
        .chain(dag.exposed_outputs().iter().map(|e| (DIRECTION_OUT, e)));
    for (index, (direction, (name, bus, ty))) in exposed.enumerate() {
        let subject = scoped(dag, "exposed", &[direction, name]);
        add_triple(
            graph,
            subject.clone(),
            PRED_EXPOSED_INDEX,
            RdfTerm::literal(index.to_string()),
        )?;
        add_triple(graph, subject.clone(), RDF_TYPE, RdfTerm::iri(TYPE_EXPOSED))?;
        add_triple(
            graph,
            subject.clone(),
            PRED_MEMBER_OF,
            graph_subject.clone(),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_EXPOSED_NAME,
            RdfTerm::literal(name.clone()),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_EXPOSED_BLOCK,
            RdfTerm::literal(bus.block.to_string()),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_EXPOSED_BUS,
            RdfTerm::literal(bus.bus.clone()),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_EXPOSED_TYPE,
            RdfTerm::literal(ty.encode()),
        )?;
        add_triple(
            graph,
            subject,
            PRED_EXPOSED_DIRECTION,
            RdfTerm::literal(direction),
        )?;
    }

    for (target, value) in dag.constants() {
        let subject = scoped(dag, "constant", &[&block(target.block), &target.bus]);
        add_triple(
            graph,
            subject.clone(),
            RDF_TYPE,
            RdfTerm::iri(TYPE_CONSTANT),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_MEMBER_OF,
            graph_subject.clone(),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_CONST_BLOCK,
            RdfTerm::literal(target.block.to_string()),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_CONST_BUS,
            RdfTerm::literal(target.bus.clone()),
        )?;
        add_triple(
            graph,
            subject,
            PRED_CONST_VALUE,
            RdfTerm::literal(
                // Least significant word first, space separated. A bus may
                // be wider than a word (a curve of sixty-four points is
                // 1,024 bits), and a file written when every bus was a
                // word holds exactly one number, which reads back as a
                // one-word list unchanged.
                value
                    .iter()
                    .map(|word| word.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        )?;
    }

    for (tag, target) in dag.bindings() {
        let subject = scoped(dag, "binding", &[tag, &block(target.block), &target.bus]);
        add_triple(graph, subject.clone(), RDF_TYPE, RdfTerm::iri(TYPE_BINDING))?;
        add_triple(
            graph,
            subject.clone(),
            PRED_MEMBER_OF,
            graph_subject.clone(),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_BIND_TAG,
            RdfTerm::literal(tag.clone()),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_BIND_BLOCK,
            RdfTerm::literal(target.block.to_string()),
        )?;
        add_triple(
            graph,
            subject,
            PRED_BIND_BUS,
            RdfTerm::literal(target.bus.clone()),
        )?;
    }
    Ok(())
}

/// Reads a block-DAG, or `None` if the graph has no `circuit:BlockGraph`.
pub fn read_block_dag(graph: &RdfGraph) -> Result<Option<BlockDag>> {
    let properties = Properties::new(graph);
    let graphs = properties.subjects_of_type(TYPE_BLOCK_GRAPH);
    let graph_subject = match graphs.as_slice() {
        [] => return Ok(None),
        [single] => RdfTerm::iri(*single),
        _ => {
            return Err(CircuitCrdfError::BadValue {
                predicate: TYPE_BLOCK_GRAPH.to_string(),
                value: "more than one block graph in document".to_string(),
            });
        }
    };
    // The graph's identity is its node's: a save from before identities
    // were kept brings its random one, and every later save keeps it.
    let dag_id = parse_uuid_iri(&graph_subject, ENTITY_PREFIX, TYPE_BLOCK_GRAPH)?;

    // Block instances, ordered by their declared index.
    let mut instances = Vec::new();
    for subject in
        properties.subjects_of_type_owned_by(TYPE_BLOCK_INSTANCE, PRED_MEMBER_OF, &graph_subject)
    {
        let index = parse_usize(properties.one(subject, PRED_BLOCK_INDEX)?, PRED_BLOCK_INDEX)?;
        let id = parse_uuid_iri(&RdfTerm::iri(subject), ENTITY_PREFIX, PRED_BLOCK_INDEX)?;
        let module = ModuleId(parse_uuid_iri(
            properties.one(subject, PRED_BLOCK_MODULE)?,
            MODULE_PREFIX,
            PRED_BLOCK_MODULE,
        )?);
        let (inputs, outputs) = read_buses_owned_by(&properties, &RdfTerm::iri(subject))?;
        let label = properties
            .optional(subject, PRED_BLOCK_LABEL)?
            .map(|term| literal_text(term, PRED_BLOCK_LABEL).map(str::to_string))
            .transpose()?;
        let position = match (
            properties.optional(subject, PRED_BLOCK_POSITION_X)?,
            properties.optional(subject, PRED_BLOCK_POSITION_Y)?,
        ) {
            (None, None) => None,
            (Some(x), Some(y)) => Some([
                parse_f32(x, PRED_BLOCK_POSITION_X)?,
                parse_f32(y, PRED_BLOCK_POSITION_Y)?,
            ]),
            _ => {
                return Err(CircuitCrdfError::BadValue {
                    predicate: PRED_BLOCK_POSITION_X.to_string(),
                    value: "block position must contain both X and Y".to_string(),
                });
            }
        };
        let widget_style = properties
            .optional(subject, PRED_BLOCK_WIDGET_STYLE)?
            .map(|term| literal_text(term, PRED_BLOCK_WIDGET_STYLE).map(str::to_string))
            .transpose()?;
        let manual_value = properties
            .optional(subject, PRED_BLOCK_MANUAL_VALUE)?
            .map(|term| parse_u64(term, PRED_BLOCK_MANUAL_VALUE))
            .transpose()?;
        let default_value = properties
            .optional(subject, PRED_BLOCK_DEFAULT_VALUE)?
            .map(|term| parse_u64(term, PRED_BLOCK_DEFAULT_VALUE))
            .transpose()?;
        let control_slot = properties
            .optional(subject, PRED_BLOCK_CONTROL_SLOT)?
            .map(|term| parse_u8(term, PRED_BLOCK_CONTROL_SLOT))
            .transpose()?;
        let view = properties
            .optional(subject, PRED_BLOCK_VIEW)?
            .map(|term| literal_text(term, PRED_BLOCK_VIEW).map(str::to_string))
            .transpose()?;
        let docked = properties
            .optional(subject, PRED_BLOCK_DOCKED)?
            .map(|term| literal_text(term, PRED_BLOCK_DOCKED).map(|text| text == "true"))
            .transpose()?
            .unwrap_or(false);
        instances.push((
            index,
            BlockInterface {
                module,
                inputs,
                outputs,
            },
            BlockMetadata {
                id,
                label,
                position,
                widget_style,
                manual_value,
                default_value,
                control_slot,
                view,
                docked,
            },
        ));
    }
    instances.sort_by_key(|(index, _, _)| *index);

    let mut dag = BlockDag::new();
    dag.set_id(dag_id);
    for (expected, (index, iface, metadata)) in instances.into_iter().enumerate() {
        if index != expected {
            return Err(CircuitCrdfError::BadValue {
                predicate: PRED_BLOCK_INDEX.to_string(),
                value: format!("non-contiguous block index {index}"),
            });
        }
        dag.add_block_with_metadata(iface, metadata);
    }

    // Boundary-node placement. Sorted so a graph always rebuilds in the
    // same order regardless of how the triples were stored.
    let mut boundaries = Vec::new();
    for subject in
        properties.subjects_of_type_owned_by(TYPE_BOUNDARY_NODE, PRED_MEMBER_OF, &graph_subject)
    {
        let name = literal_text(
            properties.one(subject, PRED_BOUNDARY_NAME)?,
            PRED_BOUNDARY_NAME,
        )?
        .to_string();
        let x = parse_f32(
            properties.one(subject, PRED_BOUNDARY_POSITION_X)?,
            PRED_BOUNDARY_POSITION_X,
        )?;
        let y = parse_f32(
            properties.one(subject, PRED_BOUNDARY_POSITION_Y)?,
            PRED_BOUNDARY_POSITION_Y,
        )?;
        let index = properties
            .optional(subject, PRED_BOUNDARY_INDEX)?
            .map(|term| parse_usize(term, PRED_BOUNDARY_INDEX))
            .transpose()?;
        boundaries.push((index, name, [x, y]));
    }
    boundaries.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    for (_, name, pos) in boundaries {
        dag.set_boundary_position(name, pos);
    }

    let mut connections = Vec::new();
    for subject in
        properties.subjects_of_type_owned_by(TYPE_CONNECTION, PRED_MEMBER_OF, &graph_subject)
    {
        let from_block = parse_usize(
            properties.one(subject, PRED_CONN_FROM_BLOCK)?,
            PRED_CONN_FROM_BLOCK,
        )?;
        let from_bus = literal_text(
            properties.one(subject, PRED_CONN_FROM_BUS)?,
            PRED_CONN_FROM_BUS,
        )?
        .to_string();
        let to_block = parse_usize(
            properties.one(subject, PRED_CONN_TO_BLOCK)?,
            PRED_CONN_TO_BLOCK,
        )?;
        let to_bus =
            literal_text(properties.one(subject, PRED_CONN_TO_BUS)?, PRED_CONN_TO_BUS)?.to_string();
        connections.push((from_block, from_bus, to_block, to_bus));
    }
    connections.sort();
    for (from_block, from_bus, to_block, to_bus) in connections {
        dag.connect(from_block, &from_bus, to_block, &to_bus);
    }

    let mut exposed = Vec::new();
    for subject in
        properties.subjects_of_type_owned_by(TYPE_EXPOSED, PRED_MEMBER_OF, &graph_subject)
    {
        let direction = literal_text(
            properties.one(subject, PRED_EXPOSED_DIRECTION)?,
            PRED_EXPOSED_DIRECTION,
        )?
        .to_string();
        let name = literal_text(
            properties.one(subject, PRED_EXPOSED_NAME)?,
            PRED_EXPOSED_NAME,
        )?
        .to_string();
        let block = parse_usize(
            properties.one(subject, PRED_EXPOSED_BLOCK)?,
            PRED_EXPOSED_BLOCK,
        )?;
        let bus =
            literal_text(properties.one(subject, PRED_EXPOSED_BUS)?, PRED_EXPOSED_BUS)?.to_string();
        let type_text = literal_text(
            properties.one(subject, PRED_EXPOSED_TYPE)?,
            PRED_EXPOSED_TYPE,
        )?;
        let ty = Type::decode(type_text).map_err(|e| CircuitCrdfError::BadValue {
            predicate: PRED_EXPOSED_TYPE.to_string(),
            value: format!("{type_text:?}: {e}"),
        })?;
        let index = properties
            .optional(subject, PRED_EXPOSED_INDEX)?
            .map(|term| parse_usize(term, PRED_EXPOSED_INDEX))
            .transpose()?;
        exposed.push((index, direction, name, block, bus, ty));
    }
    exposed.sort_by(|a, b| (&a.0, &a.1, &a.2, a.3, &a.4).cmp(&(&b.0, &b.1, &b.2, b.3, &b.4)));
    for (_, direction, name, block, bus, ty) in exposed {
        match direction.as_str() {
            DIRECTION_IN => dag.expose_input(&name, block, &bus, ty),
            DIRECTION_OUT => dag.expose_output(&name, block, &bus, ty),
            other => {
                return Err(CircuitCrdfError::BadValue {
                    predicate: PRED_EXPOSED_DIRECTION.to_string(),
                    value: other.to_string(),
                });
            }
        }
    }

    let mut constants = Vec::new();
    for subject in
        properties.subjects_of_type_owned_by(TYPE_CONSTANT, PRED_MEMBER_OF, &graph_subject)
    {
        let block = parse_usize(properties.one(subject, PRED_CONST_BLOCK)?, PRED_CONST_BLOCK)?;
        let bus =
            literal_text(properties.one(subject, PRED_CONST_BUS)?, PRED_CONST_BUS)?.to_string();
        let value_text =
            literal_text(properties.one(subject, PRED_CONST_VALUE)?, PRED_CONST_VALUE)?;
        let value = value_text
            .split_whitespace()
            .map(|word| word.parse::<u64>())
            .collect::<std::result::Result<Vec<u64>, _>>()
            .map_err(|_| CircuitCrdfError::BadValue {
                predicate: PRED_CONST_VALUE.to_string(),
                value: value_text.to_string(),
            })?;
        constants.push((block, bus, value));
    }
    constants.sort();
    for (block, bus, value) in constants {
        dag.set_constant_words(block, &bus, &value);
    }

    let mut bindings = Vec::new();
    for subject in
        properties.subjects_of_type_owned_by(TYPE_BINDING, PRED_MEMBER_OF, &graph_subject)
    {
        let tag = literal_text(properties.one(subject, PRED_BIND_TAG)?, PRED_BIND_TAG)?.to_string();
        let block = parse_usize(properties.one(subject, PRED_BIND_BLOCK)?, PRED_BIND_BLOCK)?;
        let bus = literal_text(properties.one(subject, PRED_BIND_BUS)?, PRED_BIND_BUS)?.to_string();
        bindings.push((tag, block, bus));
    }
    bindings.sort();
    for (tag, block, bus) in bindings {
        dag.add_binding(&tag, block, &bus);
    }

    Ok(Some(dag))
}

fn parse_usize(term: &RdfTerm, predicate: &str) -> Result<usize> {
    literal_text(term, predicate)?
        .parse::<usize>()
        .map_err(|_| CircuitCrdfError::BadValue {
            predicate: predicate.to_string(),
            value: format!("{term:?}"),
        })
}

fn parse_u64(term: &RdfTerm, predicate: &str) -> Result<u64> {
    literal_text(term, predicate)?
        .parse::<u64>()
        .map_err(|_| CircuitCrdfError::BadValue {
            predicate: predicate.to_string(),
            value: format!("{term:?}"),
        })
}

fn parse_u8(term: &RdfTerm, predicate: &str) -> Result<u8> {
    literal_text(term, predicate)?
        .parse::<u8>()
        .map_err(|_| CircuitCrdfError::BadValue {
            predicate: predicate.to_string(),
            value: format!("{term:?}"),
        })
}

fn parse_f32(term: &RdfTerm, predicate: &str) -> Result<f32> {
    let value =
        literal_text(term, predicate)?
            .parse::<f32>()
            .map_err(|_| CircuitCrdfError::BadValue {
                predicate: predicate.to_string(),
                value: format!("{term:?}"),
            })?;
    if !value.is_finite() {
        return Err(CircuitCrdfError::BadValue {
            predicate: predicate.to_string(),
            value: format!("{term:?}"),
        });
    }
    Ok(value)
}

fn write_module_fresh(graph: &mut RdfGraph, module: &Module) -> Result<()> {
    let module_subject = iri(MODULE_PREFIX, module.id.0);
    add_triple(
        graph,
        module_subject.clone(),
        RDF_TYPE,
        RdfTerm::iri(TYPE_MODULE),
    )?;
    add_triple(
        graph,
        module_subject.clone(),
        PRED_MODULE_NAME,
        RdfTerm::literal(module.name.clone()),
    )?;

    for port in &module.ports {
        let subject = iri(PORT_PREFIX, port.id.0);
        add_triple(graph, subject.clone(), RDF_TYPE, RdfTerm::iri(TYPE_PORT))?;
        add_triple(graph, subject.clone(), PRED_PORT_OF, module_subject.clone())?;
        add_triple(
            graph,
            subject.clone(),
            PRED_PORT_NAME,
            RdfTerm::literal(port.name.clone()),
        )?;
        let direction = match port.direction {
            PortDirection::Input => DIRECTION_IN,
            PortDirection::Output => DIRECTION_OUT,
        };
        add_triple(
            graph,
            subject,
            PRED_PORT_DIRECTION,
            RdfTerm::literal(direction),
        )?;
    }

    // An intrinsic declaration has to survive a save. `Structure` above
    // is provenance and may be lost without changing anything; this is
    // read by the compiler, so losing it would make a circuit's speed
    // depend on whether it had been through a file.
    //
    // The triples hang on the **module's own subject**, not on a fresh
    // entity of their own. A module has at most one declaration, so it
    // needs no identity, and giving it one would mean minting a new
    // random subject on every save: writing the same asset twice would
    // leave two declarations, and a merge of two replicas that disagree
    // would leave two subjects rather than one conflict. On the module
    // subject, a second save is a no-op and a genuine disagreement
    // surfaces through `one()` as the conflict it is.
    if let Some(decl) = &module.intrinsic {
        add_triple(
            graph,
            module_subject.clone(),
            PRED_INTRINSIC_NAME,
            RdfTerm::literal(decl.name.clone()),
        )?;
        add_triple(
            graph,
            module_subject.clone(),
            PRED_INTRINSIC_REVISION,
            RdfTerm::literal(decl.revision.to_string()),
        )?;
        let port_list = |ports: &[PortId]| {
            ports
                .iter()
                .map(|p| p.0.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        };
        add_triple(
            graph,
            module_subject.clone(),
            PRED_INTRINSIC_INPUTS,
            RdfTerm::literal(port_list(&decl.inputs)),
        )?;
        add_triple(
            graph,
            module_subject.clone(),
            PRED_INTRINSIC_OUTPUTS,
            RdfTerm::literal(port_list(&decl.outputs)),
        )?;
    }

    for (index, term) in module.structures.iter().enumerate() {
        let (kind, label, body, levels) = match term {
            // The kind is written out and read back, so a map must not
            // travel as a chain: the two differ only in what they
            // promise about the cells, and a save that loses the
            // distinction turns a true term into a false one.
            crate::structure::Structure::Map(lane) => {
                ("map", &lane.label, lane.body, vec![lane.cells.clone()])
            }
            crate::structure::Structure::Chain(lane) => {
                ("chain", &lane.label, lane.body, vec![lane.cells.clone()])
            }
            crate::structure::Structure::Fold(tree) => {
                ("fold", &tree.label, tree.body, tree.levels.clone())
            }
        };
        // By the module and its place in the list: the same module
        // writes the same facts.
        let subject = iri(
            ENTITY_PREFIX,
            canonical_uuid(&format!("structure/{}/{index}", module.id.0)),
        );
        add_triple(
            graph,
            subject.clone(),
            RDF_TYPE,
            RdfTerm::iri(TYPE_STRUCTURE),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_STRUCTURE_OF,
            module_subject.clone(),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_STRUCTURE_KIND,
            RdfTerm::literal(kind),
        )?;
        // Its place in the list. Each structure gets a fresh random
        // subject and reading walks subjects in sorted order, so without
        // this the terms come back shuffled.
        add_triple(
            graph,
            subject.clone(),
            PRED_STRUCTURE_INDEX,
            RdfTerm::literal(index.to_string()),
        )?;
        add_triple(
            graph,
            subject.clone(),
            PRED_STRUCTURE_LABEL,
            RdfTerm::literal(label.clone()),
        )?;
        // The kind always; the module only when there is one. A bank of
        // registers has no module to name, and writing the kind by
        // omission would make it indistinguishable from an instance
        // body on the way back in.
        add_triple(
            graph,
            subject.clone(),
            PRED_STRUCTURE_BODY_KIND,
            RdfTerm::literal(match body {
                crate::model::CellKind::Instance { .. } => "instance",
                crate::model::CellKind::Nand => "nand",
                crate::model::CellKind::Reg { init: false } => "reg0",
                crate::model::CellKind::Reg { init: true } => "reg1",
            }),
        )?;
        if let crate::model::CellKind::Instance { module } = body {
            add_triple(
                graph,
                subject.clone(),
                PRED_STRUCTURE_BODY,
                iri(MODULE_PREFIX, module.0),
            )?;
        }
        add_triple(
            graph,
            subject,
            PRED_STRUCTURE_CELLS,
            RdfTerm::literal(
                levels
                    .iter()
                    .map(|level| {
                        level
                            .iter()
                            .map(|cell| cell.0.to_string())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .collect::<Vec<_>>()
                    .join("|"),
            ),
        )?;
    }

    for cell in &module.cells {
        let subject = iri(CELL_PREFIX, cell.id.0);
        add_triple(graph, subject.clone(), RDF_TYPE, RdfTerm::iri(TYPE_CELL))?;
        add_triple(graph, subject.clone(), PRED_CELL_OF, module_subject.clone())?;
        match cell.kind {
            CellKind::Nand => {
                add_triple(graph, subject, PRED_INSTANCE_OF, RdfTerm::iri(IRI_NAND))?;
            }
            CellKind::Reg { init } => {
                add_triple(
                    graph,
                    subject.clone(),
                    PRED_INSTANCE_OF,
                    RdfTerm::iri(IRI_REG),
                )?;
                add_triple(
                    graph,
                    subject,
                    PRED_REG_INIT,
                    RdfTerm::literal(if init { "true" } else { "false" }),
                )?;
            }
            CellKind::Instance { module: inner } => {
                add_triple(
                    graph,
                    subject,
                    PRED_INSTANCE_OF,
                    iri(MODULE_PREFIX, inner.0),
                )?;
            }
        }
    }

    for wire in &module.wires {
        let subject = iri(WIRE_PREFIX, wire.id.0);
        add_triple(graph, subject.clone(), RDF_TYPE, RdfTerm::iri(TYPE_WIRE))?;
        add_triple(graph, subject.clone(), PRED_WIRE_OF, module_subject.clone())?;
        write_endpoint(
            graph,
            &subject,
            wire.from,
            PRED_WIRE_FROM_CELL,
            PRED_WIRE_FROM_PORT,
        )?;
        write_endpoint(
            graph,
            &subject,
            wire.to,
            PRED_WIRE_TO_CELL,
            PRED_WIRE_TO_PORT,
        )?;
    }
    Ok(())
}

/// Refuses a graph giving one subject two values for one predicate, as
/// [`merge_graph_atomically`] would when merging it into an empty graph.
/// `rdf:type` may take several.
fn single_valued(graph: &RdfGraph) -> Result<()> {
    let mut values: HashMap<(RdfTerm, String), RdfTerm> = HashMap::new();
    for triple in graph.triples() {
        if triple.predicate == RDF_TYPE {
            continue;
        }
        let key = (triple.subject.clone(), triple.predicate.clone());
        match values.get(&key) {
            Some(object) if object != &triple.object => {
                return Err(CircuitCrdfError::ConflictingValues {
                    subject: triple
                        .subject
                        .as_iri()
                        .map_or_else(|| format!("{:?}", triple.subject), str::to_owned),
                    predicate: triple.predicate,
                });
            }
            Some(_) => {}
            None => {
                values.insert(key, triple.object);
            }
        }
    }
    Ok(())
}

/// Merges a freshly-built circuit subgraph into `graph` without
/// duplicating already-active triples. Circuit properties are
/// functional (except `rdf:type`): importing a different value for an
/// existing subject/predicate is rejected instead of manufacturing a
/// document that the loader would later reject. Mutations are staged on
/// a clone so any error leaves the caller's graph unchanged.
fn merge_graph_atomically(graph: &mut RdfGraph, candidate: &RdfGraph) -> Result<Vec<RdfOperation>> {
    // What the checks below ask of the staged graph, indexed once and kept
    // up to date as triples are staged. Asking the graph itself rescans all
    // of it for every triple — `objects_for_subject_predicate` rebuilds its
    // term map each time — and writing a patch of 1,095 facts into a fresh
    // graph took 140 ms in a release build, in an editor that writes one on
    // every edit.
    let present = graph.triples();
    let mut values: HashMap<(RdfTerm, String), Vec<RdfTerm>> = HashMap::new();
    for triple in &present {
        if triple.predicate != RDF_TYPE {
            values
                .entry((triple.subject.clone(), triple.predicate.clone()))
                .or_default()
                .push(triple.object.clone());
        }
    }
    let mut present: HashSet<crdf::Triple> = present.into_iter().collect();
    let mut staged = graph.clone();
    let mut operations = Vec::new();
    for triple in candidate.triples() {
        if triple.predicate != RDF_TYPE {
            let existing = values
                .entry((triple.subject.clone(), triple.predicate.clone()))
                .or_default();
            if existing.iter().any(|object| object != &triple.object) {
                return Err(CircuitCrdfError::ConflictingValues {
                    subject: triple
                        .subject
                        .as_iri()
                        .map_or_else(|| format!("{:?}", triple.subject), str::to_owned),
                    predicate: triple.predicate,
                });
            }
            if existing.is_empty() {
                existing.push(triple.object.clone());
            }
        }
        if present.insert(triple.clone()) {
            operations.push(add_triple(
                &mut staged,
                triple.subject,
                &triple.predicate,
                triple.object,
            )?);
        }
    }
    *graph = staged;
    Ok(operations)
}

/// The set of modules reachable from the asset root through instance
/// cells. A reference to a module missing from the library is an error.
fn reachable_modules(asset: &CircuitAsset) -> Result<std::collections::BTreeSet<ModuleId>> {
    let mut reachable = std::collections::BTreeSet::new();
    let mut queue = vec![asset.root];
    while let Some(id) = queue.pop() {
        if !reachable.insert(id) {
            continue;
        }
        let module = asset
            .library
            .get(id)
            .ok_or_else(|| CircuitCrdfError::DanglingReference {
                subject: format!("{ASSET_PREFIX}{}", asset.id.0),
                reference: format!("{MODULE_PREFIX}{}", id.0),
            })?;
        for cell in &module.cells {
            if let CellKind::Instance { module: inner } = cell.kind {
                queue.push(inner);
            }
        }
    }
    Ok(reachable)
}

pub(crate) fn write_endpoint(
    graph: &mut RdfGraph,
    wire_subject: &RdfTerm,
    endpoint: Endpoint,
    cell_predicate: &str,
    port_predicate: &str,
) -> Result<()> {
    match endpoint {
        Endpoint::Module { port } => add_triple(
            graph,
            wire_subject.clone(),
            port_predicate,
            iri(PORT_PREFIX, port.0),
        )
        .map(|_| ()),
        Endpoint::Cell { cell, port } => {
            add_triple(
                graph,
                wire_subject.clone(),
                cell_predicate,
                iri(CELL_PREFIX, cell.0),
            )?;
            let port_term = match port {
                PortRef::NandA => RdfTerm::iri(IRI_NAND_A),
                PortRef::NandB => RdfTerm::iri(IRI_NAND_B),
                PortRef::NandY => RdfTerm::iri(IRI_NAND_Y),
                PortRef::RegD => RdfTerm::iri(IRI_REG_D),
                PortRef::RegQ => RdfTerm::iri(IRI_REG_Q),
                PortRef::Inner(inner) => iri(PORT_PREFIX, inner.0),
            };
            add_triple(graph, wire_subject.clone(), port_predicate, port_term).map(|_| ())
        }
    }
}

/// Property table built once from the triple list: subject IRI string →
/// predicate → distinct objects. Identical duplicate triples collapse;
/// distinct values for a functional property are reported as conflicts.
struct Properties {
    by_subject: BTreeMap<String, BTreeMap<String, HashSet<RdfTerm>>>,
}

impl Properties {
    fn new(graph: &RdfGraph) -> Self {
        let mut by_subject: BTreeMap<String, BTreeMap<String, HashSet<RdfTerm>>> = BTreeMap::new();
        for triple in graph.triples() {
            let Some(subject) = triple.subject.as_iri() else {
                continue;
            };
            by_subject
                .entry(subject.to_string())
                .or_default()
                .entry(triple.predicate.clone())
                .or_default()
                .insert(triple.object.clone());
        }
        Self { by_subject }
    }

    fn subjects_of_type(&self, type_iri: &str) -> Vec<&str> {
        let target = RdfTerm::iri(type_iri);
        self.by_subject
            .iter()
            .filter(|(_, predicates)| {
                predicates
                    .get(RDF_TYPE)
                    .is_some_and(|objects| objects.contains(&target))
            })
            .map(|(subject, _)| subject.as_str())
            .collect()
    }

    fn has_type(&self, subject: &str, type_iri: &str) -> bool {
        self.by_subject
            .get(subject)
            .and_then(|predicates| predicates.get(RDF_TYPE))
            .is_some_and(|objects| objects.contains(&RdfTerm::iri(type_iri)))
    }

    /// Typed subjects whose functional owner property points at
    /// `owner`. The later call to [`Self::one`] still detects an owner
    /// conflict; this query only limits loading to one asset closure.
    fn subjects_of_type_owned_by(
        &self,
        type_iri: &str,
        owner_predicate: &str,
        owner: &RdfTerm,
    ) -> Vec<&str> {
        let target_type = RdfTerm::iri(type_iri);
        self.by_subject
            .iter()
            .filter(|(_, predicates)| {
                predicates
                    .get(RDF_TYPE)
                    .is_some_and(|objects| objects.contains(&target_type))
                    && predicates
                        .get(owner_predicate)
                        .is_some_and(|objects| objects.contains(owner))
            })
            .map(|(subject, _)| subject.as_str())
            .collect()
    }

    fn one(&self, subject: &str, predicate: &str) -> Result<&RdfTerm> {
        let objects = self
            .by_subject
            .get(subject)
            .and_then(|predicates| predicates.get(predicate))
            .ok_or_else(|| CircuitCrdfError::MissingValue {
                subject: subject.to_string(),
                predicate: predicate.to_string(),
            })?;
        match objects.len() {
            1 => Ok(objects.iter().next().expect("len 1")),
            0 => Err(CircuitCrdfError::MissingValue {
                subject: subject.to_string(),
                predicate: predicate.to_string(),
            }),
            _ => Err(CircuitCrdfError::ConflictingValues {
                subject: subject.to_string(),
                predicate: predicate.to_string(),
            }),
        }
    }

    fn optional(&self, subject: &str, predicate: &str) -> Result<Option<&RdfTerm>> {
        let Some(objects) = self
            .by_subject
            .get(subject)
            .and_then(|predicates| predicates.get(predicate))
        else {
            return Ok(None);
        };
        match objects.len() {
            0 => Ok(None),
            1 => Ok(Some(objects.iter().next().expect("len 1"))),
            _ => Err(CircuitCrdfError::ConflictingValues {
                subject: subject.to_string(),
                predicate: predicate.to_string(),
            }),
        }
    }
}

fn literal_text<'a>(term: &'a RdfTerm, predicate: &str) -> Result<&'a str> {
    term.as_literal()
        .map(|literal| literal.value())
        .ok_or_else(|| CircuitCrdfError::BadValue {
            predicate: predicate.to_string(),
            value: format!("{term:?}"),
        })
}

/// Lists every circuit asset in a graph, sorted by id.
pub fn list_assets(graph: &RdfGraph) -> Result<Vec<AssetId>> {
    let properties = Properties::new(graph);
    let mut ids = Vec::new();
    for subject in properties.subjects_of_type(TYPE_ASSET) {
        ids.push(AssetId(parse_uuid_iri(
            &RdfTerm::iri(subject),
            ASSET_PREFIX,
            RDF_TYPE,
        )?));
    }
    ids.sort();
    Ok(ids)
}

/// Loads one asset (by id) from a graph that may hold several assets;
/// module definitions are shared between them. The returned library
/// contains only the selected root's transitive module closure, so an
/// invalid resource owned exclusively by another asset cannot poison
/// this load.
pub fn asset_from_rdf_by_id(graph: &RdfGraph, id: AssetId) -> Result<CircuitAsset> {
    let properties = Properties::new(graph);
    let asset_subject = format!("{ASSET_PREFIX}{}", id.0);
    if !properties
        .subjects_of_type(TYPE_ASSET)
        .contains(&asset_subject.as_str())
    {
        return Err(CircuitCrdfError::MissingAsset);
    }
    load_asset(&properties, id, &asset_subject)
}

/// Convenience: loads the single asset of a standalone graph. Errors
/// when the graph holds none or several assets; use [`list_assets`] +
/// [`asset_from_rdf_by_id`] for multi-asset graphs.
pub fn asset_from_rdf(graph: &RdfGraph) -> Result<CircuitAsset> {
    let assets = list_assets(graph)?;
    match assets.as_slice() {
        [] => Err(CircuitCrdfError::MissingAsset),
        [one] => asset_from_rdf_by_id(graph, *one),
        _ => Err(CircuitCrdfError::MultipleAssets),
    }
}

fn load_asset(properties: &Properties, id: AssetId, asset_subject: &str) -> Result<CircuitAsset> {
    let version_text = literal_text(
        properties.one(asset_subject, PRED_SCHEMA_VERSION)?,
        PRED_SCHEMA_VERSION,
    )?;
    let version: u32 = version_text
        .parse()
        .map_err(|_| CircuitCrdfError::BadValue {
            predicate: PRED_SCHEMA_VERSION.to_string(),
            value: version_text.to_string(),
        })?;
    if version != CIRCUIT_CRDF_SCHEMA_VERSION {
        return Err(CircuitCrdfError::UnsupportedSchemaVersion(version));
    }

    let root = ModuleId(parse_uuid_iri(
        properties.one(asset_subject, PRED_ROOT_MODULE)?,
        MODULE_PREFIX,
        PRED_ROOT_MODULE,
    )?);
    let ticks_text = literal_text(
        properties.one(asset_subject, PRED_TICKS_PER_STEP)?,
        PRED_TICKS_PER_STEP,
    )?;
    let ticks_per_step: u8 = ticks_text.parse().map_err(|_| CircuitCrdfError::BadValue {
        predicate: PRED_TICKS_PER_STEP.to_string(),
        value: ticks_text.to_string(),
    })?;

    // Load only the root's transitive module closure. A malformed
    // resource owned by another asset must not prevent this asset from
    // compiling; malformed resources inside the closure still fail.
    let mut modules: BTreeMap<ModuleId, Module> = BTreeMap::new();
    let mut port_owner: BTreeMap<PortId, ModuleId> = BTreeMap::new();
    let mut cell_owner: BTreeMap<CellId, ModuleId> = BTreeMap::new();
    let mut cell_kind: BTreeMap<CellId, CellKind> = BTreeMap::new();
    let mut queue = vec![(root, asset_subject.to_string())];

    while let Some((module_id, referrer)) = queue.pop() {
        if modules.contains_key(&module_id) {
            continue;
        }
        let module_subject = format!("{MODULE_PREFIX}{}", module_id.0);
        if !properties.has_type(&module_subject, TYPE_MODULE) {
            return Err(CircuitCrdfError::DanglingReference {
                subject: referrer,
                reference: module_subject,
            });
        }
        let name = literal_text(
            properties.one(&module_subject, PRED_MODULE_NAME)?,
            PRED_MODULE_NAME,
        )?;
        modules.insert(
            module_id,
            Module {
                id: module_id,
                name: name.to_string(),
                ports: Vec::new(),
                cells: Vec::new(),
                wires: Vec::new(),
                // Provenance is not persisted yet, so a reloaded module
                // is undescribed rather than wrongly described.
                structures: Vec::new(),
                intrinsic: None,
            },
        );
        let owner_term = iri(MODULE_PREFIX, module_id.0);

        for subject in properties.subjects_of_type_owned_by(TYPE_PORT, PRED_PORT_OF, &owner_term) {
            // Enforce functional ownership even though the reverse
            // lookup above already found this owner among the values.
            let declared_owner = ModuleId(parse_uuid_iri(
                properties.one(subject, PRED_PORT_OF)?,
                MODULE_PREFIX,
                PRED_PORT_OF,
            )?);
            debug_assert_eq!(declared_owner, module_id);
            let id = PortId(parse_uuid_iri(
                &RdfTerm::iri(subject),
                PORT_PREFIX,
                RDF_TYPE,
            )?);
            let name = literal_text(properties.one(subject, PRED_PORT_NAME)?, PRED_PORT_NAME)?;
            let direction_text = literal_text(
                properties.one(subject, PRED_PORT_DIRECTION)?,
                PRED_PORT_DIRECTION,
            )?;
            let direction = match direction_text {
                DIRECTION_IN => PortDirection::Input,
                DIRECTION_OUT => PortDirection::Output,
                other => {
                    return Err(CircuitCrdfError::BadValue {
                        predicate: PRED_PORT_DIRECTION.to_string(),
                        value: other.to_string(),
                    });
                }
            };
            port_owner.insert(id, module_id);
            modules
                .get_mut(&module_id)
                .expect("module inserted")
                .ports
                .push(Port {
                    id,
                    name: name.to_string(),
                    direction,
                });
        }

        // The declaration hangs on the module's own subject, so its
        // presence is simply whether the name is there.
        if let Some(name_term) = properties.optional(&module_subject, PRED_INTRINSIC_NAME)? {
            let name = literal_text(name_term, PRED_INTRINSIC_NAME)?.to_string();
            let revision_text = literal_text(
                properties.one(&module_subject, PRED_INTRINSIC_REVISION)?,
                PRED_INTRINSIC_REVISION,
            )?;
            let revision: u32 = revision_text
                .parse()
                .map_err(|_| CircuitCrdfError::BadValue {
                    predicate: PRED_INTRINSIC_REVISION.to_string(),
                    value: revision_text.to_string(),
                })?;
            let port_list = |text: &str, predicate: &str| -> Result<Vec<PortId>> {
                text.split_whitespace()
                    .map(|word| {
                        Uuid::parse_str(word)
                            .map(PortId)
                            .map_err(|_| CircuitCrdfError::BadValue {
                                predicate: predicate.to_string(),
                                value: word.to_string(),
                            })
                    })
                    .collect()
            };
            let inputs = port_list(
                literal_text(
                    properties.one(&module_subject, PRED_INTRINSIC_INPUTS)?,
                    PRED_INTRINSIC_INPUTS,
                )?,
                PRED_INTRINSIC_INPUTS,
            )?;
            let outputs = port_list(
                literal_text(
                    properties.one(&module_subject, PRED_INTRINSIC_OUTPUTS)?,
                    PRED_INTRINSIC_OUTPUTS,
                )?,
                PRED_INTRINSIC_OUTPUTS,
            )?;
            modules
                .get_mut(&module_id)
                .expect("module exists")
                .intrinsic = Some(crate::model::IntrinsicDecl {
                name,
                revision,
                inputs,
                outputs,
            });
        }

        let mut ordered_terms: BTreeMap<ModuleId, Vec<(usize, crate::structure::Structure)>> =
            BTreeMap::new();
        for subject in
            properties.subjects_of_type_owned_by(TYPE_STRUCTURE, PRED_STRUCTURE_OF, &owner_term)
        {
            let kind = literal_text(
                properties.one(subject, PRED_STRUCTURE_KIND)?,
                PRED_STRUCTURE_KIND,
            )?
            .to_string();
            let label = literal_text(
                properties.one(subject, PRED_STRUCTURE_LABEL)?,
                PRED_STRUCTURE_LABEL,
            )?
            .to_string();
            // The module, when the file names one.
            let module = properties
                .optional(subject, PRED_STRUCTURE_BODY)
                .ok()
                .flatten()
                .and_then(|term| parse_uuid_iri(term, MODULE_PREFIX, PRED_STRUCTURE_BODY).ok())
                .map(ModuleId);
            // **A file written before primitives could be a body names
            // a module and no kind.** An instance body is the only thing
            // such a file can mean, so that is what it reads as; there
            // is no version to check and nothing to migrate.
            let body = match properties
                .optional(subject, PRED_STRUCTURE_BODY_KIND)
                .ok()
                .flatten()
                .and_then(|term| literal_text(term, PRED_STRUCTURE_BODY_KIND).ok())
            {
                Some("nand") => crate::model::CellKind::Nand,
                Some("reg0") => crate::model::CellKind::Reg { init: false },
                Some("reg1") => crate::model::CellKind::Reg { init: true },
                // "instance", or an older file with no kind at all.
                _ => match module {
                    Some(module) => crate::model::CellKind::Instance { module },
                    // A term naming neither a kind this reader knows nor
                    // a module is not something to guess at.
                    None => continue,
                },
            };
            let cells_text = literal_text(
                properties.one(subject, PRED_STRUCTURE_CELLS)?,
                PRED_STRUCTURE_CELLS,
            )?;
            let levels: Vec<Vec<CellId>> = cells_text
                .split('|')
                .map(|level| {
                    level
                        .split_whitespace()
                        .filter_map(|id| Uuid::parse_str(id).ok().map(CellId))
                        .collect()
                })
                .collect();
            // A term nobody can read back is worse than none, so an
            // unrecognised kind is dropped rather than guessed at.
            let term = match kind.as_str() {
                "map" => Some(crate::structure::Structure::Map(crate::structure::Lane {
                    label: label.clone(),
                    body,
                    cells: levels.iter().flatten().copied().collect(),
                })),
                "chain" => Some(crate::structure::Structure::Chain(crate::structure::Lane {
                    label,
                    body,
                    cells: levels.into_iter().flatten().collect(),
                })),
                "fold" => Some(crate::structure::Structure::Fold(crate::structure::Tree {
                    label,
                    body,
                    levels,
                })),
                _ => None,
            };
            if let Some(term) = term {
                // Keyed by the written index so the list comes back in the
                // order it went out. A file from before the index existed
                // has none, and its terms keep the order the subjects
                // happen to sort in — which is what it always did.
                let index = properties
                    .optional(subject, PRED_STRUCTURE_INDEX)
                    .ok()
                    .flatten()
                    .and_then(|t| literal_text(t, PRED_STRUCTURE_INDEX).ok())
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(usize::MAX);
                ordered_terms
                    .entry(module_id)
                    .or_default()
                    .push((index, term));
            }
        }
        for (module_id, mut terms) in std::mem::take(&mut ordered_terms) {
            terms.sort_by_key(|(index, _)| *index);
            let module = modules.get_mut(&module_id).expect("module exists");
            module.structures = terms.into_iter().map(|(_, term)| term).collect();
        }

        for subject in properties.subjects_of_type_owned_by(TYPE_CELL, PRED_CELL_OF, &owner_term) {
            let declared_owner = ModuleId(parse_uuid_iri(
                properties.one(subject, PRED_CELL_OF)?,
                MODULE_PREFIX,
                PRED_CELL_OF,
            )?);
            debug_assert_eq!(declared_owner, module_id);
            let id = CellId(parse_uuid_iri(
                &RdfTerm::iri(subject),
                CELL_PREFIX,
                RDF_TYPE,
            )?);
            let instance_of = properties.one(subject, PRED_INSTANCE_OF)?;
            let kind = match instance_of.as_iri() {
                Some(IRI_NAND) => CellKind::Nand,
                Some(IRI_REG) => {
                    let init_text =
                        literal_text(properties.one(subject, PRED_REG_INIT)?, PRED_REG_INIT)?;
                    let init = match init_text {
                        "true" => true,
                        "false" => false,
                        other => {
                            return Err(CircuitCrdfError::BadValue {
                                predicate: PRED_REG_INIT.to_string(),
                                value: other.to_string(),
                            });
                        }
                    };
                    CellKind::Reg { init }
                }
                _ => {
                    let inner = ModuleId(parse_uuid_iri(
                        instance_of,
                        MODULE_PREFIX,
                        PRED_INSTANCE_OF,
                    )?);
                    queue.push((inner, subject.to_string()));
                    CellKind::Instance { module: inner }
                }
            };
            if !matches!(kind, CellKind::Reg { .. })
                && properties.optional(subject, PRED_REG_INIT)?.is_some()
            {
                return Err(CircuitCrdfError::BadValue {
                    predicate: PRED_REG_INIT.to_string(),
                    value: format!("{subject} is not a circuit:reg cell"),
                });
            }
            cell_owner.insert(id, module_id);
            cell_kind.insert(id, kind);
            modules
                .get_mut(&module_id)
                .expect("module inserted")
                .cells
                .push(Cell { id, kind });
        }
    }

    // Parse wires after every reachable module's ports and cells are
    // indexed, because instance endpoints refer to the child module's
    // port identities.
    let module_ids: Vec<ModuleId> = modules.keys().copied().collect();
    for owner in module_ids {
        let owner_term = iri(MODULE_PREFIX, owner.0);
        for subject in properties.subjects_of_type_owned_by(TYPE_WIRE, PRED_WIRE_OF, &owner_term) {
            let declared_owner = ModuleId(parse_uuid_iri(
                properties.one(subject, PRED_WIRE_OF)?,
                MODULE_PREFIX,
                PRED_WIRE_OF,
            )?);
            debug_assert_eq!(declared_owner, owner);
            let id = WireId(parse_uuid_iri(
                &RdfTerm::iri(subject),
                WIRE_PREFIX,
                RDF_TYPE,
            )?);
            let context = EndpointContext {
                owner,
                cell_owner: &cell_owner,
                cell_kind: &cell_kind,
                port_owner: &port_owner,
            };
            let from = read_endpoint(
                properties,
                subject,
                PRED_WIRE_FROM_CELL,
                PRED_WIRE_FROM_PORT,
                &context,
            )?;
            let to = read_endpoint(
                properties,
                subject,
                PRED_WIRE_TO_CELL,
                PRED_WIRE_TO_PORT,
                &context,
            )?;
            modules
                .get_mut(&owner)
                .expect("module loaded")
                .wires
                .push(Wire { id, from, to });
        }
    }

    // Canonical member order: sort by UUID.
    let mut library = ModuleLibrary::new();
    for (_, mut module) in modules {
        module.ports.sort_by_key(|port| port.id);
        module.cells.sort_by_key(|cell| cell.id);
        module.wires.sort_by_key(|wire| wire.id);
        library.insert(module);
    }

    if library.get(root).is_none() {
        return Err(CircuitCrdfError::DanglingReference {
            subject: asset_subject.to_string(),
            reference: format!("{MODULE_PREFIX}{}", root.0),
        });
    }

    Ok(CircuitAsset {
        id,
        root,
        ticks_per_step,
        library,
    })
}

/// Ownership/kind context for validating one wire's endpoints.
struct EndpointContext<'a> {
    /// The module the wire belongs to (`crdf:wireOf`).
    owner: ModuleId,
    cell_owner: &'a BTreeMap<CellId, ModuleId>,
    cell_kind: &'a BTreeMap<CellId, CellKind>,
    port_owner: &'a BTreeMap<PortId, ModuleId>,
}

fn read_endpoint(
    properties: &Properties,
    wire_subject: &str,
    cell_predicate: &str,
    port_predicate: &str,
    context: &EndpointContext<'_>,
) -> Result<Endpoint> {
    let port_term = properties.one(wire_subject, port_predicate)?;
    let cell_term = properties.optional(wire_subject, cell_predicate)?;

    let bad = |reason: &str| CircuitCrdfError::BadEndpoint {
        wire: wire_subject.to_string(),
        reason: reason.to_string(),
    };

    match cell_term {
        None => {
            // Own module port.
            let port = PortId(parse_uuid_iri(port_term, PORT_PREFIX, port_predicate)?);
            match context.port_owner.get(&port) {
                None => Err(bad("references an unknown port")),
                Some(owner) if *owner != context.owner => {
                    Err(bad("references a port of a different module"))
                }
                Some(_) => Ok(Endpoint::Module { port }),
            }
        }
        Some(cell_term) => {
            let cell = CellId(parse_uuid_iri(cell_term, CELL_PREFIX, cell_predicate)?);
            match context.cell_owner.get(&cell) {
                None => return Err(bad("references an unknown cell")),
                Some(owner) if *owner != context.owner => {
                    return Err(bad("references a cell of a different module"));
                }
                Some(_) => {}
            }
            let kind = context.cell_kind.get(&cell).expect("owner implies kind");
            let port = match port_term.as_iri() {
                Some(IRI_NAND_A) => PortRef::NandA,
                Some(IRI_NAND_B) => PortRef::NandB,
                Some(IRI_NAND_Y) => PortRef::NandY,
                Some(IRI_REG_D) => PortRef::RegD,
                Some(IRI_REG_Q) => PortRef::RegQ,
                _ => {
                    let inner = PortId(parse_uuid_iri(port_term, PORT_PREFIX, port_predicate)?);
                    let CellKind::Instance { module: target } = kind else {
                        return Err(bad("instance port used on an axiom cell"));
                    };
                    match context.port_owner.get(&inner) {
                        None => return Err(bad("references an unknown instance port")),
                        Some(owner) if owner != target => {
                            return Err(bad(
                                "references a port that does not belong to the instantiated module",
                            ));
                        }
                        Some(_) => {}
                    }
                    PortRef::Inner(inner)
                }
            };
            match (kind, port) {
                (CellKind::Nand, PortRef::NandA | PortRef::NandB | PortRef::NandY)
                | (CellKind::Reg { .. }, PortRef::RegD | PortRef::RegQ)
                | (CellKind::Instance { .. }, PortRef::Inner(_)) => {}
                _ => return Err(bad("pin does not exist on that cell kind")),
            }
            Ok(Endpoint::Cell { cell, port })
        }
    }
}

/// Saves an asset as a standalone binary `.crdf` file.
pub fn save_asset_file(asset: &CircuitAsset, path: impl AsRef<Path>) -> Result<()> {
    let rdf = asset_to_rdf(asset)?;
    rdf.write_rdf_file(path, RdfFileFormat::FlatBuffers)
        .map_err(|e| CircuitCrdfError::File(format!("{e:?}")))
}

/// Loads an asset from a standalone binary `.crdf` file.
pub fn load_asset_file(path: impl AsRef<Path>) -> Result<CircuitAsset> {
    let rdf = RdfGraph::read_flatbuffers_file(path)
        .map_err(|e| CircuitCrdfError::File(format!("{e:?}")))?;
    asset_from_rdf(&rdf)
}

/// Saves a block-DAG (its editable structure) to a standalone `.crdf` file.
pub fn save_block_dag_file(dag: &BlockDag, path: impl AsRef<Path>) -> Result<()> {
    let mut rdf = RdfGraph::new();
    write_block_dag_into(&mut rdf, dag)?;
    rdf.write_rdf_file(path, RdfFileFormat::FlatBuffers)
        .map_err(|e| CircuitCrdfError::File(format!("{e:?}")))
}

/// Loads a block-DAG from a `.crdf` file, or `None` if it holds none.
pub fn load_block_dag_file(path: impl AsRef<Path>) -> Result<Option<BlockDag>> {
    let rdf = RdfGraph::read_flatbuffers_file(path)
        .map_err(|e| CircuitCrdfError::File(format!("{e:?}")))?;
    read_block_dag(&rdf)
}

#[cfg(test)]
mod bus_interface_tests {
    use super::*;
    use crate::types::Word;
    use crdf::RdfGraph;

    fn signal() -> (Type, RoleTag) {
        (
            Type::word(Word::signed_q(1, 15)),
            RoleTag::from_static("signal"),
        )
    }
    fn level() -> (Type, RoleTag) {
        (
            Type::word(Word::unsigned_q(0, 15)),
            RoleTag::from_static("level"),
        )
    }

    #[test]
    fn block_interface_round_trips_through_crdf() {
        let module = ModuleId::new_random();
        let (a_ty, a_role) = signal();
        let (c_ty, c_role) = level();
        let iface = BlockInterface {
            module,
            inputs: vec![
                ("in".to_string(), a_ty.clone(), a_role.clone()),
                ("f".to_string(), c_ty.clone(), c_role.clone()),
                ("q".to_string(), c_ty, c_role),
            ],
            outputs: vec![
                ("y0".to_string(), a_ty.clone(), a_role.clone()),
                ("y1".to_string(), a_ty, a_role),
            ],
        };

        let mut graph = RdfGraph::new();
        write_block_interface_into(&mut graph, &iface).expect("write");
        // Deterministic bus ids ⇒ writing the same interface again is a
        // no-op merge, not a conflict.
        write_block_interface_into(&mut graph, &iface).expect("idempotent write");

        let read = read_block_interface(&graph, module)
            .expect("read ok")
            .expect("interface present");
        assert_eq!(read.module, module);
        assert_eq!(read.inputs.len(), 3);
        assert_eq!(read.outputs.len(), 2);

        // Bus order is not significant; compare by name lookup (type + role).
        let find = |buses: &[(String, Type, RoleTag)], name: &str| {
            buses
                .iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, t, r)| (t.clone(), r.clone()))
        };
        for (name, ty, role) in iface.inputs.iter().chain(&iface.outputs) {
            let side = if iface.inputs.iter().any(|(n, _, _)| n == name) {
                &read.inputs
            } else {
                &read.outputs
            };
            assert_eq!(
                find(side, name),
                Some((ty.clone(), role.clone())),
                "bus {name}"
            );
        }
    }

    #[test]
    fn missing_interface_reads_none() {
        let graph = RdfGraph::new();
        assert_eq!(
            read_block_interface(&graph, ModuleId::new_random()).expect("read ok"),
            None
        );
    }

    fn word(w: Word) -> Type {
        Type::word(w)
    }

    fn bus(name: &str, tr: (Type, RoleTag)) -> (String, Type, RoleTag) {
        (name.to_string(), tr.0, tr.1)
    }

    /// Saving an unchanged block graph gives the same facts every time, and
    /// so does saving what was read back — so a history that diffs two saves
    /// finds only what changed. Another graph's identity names different
    /// things even with the same blocks.
    #[test]
    fn an_unchanged_block_dag_writes_the_same_facts() {
        let mut dag = BlockDag::new();
        let a = dag.add_block(BlockInterface {
            module: ModuleId::new_random(),
            inputs: vec![bus(
                "inc",
                (word(Word::uint(24)), RoleTag::from_static("increment")),
            )],
            outputs: vec![bus("signal", signal())],
        });
        let b = dag.add_block(BlockInterface {
            module: ModuleId::new_random(),
            inputs: vec![bus("in", signal()), bus("f", level())],
            outputs: vec![bus("y0", signal())],
        });
        dag.connect(a, "signal", b, "in");
        dag.expose_input("inc", a, "inc", word(Word::uint(24)));
        dag.expose_output("signal", b, "y0", signal().0);
        dag.set_constant(b, "f", 0x4321);
        dag.add_binding("external", a, "inc");
        dag.set_boundary_position("IN", [10.0, 20.0]);
        let facts = |dag: &BlockDag| -> HashSet<crdf::Triple> {
            let mut graph = RdfGraph::new();
            write_block_dag_into(&mut graph, dag).expect("write");
            graph.triples().into_iter().collect()
        };
        assert_eq!(facts(&dag), facts(&dag));

        let mut graph = RdfGraph::new();
        write_block_dag_into(&mut graph, &dag).expect("write");
        let read = read_block_dag(&graph).expect("read").expect("present");
        assert_eq!(read.id(), dag.id(), "the identity comes back");
        assert_eq!(facts(&read), facts(&dag), "a save of what was read");

        let subjects = |dag: &BlockDag| -> HashSet<RdfTerm> {
            facts(dag)
                .into_iter()
                .filter(|fact| fact.predicate == PRED_MEMBER_OF)
                .map(|fact| fact.subject)
                .filter(|subject| {
                    !dag.block_metadata()
                        .iter()
                        .any(|metadata| *subject == iri(ENTITY_PREFIX, metadata.id))
                })
                .collect()
        };
        let mut other = dag.clone();
        other.set_id(Uuid::new_v4());
        assert!(!subjects(&dag).is_empty());
        assert!(
            subjects(&dag).is_disjoint(&subjects(&other)),
            "another graph names its own connections, ports and nodes"
        );
    }

    /// A graph with history but no facts left still goes through the
    /// merge, which keeps that history. Only a graph nothing was ever
    /// written to takes the candidate as it is.
    #[test]
    fn writing_into_a_graph_with_history_keeps_it() {
        let mut dag = BlockDag::new();
        dag.add_block(BlockInterface {
            module: ModuleId::new_random(),
            inputs: vec![bus("in", signal())],
            outputs: vec![bus("out", signal())],
        });
        let mut fresh = RdfGraph::new();
        write_block_dag_into(&mut fresh, &dag).expect("write");

        let mut used = RdfGraph::new();
        let (subject, object) = (RdfTerm::iri("urn:test:a"), RdfTerm::literal("1"));
        used.add_triple(subject.clone(), "urn:test:p", object.clone())
            .unwrap();
        used.remove_triple(&subject, "urn:test:p", &object).unwrap();
        assert!(used.is_empty() && !used.all_edges_removed().is_empty());
        write_block_dag_into(&mut used, &dag).expect("write");
        assert!(!used.all_edges_removed().is_empty(), "the removal is kept");
        let facts =
            |graph: &RdfGraph| -> HashSet<crdf::Triple> { graph.triples().into_iter().collect() };
        assert_eq!(facts(&used), facts(&fresh));
    }

    #[test]
    fn a_merge_refuses_a_second_value_for_a_property() {
        let subject = RdfTerm::iri("urn:test:a");
        let graph_of = |facts: &[(&str, &str)]| {
            let mut graph = RdfGraph::new();
            for (predicate, value) in facts {
                graph
                    .add_triple(subject.clone(), *predicate, RdfTerm::literal(*value))
                    .unwrap();
            }
            graph
        };
        let facts =
            |graph: &RdfGraph| -> HashSet<crdf::Triple> { graph.triples().into_iter().collect() };
        // The first value is in the target.
        let mut target = graph_of(&[("urn:test:p", "1")]);
        let before = facts(&target);
        let candidate = graph_of(&[("urn:test:p", "2")]);
        assert!(matches!(
            merge_graph_atomically(&mut target, &candidate),
            Err(CircuitCrdfError::ConflictingValues { .. })
        ));
        assert_eq!(facts(&target), before, "a refused merge changes nothing");
        // The first value is earlier in the candidate itself.
        let mut target = graph_of(&[("urn:test:other", "x")]);
        let candidate = graph_of(&[("urn:test:p", "1"), ("urn:test:p", "2")]);
        assert!(matches!(
            merge_graph_atomically(&mut target, &candidate),
            Err(CircuitCrdfError::ConflictingValues { .. })
        ));
        // The same value again is no conflict, and is not added twice.
        let mut target = graph_of(&[("urn:test:p", "1")]);
        let candidate = graph_of(&[("urn:test:p", "1"), ("urn:test:q", "2")]);
        let operations = merge_graph_atomically(&mut target, &candidate).unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(target.len(), 2);
    }

    #[test]
    fn a_graph_with_two_values_for_one_property_is_refused() {
        let mut graph = RdfGraph::new();
        let subject = RdfTerm::iri("urn:test:a");
        for value in ["1", "2"] {
            graph
                .add_triple(subject.clone(), "urn:test:value", RdfTerm::literal(value))
                .unwrap();
        }
        assert!(matches!(
            single_valued(&graph),
            Err(CircuitCrdfError::ConflictingValues { .. })
        ));

        let mut typed = RdfGraph::new();
        for class in ["urn:test:A", "urn:test:B"] {
            typed
                .add_triple(subject.clone(), RDF_TYPE, RdfTerm::iri(class))
                .unwrap();
        }
        assert!(single_valued(&typed).is_ok(), "rdf:type may take several");
    }

    #[test]
    fn an_unchanged_graph_rejects_duplicate_block_identity() {
        let mut dag = BlockDag::new();
        let iface = BlockInterface {
            module: ModuleId::new_random(),
            inputs: vec![],
            outputs: vec![],
        };
        let metadata = BlockMetadata::default();
        dag.add_block_with_metadata(iface.clone(), metadata.clone());
        dag.add_block_with_metadata(iface, metadata);
        let mut graph = RdfGraph::new();
        assert!(write_block_dag_into(&mut graph, &dag).is_err());
        assert!(graph.is_empty());
    }

    #[test]
    fn an_unchanged_graph_keys_are_unambiguous() {
        let dag = BlockDag::new();
        assert_ne!(
            scoped(&dag, "connection", &["a", "x/b/y", "b", "z"]),
            scoped(&dag, "connection", &["a", "x", "b", "y/b/z"])
        );
        assert_ne!(
            scoped(&dag, "binding", &["tag/b/x", "b", "y"]),
            scoped(&dag, "binding", &["tag", "b", "x/b/y"])
        );
    }

    #[test]
    fn an_unchanged_graph_preserves_boundary_order() {
        let mut dag = BlockDag::new();
        let b = dag.add_block(BlockInterface {
            module: ModuleId::new_random(),
            inputs: vec![bus("z", signal()), bus("a", signal())],
            outputs: vec![],
        });
        dag.expose_input("z", b, "z", signal().0);
        dag.expose_input("a", b, "a", signal().0);
        dag.set_boundary_position("z", [1.0, 2.0]);
        dag.set_boundary_position("a", [3.0, 4.0]);
        let mut graph = RdfGraph::new();
        write_block_dag_into(&mut graph, &dag).unwrap();
        let read = read_block_dag(&graph).unwrap().unwrap();
        assert_eq!(read.exposed_inputs(), dag.exposed_inputs());
        assert_eq!(read.boundary_positions(), dag.boundary_positions());
    }

    #[test]
    fn block_dag_round_trips_through_crdf() {
        // Two blocks, a connection, an exposed input and output.
        let osc = ModuleId::new_random();
        let filt = ModuleId::new_random();
        let mut dag = BlockDag::new();
        let a = dag.add_block(BlockInterface {
            module: osc,
            inputs: vec![bus(
                "inc",
                (word(Word::uint(24)), RoleTag::from_static("increment")),
            )],
            outputs: vec![bus("signal", signal())],
        });
        let b = dag.add_block(BlockInterface {
            module: filt,
            inputs: vec![bus("in", signal()), bus("f", level())],
            outputs: vec![bus("y0", signal())],
        });
        let first_id = dag.block_metadata()[a].id;
        dag.connect(a, "signal", b, "in");
        dag.expose_input("inc", a, "inc", word(Word::uint(24)));
        dag.set_constant(b, "f", 0x4321);
        dag.expose_output("signal", b, "y0", signal().0);

        let mut graph = RdfGraph::new();
        write_block_dag_into(&mut graph, &dag).expect("write");

        let read = read_block_dag(&graph).expect("read ok").expect("present");
        assert_eq!(read.blocks().len(), 2);
        assert_eq!(read.blocks()[0].module, osc);
        assert_eq!(read.blocks()[1].module, filt);
        assert_eq!(read.block_metadata()[0].id, first_id);
        assert_eq!(read.connections().len(), 1);
        let (from, to) = &read.connections()[0];
        assert_eq!((from.block, from.bus.as_str()), (0, "signal"));
        assert_eq!((to.block, to.bus.as_str()), (1, "in"));
        assert_eq!(read.exposed_inputs().len(), 1);
        assert_eq!(read.exposed_outputs().len(), 1);
        assert_eq!(read.exposed_outputs()[0].0, "signal");
        assert_eq!(read.constants().len(), 1);
        let (target, value) = &read.constants()[0];
        assert_eq!(
            (target.block, target.bus.as_str(), value.as_slice()),
            (1, "f", [0x4321].as_slice())
        );

        // The reloaded DAG must still compile to a bit circuit (with the
        // block modules present in the library).
        let stdlib = crate::stdlib::Stdlib::build();
        let mut dag2 = BlockDag::new();
        // Use real modules this time so compile succeeds end to end.
        let ai = dag2.add_block(BlockInterface {
            module: stdlib.mux,
            inputs: vec![
                bus("a", (word(Word::bit()), RoleTag::RAW)),
                bus("b", (word(Word::bit()), RoleTag::RAW)),
                bus("sel", (word(Word::bit()), RoleTag::RAW)),
            ],
            outputs: vec![bus("y", (word(Word::bit()), RoleTag::RAW))],
        });
        dag2.expose_input("a", ai, "a", word(Word::bit()));
        dag2.expose_input("b", ai, "b", word(Word::bit()));
        dag2.expose_input("sel", ai, "sel", word(Word::bit()));
        dag2.expose_output("y", ai, "y", word(Word::bit()));
        let mut g2 = RdfGraph::new();
        write_block_dag_into(&mut g2, &dag2).unwrap();
        let reloaded = read_block_dag(&g2).unwrap().unwrap();
        assert!(
            reloaded.compile(&stdlib, &stdlib.library, 1).is_ok(),
            "a reloaded DAG of real modules must compile"
        );
    }

    #[test]
    fn missing_block_dag_reads_none() {
        assert!(read_block_dag(&RdfGraph::new()).expect("ok").is_none());
    }

    #[test]
    fn interface_annotations_survive_alongside_a_module() {
        // The annotation coexists with the module's own triples in one graph.
        let stdlib = crate::stdlib::Stdlib::build();
        let module = stdlib.mux; // any real module
        let iface = BlockInterface {
            module,
            inputs: vec![bus("sel", (Type::word(Word::bit()), RoleTag::RAW))],
            outputs: vec![bus("y", (Type::word(Word::bit()), RoleTag::RAW))],
        };
        let mut graph = RdfGraph::new();
        write_module_into(&mut graph, stdlib.library.get(module).unwrap()).expect("module");
        write_block_interface_into(&mut graph, &iface).expect("interface");
        let read = read_block_interface(&graph, module).unwrap().unwrap();
        assert_eq!(read.inputs.len(), 1);
        assert_eq!(read.outputs.len(), 1);
    }
}
