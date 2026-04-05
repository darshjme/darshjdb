# Lessons from Ontotext GraphDB for DarshJDB

Research date: 2026-04-05

## What GraphDB Does That We Should Learn From

### 1. Entity Pool (Integer ID Mapping)

GraphDB converts all URIs, blank nodes, and literals to internal 32-bit or 40-bit integer IDs via an "Entity Pool". All internal operations (joins, index lookups, inference) use integers. URIs are resolved only at the API boundary.

**Why this matters for DarshJDB:** We store raw UUIDs (16 bytes) and text attribute names in every triple row. Integer IDs would:
- Reduce index size by 4-8x
- Make JOIN operations faster (integer comparison vs UUID)
- Enable more efficient caching (integer keys hash faster)

**Implementation:** Add `entity_pool` and `attribute_pool` tables. Map UUID → i64, attribute_name → i32. Use integer IDs in the triples table. Resolve at REST/WebSocket boundary.

### 2. Dual Index Strategy (PSO + POS)

GraphDB maintains exactly two primary indexes:
- **PSO** (predicate-subject-object): "Find all values of attribute X for entity Y"
- **POS** (predicate-object-subject): "Find all entities where attribute X = value V"

These cover the two fundamental access patterns. DarshJDB has 5 indexes but they may not be optimally structured for these exact patterns.

**Action:** Audit our index usage. Consider consolidating to match the PSO/POS pattern.

### 3. Connector Architecture (Automatic Search Sync)

GraphDB's connectors keep external search engines (Elasticsearch, Lucene, Solr) automatically in sync at the entity level. When a triple changes, the connector identifies the affected entity and updates its search document.

**Key insight:** Sync happens at the ENTITY level, not the triple level. When one triple changes, the entire entity's search document is rebuilt.

**DarshJDB already has the infrastructure for this:** The ChangeEvent broadcast channel emits entity_id + attribute on every mutation. A connector architecture would:
1. Listen on the broadcast channel
2. Identify affected entity
3. Rebuild that entity's search document
4. Push to Elasticsearch/Meilisearch/Typesense

### 4. Global Shared Cache

GraphDB uses one global cache shared across all repositories. The cache dynamically allocates slots based on which repository is most active. No per-repository configuration needed.

**For DarshJDB:** Our LRU plan cache is per-query-shape. A global result cache that dynamically prioritizes hot queries would reduce Postgres load.

### 5. Forward-Chaining Rule Engine (TRREE)

GraphDB's Triple Reasoning and Rule Entailment Engine (TRREE) automatically generates implied triples when data is inserted. Example: if you insert "Alice is a Manager" and a rule says "Managers can approve expenses", GraphDB materializes "Alice can approve expenses" at insert time.

**For DarshJDB:** This could power:
- Computed attributes (e.g., fullName = firstName + " " + lastName)
- Permission propagation (if user joins team, inherit team permissions)
- Denormalization (automatically maintain counts, aggregates)

### 6. Batch Loading Performance

GraphDB achieves 200,000-500,000 statements per second during bulk loading on commodity hardware. They use:
- Batch inserts (not individual INSERT statements)
- Deferred index updates
- Memory-mapped I/O

**For DarshJDB:** Our migration tooling should use PostgreSQL COPY command for bulk loading instead of batched INSERT via /api/mutate. This alone could give 10-50x speedup for migrations.

### 7. RDF-star (Triples About Triples)

GraphDB supports embedded triples — a triple can be the subject or object of another triple. This enables:
- Provenance: "Triple X was asserted by Source Y"
- Confidence: "Triple X has confidence 0.95"
- Temporal: "Triple X was true from date A to date B"

**For DarshJDB:** Our value_type system could add type 7 = "triple_ref" pointing to another triple's ID. This would enable audit trails, data lineage, and temporal queries without a separate audit table.

## What GraphDB Gets Wrong (For Our Use Case)

1. **SPARQL complexity** — GraphDB uses SPARQL, which is powerful but hard for application developers. DarshJDB's DarshanQL is simpler and more intuitive. Keep it.

2. **No real-time subscriptions** — GraphDB is batch-oriented. It has no WebSocket push. DarshJDB's reactive subscription model is a genuine differentiator.

3. **No built-in auth** — GraphDB has basic HTTP auth but no JWT, OAuth, or row-level security. DarshJDB's integrated auth+permissions is a major advantage.

4. **Java monolith** — GraphDB is a Java application requiring JVM tuning. DarshJDB's single Rust binary with zero GC is operationally simpler.

5. **No client SDKs** — GraphDB expects you to write SPARQL. DarshJDB's React/Angular/Next.js/PHP/Python SDKs are the developer experience that makes it accessible.

## Priority Actions

1. **Entity Pool** — Highest impact. Implement integer ID mapping. This is the single biggest performance improvement we can make.
2. **COPY-based bulk loading** — Add `ddb import --bulk` using PostgreSQL COPY protocol.
3. **Connector architecture** — Generalize the ChangeEvent broadcast into a plugin system for search engines.
4. **Forward-chaining rules** — Design a rule engine for computed attributes and permission propagation.

## Sources

- [GraphDB Architecture and Components](https://graphdb.ontotext.com/documentation/11.3/architecture-components.html)
- [GraphDB Data Storage](https://graphdb.ontotext.com/documentation/master/storage.html)
- [GraphDB Connectors for Full-Text Search](https://graphdb.ontotext.com/documentation/master/general-full-text-search-with-connectors.html)
- [GraphDB Rules Optimizations](https://graphdb.ontotext.com/documentation/master/rules-optimisations.html)
- [GraphDB Data Loading Optimizations](https://graphdb.ontotext.com/documentation/10.7/data-loading-query-optimisations.html)
- [RDF-star Support](https://graphdb.ontotext.com/documentation/11.3/rdf-sparql-star.html)
- [Large Triple Stores (W3C)](https://www.w3.org/wiki/LargeTripleStores)
- [Triple Store Architectures (Tutorial)](https://www.iaria.org/conferences2018/filesDBKDA18/IztokSavnik_Tutorial_3store-arch.pdf)
- [What is an RDF Triplestore (Ontotext)](https://www.ontotext.com/knowledgehub/fundamentals/what-is-rdf-triplestore/)
