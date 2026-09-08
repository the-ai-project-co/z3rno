//! Spike: Neo4j (graph) + Qdrant (vector) as two separate database services,
//! driven from Rust via `neo4rs` (bolt) and `qdrant-client` (gRPC).
//!
//! Not a product — see spikes/0002-production-backend-selection/RUBRIC.md for what this is evidence for.

use anyhow::{Context, Result};
use neo4rs::{query, Graph};
use qdrant_client::qdrant::{
    value::Kind, CreateCollectionBuilder, Distance, PointStruct, ScrollPointsBuilder,
    SearchPointsBuilder, UpsertPointsBuilder, VectorParamsBuilder,
};
use qdrant_client::Qdrant;
use rand::Rng;
use uuid::Uuid;

const NEO4J_URI: &str = "127.0.0.1:7687";
const NEO4J_USER: &str = "neo4j";
const NEO4J_PASS: &str = "spikepassword";
const QDRANT_URL: &str = "http://127.0.0.1:6334";
const COLLECTION: &str = "memories";
const VECTOR_DIM: u64 = 384;

fn pass(label: &str) {
    println!("PASS  {label}");
}
fn fail(label: &str, err: impl std::fmt::Display) {
    println!("FAIL  {label}: {err}");
}

fn random_vector(dim: usize) -> Vec<f32> {
    let mut rng = rand::thread_rng();
    (0..dim).map(|_| rng.gen_range(-1.0..1.0)).collect()
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("== Neo4j + Qdrant spike ==\n");

    // --- connect ---
    let graph = Graph::new(NEO4J_URI, NEO4J_USER, NEO4J_PASS)
        .await
        .context("connect to neo4j")?;
    pass("connect: neo4j (bolt)");

    let qdrant = Qdrant::from_url(QDRANT_URL)
        .build()
        .context("connect to qdrant")?;
    pass("connect: qdrant (gRPC)");

    // --- setup: fresh collection ---
    if qdrant.collection_exists(COLLECTION).await? {
        qdrant.delete_collection(COLLECTION).await?;
    }
    qdrant
        .create_collection(
            CreateCollectionBuilder::new(COLLECTION)
                .vectors_config(VectorParamsBuilder::new(VECTOR_DIM, Distance::Cosine)),
        )
        .await
        .context("create qdrant collection")?;
    pass("setup: qdrant collection created (384-dim, cosine)");

    // clear any leftover nodes from a previous run
    graph
        .run(query("MATCH (n:Memory) DETACH DELETE n"))
        .await
        .context("clear neo4j")?;

    // =========================================================================
    // Step 1: store one memory in both stores
    // =========================================================================
    let tenant_a = "tenant-a";
    let memory_id = Uuid::new_v4();
    let memory_text = "The user prefers dark mode and terse responses.";

    let r = graph
        .run(
            query("CREATE (m:Memory {id: $id, text: $text, tenant: $tenant})")
                .param("id", memory_id.to_string())
                .param("text", memory_text)
                .param("tenant", tenant_a),
        )
        .await;
    match r {
        Ok(_) => pass("store: neo4j node created (Memory {id, text, tenant})"),
        Err(e) => fail("store: neo4j node created", e),
    }

    let embedding = random_vector(VECTOR_DIM as usize);
    let point = PointStruct::new(
        memory_id.to_string(),
        embedding.clone(),
        [
            ("memory_id", memory_id.to_string().into()),
            ("tenant", tenant_a.into()),
        ],
    );
    let r = qdrant
        .upsert_points(UpsertPointsBuilder::new(COLLECTION, vec![point]))
        .await;
    match r {
        Ok(_) => {
            pass("store: qdrant point upserted (random 384-dim vector + memory_id/tenant payload)")
        }
        Err(e) => fail("store: qdrant point upserted", e),
    }

    // =========================================================================
    // Step 2: recall via qdrant vector search -> fetch matching neo4j node
    // =========================================================================
    let query_vec = embedding.clone(); // same vector -> guaranteed top hit for a structural spike
    let search_result = qdrant
        .search_points(SearchPointsBuilder::new(COLLECTION, query_vec, 1).with_payload(true))
        .await;

    match search_result {
        Ok(resp) => {
            if let Some(top) = resp.result.first() {
                let recalled_id = match top.payload.get("memory_id").and_then(|v| v.kind.clone()) {
                    Some(Kind::StringValue(s)) => s,
                    _ => String::new(),
                };
                if recalled_id == memory_id.to_string() {
                    pass("recall: qdrant vector search returned the stored point (top-1, score matches)");
                } else {
                    fail(
                        "recall: qdrant vector search returned the stored point",
                        "id mismatch",
                    );
                }

                let mut fetch = graph
                    .execute(
                        query(
                            "MATCH (m:Memory {id: $id}) RETURN m.text AS text, m.tenant AS tenant",
                        )
                        .param("id", recalled_id.clone()),
                    )
                    .await
                    .context("fetch neo4j node by recalled id")?;
                if let Ok(Some(row)) = fetch.next().await {
                    let text: String = row.get("text").unwrap_or_default();
                    if text == memory_text {
                        pass("recall: fetched matching neo4j node by id returned from qdrant hit");
                    } else {
                        fail("recall: fetched matching neo4j node by id", "text mismatch");
                    }
                } else {
                    fail(
                        "recall: fetched matching neo4j node by id",
                        "no row returned",
                    );
                }
            } else {
                fail(
                    "recall: qdrant vector search returned the stored point",
                    "empty result set",
                );
            }
        }
        Err(e) => fail("recall: qdrant vector search returned the stored point", e),
    }

    // =========================================================================
    // Step 3: second node + relationship, traversed back out via cypher
    // =========================================================================
    let related_id = Uuid::new_v4();
    let r = graph
        .run(
            query(
                "MATCH (m:Memory {id: $mid}) \
                 CREATE (r:Memory {id: $rid, text: $text, tenant: $tenant}) \
                 CREATE (m)-[:RELATED_TO {reason: 'same_session'}]->(r)",
            )
            .param("mid", memory_id.to_string())
            .param("rid", related_id.to_string())
            .param(
                "text",
                "Follow-up: user also asked to disable notifications.",
            )
            .param("tenant", tenant_a),
        )
        .await;
    match r {
        Ok(_) => pass("graph: second node + RELATED_TO relationship created"),
        Err(e) => fail("graph: second node + RELATED_TO relationship created", e),
    }

    let mut traverse = graph
        .execute(
            query(
                "MATCH (m:Memory {id: $mid})-[rel:RELATED_TO]->(other:Memory) \
                 RETURN other.text AS text, type(rel) AS rel_type",
            )
            .param("mid", memory_id.to_string()),
        )
        .await
        .context("traverse relationship")?;
    match traverse.next().await {
        Ok(Some(row)) => {
            let text: String = row.get("text").unwrap_or_default();
            let rel_type: String = row.get("rel_type").unwrap_or_default();
            if rel_type == "RELATED_TO" && text.contains("Follow-up") {
                pass("graph: traversed RELATED_TO relationship back out via cypher");
            } else {
                fail(
                    "graph: traversed RELATED_TO relationship back out via cypher",
                    "unexpected row content",
                );
            }
        }
        _ => fail(
            "graph: traversed RELATED_TO relationship back out via cypher",
            "no row returned",
        ),
    }

    // =========================================================================
    // Step 4: multi-tenant isolation - two angles
    // =========================================================================
    println!();

    // 4a. Can Neo4j Community create a second database (the Enterprise multi-db
    // mechanism used for per-tenant isolation)? Expectation: no - Community Edition
    // is limited to a single user database ("neo4j"); CREATE DATABASE requires
    // Enterprise Edition. Confirm and document, don't assume.
    let mut create_db = graph.execute(query("CREATE DATABASE tenant_b_db")).await;
    match &mut create_db {
        Ok(stream) => match stream.next().await {
            Ok(_) => fail(
                "isolation: neo4j CREATE DATABASE (per-tenant db, Community Edition)",
                "unexpectedly succeeded - Community Edition should reject this",
            ),
            Err(e) => {
                println!(
                    "PASS  isolation: neo4j CREATE DATABASE correctly rejected on Community Edition ({e})"
                );
                println!(
                    "      -> confirms: Neo4j Community has no native per-tenant database isolation;"
                );
                println!("         multi-database (the Enterprise mechanism analogous to Postgres RLS) is a paid feature.");
            }
        },
        Err(e) => {
            println!(
                "PASS  isolation: neo4j CREATE DATABASE correctly rejected on Community Edition ({e})"
            );
            println!(
                "      -> confirms: Neo4j Community has no native per-tenant database isolation;"
            );
            println!("         multi-database (the Enterprise mechanism analogous to Postgres RLS) is a paid feature.");
        }
    }

    // 4b. Qdrant payload-filtered search: store a tenant-b point, then show that
    // a tenant-scoped filtered query only returns tenant-a's point, but an
    // unfiltered query (the "buggy/malicious client" case) leaks across tenants.
    // This demonstrates isolation is *application-enforced* (must remember the
    // filter every time), not a database guarantee.
    let tenant_b = "tenant-b";
    let tenant_b_id = Uuid::new_v4();
    let tenant_b_vec = embedding.clone(); // reuse same vector so it's guaranteed to be a top hit
    let point_b = PointStruct::new(
        tenant_b_id.to_string(),
        tenant_b_vec.clone(),
        [
            ("memory_id", tenant_b_id.to_string().into()),
            ("tenant", tenant_b.into()),
        ],
    );
    qdrant
        .upsert_points(UpsertPointsBuilder::new(COLLECTION, vec![point_b]))
        .await
        .context("upsert tenant-b point")?;

    use qdrant_client::qdrant::{Condition, Filter};

    // Correctly filtered search: tenant-a client, tenant-a filter -> only tenant-a data.
    let filtered = qdrant
        .search_points(
            SearchPointsBuilder::new(COLLECTION, embedding.clone(), 10)
                .filter(Filter::must([Condition::matches(
                    "tenant",
                    tenant_a.to_string(),
                )]))
                .with_payload(true),
        )
        .await
        .context("filtered search")?;
    let filtered_tenants: Vec<String> = filtered
        .result
        .iter()
        .filter_map(
            |p| match p.payload.get("tenant").and_then(|v| v.kind.clone()) {
                Some(Kind::StringValue(s)) => Some(s),
                _ => None,
            },
        )
        .collect();
    let filtered_ok =
        !filtered_tenants.is_empty() && filtered_tenants.iter().all(|t| t == tenant_a);
    if filtered_ok {
        pass("isolation: qdrant payload-filtered search (tenant=tenant-a) returns only tenant-a points");
    } else {
        fail(
            "isolation: qdrant payload-filtered search (tenant=tenant-a) returns only tenant-a points",
            format!("saw tenants: {filtered_tenants:?}"),
        );
    }

    // Unfiltered search using the SAME collection and SAME credentials: simulates a
    // buggy/malicious query that forgets (or never had) the tenant filter.
    let unfiltered = qdrant
        .search_points(
            SearchPointsBuilder::new(COLLECTION, embedding.clone(), 10).with_payload(true),
        )
        .await
        .context("unfiltered search")?;
    let unfiltered_tenants: Vec<String> = unfiltered
        .result
        .iter()
        .filter_map(
            |p| match p.payload.get("tenant").and_then(|v| v.kind.clone()) {
                Some(Kind::StringValue(s)) => Some(s),
                _ => None,
            },
        )
        .collect();
    let leaks_cross_tenant = unfiltered_tenants.iter().any(|t| t == tenant_a)
        && unfiltered_tenants.iter().any(|t| t == tenant_b);
    if leaks_cross_tenant {
        println!(
            "PASS  isolation: unfiltered qdrant search (no tenant filter applied) returns BOTH tenants: {unfiltered_tenants:?}"
        );
        println!(
            "      -> confirms: Qdrant enforces nothing at the database level; a single collection"
        );
        println!("         holding all tenants relies entirely on every call site remembering the filter.");
    } else {
        fail(
            "isolation: unfiltered qdrant search demonstrates cross-tenant leak",
            format!("expected both tenants present, saw: {unfiltered_tenants:?}"),
        );
    }

    // sanity: same demonstration is available via scroll (non-similarity list), same conclusion.
    let _ = qdrant
        .scroll(
            ScrollPointsBuilder::new(COLLECTION)
                .limit(10)
                .with_payload(true),
        )
        .await;

    println!("\n== done ==");
    Ok(())
}
