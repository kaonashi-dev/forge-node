//! A project's codebase graph. Ids are the graph's own hashes; kinds are
//! explicit tags, like every other enum column.

use codebase_graph::{Delta, Edge, EdgeKind, Graph, Node, NodeId, NodeKind};
use domain::ProjectId;
use rusqlite::{params, params_from_iter, Connection, Row, Transaction};

use crate::db::DbError;

const NODE_COLUMNS: &str =
    "id, kind, path, name, span_start, span_end, line, end_line, lang, signature";

fn node_tag(kind: NodeKind) -> Result<&'static str, DbError> {
    match kind {
        NodeKind::File => Ok("File"),
        NodeKind::Function => Ok("Function"),
        NodeKind::Class => Ok("Class"),
        NodeKind::Variable => Ok("Variable"),
        NodeKind::ExternalCall => Ok("ExternalCall"),
        other => Err(DbError::Encode(format!("unhandled NodeKind: {other:?}"))),
    }
}

fn node_kind(tag: &str) -> Result<NodeKind, DbError> {
    match tag {
        "File" => Ok(NodeKind::File),
        "Function" => Ok(NodeKind::Function),
        "Class" => Ok(NodeKind::Class),
        "Variable" => Ok(NodeKind::Variable),
        "ExternalCall" => Ok(NodeKind::ExternalCall),
        _ => Err(DbError::decode_msg("NodeKind", tag, "unknown kind tag")),
    }
}

fn edge_tag(kind: EdgeKind) -> Result<&'static str, DbError> {
    match kind {
        EdgeKind::Calls => Ok("Calls"),
        EdgeKind::Imports => Ok("Imports"),
        EdgeKind::Uses => Ok("Uses"),
        EdgeKind::Declares => Ok("Declares"),
        EdgeKind::Inherits => Ok("Inherits"),
        EdgeKind::Defines => Ok("Defines"),
        other => Err(DbError::Encode(format!("unhandled EdgeKind: {other:?}"))),
    }
}

fn edge_kind(tag: &str) -> Result<EdgeKind, DbError> {
    match tag {
        "Calls" => Ok(EdgeKind::Calls),
        "Imports" => Ok(EdgeKind::Imports),
        "Uses" => Ok(EdgeKind::Uses),
        "Declares" => Ok(EdgeKind::Declares),
        "Inherits" => Ok(EdgeKind::Inherits),
        "Defines" => Ok(EdgeKind::Defines),
        _ => Err(DbError::decode_msg("EdgeKind", tag, "unknown kind tag")),
    }
}

fn id_of(bytes: &[u8]) -> Result<NodeId, DbError> {
    NodeId::try_from(bytes).map_err(|_| DbError::decode_msg("NodeId", "blob", "not 16 bytes"))
}

fn node_row(row: &Row<'_>) -> rusqlite::Result<(Vec<u8>, String, Node)> {
    let id: Vec<u8> = row.get(0)?;
    let kind: String = row.get(1)?;
    Ok((
        id,
        kind,
        Node {
            id: [0; 16],
            kind: NodeKind::File,
            path: row.get(2)?,
            name: row.get(3)?,
            span: (row.get(4)?, row.get(5)?),
            line: row.get(6)?,
            end_line: row.get(7)?,
            lang: row.get(8)?,
            signature: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
        },
    ))
}

fn decode_node((id, kind, mut node): (Vec<u8>, String, Node)) -> Result<Node, DbError> {
    node.id = id_of(&id)?;
    node.kind = node_kind(&kind)?;
    Ok(node)
}

fn insert(
    tx: &Transaction<'_>,
    project: ProjectId,
    git_oid: &str,
    nodes: &[Node],
    edges: &[Edge],
) -> Result<(), DbError> {
    let project = project.to_string();
    let mut node = tx.prepare_cached(&format!(
        "INSERT OR REPLACE INTO codebase_graph_nodes (project_id, git_oid, {NODE_COLUMNS}) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
    ))?;
    for n in nodes {
        node.execute(params![
            project,
            git_oid,
            n.id.as_slice(),
            node_tag(n.kind)?,
            n.path,
            n.name,
            n.span.0,
            n.span.1,
            n.line,
            n.end_line,
            n.lang,
            n.signature,
        ])?;
    }
    let mut edge = tx.prepare_cached(
        "INSERT OR REPLACE INTO codebase_graph_edges (project_id, src, dst, kind, weight) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for e in edges {
        edge.execute(params![
            project,
            e.src.as_slice(),
            e.dst.as_slice(),
            edge_tag(e.kind)?,
            e.weight
        ])?;
    }
    Ok(())
}

/// Every row of the project's graph replaced by `graph`'s, and its dirty paths by `dirty`.
///
/// # Errors
/// Returns [`DbError`] on a failed statement; the transaction is rolled back.
pub fn replace(
    conn: &mut Connection,
    project: ProjectId,
    graph: &Graph,
    dirty: &[String],
) -> Result<(), DbError> {
    let tx = conn.transaction()?;
    let id = project.to_string();
    tx.execute(
        "DELETE FROM codebase_graph_edges WHERE project_id = ?1",
        [&id],
    )?;
    tx.execute(
        "DELETE FROM codebase_graph_nodes WHERE project_id = ?1",
        [&id],
    )?;
    insert(&tx, project, &graph.git_oid, &graph.nodes, &graph.edges)?;
    set_dirty(&tx, project, dirty)?;
    tx.commit()?;
    Ok(())
}

/// The nodes of `paths` and every edge touching them replaced by `delta`'s,
/// with its stale edges deleted; external calls nothing calls any more are
/// dropped, the whole graph is stamped with `git_oid` and its dirty paths
/// replaced by `dirty`.
///
/// # Errors
/// Returns [`DbError`] on a failed statement; the transaction is rolled back.
pub fn replace_paths(
    conn: &mut Connection,
    project: ProjectId,
    git_oid: &str,
    paths: &[String],
    delta: &Delta,
    dirty: &[String],
) -> Result<(), DbError> {
    let tx = conn.transaction()?;
    let id = project.to_string();
    tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS graph_paths (path TEXT PRIMARY KEY); DELETE FROM graph_paths;")?;
    {
        let mut add = tx.prepare_cached("INSERT OR IGNORE INTO graph_paths (path) VALUES (?1)")?;
        for path in paths {
            add.execute([path])?;
        }
    }
    tx.execute(
        "DELETE FROM codebase_graph_edges WHERE project_id = ?1 AND (src IN \
         (SELECT id FROM codebase_graph_nodes WHERE project_id = ?1 AND path IN (SELECT path FROM graph_paths)) \
         OR dst IN (SELECT id FROM codebase_graph_nodes WHERE project_id = ?1 AND path IN (SELECT path FROM graph_paths)))",
        [&id],
    )?;
    tx.execute(
        "DELETE FROM codebase_graph_nodes WHERE project_id = ?1 AND path IN (SELECT path FROM graph_paths)",
        [&id],
    )?;
    {
        let mut stale = tx.prepare_cached(
            "DELETE FROM codebase_graph_edges \
             WHERE project_id = ?1 AND src = ?2 AND dst = ?3 AND kind = ?4",
        )?;
        for e in &delta.stale {
            stale.execute(params![
                id,
                e.src.as_slice(),
                e.dst.as_slice(),
                edge_tag(e.kind)?
            ])?;
        }
    }
    insert(&tx, project, git_oid, &delta.nodes, &delta.edges)?;
    set_dirty(&tx, project, dirty)?;
    tx.execute(
        "DELETE FROM codebase_graph_nodes WHERE project_id = ?1 AND kind = 'ExternalCall' \
         AND id NOT IN (SELECT dst FROM codebase_graph_edges WHERE project_id = ?1)",
        [&id],
    )?;
    tx.execute(
        "UPDATE codebase_graph_nodes SET git_oid = ?2 WHERE project_id = ?1",
        params![id, git_oid],
    )?;
    tx.execute_batch("DELETE FROM graph_paths;")?;
    tx.commit()?;
    Ok(())
}

fn set_dirty(tx: &Transaction<'_>, project: ProjectId, dirty: &[String]) -> Result<(), DbError> {
    let id = project.to_string();
    tx.execute(
        "DELETE FROM codebase_graph_dirty WHERE project_id = ?1",
        [&id],
    )?;
    let mut add = tx.prepare_cached(
        "INSERT OR IGNORE INTO codebase_graph_dirty (project_id, path) VALUES (?1, ?2)",
    )?;
    for path in dirty {
        add.execute(params![id, path])?;
    }
    Ok(())
}

/// The paths the project's last graph build read from a dirty working tree.
///
/// # Errors
/// Returns [`DbError`] on a failed statement.
pub fn dirty(conn: &Connection, project: ProjectId) -> Result<Vec<String>, DbError> {
    let mut statement = conn.prepare_cached(
        "SELECT path FROM codebase_graph_dirty WHERE project_id = ?1 ORDER BY path",
    )?;
    let paths = statement
        .query_map([project.to_string()], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(paths)
}

/// `None` when the project has no graph.
///
/// # Errors
/// Returns [`DbError`] on a failed statement or an undecodable row.
pub fn load(conn: &Connection, project: ProjectId) -> Result<Option<Graph>, DbError> {
    let id = project.to_string();
    let git_oid: Option<String> = conn.query_row(
        "SELECT MAX(git_oid) FROM codebase_graph_nodes WHERE project_id = ?1",
        [&id],
        |row| row.get(0),
    )?;
    let Some(git_oid) = git_oid else {
        return Ok(None);
    };
    let mut statement = conn.prepare_cached(&format!(
        "SELECT {NODE_COLUMNS} FROM codebase_graph_nodes WHERE project_id = ?1 \
         ORDER BY path, span_start, span_end, name"
    ))?;
    let nodes = statement
        .query_map([&id], node_row)?
        .map(|row| decode_node(row?))
        .collect::<Result<Vec<_>, _>>()?;
    let mut statement = conn.prepare_cached(
        "SELECT src, dst, kind, weight FROM codebase_graph_edges WHERE project_id = ?1 \
         ORDER BY src, dst, kind",
    )?;
    let edges = statement
        .query_map([&id], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u8>(3)?,
            ))
        })?
        .map(|row| {
            let (src, dst, kind, weight) = row?;
            Ok(Edge {
                src: id_of(&src)?,
                dst: id_of(&dst)?,
                kind: edge_kind(&kind)?,
                weight,
            })
        })
        .collect::<Result<Vec<_>, DbError>>()?;
    Ok(Some(Graph {
        git_oid,
        nodes,
        edges,
    }))
}

/// The project's nodes named `name`, by path.
///
/// # Errors
/// Returns [`DbError`] on a failed statement or an undecodable row.
pub fn named(conn: &Connection, project: ProjectId, name: &str) -> Result<Vec<Node>, DbError> {
    let mut statement = conn.prepare_cached(&format!(
        "SELECT {NODE_COLUMNS} FROM codebase_graph_nodes WHERE project_id = ?1 AND name = ?2 \
         ORDER BY path, span_start"
    ))?;
    let rows = statement.query_map(
        params_from_iter([project.to_string(), name.to_owned()]),
        node_row,
    )?;
    rows.map(|row| decode_node(row?)).collect()
}
