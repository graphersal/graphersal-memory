graphersal-memory (develop): tools to improve the structure of the local memory graph, beside
the memory functions themselves (memories, memory, remember, forget, pin, conflicts, settle,
sessions, session, recall; each a tool of its own, folder "memory").

- Read first: get_schema (the labels memory, session, turn, fact; edges relates_to, supersedes,
  conflicts_with, narrows, about, contains, edited), list_saved_queries, statistics.
- Decisions between memories are data: supersedes (newer -> older, `at`), conflicts_with (an
  open conflict), narrows (project -> global); an imported memory has `source` (its file),
  `imported_at` and, until checked, `unchecked`. Settle them with remember(supersedes) and
  settle, not by editing edges.
- gql (read queries; the supported subset is the resource graphersal://gql-reference) and query
  (Gremlin in Rhai; also writes and maintenance) for everything else.
- Every memory function's `project` parameter is set by the server to the current project; a
  vertex with a `project` property is visible only from that project, one without it from every
  project. Call your own queries with run_saved_query.
- The memory functions and the schema are the product's (meta origin "product"): an upgrade of
  graphersal-memory replaces them. Put your own queries in another folder.
- Change data only when the user asked for it.
